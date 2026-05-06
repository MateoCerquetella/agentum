//! Persistent TUI preferences — what to show on the status bar.
//!
//! Lives next to the existing `theme` file under `data_dir()`. The on-disk
//! representation is TOML so users can hand-edit if they want. Bad / missing
//! files fall back to defaults silently — preferences are convenience, not
//! correctness, and crashing the whole TUI over a missing file would feel
//! spiteful.
//!
//! Each chip is opt-out so first-launch matches the historical look. The
//! palette exposes a per-chip toggle action and writes the file on every
//! change, so the next launch comes up the way the user left it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusChip {
    Workdir,
    Tool,
    Conn,
    Lazygit,
    Theme,
    Io,
    IoTotals,
    PaletteHint,
    HelpHint,
}

impl StatusChip {
    pub const ALL: &'static [Self] = &[
        Self::Workdir,
        Self::Tool,
        Self::Conn,
        Self::Lazygit,
        Self::Theme,
        Self::Io,
        Self::IoTotals,
        Self::PaletteHint,
        Self::HelpHint,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Workdir => "workdir",
            Self::Tool => "tool",
            Self::Conn => "connection",
            Self::Lazygit => "lazygit",
            Self::Theme => "theme",
            Self::Io => "I/O speeds",
            Self::IoTotals => "I/O totals",
            Self::PaletteHint => "palette hint",
            Self::HelpHint => "help hint",
        }
    }
}

/// Mirror of the disk file. Keep field names stable — they're the TOML
/// keys users will see if they peek at the file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Prefs {
    pub show_workdir: bool,
    pub show_tool: bool,
    pub show_conn: bool,
    pub show_lazygit: bool,
    pub show_theme: bool,
    pub show_io: bool,
    pub show_io_totals: bool,
    pub show_palette_hint: bool,
    pub show_help_hint: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            show_workdir: true,
            show_tool: true,
            show_conn: true,
            show_lazygit: true,
            show_theme: true,
            // I/O speeds default ON so the user sees the new feature on
            // first launch without needing to discover it. They can flip
            // it off through the palette if they don't want it.
            show_io: true,
            // Lifetime totals are quieter — opt-in. Most users will only
            // want the live rate.
            show_io_totals: false,
            show_palette_hint: true,
            show_help_hint: true,
        }
    }
}

impl Prefs {
    pub fn get(&self, chip: StatusChip) -> bool {
        match chip {
            StatusChip::Workdir => self.show_workdir,
            StatusChip::Tool => self.show_tool,
            StatusChip::Conn => self.show_conn,
            StatusChip::Lazygit => self.show_lazygit,
            StatusChip::Theme => self.show_theme,
            StatusChip::Io => self.show_io,
            StatusChip::IoTotals => self.show_io_totals,
            StatusChip::PaletteHint => self.show_palette_hint,
            StatusChip::HelpHint => self.show_help_hint,
        }
    }

    pub fn set(&mut self, chip: StatusChip, on: bool) {
        match chip {
            StatusChip::Workdir => self.show_workdir = on,
            StatusChip::Tool => self.show_tool = on,
            StatusChip::Conn => self.show_conn = on,
            StatusChip::Lazygit => self.show_lazygit = on,
            StatusChip::Theme => self.show_theme = on,
            StatusChip::Io => self.show_io = on,
            StatusChip::IoTotals => self.show_io_totals = on,
            StatusChip::PaletteHint => self.show_palette_hint = on,
            StatusChip::HelpHint => self.show_help_hint = on,
        }
    }

    pub fn toggle(&mut self, chip: StatusChip) -> bool {
        let next = !self.get(chip);
        self.set(chip, next);
        next
    }
}

fn prefs_file() -> Option<PathBuf> {
    let dir = agentum_store::paths::data_dir().ok()?;
    Some(dir.join("tui_prefs.toml"))
}

pub fn load() -> Prefs {
    let Some(path) = prefs_file() else {
        return Prefs::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Prefs::default();
    };
    toml::from_str::<Prefs>(&raw).unwrap_or_default()
}

pub fn save(prefs: &Prefs) {
    let Some(path) = prefs_file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(s) = toml::to_string_pretty(prefs) {
        let _ = std::fs::write(&path, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_io_on_totals_off() {
        let p = Prefs::default();
        assert!(p.show_io);
        assert!(!p.show_io_totals);
        assert!(p.show_workdir);
    }

    #[test]
    fn toggle_round_trips() {
        let mut p = Prefs::default();
        let was = p.get(StatusChip::Io);
        let now = p.toggle(StatusChip::Io);
        assert_ne!(was, now);
        assert_eq!(p.get(StatusChip::Io), now);
    }

    #[test]
    fn round_trip_through_toml() {
        let mut p = Prefs::default();
        p.set(StatusChip::Io, false);
        p.set(StatusChip::IoTotals, true);
        let s = toml::to_string_pretty(&p).expect("serialize");
        let r: Prefs = toml::from_str(&s).expect("deserialize");
        assert!(!r.show_io);
        assert!(r.show_io_totals);
    }

    #[test]
    fn missing_keys_use_defaults() {
        // An older config file from before we added I/O fields should
        // still load — `serde(default)` fills in the gaps.
        let r: Prefs = toml::from_str("show_workdir = false").expect("deserialize");
        assert!(!r.show_workdir);
        // Newer fields default in.
        assert!(r.show_io);
    }
}
