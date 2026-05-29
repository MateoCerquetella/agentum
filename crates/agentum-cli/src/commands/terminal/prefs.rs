//! Persistent TUI preferences — status-bar chips, layout sizes, panel
//! visibility, and notification behaviour.
//!
//! Lives next to the existing `theme` file under `data_dir()`. The on-disk
//! representation is TOML so users can hand-edit if they want. Bad / missing
//! files fall back to defaults silently — preferences are convenience, not
//! correctness, and crashing the whole TUI over a missing file would feel
//! spiteful.
//!
//! Status-bar chips are opt-out so first-launch matches the historical
//! look. Layout sizes default to the same values App::new used to hardcode
//! (tree 32, lazygit 60, split 50/50) so existing users see no change after
//! upgrade. Notification TTLs use the NOTIF_TTL_*_DEFAULT_MS constants.
//!
//! Both the command palette and the Settings overlay write the file on
//! every change, so the next launch comes up the way the user left it.

use std::path::PathBuf;
use std::time::Duration;

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

/// Severity for a toast — duplicated here from `app::NotifKind` so this
/// module stays free of upward dependencies. Mapped 1:1 at every call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    Info,
    Warn,
    Error,
}

impl SoundKind {
    pub const ALL: &'static [Self] = &[Self::Info, Self::Warn, Self::Error];
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Hard floor / ceiling for a notification TTL. 1s is short enough to feel
/// like "barely flashed up"; 30s is the longest a toast can hang around
/// before it starts feeling like a permanent banner.
pub const NOTIF_TTL_MIN_MS: u64 = 1000;
pub const NOTIF_TTL_MAX_MS: u64 = 30_000;
/// Step size for the TTL +/- controls in the Settings overlay and palette.
pub const NOTIF_TTL_STEP_MS: u64 = 500;
/// Default TTLs per severity, mirroring `dashboard/src/lib/stores/events.ts`.
pub const NOTIF_TTL_INFO_DEFAULT_MS: u64 = 6000;
pub const NOTIF_TTL_WARN_DEFAULT_MS: u64 = 4000;
pub const NOTIF_TTL_ERROR_DEFAULT_MS: u64 = 12_000;

/// How often the TUI re-polls `/api/usage/claude` for the bottom-left
/// readout (spec 001). 60s default; floored at [`USAGE_REFRESH_MIN_SECS`]
/// at every read site because the usage endpoint is itself rate-limitable
/// and a too-low value could self-throttle the account.
pub const USAGE_REFRESH_DEFAULT_SECS: u64 = 60;
pub const USAGE_REFRESH_MIN_SECS: u64 = 30;

/// Mirror of the disk file. Keep field names stable — they're the TOML
/// keys users will see if they peek at the file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Prefs {
    // ---- status-bar chips ----------------------------------------------
    pub show_workdir: bool,
    pub show_tool: bool,
    pub show_conn: bool,
    pub show_lazygit: bool,
    pub show_theme: bool,
    pub show_io: bool,
    pub show_io_totals: bool,
    pub show_palette_hint: bool,
    pub show_help_hint: bool,

    // ---- layout (previously hardcoded in App::new) ---------------------
    pub tree_width: u16,
    pub lazygit_width: u16,
    pub term_split_pct: u16,
    pub sidebar_hidden: bool,
    pub right_panel_visible: bool,
    /// Collapse the SERVERS section in the tree sidebar. The section
    /// still renders a single-line header (so the user can find it),
    /// but its rows are hidden and j/k skip it during navigation.
    /// Persisted because the choice survives session restarts — most
    /// users either want the section in their face or not at all.
    pub servers_collapsed: bool,
    /// Show sessions from every reachable server in the sidebar tree
    /// (default), versus scoping the tree to just the active server.
    /// Default ON is the recommended setup: the TUI already fans out
    /// to every configured profile at startup, and showing the whole
    /// fleet in one tree is the feature multi-server users boot the
    /// TUI for. Flipping it off when the fleet is noisy lets a user
    /// concentrate on one server without losing access to the others
    /// (the SERVERS section still lists every profile so Enter can
    /// retarget).
    pub show_all_servers: bool,

    // ---- notifications -------------------------------------------------
    /// Master sound switch. When `false`, no notification sounds play
    /// regardless of the per-kind toggles below. Layered with the
    /// `--no-sound` CLI override (CLI wins when set).
    pub sound_master: bool,
    pub sound_info: bool,
    pub sound_warn: bool,
    pub sound_error: bool,
    /// Toast lifetimes in milliseconds, clamped 1000..=30000 at every
    /// read site so a hand-edited file with a bogus value still produces
    /// usable timing.
    pub notif_ttl_info_ms: u64,
    pub notif_ttl_warn_ms: u64,
    pub notif_ttl_error_ms: u64,

    // ---- usage readout (spec 001) --------------------------------------
    /// Seconds between `/api/usage/claude` polls for the bottom-left
    /// readout. Read via [`Prefs::usage_refresh`], which clamps to
    /// [`USAGE_REFRESH_MIN_SECS`] so a hand-edited file can't drive the
    /// poll under the safe floor.
    pub usage_refresh_secs: u64,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            show_workdir: true,
            show_tool: true,
            show_conn: true,
            show_lazygit: true,
            show_theme: true,
            show_io: true,
            show_io_totals: false,
            show_palette_hint: true,
            show_help_hint: true,

            tree_width: 32,
            lazygit_width: 60,
            term_split_pct: 50,
            sidebar_hidden: false,
            right_panel_visible: true,
            servers_collapsed: false,
            show_all_servers: true,

            sound_master: true,
            sound_info: true,
            sound_warn: true,
            sound_error: true,
            notif_ttl_info_ms: NOTIF_TTL_INFO_DEFAULT_MS,
            notif_ttl_warn_ms: NOTIF_TTL_WARN_DEFAULT_MS,
            notif_ttl_error_ms: NOTIF_TTL_ERROR_DEFAULT_MS,

            usage_refresh_secs: USAGE_REFRESH_DEFAULT_SECS,
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

    /// Effective TTL for a notification of the given severity. Clamped to
    /// the safe range so a bad on-disk value can't produce an instantly
    /// expiring (or never-expiring) toast.
    pub fn ttl_for(&self, kind: SoundKind) -> Duration {
        Duration::from_millis(self.ttl_ms(kind))
    }

    /// Effective usage-readout poll interval. Floored at
    /// [`USAGE_REFRESH_MIN_SECS`] so a hand-edited file can't drive the
    /// poll cadence below the rate-limit-safe minimum.
    pub fn usage_refresh(&self) -> Duration {
        Duration::from_secs(self.usage_refresh_secs.max(USAGE_REFRESH_MIN_SECS))
    }

    pub fn ttl_ms(&self, kind: SoundKind) -> u64 {
        let raw = match kind {
            SoundKind::Info => self.notif_ttl_info_ms,
            SoundKind::Warn => self.notif_ttl_warn_ms,
            SoundKind::Error => self.notif_ttl_error_ms,
        };
        raw.clamp(NOTIF_TTL_MIN_MS, NOTIF_TTL_MAX_MS)
    }

    /// True when both the master switch AND the per-kind switch allow a
    /// sound. The `--no-sound` CLI override is layered on top in
    /// `app.rs::push_notification` and is independent of this check.
    pub fn sound_enabled_for(&self, kind: SoundKind) -> bool {
        if !self.sound_master {
            return false;
        }
        self.sound_kind_on(kind)
    }

    pub fn sound_kind_on(&self, kind: SoundKind) -> bool {
        match kind {
            SoundKind::Info => self.sound_info,
            SoundKind::Warn => self.sound_warn,
            SoundKind::Error => self.sound_error,
        }
    }

    /// Apply a +/- step (in ms) to a single TTL field, clamping to the
    /// safe range. Returns the new value so callers can echo it.
    pub fn bump_ttl(&mut self, kind: SoundKind, delta: i64) -> u64 {
        let slot = match kind {
            SoundKind::Info => &mut self.notif_ttl_info_ms,
            SoundKind::Warn => &mut self.notif_ttl_warn_ms,
            SoundKind::Error => &mut self.notif_ttl_error_ms,
        };
        let next = (*slot as i64).saturating_add(delta).max(0) as u64;
        *slot = next.clamp(NOTIF_TTL_MIN_MS, NOTIF_TTL_MAX_MS);
        *slot
    }

    pub fn toggle_sound_master(&mut self) -> bool {
        self.sound_master = !self.sound_master;
        self.sound_master
    }

    pub fn toggle_sound_kind(&mut self, kind: SoundKind) -> bool {
        let slot = match kind {
            SoundKind::Info => &mut self.sound_info,
            SoundKind::Warn => &mut self.sound_warn,
            SoundKind::Error => &mut self.sound_error,
        };
        *slot = !*slot;
        *slot
    }

    pub fn reset(&mut self) {
        *self = Self::default();
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
#[allow(clippy::field_reassign_with_default)] // tests mutate between assertions
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
    fn missing_keys_use_defaults() {
        let r: Prefs = toml::from_str("show_workdir = false").expect("deserialize");
        assert!(!r.show_workdir);
        assert!(r.show_io);
        assert_eq!(r.tree_width, 32);
        assert!(r.sound_master);
    }

    #[test]
    fn ttl_clamped_to_safe_range() {
        let mut p = Prefs::default();
        p.notif_ttl_info_ms = 50;
        assert_eq!(p.ttl_ms(SoundKind::Info), NOTIF_TTL_MIN_MS);
        p.notif_ttl_warn_ms = 1_000_000;
        assert_eq!(p.ttl_ms(SoundKind::Warn), NOTIF_TTL_MAX_MS);
    }

    #[test]
    fn sound_master_overrides_per_kind() {
        let mut p = Prefs::default();
        assert!(p.sound_enabled_for(SoundKind::Info));
        p.sound_master = false;
        assert!(!p.sound_enabled_for(SoundKind::Info));
    }

    #[test]
    fn reset_restores_defaults() {
        let mut p = Prefs::default();
        p.tree_width = 99;
        p.sound_master = false;
        p.reset();
        assert_eq!(p.tree_width, 32);
        assert!(p.sound_master);
    }

    #[test]
    fn usage_refresh_defaults_and_clamps() {
        let p = Prefs::default();
        assert_eq!(p.usage_refresh_secs, USAGE_REFRESH_DEFAULT_SECS);
        assert_eq!(p.usage_refresh().as_secs(), 60);

        // Below the floor is raised to the minimum.
        let mut low = Prefs::default();
        low.usage_refresh_secs = 5;
        assert_eq!(low.usage_refresh().as_secs(), USAGE_REFRESH_MIN_SECS);

        // A sane higher value passes through.
        let mut high = Prefs::default();
        high.usage_refresh_secs = 120;
        assert_eq!(high.usage_refresh().as_secs(), 120);
    }

    #[test]
    fn usage_refresh_missing_key_uses_default() {
        let r: Prefs = toml::from_str("show_workdir = true").expect("deserialize");
        assert_eq!(r.usage_refresh_secs, USAGE_REFRESH_DEFAULT_SECS);
    }
}
