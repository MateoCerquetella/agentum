//! Color palette + persistence for the terminal dashboard.
//!
//! Three modes — `dark`, `light`, and `system` — picked at startup from
//! `~/.local/share/agentum/theme` (or `$AGENTUM_THEME`) and cycled at runtime
//! with `T`. `system` is best-effort: we sniff `COLORFGBG` (set by VTE / iTerm
//! when the user has a light scheme) and fall back to dark.

use std::path::PathBuf;

use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
    System,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
            ThemeMode::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "system" | "auto" => Some(Self::System),
            _ => None,
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::System,
            ThemeMode::System => ThemeMode::Dark,
        }
    }

    /// Resolve `System` to a concrete palette. Best-effort — on terminals
    /// that don't advertise their scheme we default to dark.
    fn resolved(self) -> ThemeMode {
        match self {
            ThemeMode::System => detect_system(),
            other => other,
        }
    }
}

fn detect_system() -> ThemeMode {
    // COLORFGBG="<fg>;<bg>" — bg ≥ 8 is conventionally a light background.
    if let Ok(s) = std::env::var("COLORFGBG")
        && let Some(bg) = s.split(';').nth(1)
        && let Ok(n) = bg.trim().parse::<u32>()
        && (7..=15).contains(&n)
    {
        return ThemeMode::Light;
    }
    ThemeMode::Dark
}

/// Concrete palette consumed by `ui.rs`. All colors flow through this struct
/// — no hardcoded `Color::*` should remain in draw functions.
#[derive(Clone, Copy)]
pub struct Palette {
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub idle_border: Color,
    pub focus_border: Color,
    pub cursor_bg: Color,
    pub status_bar_bg: Color,
    pub status_bar_fg: Color,
    pub title_fg: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub chip_active_bg: Color,
    pub chip_active_fg: Color,
}

pub struct Theme {
    pub mode: ThemeMode,
    pub palette: Palette,
}

impl Theme {
    pub fn new(mode: ThemeMode) -> Self {
        let palette = match mode.resolved() {
            ThemeMode::Light => light(),
            _ => dark(),
        };
        Self { mode, palette }
    }
}

fn dark() -> Palette {
    Palette {
        fg: Color::White,
        muted: Color::DarkGray,
        accent: Color::Cyan,
        idle_border: Color::DarkGray,
        focus_border: Color::Cyan,
        cursor_bg: Color::Rgb(40, 60, 70),
        status_bar_bg: Color::Rgb(20, 25, 35),
        status_bar_fg: Color::Gray,
        title_fg: Color::White,
        success: Color::Green,
        warning: Color::Yellow,
        error: Color::Red,
        chip_active_bg: Color::Rgb(60, 30, 70),
        chip_active_fg: Color::White,
    }
}

fn light() -> Palette {
    Palette {
        fg: Color::Rgb(20, 22, 28),
        muted: Color::Rgb(120, 124, 132),
        accent: Color::Rgb(0, 95, 175),
        idle_border: Color::Rgb(180, 184, 192),
        focus_border: Color::Rgb(0, 95, 175),
        cursor_bg: Color::Rgb(220, 230, 245),
        status_bar_bg: Color::Rgb(232, 234, 240),
        status_bar_fg: Color::Rgb(40, 45, 55),
        title_fg: Color::Rgb(20, 22, 28),
        success: Color::Rgb(0, 120, 50),
        warning: Color::Rgb(180, 110, 0),
        error: Color::Rgb(190, 30, 30),
        chip_active_bg: Color::Rgb(220, 210, 235),
        chip_active_fg: Color::Rgb(60, 30, 110),
    }
}

// ---------- persistence ----------

fn theme_file() -> Option<PathBuf> {
    let dir = agentum_store::paths::data_dir().ok()?;
    Some(dir.join("theme"))
}

pub fn load() -> ThemeMode {
    if let Ok(s) = std::env::var("AGENTUM_THEME")
        && let Some(m) = ThemeMode::parse(&s)
    {
        return m;
    }
    if let Some(path) = theme_file()
        && let Ok(raw) = std::fs::read_to_string(&path)
        && let Some(m) = ThemeMode::parse(&raw)
    {
        return m;
    }
    ThemeMode::System
}

pub fn save(mode: ThemeMode) {
    let Some(path) = theme_file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{}\n", mode.label()));
}
