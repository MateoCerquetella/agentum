//! Themes — name-keyed palette registry with real backgrounds.
//!
//! Six built-ins: `system` (default — inherits the host terminal's colour
//! scheme exactly the way alacritty / opencode do), `midnight` (Tokyo
//! Night-ish), `dusk` (One Dark warm charcoal), `slate` (cool charcoal +
//! neon cyan), `paper` (Solarized-light-ish), `mono` (high-contrast B/W).
//! Persisted in `~/.local/share/agentum/theme`, overridable via
//! `$AGENTUM_THEME`. `T` cycles forward; the command palette
//! (Ctrl-P / Ctrl-Shift-P) lets you pick by name. Everything ui.rs draws goes
//! through `Palette` — there are no hardcoded `Color::*` in draw code.
//!
//! ## How `system` works
//!
//! Backgrounds resolve to `Color::Reset` so the host terminal's actual
//! background paints through — if you've set Alacritty / iTerm /
//! WezTerm / Ghostty to a custom bg, agentum just inherits it. Foreground
//! / accent slots use the **named** ANSI colours (Cyan, Yellow, …),
//! which terminals colourise from their own scheme. Result: change your
//! terminal theme and agentum follows automatically. Same model
//! alacritty itself uses for its UI chrome and the one
//! [opencode](https://github.com/sst/opencode) ships under "system".

use std::path::PathBuf;

use ratatui::style::Color;

/// Concrete palette consumed by `ui.rs`. Every visible color goes through
/// here, including the panel backgrounds — so themes actually look like
/// themes, not just border-color swaps.
#[derive(Clone, Copy)]
pub struct Palette {
    // Layered backgrounds. `body_bg` paints the void around panels;
    // `panel_bg` is the inside of each pane block; `surface_bg` is for
    // raised UI like the input bar and cursor row; `chrome_bg` is the
    // status bar.
    pub body_bg: Color,
    pub panel_bg: Color,
    pub surface_bg: Color,
    pub chrome_bg: Color,

    pub fg: Color,
    pub fg_strong: Color,
    pub muted: Color,
    pub subtle: Color,

    pub accent: Color,
    pub accent_alt: Color,
    pub idle_border: Color,
    pub focus_border: Color,

    pub cursor_bg: Color,
    pub cursor_fg: Color,

    pub success: Color,
    pub warning: Color,
    pub error: Color,

    pub chip_bg: Color,
    pub chip_fg: Color,
}

pub struct Theme {
    pub name: &'static str,
    pub palette: Palette,
}

impl Theme {
    /// Resolve a name to a built-in theme. Falls back to `midnight` for
    /// unknown names (including the special "system" sentinel — that's
    /// turned into a concrete name before getting here).
    pub fn by_name(name: &str) -> &'static Theme {
        BUILTINS
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .unwrap_or(&BUILTINS[0])
    }

    /// Cycle to the next theme in the registry.
    pub fn next(current: &str) -> &'static Theme {
        let idx = BUILTINS
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(current))
            .unwrap_or(0);
        &BUILTINS[(idx + 1) % BUILTINS.len()]
    }
}

pub fn all() -> &'static [Theme] {
    BUILTINS
}

// ---------- built-in palettes ----------
//
// Order matters: the first entry is the default and the cycle starts here.

const BUILTINS: &[Theme] = &[
    // `system` — inherit the host terminal's palette. All backgrounds are
    // `Color::Reset` (the host bg shows through), and foreground / accent
    // slots use named ANSI colours that the terminal renders from its own
    // 16-colour scheme. Change your terminal theme → agentum follows.
    // This is the same approach alacritty uses for its own UI and the
    // model opencode ships under the "system" name.
    Theme {
        name: "system",
        palette: Palette {
            body_bg: Color::Reset,
            panel_bg: Color::Reset,
            surface_bg: Color::Reset,
            chrome_bg: Color::Reset,

            fg: Color::Reset,
            fg_strong: Color::White,
            muted: Color::DarkGray,
            subtle: Color::DarkGray,

            accent: Color::Cyan,
            accent_alt: Color::Magenta,
            idle_border: Color::DarkGray,
            focus_border: Color::Cyan,

            cursor_bg: Color::DarkGray,
            cursor_fg: Color::White,

            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,

            chip_bg: Color::DarkGray,
            chip_fg: Color::White,
        },
    },
    Theme {
        name: "midnight",
        palette: Palette {
            body_bg: Color::Rgb(13, 17, 28),
            panel_bg: Color::Rgb(17, 22, 35),
            surface_bg: Color::Rgb(24, 30, 47),
            chrome_bg: Color::Rgb(11, 14, 23),

            fg: Color::Rgb(196, 203, 224),
            fg_strong: Color::Rgb(232, 236, 248),
            muted: Color::Rgb(115, 124, 153),
            subtle: Color::Rgb(72, 82, 110),

            accent: Color::Rgb(122, 162, 247),
            accent_alt: Color::Rgb(187, 154, 247),
            idle_border: Color::Rgb(40, 50, 75),
            focus_border: Color::Rgb(122, 162, 247),

            cursor_bg: Color::Rgb(40, 56, 95),
            cursor_fg: Color::Rgb(232, 236, 248),

            success: Color::Rgb(158, 206, 106),
            warning: Color::Rgb(224, 175, 104),
            error: Color::Rgb(247, 118, 142),

            chip_bg: Color::Rgb(45, 38, 70),
            chip_fg: Color::Rgb(206, 188, 247),
        },
    },
    Theme {
        name: "dusk",
        palette: Palette {
            body_bg: Color::Rgb(30, 32, 38),
            panel_bg: Color::Rgb(40, 44, 52),
            surface_bg: Color::Rgb(50, 55, 64),
            chrome_bg: Color::Rgb(24, 26, 31),

            fg: Color::Rgb(190, 195, 207),
            fg_strong: Color::Rgb(230, 233, 240),
            muted: Color::Rgb(125, 134, 152),
            subtle: Color::Rgb(78, 84, 96),

            accent: Color::Rgb(97, 175, 239),
            accent_alt: Color::Rgb(198, 120, 221),
            idle_border: Color::Rgb(60, 66, 78),
            focus_border: Color::Rgb(97, 175, 239),

            cursor_bg: Color::Rgb(56, 70, 92),
            cursor_fg: Color::Rgb(230, 233, 240),

            success: Color::Rgb(152, 195, 121),
            warning: Color::Rgb(229, 192, 123),
            error: Color::Rgb(224, 108, 117),

            chip_bg: Color::Rgb(70, 50, 92),
            chip_fg: Color::Rgb(214, 178, 239),
        },
    },
    Theme {
        name: "slate",
        palette: Palette {
            body_bg: Color::Rgb(8, 10, 14),
            panel_bg: Color::Rgb(14, 17, 22),
            surface_bg: Color::Rgb(22, 27, 35),
            chrome_bg: Color::Rgb(5, 7, 10),

            fg: Color::Rgb(200, 210, 220),
            fg_strong: Color::Rgb(240, 246, 252),
            muted: Color::Rgb(110, 120, 132),
            subtle: Color::Rgb(60, 70, 82),

            accent: Color::Rgb(100, 224, 224),
            accent_alt: Color::Rgb(255, 121, 198),
            idle_border: Color::Rgb(35, 42, 52),
            focus_border: Color::Rgb(100, 224, 224),

            cursor_bg: Color::Rgb(20, 70, 80),
            cursor_fg: Color::Rgb(240, 246, 252),

            success: Color::Rgb(80, 250, 123),
            warning: Color::Rgb(241, 250, 140),
            error: Color::Rgb(255, 85, 85),

            chip_bg: Color::Rgb(40, 24, 56),
            chip_fg: Color::Rgb(255, 184, 218),
        },
    },
    Theme {
        name: "paper",
        palette: Palette {
            body_bg: Color::Rgb(248, 246, 240),
            panel_bg: Color::Rgb(253, 252, 247),
            surface_bg: Color::Rgb(244, 240, 230),
            chrome_bg: Color::Rgb(232, 228, 218),

            fg: Color::Rgb(40, 42, 46),
            fg_strong: Color::Rgb(15, 17, 20),
            muted: Color::Rgb(120, 120, 116),
            subtle: Color::Rgb(180, 178, 170),

            accent: Color::Rgb(0, 95, 175),
            accent_alt: Color::Rgb(160, 70, 130),
            idle_border: Color::Rgb(200, 195, 180),
            focus_border: Color::Rgb(0, 95, 175),

            cursor_bg: Color::Rgb(225, 232, 244),
            cursor_fg: Color::Rgb(15, 17, 20),

            success: Color::Rgb(0, 120, 50),
            warning: Color::Rgb(180, 110, 0),
            error: Color::Rgb(190, 30, 30),

            chip_bg: Color::Rgb(228, 218, 240),
            chip_fg: Color::Rgb(80, 40, 130),
        },
    },
    Theme {
        name: "mono",
        palette: Palette {
            body_bg: Color::Rgb(0, 0, 0),
            panel_bg: Color::Rgb(8, 8, 8),
            surface_bg: Color::Rgb(20, 20, 20),
            chrome_bg: Color::Rgb(0, 0, 0),

            fg: Color::Rgb(220, 220, 220),
            fg_strong: Color::Rgb(255, 255, 255),
            muted: Color::Rgb(140, 140, 140),
            subtle: Color::Rgb(80, 80, 80),

            accent: Color::Rgb(255, 255, 255),
            accent_alt: Color::Rgb(200, 200, 200),
            idle_border: Color::Rgb(60, 60, 60),
            focus_border: Color::Rgb(255, 255, 255),

            cursor_bg: Color::Rgb(50, 50, 50),
            cursor_fg: Color::Rgb(255, 255, 255),

            success: Color::Rgb(220, 220, 220),
            warning: Color::Rgb(220, 220, 220),
            error: Color::Rgb(255, 255, 255),

            chip_bg: Color::Rgb(40, 40, 40),
            chip_fg: Color::Rgb(255, 255, 255),
        },
    },
];

// ---------- persistence ----------

fn theme_file() -> Option<PathBuf> {
    let dir = agentum_store::paths::data_dir().ok()?;
    Some(dir.join("theme"))
}

pub fn load() -> &'static Theme {
    // `auto` is accepted as an alias for `system` for back-compat with
    // older saved files / scripts.
    let normalize = |s: &str| -> String {
        let t = s.trim();
        if t.eq_ignore_ascii_case("auto") {
            "system".into()
        } else {
            t.to_string()
        }
    };
    if let Ok(s) = std::env::var("AGENTUM_THEME") {
        return Theme::by_name(&normalize(&s));
    }
    if let Some(path) = theme_file()
        && let Ok(raw) = std::fs::read_to_string(&path)
    {
        return Theme::by_name(&normalize(&raw));
    }
    Theme::by_name("system")
}

pub fn save(name: &str) {
    let Some(path) = theme_file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{name}\n"));
}
