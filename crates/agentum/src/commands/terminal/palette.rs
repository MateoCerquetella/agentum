//! Command palette — VSCode-style action picker.
//!
//! Opened with Ctrl-P or Ctrl-K. Type to filter, ↑/↓ to move, Enter to
//! run. Actions are produced fresh each frame so dynamic entries (active
//! sessions, theme registry) reflect current state. Filtering is a cheap
//! case-insensitive subsequence match so "thmid" matches "Theme: midnight".

use uuid::Uuid;

use super::theme;

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
    FocusTree,
    FocusTerm,
    FocusInput,
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
        a.push(Action {
            label: "Focus: Input".into(),
            hint: "3".into(),
            group: "focus",
            kind: ActionKind::FocusInput,
        });
        if lazygit_open {
            a.push(Action {
                label: "Focus: Lazygit".into(),
                hint: "4".into(),
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

    /// Subsequence-match filter. Returns indices into `self.actions` so
    /// the caller can render with original indexes preserved if needed.
    pub fn filtered<'a>(&'a self, query: &str) -> Vec<&'a Action> {
        if query.trim().is_empty() {
            return self.actions.iter().collect();
        }
        let needle: String = query.to_lowercase();
        self.actions
            .iter()
            .filter(|a| {
                let hay = format!("{} {} {}", a.group, a.label, a.hint).to_lowercase();
                subsequence_match(&hay, &needle)
            })
            .collect()
    }
}

fn subsequence_match(haystack: &str, needle: &str) -> bool {
    let mut hay = haystack.chars();
    'outer: for n in needle.chars() {
        if n.is_whitespace() {
            continue;
        }
        for h in hay.by_ref() {
            if h == n {
                continue 'outer;
            }
        }
        return false;
    }
    true
}
