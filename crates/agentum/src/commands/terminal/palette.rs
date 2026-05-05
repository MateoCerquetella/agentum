//! Command palette — Fresh-IDE-style prefix-routed picker.
//!
//! Opened with Ctrl-P or Ctrl-K. Prefix routing matches Fresh:
//!
//!   (no prefix)  fuzzy across everything (default)
//!   `>`          commands only (focus, theme, lazygit, refresh, quit)
//!   `#`          sessions only — like Fresh's buffer switcher
//!   `@`          themes only
//!
//! Type to filter, ↑/↓ to move, Enter to run. Catalogue is rebuilt every
//! frame so dynamic entries (sessions, themes) stay current. Filtering
//! is a cheap case-insensitive subsequence match so "thmid" matches
//! "Theme: midnight".

use uuid::Uuid;

use super::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    All,
    Commands,
    Sessions,
    Themes,
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
            _ => (Self::All, raw),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Commands => "commands",
            Self::Sessions => "sessions",
            Self::Themes => "themes",
        }
    }

    pub fn keep(self, group: &str) -> bool {
        match self {
            Self::All => true,
            Self::Commands => matches!(group, "general" | "focus" | "extensions" | "appearance"),
            Self::Sessions => group == "sessions",
            Self::Themes => group == "appearance",
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
}

pub struct Catalog {
    pub actions: Vec<Action>,
}

impl Catalog {
    /// Build the live action list. Pure — call from a draw or key handler.
    pub fn build(
        lazygit_open: bool,
        sessions: &[(Uuid, String, String)], // (id, name, workdir)
    ) -> Self {
        let mut a = Vec::with_capacity(32);

        a.push(Action {
            label: "Quit agentum".into(),
            hint: "q · Ctrl-C".into(),
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
            label: "Refresh sessions".into(),
            hint: "r".into(),
            group: "general",
            kind: ActionKind::Refresh,
        });
        a.push(Action {
            label: "Spawn plain terminal (bash)".into(),
            hint: "t".into(),
            group: "general",
            kind: ActionKind::SpawnTerminal,
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

        // Sessions.
        for (id, name, workdir) in sessions {
            a.push(Action {
                label: format!("Session: {name}"),
                hint: workdir.clone(),
                group: "sessions",
                kind: ActionKind::SelectSession(*id),
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
