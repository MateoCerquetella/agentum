//! Command palette — Fresh-IDE-style prefix-routed picker.
//!
//! Opened with Ctrl-P or Ctrl-Shift-P. Prefix routing matches Fresh:
//!
//!   (no prefix)  fuzzy across everything (default)
//!   `>`          commands only (focus, theme, lazygit, refresh, quit)
//!   `#`          sessions only — like Fresh's buffer switcher
//!   `@`          themes only
//!   `~`          settings — status-bar chip toggles, view toggles
//!
//! Type to filter, ↑/↓ to move, Enter to run. Catalogue is rebuilt every
//! frame so dynamic entries (sessions, themes) stay current. Filtering
//! is a cheap case-insensitive subsequence match so "thmid" matches
//! "Theme: midnight".

use uuid::Uuid;

use super::prefs::StatusChip;
use super::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    All,
    Commands,
    Sessions,
    Themes,
    Settings,
}

impl Mode {
    /// Inspect the leading char of `raw` and return (mode, remaining_query).
    /// The prefix character itself is consumed. An empty/no-prefix string
    /// stays in `All` mode.
    pub fn from_query(raw: &str) -> (Self, &str) {
        match raw.chars().next() {
            Some('>') => (Self::Commands, raw[1..].trim_start()),
            Some('#') => (Self::Sessions, raw[1..].trim_start()),
            Some('@') => (Self::Themes, raw[1..].trim_start()),
            Some('~') => (Self::Settings, raw[1..].trim_start()),
            _ => (Self::All, raw),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Commands => "commands",
            Self::Sessions => "sessions",
            Self::Themes => "themes",
            Self::Settings => "settings",
        }
    }

    pub fn keep(self, group: &str) -> bool {
        match self {
            Self::All => true,
            Self::Commands => matches!(
                group,
                "general" | "focus" | "extensions" | "appearance" | "view" | "settings"
            ),
            Self::Sessions => group == "sessions",
            Self::Themes => group == "appearance",
            Self::Settings => matches!(group, "settings" | "view"),
        }
    }
}

/// A single picker entry. The action is a tagged enum so the key handler
/// can dispatch without holding a closure (which would borrow App).
#[derive(Clone)]
pub struct Action {
    pub label: String,
    pub hint: String,
    pub group: &'static str,
    pub kind: ActionKind,
}

#[derive(Clone)]
pub enum ActionKind {
    Quit,
    ToggleHelp,
    /// Open the recent-error log overlay. Bound to `!` from tree focus.
    ShowErrors,
    ToggleLazygit,
    LazygitCheats,
    Refresh,
    SpawnTerminal,
    FocusTree,
    FocusTerm,
    FocusLazygit,
    SetTheme(&'static str),
    CycleTheme,
    SelectSession(Uuid),
    /// Open the destructive-confirm overlay for `Kill` against this
    /// session. Routed through the same prompt as Shift-K / Shift-D / x
    /// from tree focus so a misclick in the palette can't drop a
    /// process by accident. Kill is the single destructive verb — it
    /// stops the process and removes the session in one step.
    KillSession(Uuid),
    /// View toggles — same primitives the Ctrl-* shortcuts already
    /// drive, surfaced through the palette so users can find them
    /// without memorising chords.
    ToggleSidebar,
    ToggleRightPanel,
    ToggleFullscreen,
    ToggleSplit,
    /// Toggle visibility of one status-bar chip. Persists to disk.
    ToggleStatusChip(StatusChip),
    /// Reset all status-bar prefs to their defaults.
    ResetStatusBar,
    /// Open the Settings overlay. Mirrors the `Ctrl-,` keybinding so the
    /// settings UI is also reachable through palette filtering (`~`).
    OpenSettings,
    /// Toggle the master sound switch. Persists to disk.
    ToggleSoundMaster,
    /// Toggle a per-kind notification sound. Persists to disk.
    ToggleSoundKind(super::prefs::SoundKind),
    /// Adjust a per-kind notification TTL by `±NOTIF_TTL_STEP_MS`.
    /// Persists to disk.
    BumpTtl(super::prefs::SoundKind, i64),
    /// Reset every persisted setting to its default. Touches more than
    /// `ResetStatusBar` — drops layout sizes, sounds, and TTLs back to
    /// the ship-with values.
    ResetAllPrefs,
    /// Open the server switcher overlay. Mirrors the Ctrl-S
    /// keybinding so the profile picker is reachable through palette
    /// filtering ("server" / "profile" / "switch").
    OpenProfiles,
    /// Open the SSH hosts overlay (readiness reports). Mirrors the
    /// `Ctrl-H` keybinding so the hosts manager is reachable through
    /// palette filtering ("host" / "readiness" / "ssh").
    OpenHosts,
    /// Collapse / expand the SERVERS section of the sidebar. Mirrors
    /// the `Ctrl-K V` chord. Persisted across runs via prefs.
    ToggleServers,
    /// Toggle "show sessions from every reachable server" (default,
    /// recommended) vs "scope the tree to the active server only".
    /// Persisted across runs via prefs. Reachable from the command
    /// palette and the Ctrl-S profiles overlay's `s` key.
    ToggleShowAllServers,

    // ── Session CRUD ──────────────────────────────────────────────
    /// Open the New Session form. Mirrors the `n` keybinding in the
    /// tree so users can spawn an agent from the palette without
    /// switching focus first.
    NewSession,
    /// Open the inline rename prompt for the given session. Mirrors
    /// `Ctrl-R` from tree focus.
    RenameSession(Uuid),
    /// Confirm-start a stopped/idle session. Mirrors `u` / `Shift-U`
    /// from tree focus.
    StartSession(Uuid),
    /// Confirm-stop a running session. Mirrors `s` / `Shift-S` from
    /// tree focus.
    StopSession(Uuid),
}

pub struct Catalog {
    pub actions: Vec<Action>,
}

/// Snapshot of view + prefs state passed into `Catalog::build`. Lets
/// the Settings-group actions render their current value ("[x]" /
/// "[ ]") without the catalogue having to take a `&App` reference (the
/// borrow checker hates that — `App` owns its own catalogue derivation).
#[derive(Clone, Copy)]
pub struct ViewState {
    pub sidebar_hidden: bool,
    pub right_panel_visible: bool,
    pub fullscreen: bool,
    pub split_open: bool,
    pub servers_collapsed: bool,
    pub show_all_servers: bool,
}

impl Catalog {
    /// Build the live action list. Pure — call from a draw or key handler.
    /// `selected` is the currently-highlighted session (if any). When
    /// present, its kill entry is pinned to the very top so
    /// the most likely action ("get rid of *this* terminal") is one
    /// keystroke away with no typing.
    ///
    /// Each session tuple is `(id, name, workdir, is_running)` where
    /// `is_running` is true for Running status — used to gate Start / Stop
    /// entries so the palette only shows the applicable verb.
    pub fn build(
        lazygit_open: bool,
        sessions: &[(Uuid, String, String, bool)], // (id, name, workdir, is_running)
        selected: Option<Uuid>,
        view: ViewState,
        prefs: &super::prefs::Prefs,
    ) -> Self {
        let mut a = Vec::with_capacity(32);

        // Lead with the destructive action for the active session — this
        // is what users hit Ctrl-P looking for ("close this thing").
        // Resolve the name from `sessions` so a stale `selected` (id no
        // longer in the list) silently drops without crashing.
        if let Some(id) = selected
            && let Some((_, name, _, _)) = sessions.iter().find(|(sid, _, _, _)| *sid == id)
        {
            a.push(Action {
                label: format!("Kill this terminal: {name}"),
                hint: "Shift-K · x · Shift-D".into(),
                group: "sessions",
                kind: ActionKind::KillSession(id),
            });
        }

        a.push(Action {
            label: "Quit agentum".into(),
            hint: "Ctrl-Q · Ctrl-C".into(),
            group: "general",
            kind: ActionKind::Quit,
        });
        a.push(Action {
            label: "Help · keyboard reference".into(),
            hint: "?".into(),
            group: "general",
            kind: ActionKind::ToggleHelp,
        });
        a.push(Action {
            label: "Errors · view recent error log".into(),
            hint: "!".into(),
            group: "general",
            kind: ActionKind::ShowErrors,
        });
        a.push(Action {
            label: "Refresh sessions".into(),
            hint: "r".into(),
            group: "general",
            kind: ActionKind::Refresh,
        });
        a.push(Action {
            label: "Spawn terminal ($SHELL)".into(),
            hint: "t".into(),
            group: "general",
            kind: ActionKind::SpawnTerminal,
        });
        a.push(Action {
            label: "New session…  (create agent)".into(),
            hint: "n".into(),
            group: "sessions",
            kind: ActionKind::NewSession,
        });

        // Focus shortcuts.
        a.push(Action {
            label: "Focus: Tree".into(),
            hint: "1".into(),
            group: "focus",
            kind: ActionKind::FocusTree,
        });
        a.push(Action {
            label: "Focus: Terminal".into(),
            hint: "2".into(),
            group: "focus",
            kind: ActionKind::FocusTerm,
        });
        if lazygit_open {
            a.push(Action {
                label: "Focus: Lazygit".into(),
                hint: "3".into(),
                group: "focus",
                kind: ActionKind::FocusLazygit,
            });
        }

        // Lazygit.
        a.push(Action {
            label: if lazygit_open {
                "Lazygit: close".into()
            } else {
                "Lazygit: open side pane".into()
            },
            hint: "g".into(),
            group: "extensions",
            kind: ActionKind::ToggleLazygit,
        });
        a.push(Action {
            label: "Lazygit: cheat sheet".into(),
            hint: "G".into(),
            group: "extensions",
            kind: ActionKind::LazygitCheats,
        });

        // View toggles. Each entry shows current state inline so the
        // user can read off what's on without flipping it first.
        let onoff = |b: bool| if b { "on" } else { "off" };
        a.push(Action {
            label: format!("View: sidebar [{}]", onoff(!view.sidebar_hidden)),
            hint: "Ctrl-B".into(),
            group: "view",
            kind: ActionKind::ToggleSidebar,
        });
        a.push(Action {
            label: format!("View: agent panel [{}]", onoff(view.right_panel_visible)),
            hint: "Ctrl-T".into(),
            group: "view",
            kind: ActionKind::ToggleRightPanel,
        });
        a.push(Action {
            label: format!("View: fullscreen [{}]", onoff(view.fullscreen)),
            hint: "Shift-F · Ctrl-K Z".into(),
            group: "view",
            kind: ActionKind::ToggleFullscreen,
        });
        a.push(Action {
            label: format!("View: split terminal [{}]", onoff(view.split_open)),
            hint: "Ctrl-\\".into(),
            group: "view",
            kind: ActionKind::ToggleSplit,
        });
        // Servers section visibility — when collapsed the sidebar
        // shows only the section header; expanded shows every profile
        // row. Independent of `Ctrl-B`, which hides the whole sidebar.
        a.push(Action {
            label: format!("View: servers section [{}]", onoff(!view.servers_collapsed)),
            hint: "Ctrl-K V".into(),
            group: "view",
            kind: ActionKind::ToggleServers,
        });
        // Tree scope — show every reachable server's sessions
        // (default, recommended) vs scope to just the active server.
        // Calling out the recommendation in-label so a user browsing
        // the palette knows which way to land.
        a.push(Action {
            label: format!(
                "View: show all servers' sessions [{}] (recommended on)",
                onoff(view.show_all_servers)
            ),
            hint: "Ctrl-S · s".into(),
            group: "view",
            kind: ActionKind::ToggleShowAllServers,
        });

        // Status-bar settings — one entry per chip. Renders the chip
        // label and current value so the user can see what each
        // does before flipping it.
        for chip in StatusChip::ALL {
            a.push(Action {
                label: format!("Status bar: {} [{}]", chip.label(), onoff(prefs.get(*chip))),
                hint: "".into(),
                group: "settings",
                kind: ActionKind::ToggleStatusChip(*chip),
            });
        }
        a.push(Action {
            label: "Status bar: reset to defaults".into(),
            hint: "".into(),
            group: "settings",
            kind: ActionKind::ResetStatusBar,
        });

        // ---- Settings overlay & notification toggles ------------------
        // Pinned at the top of the Settings group so `~` users see them
        // first.  Sound + TTL toggles reuse the prefs helpers so the
        // palette and Settings overlay stay in lockstep automatically.
        a.push(Action {
            label: "Settings…  (open overlay)".into(),
            hint: "Ctrl-,".into(),
            group: "settings",
            kind: ActionKind::OpenSettings,
        });
        // Server switcher — palette parity with Ctrl-S so users
        // typing "server" / "profile" / "switch server" find it.
        a.push(Action {
            label: "Switch server…  (agentum daemon)".into(),
            hint: "Ctrl-S".into(),
            group: "general",
            kind: ActionKind::OpenProfiles,
        });
        a.push(Action {
            label: "Hosts…  (SSH readiness)".into(),
            hint: "Ctrl-H".into(),
            group: "general",
            kind: ActionKind::OpenHosts,
        });
        a.push(Action {
            label: format!("Sound: master [{}]", onoff(prefs.sound_master)),
            hint: "".into(),
            group: "settings",
            kind: ActionKind::ToggleSoundMaster,
        });
        for kind in super::prefs::SoundKind::ALL {
            a.push(Action {
                label: format!(
                    "Sound: {} [{}]",
                    kind.label(),
                    onoff(prefs.sound_kind_on(*kind))
                ),
                hint: "".into(),
                group: "settings",
                kind: ActionKind::ToggleSoundKind(*kind),
            });
        }
        for kind in super::prefs::SoundKind::ALL {
            let secs = prefs.ttl_ms(*kind) as f64 / 1000.0;
            a.push(Action {
                label: format!("Notification TTL: {} − 0.5s ({:.1}s)", kind.label(), secs),
                hint: "".into(),
                group: "settings",
                kind: ActionKind::BumpTtl(*kind, -(super::prefs::NOTIF_TTL_STEP_MS as i64)),
            });
            a.push(Action {
                label: format!("Notification TTL: {} + 0.5s ({:.1}s)", kind.label(), secs),
                hint: "".into(),
                group: "settings",
                kind: ActionKind::BumpTtl(*kind, super::prefs::NOTIF_TTL_STEP_MS as i64),
            });
        }
        a.push(Action {
            label: "Settings: reset everything to defaults".into(),
            hint: "".into(),
            group: "settings",
            kind: ActionKind::ResetAllPrefs,
        });

        // Themes.
        for t in theme::all() {
            a.push(Action {
                label: format!("Theme: {}", t.name),
                hint: "".into(),
                group: "appearance",
                kind: ActionKind::SetTheme(t.name),
            });
        }
        a.push(Action {
            label: "Theme: cycle".into(),
            hint: "T".into(),
            group: "appearance",
            kind: ActionKind::CycleTheme,
        });

        // Sessions. Each session contributes entries for select, rename,
        // start/stop (based on current status), and kill — full CRUD from
        // the palette without forcing the user to navigate the tree first.
        // They share the `sessions` group so the `#` prefix surfaces all
        // of them.
        //
        // The currently-selected session's kill entry is already pinned
        // at the top (above), so we skip emitting it again here — keeps
        // the catalogue tidy and stops the active session's kill from
        // showing up twice.
        for (id, name, workdir, is_running) in sessions {
            a.push(Action {
                label: format!("Session: {name}"),
                hint: workdir.clone(),
                group: "sessions",
                kind: ActionKind::SelectSession(*id),
            });
            a.push(Action {
                label: format!("Rename session: {name}"),
                hint: "Ctrl-R".into(),
                group: "sessions",
                kind: ActionKind::RenameSession(*id),
            });
            if *is_running {
                a.push(Action {
                    label: format!("Stop session: {name}"),
                    hint: "s · Shift-S".into(),
                    group: "sessions",
                    kind: ActionKind::StopSession(*id),
                });
            } else {
                a.push(Action {
                    label: format!("Start session: {name}"),
                    hint: "u · Shift-U".into(),
                    group: "sessions",
                    kind: ActionKind::StartSession(*id),
                });
            }
            if Some(*id) == selected {
                continue;
            }
            a.push(Action {
                label: format!("Kill session: {name}"),
                hint: "Shift-K · x · Shift-D".into(),
                group: "sessions",
                kind: ActionKind::KillSession(*id),
            });
        }

        Catalog { actions: a }
    }

    /// Apply prefix routing + subsequence filter. Returns the slice of
    /// actions matching the active mode and query in order. Whitespace-
    /// separated tokens in the query each must match (Fresh's behaviour).
    pub fn filtered<'a>(&'a self, raw_query: &str) -> (Mode, Vec<&'a Action>) {
        let (mode, rest) = Mode::from_query(raw_query);
        let needle = rest.trim().to_lowercase();
        let tokens: Vec<&str> = needle.split_whitespace().collect();
        let out: Vec<&Action> = self
            .actions
            .iter()
            .filter(|a| {
                if !mode.keep(a.group) {
                    return false;
                }
                if tokens.is_empty() {
                    return true;
                }
                let hay = format!("{} {} {}", a.group, a.label, a.hint).to_lowercase();
                tokens.iter().all(|t| subsequence_match(&hay, t))
            })
            .collect();
        (mode, out)
    }
}

fn subsequence_match(haystack: &str, needle: &str) -> bool {
    let mut hay = haystack.chars();
    'outer: for n in needle.chars() {
        for h in hay.by_ref() {
            if h == n {
                continue 'outer;
            }
        }
        return false;
    }
    true
}
