//! TUI app state, key dispatch, and event loop.

use std::collections::{HashMap, HashSet};
use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use agentum_core::{Event, Session, Status, transcript::AgentTaskState};
use anyhow::Result;
use crossterm::event::{
    Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval};
use uuid::Uuid;

use super::api::{Client, EventMsg, TermOut, TerminalMsg};
use super::extensions::{self, LAZYGIT};
use super::iometer::IoMeter;
use super::palette::{ActionKind, Catalog, ViewState};
use super::prefs::{self, Prefs};
use super::pty::{LocalPty, PtyMsg};
use super::term::TerminalPane;
use super::theme::{self, Theme};
use super::ui;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const TICK_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum visible notification toasts in the bottom-left stack. Older
/// entries are evicted FIFO when a new one arrives — same ceiling as
/// the dashboard's `ToastStack`.
pub const MAX_NOTIFS: usize = 4;
/// Severity buckets for bottom-left toasts. Drives both the colour of
/// the toast border and which system sound `sound::play` triggers.
/// Mirrors `Toast['kind']` in the dashboard so the two surfaces stay in
/// lockstep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifKind {
    Info,
    Warn,
    Error,
}

/// One entry in the bottom-left toast stack. Constructed by
/// `apply_event` from incoming bus events. Expires automatically once
/// `created_at.elapsed() >= ttl` — see `App::tick_expire`.
#[derive(Clone, Debug)]
#[allow(dead_code)] // fields populated for future toast-dedup logic
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub body: Option<String>,
    pub kind: NotifKind,
    pub created_at: Instant,
    pub ttl: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    /// Left / primary terminal pane.
    Term,
    /// Right pane in a split. Only meaningful when `App::split_right`
    /// is `Some`; closing the split reverts focus to `Term`.
    TermRight,
    Lazygit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Per-pane terminal state when split-screen is in play. The left slot
/// keeps using `App`'s flat `term`/`selected`/`term_in`/`term_size` fields
/// (so the legacy single-pane code stays unchanged); a right split lives
/// here. Drop the slot to close the split — its `Drop` impl on
/// `term_in` / the stream handle aborts the WS worker.
pub struct TermSlot {
    pub selected: Option<Uuid>,
    pub term: TerminalPane,
    pub term_in: Option<mpsc::UnboundedSender<TermOut>>,
    pub term_size: (u16, u16),
    /// True between a `TerminalMsg::Reconnecting` and the next
    /// `TerminalMsg::Connected` so the byte handler can snap any active
    /// scrollback back to the live tail when the gap closes — mid-
    /// disconnect scrollback positions are nearly always stale once
    /// the server replays a delta or fresh snapshot.
    pub term_reconnect_pending: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    #[default]
    Connecting,
    Connected,
    Reconnecting { attempt: u32, delay_ms: u64 },
    Disconnected,
}

/// Active mouse-driven text selection inside a terminal pane. Tracks
/// the anchor (first click), the live cursor (drag head), and which
/// pane the selection belongs to. `dragging` flips false the instant
/// the user releases the button — at which point `handle_mouse` reads
/// the selection one last time, copies the text via OSC 52, and drops
/// the field. Stays `Some` purely for the brief moment before drop so
/// the renderer can paint the highlight one final time.
///
/// Coords are 1-based pane-local (xterm/vt100 convention) so they
/// translate cleanly into both `vt100::Screen::cell(row, col)` lookups
/// and ratatui buffer offsets `(inner.x + col - 1, inner.y + row - 1)`.
#[derive(Clone, Copy)]
pub struct TermSelection {
    pub side: Side,
    pub anchor: (u16, u16),
    pub cursor: (u16, u16),
    pub dragging: bool,
}

impl TermSelection {
    /// Lexicographically smaller endpoint (top-left of the row range,
    /// modulo same-row reverse drag). Used by both the renderer and
    /// the text extractor so they walk the cells in the same order.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let (a, b) = (self.anchor, self.cursor);
        if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) }
    }

    /// True when the selection covers no cells (single-cell click with
    /// no drag). We skip the copy on these so a stray click in the
    /// pane doesn't overwrite the user's clipboard with a single
    /// character or empty string.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }
}

/// Modal overlays. At most one is up at a time.
///
/// `Copy` was dropped here when `NewSession` and `Confirm` were added —
/// they own owned-string fields. The handful of `Overlay` comparisons
/// that need to fit in a constraint use `==` against the `None` /
/// `Palette` variants only and still work fine via `PartialEq`.
#[derive(Clone, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    LazygitCheats,
    LazygitInstall,
    Palette,
    /// Browsable settings UI. Opened with `Ctrl-,` or via the command
    /// palette. Each row toggles a boolean, bumps a numeric, or fires a
    /// reset; every mutation persists to `tui_prefs.toml` via
    /// `prefs::save`.
    Settings(SettingsState),
    /// Scrollable view of the recent error log. Opened with `!` from
    /// tree focus or via the command palette. Replaces the status-bar
    /// counter that previously was the only place the user could tell
    /// errors had happened.
    Errors,
    /// Inline rename prompt for the highlighted session. Opened with
    /// `Ctrl-R` from tree focus or via the palette. Enter submits via
    /// PATCH; Esc cancels. Pre-filled with the current name so the user
    /// can edit rather than retype from scratch.
    Rename(RenameState),
    /// New-session form (n key on the tree).
    NewSession(Box<NewSessionForm>),
    /// Generic confirmation prompt for destructive session actions.
    Confirm(PendingAction),
    /// Server switcher. Lists configured agentum servers, lets the
    /// user switch between them or add a new one without leaving the
    /// TUI. Selecting a different profile triggers a soft restart of
    /// the run-loop so every store / WS / cache rebuilds against the
    /// new daemon.
    Profiles(ProfilesOverlay),
}

/// In-memory state for the [`Overlay::Profiles`] switcher.
#[derive(Clone, PartialEq, Eq)]
pub struct ProfilesOverlay {
    pub entries: Vec<ProfileEntry>,
    pub cursor: usize,
    pub default_name: Option<String>,
    pub error: Option<String>,
    /// `Some` when the user is editing the inline "add server" form
    /// instead of the list. Mirrors the dashboard's ServerSwitcher.
    pub add_form: Option<AddProfileForm>,
}

/// One row in the profile picker. Mirrors the on-disk profile but is
/// detached from the file so the overlay can re-render without
/// re-reading after every keystroke.
#[derive(Clone, PartialEq, Eq)]
pub struct ProfileEntry {
    pub name: String,
    pub url: String,
    pub fingerprint: Option<String>,
    pub is_default: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AddProfileForm {
    pub field: AddProfileField,
    pub name: String,
    pub url: String,
    pub fingerprint: String,
    pub set_default: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AddProfileField {
    Name,
    Url,
    Fingerprint,
    SetDefault,
}

impl AddProfileForm {
    pub fn new() -> Self {
        Self {
            field: AddProfileField::Name,
            name: String::new(),
            url: String::new(),
            fingerprint: String::new(),
            set_default: false,
            error: None,
        }
    }

    pub fn next_field(&mut self) {
        self.field = match self.field {
            AddProfileField::Name => AddProfileField::Url,
            AddProfileField::Url => AddProfileField::Fingerprint,
            AddProfileField::Fingerprint => AddProfileField::SetDefault,
            AddProfileField::SetDefault => AddProfileField::Name,
        };
    }

    pub fn prev_field(&mut self) {
        self.field = match self.field {
            AddProfileField::Name => AddProfileField::SetDefault,
            AddProfileField::Url => AddProfileField::Name,
            AddProfileField::Fingerprint => AddProfileField::Url,
            AddProfileField::SetDefault => AddProfileField::Fingerprint,
        };
    }

    pub fn field_value_mut(&mut self) -> Option<&mut String> {
        match self.field {
            AddProfileField::Name => Some(&mut self.name),
            AddProfileField::Url => Some(&mut self.url),
            AddProfileField::Fingerprint => Some(&mut self.fingerprint),
            AddProfileField::SetDefault => None,
        }
    }
}

/// What a single `run_loop` invocation wants the caller to do next.
/// `Quit` returns control to the OS; `SwitchProfile` re-enters the
/// loop after re-resolving the API base + token from the named
/// profile. The wrapper in `commands::terminal::run` owns the retry
/// machinery so `run_loop` itself stays in one connection's lifetime.
///
/// `SwitchProfile.then` carries a follow-up the wrapper hands back
/// to the next `run_loop` so a multi-step user action (e.g. submitting
/// the New Session form against a different profile) survives the
/// reconnect.
#[derive(Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Quit,
    SwitchProfile {
        name: String,
        then: Option<PendingAfterSwitch>,
    },
}

/// One observed error, surfaced to the user via the error overlay.
/// `at` is captured at push time so the overlay can render a stable
/// timestamp regardless of when the user opens it.
#[derive(Clone, Debug)]
pub struct ErrorEntry {
    pub at: SystemTime,
    pub text: String,
}

/// Suggested tool names. Mirrors the web's datalist on the New Session
/// dialog. Pressing Tab on the `Tool` field cycles through these.
pub const TOOL_SUGGESTIONS: &[&str] = &["claude", "codex", "cursor", "opencode", "aider", "bash"];

/// Returns `true` when the daemon's `/api/agents` reports availability
/// for this tool name. Mirrors `agentum_executor::probed_tools()` so
/// the TUI knows which entries should be gated. Free-form names
/// (`terminal`, `bash`, anything outside the curated list) always
/// route through PassthroughAdapter and are never gated.
pub fn is_probed_tool(tool: &str) -> bool {
    matches!(
        tool,
        "claude" | "codex" | "cursor" | "gemini" | "hermes" | "opencode" | "aider"
    )
}

/// Tools that support YOLO / skip-permissions mode. Must mirror the
/// `yoloTools` set in `dashboard/src/lib/components/NewSessionDialog.svelte`
/// AND the set of adapters whose `yolo_flag()` returns `Some(_)` in
/// `crates/agentum-executor/src/adapters.rs`. `opencode` was previously
/// listed here under the (wrong) assumption that it accepts Claude's
/// flag — that footgun was the v0.6.24 fix; only add tools back once
/// their adapter declares the correct per-tool flag. Cursor's adapter
/// translates the YOLO marker to its own `--force` switch, so it's
/// safe to include here.
pub const YOLO_TOOLS: &[&str] = &["claude", "codex", "cursor", "gemini"];

/// Wire-format YOLO marker. Both surfaces push this exact string into
/// `Session::flags` when the YOLO toggle is on, regardless of tool.
/// The executor adapter translates it to the tool-specific flag at
/// launch time (`agentum-executor::translate_yolo_marker`); never push
/// a different spelling here or you'll defeat the translation layer
/// and reintroduce the v0.6.23 codex crash (`unexpected argument
/// '--dangerously-skip-permissions'`).
pub const YOLO_FLAG: &str = "--dangerously-skip-permissions";

/// Inline new-session form. Mirrors the web `NewSessionDialog` field-for-
/// field: name, tool (with cycle-suggestions), model, workdir (with a
/// directory-picker sub-overlay), extra args, an "up after create" toggle,
/// and a YOLO toggle that appends `--dangerously-skip-permissions` for
/// permission-skipping agents.
#[derive(Clone, PartialEq, Eq)]
pub struct NewSessionForm {
    pub field: NewSessionField,
    /// Server profile this session will be created on. Empty string
    /// means "current connection" (loopback or ad-hoc `--api`); a
    /// non-empty value either matches the active profile (no-op on
    /// submit) or triggers a soft restart that re-opens the form on
    /// the new daemon. Tab cycles through `App.profiles`.
    pub profile: String,
    pub name: String,
    pub tool: String,
    pub model: String,
    pub workdir: String,
    pub args: String,
    pub up_after: bool,
    pub yolo: bool,
    pub error: Option<String>,
    pub submitting: bool,
    /// When `Some`, the directory-picker overlay is up. Field state persists
    /// inside the form so closing the picker restores the rest of the form.
    pub picker: Option<DirPickerState>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DirPickerState {
    /// Path the picker is currently listing.
    pub path: String,
    /// Parent of `path` (None at filesystem root).
    pub parent: Option<String>,
    /// Subdirectories of `path`. Loaded from `/api/fs/list`.
    pub entries: Vec<DirEntryView>,
    pub cursor: usize,
    pub error: Option<String>,
    pub loading: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DirEntryView {
    pub name: String,
    pub path: String,
}

impl NewSessionForm {
    /// Constructor that lets the caller pre-fill the profile field.
    /// Used by the run-loop to seed the form with the active profile
    /// when the user opens it from any sidebar / palette / key path.
    pub fn with_profile(default_profile: String, default_workdir: String) -> Self {
        Self {
            field: NewSessionField::Profile,
            profile: default_profile,
            name: String::new(),
            tool: "claude".into(),
            model: String::new(),
            workdir: default_workdir,
            args: String::new(),
            up_after: true,
            yolo: false,
            error: None,
            submitting: false,
            picker: None,
        }
    }

    pub fn next_field(&mut self) {
        self.field = match self.field {
            NewSessionField::Profile => NewSessionField::Name,
            NewSessionField::Name => NewSessionField::Tool,
            NewSessionField::Tool => NewSessionField::Model,
            NewSessionField::Model => NewSessionField::Workdir,
            NewSessionField::Workdir => NewSessionField::Args,
            NewSessionField::Args => NewSessionField::UpAfter,
            NewSessionField::UpAfter => NewSessionField::Yolo,
            NewSessionField::Yolo => NewSessionField::Profile,
        };
    }

    pub fn prev_field(&mut self) {
        self.field = match self.field {
            NewSessionField::Profile => NewSessionField::Yolo,
            NewSessionField::Name => NewSessionField::Profile,
            NewSessionField::Tool => NewSessionField::Name,
            NewSessionField::Model => NewSessionField::Tool,
            NewSessionField::Workdir => NewSessionField::Model,
            NewSessionField::Args => NewSessionField::Workdir,
            NewSessionField::UpAfter => NewSessionField::Args,
            NewSessionField::Yolo => NewSessionField::UpAfter,
        };
    }

    pub fn field_value_mut(&mut self) -> Option<&mut String> {
        match self.field {
            // Profile is cycle-only — typing a free-form name is
            // unreliable since the value has to match an entry in
            // `App.profiles` (or be empty for "current connection").
            // Tab cycles the value; backspace clears to the empty
            // string. See `cycle_profile`.
            NewSessionField::Profile => None,
            NewSessionField::Name => Some(&mut self.name),
            NewSessionField::Tool => Some(&mut self.tool),
            NewSessionField::Model => Some(&mut self.model),
            NewSessionField::Workdir => Some(&mut self.workdir),
            NewSessionField::Args => Some(&mut self.args),
            NewSessionField::UpAfter | NewSessionField::Yolo => None, // toggles, not text
        }
    }

    /// Cycle the profile field through `available` plus the empty
    /// string (which represents the current loopback / `--api`
    /// connection). Wraps; preserves order. Used by Tab on the
    /// Profile field.
    pub fn cycle_profile(&mut self, available: &[String]) {
        // Build the wheel: empty string ("current") → every named
        // profile in disk order. Empty string is always present so
        // an ad-hoc connection without a named profile is still a
        // selectable target.
        let mut wheel: Vec<String> = vec![String::new()];
        wheel.extend(available.iter().cloned());
        if wheel.len() <= 1 {
            return; // nothing to cycle through
        }
        let idx = wheel.iter().position(|n| n == &self.profile).unwrap_or(0);
        self.profile = wheel[(idx + 1) % wheel.len()].clone();
    }

    /// True when YOLO mode is enabled and the active tool actually supports
    /// `--dangerously-skip-permissions`. Bash and aider (and friends) ignore
    /// the toggle so the flag stays out of their argv.
    pub fn yolo_active(&self) -> bool {
        let tool = self.tool.trim();
        self.yolo && YOLO_TOOLS.contains(&tool)
    }

    /// Cycle the tool field through `TOOL_SUGGESTIONS`. Triggered by
    /// pressing Tab when the Tool field has focus. Wraps to `claude`
    /// after the last entry. `is_available` skips first-class agents
    /// whose binary isn't installed on the daemon's PATH so the user
    /// never lands on an unspawnable name. If every entry is filtered
    /// out (cold daemon, no agents installed) we leave the field
    /// alone — the user can still type a passthrough name by hand.
    pub fn cycle_tool(&mut self, is_available: impl Fn(&str) -> bool) {
        let current = self.tool.trim();
        let start = TOOL_SUGGESTIONS
            .iter()
            .position(|t| *t == current)
            .map(|i| (i + 1) % TOOL_SUGGESTIONS.len())
            .unwrap_or(0);
        for step in 0..TOOL_SUGGESTIONS.len() {
            let cand = TOOL_SUGGESTIONS[(start + step) % TOOL_SUGGESTIONS.len()];
            if is_available(cand) {
                self.tool = cand.to_string();
                return;
            }
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() {
            return Err("name is required");
        }
        if self.workdir.trim().is_empty() {
            return Err("workdir is required");
        }
        if self.tool.trim().is_empty() {
            return Err("tool is required (e.g. claude, codex, gemini, hermes)");
        }
        Ok(())
    }
}

/// Parse the `Extra args` text field the same way the web's
/// `NewSessionDialog` does:
///
/// - whitespace-separated tokens
/// - each token must contain `=`; tokens without one are dropped
/// - leading `--` on the key is stripped (idempotent)
/// - `key=true` becomes the boolean flag `--key`
/// - everything else becomes `--key=value`
///
/// Returns the list the server expects in `NewSession.flags`.
pub fn parse_args_field(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in input.split_whitespace() {
        let Some(eq) = tok.find('=') else { continue };
        let key = tok[..eq].trim_start_matches("--");
        let val = &tok[eq + 1..];
        if val == "true" {
            out.push(format!("--{key}"));
        } else {
            out.push(format!("--{key}={val}"));
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NewSessionField {
    /// Server profile this session targets. New first field — see
    /// `NewSessionForm::with_profile`. Tab cycles through configured
    /// profiles + an empty entry meaning "current connection".
    Profile,
    Name,
    Tool,
    Model,
    Workdir,
    Args,
    UpAfter,
    Yolo,
}

/// A destructive session action waiting on user confirmation. Carries the
/// target session's id + display name so the prompt reads "kill alpha?"
/// rather than referring to a UUID.
#[derive(Clone, PartialEq, Eq)]
pub enum PendingAction {
    Start {
        id: Uuid,
        name: String,
    },
    Stop {
        id: Uuid,
        name: String,
    },
    Kill {
        id: Uuid,
        name: String,
    },
    RemoveServer {
        name: String,
    },
}

impl PendingAction {
    pub fn prompt(&self) -> String {
        match self {
            PendingAction::Start { name, .. } => format!("start session `{name}`?"),
            PendingAction::Stop { name, .. } => {
                format!("stop session `{name}`? (graceful, SIGTERM then kill after 5s)")
            }
            PendingAction::Kill { name, .. } => {
                format!("kill `{name}`? Stops the process and removes the session.")
            }
            PendingAction::RemoveServer { name } => {
                format!("delete server `{name}`? This cannot be undone.")
            }
        }
    }

    pub fn is_destructive(&self) -> bool {
        !matches!(self, PendingAction::Start { .. })
    }
}

/// Command-palette state. Driven by Ctrl-P / Ctrl-Shift-P. The action list is
/// generated on demand from `palette::all_actions(&app)` so it can include
/// dynamic entries like the active session list and theme registry.
pub struct PaletteState {
    pub query: String,
    pub cursor: usize,
}

impl PaletteState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            cursor: 0,
        }
    }
}

/// One interactive row in the Settings overlay. Boolean rows respond to
/// `space` / `enter`; numeric rows respond to `←` / `→`; the lone
/// `ResetAll` row fires the prefs reset on `space` / `enter`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsRow {
    SoundMaster,
    SoundInfo,
    SoundWarn,
    SoundError,
    TtlInfo,
    TtlWarn,
    TtlError,
    SidebarHidden,
    RightPanelVisible,
    ChipWorkdir,
    ChipTool,
    ChipConn,
    ChipLazygit,
    ChipTheme,
    ChipIo,
    ChipIoTotals,
    ChipPaletteHint,
    ChipHelpHint,
    ResetAll,
}

impl SettingsRow {
    pub const ROWS: &'static [Self] = &[
        Self::SoundMaster,
        Self::SoundInfo,
        Self::SoundWarn,
        Self::SoundError,
        Self::TtlInfo,
        Self::TtlWarn,
        Self::TtlError,
        Self::SidebarHidden,
        Self::RightPanelVisible,
        Self::ChipWorkdir,
        Self::ChipTool,
        Self::ChipConn,
        Self::ChipLazygit,
        Self::ChipTheme,
        Self::ChipIo,
        Self::ChipIoTotals,
        Self::ChipPaletteHint,
        Self::ChipHelpHint,
        Self::ResetAll,
    ];

    /// Section header to render *above* this row, if any. Drives the
    /// "Notifications / Layout / Status bar / Actions" grouping in the
    /// overlay without needing a separate non-selectable row variant.
    pub fn section_header(self) -> Option<&'static str> {
        match self {
            Self::SoundMaster => Some("Notifications"),
            Self::SidebarHidden => Some("Layout"),
            Self::ChipWorkdir => Some("Status bar"),
            Self::ResetAll => Some("Actions"),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SettingsState {
    pub cursor: usize,
}

impl SettingsState {
    pub fn new() -> Self {
        Self { cursor: 0 }
    }
    pub fn current(&self) -> SettingsRow {
        SettingsRow::ROWS[self.cursor.min(SettingsRow::ROWS.len() - 1)]
    }
    pub fn move_by(&mut self, delta: isize) {
        let len = SettingsRow::ROWS.len() as isize;
        let next = (self.cursor as isize + delta).rem_euclid(len);
        self.cursor = next as usize;
    }
}

/// Buffer state for the inline rename prompt. Holds the session being
/// renamed plus the editable buffer (pre-filled with the current name).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RenameState {
    pub id: Uuid,
    pub original: String,
    pub buffer: String,
    /// Optional inline error to render under the input — populated on a
    /// failed PATCH (duplicate name, server validation rejected) so the
    /// user sees what's wrong without losing their typed text.
    pub error: Option<String>,
}

impl RenameState {
    pub fn new(id: Uuid, current_name: &str) -> Self {
        Self {
            id,
            original: current_name.to_string(),
            buffer: current_name.to_string(),
            error: None,
        }
    }
}

pub struct App {
    pub sessions: Vec<Session>,
    pub tree: Tree,
    pub selected: Option<Uuid>,
    /// The session that *was* selected before the current one. Powers
    /// Ctrl-Tab "flip back to last session" — the most-used nav action
    /// when alternating between two agents. None until the user has
    /// switched at least once.
    pub prev_selected: Option<Uuid>,
    pub term: TerminalPane,
    /// Parser state stashed when the user switches away from a session.
    /// On switch-back we restore the parser instead of opening a fresh
    /// one, and tell the daemon `{"resume":true}` so it sends the missed
    /// log delta instead of a full pane snapshot. This is what stops a
    /// session-switch round-trip from wiping visible chat history when
    /// the agent's UI happens to look mostly empty after task
    /// completion (snapshot-of-now overwriting the preserved state).
    pub parser_cache: std::collections::HashMap<Uuid, TerminalPane>,
    pub focus: Focus,
    /// Width of the sidebar tree pane in columns. User-resizable with
    /// `+` / `-`, clamped 16..=80. The terminal pane keeps a 20-column
    /// floor at draw time.
    pub tree_width: u16,
    /// When `true` the title bar, tree sidebar, and status bar are hidden
    /// so the active pane (term + optional lazygit) fills the viewport.
    /// Toggled with Shift-F. Esc exits.
    pub fullscreen: bool,
    /// When `true` the tree sidebar is hidden but the title and status
    /// bars stay. Distinct from `fullscreen`: this is VS Code's `Ctrl-B`
    /// "toggle primary side bar" — useful when you want screen real
    /// estate for the term pane but still want the breadcrumb/status.
    pub sidebar_hidden: bool,
    /// Multi-key chord prefix awaiting a follow-up. `Some('K')` means we
    /// just consumed Ctrl-K and the next keystroke decides the action
    /// (VS Code parity: Ctrl-K Z = fullscreen, Ctrl-K B = sidebar, …).
    /// Cleared automatically on the next key event regardless of match.
    pub chord: Option<char>,
    /// True while the user is typing into the tree filter prompt
    /// (entered with `/` from tree focus). While active, keystrokes
    /// extend / trim the filter instead of navigating the tree. Esc
    /// clears + exits, Enter commits + exits (filter persists; press
    /// Esc again from tree focus to clear it).
    pub filter_input_active: bool,
    pub error_count: u32,
    /// Ring of recent error messages, capped at `MAX_ERROR_LOG`. Pushed
    /// alongside every `error_count` bump so the user can open the
    /// errors overlay (`e` from tree focus) and see what actually
    /// failed instead of just a counter.
    pub errors: Vec<ErrorEntry>,
    /// Top-of-list offset for the errors overlay, in entries. Saturates
    /// at the list length when the user presses `End`.
    pub errors_scroll: usize,
    pub conn: ConnState,
    pub was_connected: bool,
    pub tick_count: u64,
    pub status_msg: Option<String>,
    /// Drained by the run-loop tick: when `Some`, the daemon's shared
    /// preferences blob is updated so the dashboard picks up the
    /// change. Set whenever the user picks a new theme from the
    /// palette or via `T`. Pulled out of the local theme handlers
    /// because the handlers don't have access to the API client.
    pub pending_pref_push: Option<String>,
    pub should_quit: bool,
    pub overlay: Overlay,
    pub palette: PaletteState,
    /// `Some` while a lazygit child is alive in the side pane.
    pub lazygit: Option<LocalPty>,
    /// The cwd we last spawned lazygit in. Compared against the freshly
    /// selected session's workdir so tree navigation can respawn the
    /// side pane when the user switches to a different project — the
    /// pane was otherwise pinned to whatever directory it was first
    /// opened in, which made the lazygit view drift out of sync with
    /// the highlighted agent.
    pub lazygit_cwd: Option<PathBuf>,
    /// Cheap clone of the run-loop's lazygit-byte channel. Lives on
    /// `App` so any handler that mutates the selection (filter input,
    /// palette pick, tree j/k, post-create refresh) can respawn the
    /// side pane via `refresh_lazygit_for_selection` without every
    /// call site having to thread an extra `&Sender` parameter.
    pub lg_tx: Option<mpsc::UnboundedSender<PtyMsg>>,
    /// Outbound key channel for the active terminal stream. `None` while
    /// no session is selected; recreated each time the stream is reopened.
    pub term_in: Option<mpsc::UnboundedSender<TermOut>>,
    /// Last `(cols, rows)` we told the server about. Tracked here so we
    /// only push a resize when the value actually changes.
    pub term_size: (u16, u16),
    /// Mirror of [`TermSlot::term_reconnect_pending`] for the left slot.
    /// True between a `TerminalMsg::Reconnecting` and the next
    /// `TerminalMsg::Connected` for the primary terminal stream.
    pub term_reconnect_pending: bool,
    /// Owned stream-task handles. Stored on `App` instead of threaded
    /// through every helper as `&mut Option<JoinHandle<()>>` — that
    /// pattern has to expand to TWO handles for split panes (left +
    /// right), and threading both through the call graph would touch
    /// every async helper. Living here means helpers see them through
    /// `&mut App` for free. Aborted (drop-on-demand) when reselecting.
    pub stream_handle_left: Option<JoinHandle<()>>,
    /// Right-pane stream task. `Some` only when the split is open and
    /// the right slot has an active session.
    pub stream_handle_right: Option<JoinHandle<()>>,
    /// Cheap clones of the run-loop's two terminal-byte channels. Living
    /// on `App` means `update_selection` can pick the correct sender by
    /// `Side` without every async helper having to thread two `&Sender`
    /// references. Set once in `run_loop` right after the channels are
    /// created.
    pub term_tx_left: Option<mpsc::UnboundedSender<TerminalMsg>>,
    pub term_tx_right: Option<mpsc::UnboundedSender<TerminalMsg>>,
    /// Right-side terminal slot — `Some` while a split is active. Holds
    /// its own selection / parser / input channel / cached size; the
    /// existing flat `term` / `selected` / `term_in` / `term_size`
    /// fields keep referring to the LEFT slot so existing call sites
    /// don't need to be rewritten.
    pub split_right: Option<TermSlot>,
    /// Which side tree-driven selection updates target. Updated on every
    /// `Term` / `TermRight` focus event so when the user releases pane
    /// focus back to the tree (Ctrl-E), j/k retargets the slot they
    /// were just typing into. Without this, navigating from the tree
    /// while the right pane is focused would silently drive the left
    /// pane and feel haunted.
    pub last_term_side: Side,
    /// Most recent computed layout — stashed by `run_loop` after each
    /// `ui::compute_layout` call. Mouse events arrive with absolute
    /// (col, row) coords; routing them to "the pane under the cursor"
    /// needs the Rects we just drew with. None until the first frame.
    pub last_areas: Option<ui::Areas>,
    pub theme: &'static Theme,
    /// Bottom-left toast stack. Each `Notification` carries its own
    /// severity, body, and TTL; expired entries are drained every tick
    /// by `tick_expire`. Capped at `MAX_NOTIFS` (oldest evicted FIFO).
    pub notifications: Vec<Notification>,
    /// Monotonic id source so renderers can use `id` as a `keyed` hint
    /// when toasts come and go between frames.
    pub next_notif_id: u64,
    /// One-shot CLI override on top of `prefs.sound_master` /
    /// `prefs.sound_<kind>`. Set from `--no-sound` /
    /// `AGENTUM_TUI_NO_SOUND` at startup. When `true`, no notification
    /// sound ever plays this run regardless of the persisted prefs;
    /// when `false`, the prefs decide.
    pub sound_muted_cli: bool,
    /// Per-session cache of plan / todos / background tasks parsed
    /// out of each agent's Claude Code transcript. Keys are session
    /// ids. Populated from `GET /api/sessions/{id}/agent-tasks` and
    /// refreshed when an `agent_tasks.updated` event lands on the bus.
    pub agent_tasks: HashMap<Uuid, AgentTaskState>,
    /// Sender for spawn-detached agent-tasks fetches. Background tasks
    /// post `(session_id, Some(state))` on success or
    /// `(session_id, None)` on transport error; the run-loop drains
    /// the receiver, clears the in-flight marker either way, and
    /// writes into `agent_tasks` on success. Letting fetches run off
    /// the keystroke path keeps j/k navigation snappy even when the
    /// daemon is remote.
    pub agent_tasks_tx: Option<mpsc::UnboundedSender<(Uuid, Option<AgentTaskState>)>>,
    /// Session ids with an in-flight `spawn_agent_tasks_fetch`. Used
    /// to coalesce duplicate fetches when navigation, events, and the
    /// 5-second slow-path all want to refresh the same id within a
    /// few hundred ms. Cleared by the `agent_tasks_rx` arm regardless
    /// of success or failure.
    pub agent_tasks_inflight: HashSet<Uuid>,
    /// Cwd we want lazygit to be in next. Set on every navigation and
    /// drained by `drive_pending_lazygit` from the tick loop. The
    /// indirection lets us debounce PTY respawns so a held-j burst
    /// across many repos triggers exactly one lazygit cold-boot at
    /// the user's *settled* destination instead of one per session.
    pub pending_lazygit_cwd: Option<PathBuf>,
    /// Wall-clock time of the most recent navigation that wants
    /// lazygit attention. Compared against `LAZYGIT_NAV_DEBOUNCE_MS`
    /// in the tick loop so we only honour `pending_lazygit_cwd` once
    /// the user has stopped moving.
    pub last_nav_at: Option<Instant>,
    /// Active or just-released mouse selection inside one of the
    /// terminal panes. `Some` from mouse-down through mouse-up; the
    /// release handler reads it, runs OSC 52, and drops the value.
    /// The renderer paints a highlight whenever this is `Some` and
    /// `side` matches the pane it's drawing.
    pub term_selection: Option<TermSelection>,
    /// Show the right-side plan/todo/task panel. Toggled with `Ctrl-T`.
    /// Default on so users see the feature without having to discover
    /// the binding.
    pub right_panel_visible: bool,
    /// Width of the lazygit pane in columns. Drives both the dedicated
    /// far-right outer column and the in-pane horizontal split fallback
    /// so the resize keys behave the same in either layout. Clamped
    /// `LAZYGIT_MIN_WIDTH..=LAZYGIT_MAX_WIDTH` at draw time.
    pub lazygit_width: u16,
    /// Percentage of the terminal area allocated to the LEFT pane when a
    /// split is open. Right pane gets the rest. Resized with
    /// `Ctrl-Shift-Left` / `Ctrl-Shift-Right`, clamped 25..=75 so neither
    /// side collapses. Ignored when no split is open.
    pub term_split_pct: u16,
    /// Sliding-window byte counter for the active WS terminal stream.
    /// Drives the I/O speed chip on the status bar. Reset on session
    /// switch so totals reflect the current stream, not an accumulation
    /// across every session the user clicked through this run.
    pub io: IoMeter,
    /// Persistent UI preferences — what to show on the status bar.
    /// Loaded at startup, persisted to disk on every change.
    pub prefs: Prefs,
    /// Sessions currently waiting on a permission prompt or other
    /// user-input gate. Driven by `agent.awaiting_input` /
    /// `agent.input_resolved` events from the watchdog. The sidebar
    /// dot renders yellow `▲` for any id in this set, regardless of
    /// the persisted `Status` — so users can see at-a-glance which
    /// agent needs attention without opening the pane.
    pub awaiting_input: HashSet<Uuid>,
    /// Sessions whose agent has finished its turn and is sitting at
    /// the prompt (ActivityState::Idle on the watchdog side). Driven
    /// by `agent.finished` and the `state: idle` payload variant of
    /// `agent.input_resolved`. The sidebar dot renders a dim `◌` for
    /// any id in this set so a "sleeping" agent is visually distinct
    /// from one that's actively working — without needing a wider
    /// 2-cell emoji that would shift the rest of the row.
    pub idle: HashSet<Uuid>,
    /// First-class agents whose binary is installed on the daemon's
    /// PATH. Populated once at startup from `/api/agents`; consulted by
    /// the New Session form and the tool-cycle key to skip agents the
    /// user can't actually launch (cursor without `cursor-agent`,
    /// codex without the codex CLI, etc.). When `None` the probe is
    /// pending OR the daemon is older than this change — both paths
    /// fail open at the call site so the picker stays usable.
    pub agent_availability: Option<HashSet<String>>,
    /// When `Some`, the run-loop is exiting because the user picked a
    /// different profile in the server switcher. `commands::terminal::run`
    /// reads this on `Quit` and re-enters the connect loop with the
    /// named profile instead of returning to the shell.
    pub pending_switch_profile: Option<String>,
    /// Optional follow-up to fire after a profile switch lands. Used
    /// when the user submitted the New Session form against a different
    /// profile than the active one — the form state survives the
    /// soft-restart so they finish the spawn on the right server.
    pub pending_after_switch: Option<PendingAfterSwitch>,
    /// Name of the active profile (or `None` if the TUI was launched
    /// with an ad-hoc `--api`). Shown in the title bar so users
    /// targeting multiple servers can tell which one they're on.
    pub active_profile: Option<String>,
    /// Configured server profiles, cached at startup so the sidebar
    /// can render an "Servers" section without re-reading the file
    /// on every frame. Refreshed via `reload_profiles` after add /
    /// remove from any surface (overlay, sidebar action, CLI).
    pub profiles: Vec<ProfileEntry>,
    /// Which sidebar section the cursor is on. Two sections share one
    /// pane: an Servers list at the top, then the Sessions tree.
    /// `j`/`k` flips between them at the boundaries.
    pub tree_section: TreeSection,
    /// Cursor index inside the Servers section (only meaningful when
    /// `tree_section == TreeSection::Servers`).
    pub servers_cursor: usize,
    /// Live clients keyed by profile name. The empty string `""` keys
    /// the loopback / `--api` connection (one launch can only have at
    /// most one of those). Each entry tracks its reachability so the
    /// sidebar can render `(unreachable)` / `(login needed)` markers
    /// in place of an empty session list. Populated at run-loop
    /// startup; never empty after that — the *default* client always
    /// has a slot, even if its `client` is `None` because the user
    /// is multi-profile-only with no loopback.
    pub clients: HashMap<String, ClientEntry>,
    /// Session-id → owning profile name. Used by ops that take a
    /// session id (start / stop / stream) to look up the right client
    /// without threading the profile through every call site. Empty
    /// string means "default / loopback / `--api`" — same key shape
    /// as `clients`.
    pub session_profile: HashMap<Uuid, String>,
}

/// One slot in [`App::clients`]. Tracks whether the server is
/// reachable, what error stopped it (if any), and the live `Client`
/// when reachability succeeded.
#[allow(dead_code)] // fields wired up but reads pending multi-server UI
pub struct ClientEntry {
    pub client: Option<Client>,
    pub status: ServerStatus,
    pub last_error: Option<String>,
    /// Cached `/api/agents` response per server so the New Session
    /// form can gate the agent picker against the right daemon's
    /// `PATH`. `None` means the probe is pending or the server
    /// pre-dates the route.
    pub agent_availability: Option<HashSet<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    /// HTTP + auth check both passed; the client is usable.
    Live,
    /// We couldn't reach the server at all (DNS, TCP, TLS, timeout).
    Unreachable,
    /// Server answered but rejected the bearer token. The user has to
    /// log in on this server before its sessions appear.
    LoginNeeded,
}

/// Which sidebar section currently has the cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeSection {
    Servers,
    Sessions,
}

/// Carry-over data used when a profile switch needs to fire a follow-up
/// action after the new connection lands. Today only one variant: re-
/// open the New Session form on the freshly-connected daemon with the
/// same fields the user already typed.
#[derive(Clone, PartialEq, Eq)]
#[allow(dead_code)] // variant reserved for post-switch session creation
pub enum PendingAfterSwitch {
    /// Re-open the New Session overlay with this form pre-populated.
    /// The Profile field gets normalised to the new active profile so
    /// hitting Enter again creates without another switch.
    OpenNewSession(Box<NewSessionForm>),
}

impl App {
    pub fn new(sessions: Vec<Session>) -> Self {
        let tree = Tree::build(&sessions, &HashMap::new());
        let selected = first_visible_session(&tree, &sessions);
        // Load persisted prefs once and seed the runtime layout fields.
        // Both `app.<field>` and `app.prefs.<field>` are kept in sync at
        // every keybinding mutation so the on-disk file is the source of
        // truth that survives a restart.
        let prefs = prefs::load();
        Self {
            sessions,
            tree,
            selected,
            prev_selected: None,
            term: TerminalPane::new(),
            parser_cache: std::collections::HashMap::new(),
            focus: Focus::Tree,
            tree_width: prefs.tree_width,
            fullscreen: false,
            sidebar_hidden: prefs.sidebar_hidden,
            chord: None,
            filter_input_active: false,
            error_count: 0,
            errors: Vec::new(),
            errors_scroll: 0,
            conn: ConnState::Connecting,
            was_connected: false,
            tick_count: 0,
            status_msg: None,
            pending_pref_push: None,
            should_quit: false,
            overlay: Overlay::None,
            palette: PaletteState::new(),
            lazygit: None,
            lazygit_cwd: None,
            lg_tx: None,
            term_in: None,
            term_size: (0, 0),
            term_reconnect_pending: false,
            stream_handle_left: None,
            stream_handle_right: None,
            term_tx_left: None,
            term_tx_right: None,
            split_right: None,
            last_term_side: Side::Left,
            last_areas: None,
            theme: theme::load(),
            notifications: Vec::new(),
            next_notif_id: 1,
            sound_muted_cli: false,
            agent_tasks: HashMap::new(),
            agent_tasks_tx: None,
            agent_tasks_inflight: HashSet::new(),
            pending_lazygit_cwd: None,
            last_nav_at: None,
            term_selection: None,
            right_panel_visible: prefs.right_panel_visible,
            lazygit_width: prefs.lazygit_width,
            term_split_pct: prefs.term_split_pct,
            io: IoMeter::new(),
            prefs,
            awaiting_input: HashSet::new(),
            idle: HashSet::new(),
            agent_availability: None,
            pending_switch_profile: None,
            pending_after_switch: None,
            active_profile: None,
            profiles: Vec::new(),
            tree_section: TreeSection::Sessions,
            servers_cursor: 0,
            clients: HashMap::new(),
            session_profile: HashMap::new(),
        }
    }

    /// Look up which profile owns `id`. Falls back to the empty key —
    /// the conventional "default / loopback" slot — for sessions that
    /// were created before the multi-client refactor or for which
    /// the tag wasn't recorded.
    pub fn profile_for_session(&self, id: Uuid) -> &str {
        self.session_profile
            .get(&id)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Borrow the live `Client` that owns `id`. `None` means the
    /// owning server is unreachable or login-needed; callers that
    /// need to operate on the session should surface a hint to the
    /// user instead of trying anyway.
    pub fn client_for_session(&self, id: Uuid) -> Option<&Client> {
        let key = self.profile_for_session(id);
        self.clients.get(key).and_then(|e| e.client.as_ref())
    }

    /// Borrow the *default* profile's client — the one new sessions
    /// land on by default and the one most legacy call sites still
    /// use. `None` only when even the default server failed to
    /// connect, which is also the cold-start failure path.
    #[allow(dead_code)] // wired up for upcoming multi-server session routing
    pub fn default_client(&self) -> Option<&Client> {
        let key = self.active_profile.as_deref().unwrap_or("");
        self.clients.get(key).and_then(|e| e.client.as_ref())
    }

    /// Iterate over `(profile_name, &Client)` for every live server.
    /// Used by the aggregating refresh so multi-server runs see a
    /// unified session list refreshed from every reachable daemon.
    pub fn live_clients(&self) -> impl Iterator<Item = (&str, &Client)> {
        self.clients
            .iter()
            .filter_map(|(name, entry)| entry.client.as_ref().map(|c| (name.as_str(), c)))
    }


    /// Load the on-disk profiles into `app.profiles`. Called once at
    /// run-loop start and again any time the user adds/removes a
    /// profile via the sidebar or overlay so the sidebar stays in
    /// sync without re-reading the file every frame. Errors are
    /// non-fatal — they leave `profiles` empty and the sidebar
    /// renders an "no servers" hint.
    pub fn reload_profiles(&mut self) {
        self.profiles = match super::profiles::Profiles::load() {
            Ok(store) => store
                .list()
                .into_iter()
                .map(|(name, p, is_default)| ProfileEntry {
                    name,
                    url: p.url,
                    fingerprint: p.fingerprint,
                    is_default,
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        // Bring the cursor back into range when the list shrank under it.
        if self.servers_cursor >= self.profiles.len() {
            self.servers_cursor = self.profiles.len().saturating_sub(1);
        }
    }

    /// Returns `true` if `tool` is selectable in the picker. Non-probed
    /// names (`terminal`, `bash`, free-form passthrough strings) always
    /// pass; probed names — anything `/api/agents` reports — are gated
    /// on the daemon probe in `agent_availability`. While the probe is
    /// pending (`None`) we fail open so users with an older daemon
    /// don't see spurious blocks.
    pub fn tool_available(&self, tool: &str) -> bool {
        let trimmed = tool.trim();
        if !is_probed_tool(trimmed) {
            return true;
        }
        match &self.agent_availability {
            Some(set) => set.contains(trimmed),
            None => true,
        }
    }

    /// Drop notifications whose TTL has elapsed. Called on every run-loop
    /// tick (~100 ms). O(`MAX_NOTIFS`) per frame — cheaper than threading
    /// a per-toast `tokio::sleep` future into the select.
    pub fn tick_expire(&mut self) {
        let now = Instant::now();
        self.notifications
            .retain(|n| now.saturating_duration_since(n.created_at) < n.ttl);
    }

    pub fn set_theme(&mut self, name: &str) {
        self.theme = Theme::by_name(name);
        theme::save(self.theme.name);
        self.status_msg = Some(format!("theme: {}", self.theme.name));
        self.pending_pref_push = Some(self.theme.name.to_string());
    }

    /// Apply a theme by name without queueing a server push. Used by
    /// the `preferences.changed` event handler so adopting an
    /// externally-pushed theme doesn't immediately echo back to the
    /// server (which would round-trip endlessly across surfaces).
    pub fn set_theme_by_name(&mut self, name: &str) {
        let resolved = Theme::by_name(name);
        if resolved.name == self.theme.name {
            return;
        }
        self.theme = resolved;
        theme::save(self.theme.name);
        self.status_msg = Some(format!("theme: {} (synced)", self.theme.name));
    }

    pub fn cycle_theme(&mut self) {
        self.theme = Theme::next(self.theme.name);
        theme::save(self.theme.name);
        self.status_msg = Some(format!("theme: {}", self.theme.name));
        self.pending_pref_push = Some(self.theme.name.to_string());
    }

    pub fn selected_session(&self) -> Option<&Session> {
        let id = self.selected?;
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Replace the session list with a freshly aggregated one and
    /// also refresh the owner map. Used by call sites that just did
    /// a multi-server fanout — the existing `refresh_sessions`
    /// would otherwise leave the owner map stale (newly-created
    /// peer sessions wouldn't be tagged).
    pub fn refresh_sessions_with_owners(
        &mut self,
        sessions: Vec<Session>,
        owners: HashMap<Uuid, String>,
    ) {
        self.session_profile = owners;
        self.refresh_sessions(sessions);
    }

    pub fn refresh_sessions(&mut self, sessions: Vec<Session>) {
        let prev_state: HashMap<String, bool> = self
            .tree
            .groups
            .iter()
            .map(|g| (format!("{}::{}", g.profile, g.workdir), g.expanded))
            .collect();
        // Preserve the active filter across rebuilds — the user's typed
        // search shouldn't vanish just because the session list changed.
        let prev_filter = self.tree.filter_str().to_string();
        self.sessions = sessions;
        self.tree =
            Tree::build_with_profiles(&self.sessions, &self.session_profile, &prev_state);
        if !prev_filter.is_empty() {
            self.tree.set_filter(&prev_filter);
        }
        if let Some(sel) = self.selected
            && !self.sessions.iter().any(|s| s.id == sel)
        {
            self.selected = first_visible_session(&self.tree, &self.sessions);
            self.term.reset();
        }
        // Make sure cursor still points at a valid row.
        self.tree.clamp_cursor();
        if let Some(id) = self.selected {
            self.tree.select_session(id);
        }
    }

    pub fn lazygit_open(&self) -> bool {
        self.lazygit.is_some()
    }

    /// True when the right-pane split is active.
    pub fn split_open(&self) -> bool {
        self.split_right.is_some()
    }

    /// Which terminal slot tree-driven selection changes should retarget.
    /// `Term` / `TermRight` focus pin it explicitly; from `Tree` /
    /// `Lazygit` we fall back to whichever side the user last typed into.
    pub fn target_side(&self) -> Side {
        match self.focus {
            Focus::Term => Side::Left,
            Focus::TermRight => Side::Right,
            _ => self.last_term_side,
        }
    }

    /// Append an error message to the visible log and bump the counter.
    /// Status bar deliberately is NOT touched here: the status row at the
    /// bottom is reserved for non-error feedback ("filter cleared",
    /// "refreshed", session-event notifications) so error chatter doesn't
    /// drown those signals out — the user opens the errors overlay (`e`
    /// from tree focus, or via the palette) to read what failed.
    pub fn push_error(&mut self, text: impl Into<String>) {
        const MAX_ERROR_LOG: usize = 200;
        // Suppress an identical error message that was just pushed —
        // typing into a dead WS channel hits `push_error` once per
        // keystroke (api.rs:1808) and used to spam the overlay with
        // 25+ identical "terminal stream closed — Ctrl-E tree" lines
        // until the user reconnected. Distinct messages, or the same
        // message after a quiet window, still pass through. 2 s is a
        // sweet spot: short enough that a real recurrence within a
        // session feels live, long enough to collapse a typing burst.
        const DEDUP_WINDOW: Duration = Duration::from_secs(2);
        let text = text.into();
        if text.is_empty() {
            return;
        }
        let now = SystemTime::now();
        if let Some(last) = self.errors.last()
            && last.text == text
            && now
                .duration_since(last.at)
                .map(|d| d < DEDUP_WINDOW)
                .unwrap_or(false)
        {
            return;
        }
        self.errors.push(ErrorEntry { at: now, text });
        let n = self.errors.len();
        if n > MAX_ERROR_LOG {
            self.errors.drain(0..n - MAX_ERROR_LOG);
        }
        self.error_count = self.error_count.saturating_add(1);
    }

    /// Drop the visible log + counter. Wired to `c` inside the errors
    /// overlay; useful after the user has read and acknowledged a batch
    /// of failures.
    pub fn clear_errors(&mut self) {
        self.errors.clear();
        self.error_count = 0;
        self.errors_scroll = 0;
    }

    /// Move focus, keeping `last_term_side` in sync. Call this instead of
    /// assigning `self.focus = …` directly so tree → term hand-off
    /// remembers which side the user was on. Tree / Lazygit focus
    /// changes are passthroughs — they don't touch `last_term_side`,
    /// so the prior value (the side the user was last typing in) is
    /// preserved across tree visits.
    pub fn set_focus(&mut self, f: Focus) {
        self.focus = f;
        match f {
            Focus::Term => self.last_term_side = Side::Left,
            Focus::TermRight => self.last_term_side = Side::Right,
            _ => {}
        }
    }
}

/// One-call helper for refresh sites: fan out across every live
/// server, then atomically replace the session list + owner map.
/// Replaces the historical `if let Ok(fresh) = client.list_sessions()`
/// pattern; aggregation never errors (per-server failures degrade
/// to an empty list for that profile).
pub async fn refresh_all(app: &mut App) {
    let (fresh, owners) = aggregate_sessions_with_owners(&*app).await;
    app.refresh_sessions_with_owners(fresh, owners);
}

/// Re-fetch the session list from every live server and merge them
/// into one aggregated `Vec<Session>` plus an updated owner map keyed
/// by session id. Used in place of `client.list_sessions()` at every
/// refresh call site so a session created on a peer server actually
/// shows up in the sidebar without forcing a profile switch.
///
/// Returns the merged session list and the owner map; the caller
/// applies both in lockstep via `App::refresh_sessions` for tree
/// rebuild + selection clamping in the same pattern as the
/// single-client path.
pub async fn aggregate_sessions_with_owners(
    app: &App,
) -> (Vec<Session>, HashMap<Uuid, String>) {
    let active_key = app.active_profile.clone().unwrap_or_default();
    let probes: Vec<_> = app
        .live_clients()
        .map(|(name, c)| {
            let name = name.to_string();
            let c = c.clone();
            async move { (name, c.list_sessions().await.unwrap_or_default()) }
        })
        .collect();
    let results = futures_util::future::join_all(probes).await;
    merge_sessions_dedup(results, &active_key)
}

/// Merge per-profile session lists into a single owner-tagged vec,
/// keeping exactly one copy of each session id. Two profiles pointing
/// at the same daemon would otherwise produce phantom duplicates that
/// flicker in/out as events repopulate the tree.
///
/// Owner preference for a contested id: active profile > any named
/// profile > loopback ("") > first-seen.
pub fn merge_sessions_dedup(
    per_profile: Vec<(String, Vec<Session>)>,
    active_key: &str,
) -> (Vec<Session>, HashMap<Uuid, String>) {
    let mut merged: Vec<Session> = Vec::new();
    let mut owners: HashMap<Uuid, String> = HashMap::new();
    let mut idx_by_id: HashMap<Uuid, usize> = HashMap::new();
    for (name, list) in per_profile {
        for s in list {
            match idx_by_id.get(&s.id).copied() {
                None => {
                    idx_by_id.insert(s.id, merged.len());
                    owners.insert(s.id, name.clone());
                    merged.push(s);
                }
                Some(idx) => {
                    let current = owners.get(&s.id).cloned().unwrap_or_default();
                    if owner_beats(&name, &current, active_key) {
                        owners.insert(s.id, name.clone());
                        merged[idx] = s;
                    }
                }
            }
        }
    }
    (merged, owners)
}

fn owner_beats(candidate: &str, current: &str, active_key: &str) -> bool {
    if candidate == current {
        return false;
    }
    if candidate == active_key {
        return true;
    }
    if current == active_key {
        return false;
    }
    // Named profile wins over the loopback "" key; otherwise first-seen.
    current.is_empty() && !candidate.is_empty()
}

// ---------- Tree ----------

pub struct Tree {
    pub groups: Vec<Group>,
    pub cursor: usize, // index into the flattened visible row list
    /// Active filter (lowercased). Empty string = no filter. Changes
    /// what `rows()` emits, which automatically threads through every
    /// cursor method (`move_cursor`, `clamp_cursor`, `current_row`,
    /// `current_session`) since they all read `self.rows()`.
    filter: String,
    /// Lowercased session names, keyed by id. Refreshed each time
    /// sessions change so `rows()` can apply the filter without
    /// borrowing the global `&[Session]`. The cache is the price of
    /// keeping `rows()` parameter-free; it stales between session
    /// refreshes, so `App::refresh_sessions` must call
    /// `Tree::refresh_names` whenever the session list changes.
    name_index: HashMap<Uuid, String>,
}

pub struct Group {
    /// Owning profile name; `""` for the default / loopback / `--api`
    /// connection. Used as the primary sort key + drives the
    /// `@profile · workdir` label in the sidebar so a multi-server
    /// fleet reads correctly even when several daemons happen to
    /// share a workdir.
    pub profile: String,
    pub workdir: String,
    pub sessions: Vec<Uuid>,
    pub expanded: bool,
}

#[derive(Clone, Copy)]
pub enum Row {
    Group(usize),
    Leaf { group: usize, leaf: usize },
}

/// Strip a single trailing `/` (but keep the root `/`) so `/foo` and
/// `/foo/` collapse to one tree group instead of two.
pub fn normalize_workdir(p: &str) -> String {
    if p.len() > 1 && p.ends_with('/') {
        p.trim_end_matches('/').to_string()
    } else {
        p.to_string()
    }
}

/// Friendly label for a tree group: the basename of the workdir, with
/// `~` for the home dir. Falls back to the (collapsed) path if there's
/// no basename — only happens for filesystem-root groups.
pub fn group_label(workdir: &str) -> String {
    let collapsed = collapse_home_str(workdir);
    if collapsed == "~" || collapsed == "/" {
        return collapsed;
    }
    let basename = std::path::Path::new(workdir)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if basename.is_empty() {
        collapsed
    } else {
        basename.to_string()
    }
}

fn collapse_home_str(p: &str) -> String {
    if let Some(home) = std::env::var_os("HOME").and_then(|h| h.into_string().ok())
        && !home.is_empty()
    {
        if p == home {
            return "~".to_string();
        }
        if let Some(rest) = p.strip_prefix(&format!("{home}/")) {
            return format!("~/{rest}");
        }
    }
    p.to_string()
}

impl Tree {
    pub fn build(sessions: &[Session], prev_expanded: &HashMap<String, bool>) -> Self {
        Self::build_with_profiles(sessions, &HashMap::new(), prev_expanded)
    }

    /// Multi-server variant: groups by `(profile, workdir)` so the
    /// sidebar can render every server's sessions inline, sorted
    /// with the default profile (empty key) first. `session_profile`
    /// maps session id → owning profile name; missing entries fall
    /// back to the default key.
    pub fn build_with_profiles(
        sessions: &[Session],
        session_profile: &HashMap<Uuid, String>,
        prev_expanded: &HashMap<String, bool>,
    ) -> Self {
        // Normalize workdirs before grouping so `/x/proj` and `/x/proj/`
        // don't show up as two separate groups. Group key is the
        // composite (profile_name, workdir) so two servers that
        // share a workdir don't accidentally merge.
        let mut by_key: HashMap<(String, String), Vec<&Session>> = HashMap::new();
        for s in sessions {
            let profile = session_profile.get(&s.id).cloned().unwrap_or_default();
            let key = (profile, normalize_workdir(&s.workdir));
            by_key.entry(key).or_default().push(s);
        }
        let mut keys: Vec<(String, String)> = by_key.keys().cloned().collect();
        // Default profile (empty key) first; then alphabetical by
        // profile name; then by workdir within each profile.
        keys.sort_by(|a, b| match (a.0.is_empty(), b.0.is_empty()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.cmp(b),
        });
        let groups: Vec<Group> = keys
            .into_iter()
            .map(|(profile, workdir)| {
                let mut sess = by_key
                    .remove(&(profile.clone(), workdir.clone()))
                    .unwrap();
                sess.sort_by(|a, b| a.name.cmp(&b.name));
                let expand_key = format!("{profile}::{workdir}");
                Group {
                    expanded: *prev_expanded.get(&expand_key).unwrap_or(&true),
                    sessions: sess.iter().map(|s| s.id).collect(),
                    profile,
                    workdir,
                }
            })
            .collect();
        let name_index = sessions
            .iter()
            .map(|s| (s.id, s.name.to_ascii_lowercase()))
            .collect();
        Self {
            groups,
            cursor: 0,
            filter: String::new(),
            name_index,
        }
    }

    /// Read the active filter (lowercased, exactly as set).
    pub fn filter_str(&self) -> &str {
        &self.filter
    }

    /// Replace the active filter and re-clamp the cursor onto a still-visible
    /// row. Filter strings are stored lowercased so matching is case-insensitive.
    pub fn set_filter(&mut self, needle: &str) {
        self.filter = needle.trim().to_ascii_lowercase();
        self.clamp_cursor();
    }

    pub fn rows(&self) -> Vec<Row> {
        let needle = self.filter.as_str();
        let filtering = !needle.is_empty();
        let mut rows = Vec::new();
        for (gi, g) in self.groups.iter().enumerate() {
            // While filtering, only count leaves whose name contains the
            // needle; while unfiltered, all leaves count.
            let visible_leaves: Vec<usize> = if filtering {
                (0..g.sessions.len())
                    .filter(|li| {
                        let id = g.sessions[*li];
                        self.name_index
                            .get(&id)
                            .is_some_and(|n| n.contains(needle))
                    })
                    .collect()
            } else {
                (0..g.sessions.len()).collect()
            };
            // Drop empty groups while filtering; otherwise keep the
            // group header so the user can expand/collapse it.
            if filtering && visible_leaves.is_empty() {
                continue;
            }
            rows.push(Row::Group(gi));
            // Filter mode forces every group expanded — flat search behaves
            // like a single list. Unfiltered mode honours `g.expanded`.
            if filtering || g.expanded {
                for li in visible_leaves {
                    rows.push(Row::Leaf {
                        group: gi,
                        leaf: li,
                    });
                }
            }
        }
        rows
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let len = self.rows().len();
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let cur = self.cursor as i32 + delta;
        let clamped = cur.clamp(0, len as i32 - 1);
        self.cursor = clamped as usize;
    }

    pub fn clamp_cursor(&mut self) {
        let len = self.rows().len();
        if len == 0 {
            self.cursor = 0;
        } else if self.cursor >= len {
            self.cursor = len - 1;
        }
    }

    pub fn current_row(&self) -> Option<Row> {
        self.rows().get(self.cursor).copied()
    }

    pub fn current_session(&self, sessions: &[Session]) -> Option<Uuid> {
        match self.current_row()? {
            Row::Leaf { group, leaf } => Some(self.groups[group].sessions[leaf]),
            Row::Group(gi) => self
                .groups
                .get(gi)
                .and_then(|g| g.sessions.first().copied())
                .filter(|_| !sessions.is_empty()),
        }
    }

    pub fn collapse(&mut self) {
        if let Some(row) = self.current_row() {
            let gi = match row {
                Row::Group(gi) => gi,
                Row::Leaf { group, .. } => group,
            };
            if let Some(g) = self.groups.get_mut(gi) {
                if g.expanded {
                    g.expanded = false;
                    // Move cursor to the group header.
                    self.cursor = self.row_index_of(Row::Group(gi)).unwrap_or(self.cursor);
                }
            }
        }
    }

    pub fn expand(&mut self) {
        if let Some(row) = self.current_row() {
            let gi = match row {
                Row::Group(gi) => gi,
                Row::Leaf { group, .. } => group,
            };
            if let Some(g) = self.groups.get_mut(gi) {
                g.expanded = true;
            }
        }
    }

    fn row_index_of(&self, target: Row) -> Option<usize> {
        for (i, r) in self.rows().iter().enumerate() {
            if matches!((r, target), (Row::Group(a), Row::Group(b)) if *a == b) {
                return Some(i);
            }
        }
        None
    }

    pub fn select_session(&mut self, id: Uuid) {
        for (i, r) in self.rows().iter().enumerate() {
            if let Row::Leaf { group, leaf } = r
                && self.groups[*group].sessions[*leaf] == id
            {
                self.cursor = i;
                return;
            }
        }
    }

    /// Move the cursor to the Nth project group (1-based) and expand it.
    /// Returns true if the group exists.
    pub fn focus_group(&mut self, n: usize) -> bool {
        if n == 0 || n > self.groups.len() {
            return false;
        }
        let gi = n - 1;
        if let Some(g) = self.groups.get_mut(gi) {
            g.expanded = true;
        }
        if let Some(idx) = self.row_index_of(Row::Group(gi)) {
            self.cursor = idx;
        }
        true
    }
}

fn first_visible_session(tree: &Tree, sessions: &[Session]) -> Option<Uuid> {
    for r in tree.rows() {
        if let Row::Leaf { group, leaf } = r {
            return Some(tree.groups[group].sessions[leaf]);
        }
    }
    sessions.first().map(|s| s.id)
}

// ---------- Event loop ----------

#[allow(clippy::too_many_arguments)]
pub async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: Client,
    sessions: Vec<Session>,
    sound_muted: bool,
    active_profile: Option<String>,
    pending: Option<PendingAfterSwitch>,
    extras: Vec<(String, super::ProfileConnect)>,
    session_profile_map: HashMap<Uuid, String>,
) -> Result<RunOutcome> {
    let mut app = App::new(sessions);
    app.sound_muted_cli = sound_muted;
    app.active_profile = active_profile.clone();
    app.reload_profiles();
    app.session_profile = session_profile_map;
    // Rebuild the tree now that we know which profile owns each
    // session. `App::new` builds with an empty profile map, which
    // would put every session in the "default" group; rebuilding
    // here gives the correct (profile, workdir) grouping.
    app.tree = Tree::build_with_profiles(
        &app.sessions,
        &app.session_profile,
        &HashMap::new(),
    );

    // Default profile: the live `client` we got from `connect_once`.
    // Keyed under the active profile name (or "" for loopback) so
    // `client_for_session` finds it via the same lookup as peers.
    let default_key = active_profile.clone().unwrap_or_default();
    app.clients.insert(
        default_key.clone(),
        ClientEntry {
            client: Some(client.clone()),
            status: ServerStatus::Live,
            last_error: None,
            agent_availability: None, // populated below by the same probe path
        },
    );
    // Peer profiles: each one carries its own ClientEntry from the
    // fanout. `extras` was already aggregated into `sessions` and
    // `session_profile` by the caller, so all we do here is insert
    // the entry so future ops can route by profile name.
    for (name, conn) in extras {
        app.clients.insert(
            name,
            ClientEntry {
                client: conn.client,
                status: conn.status,
                last_error: conn.last_error,
                agent_availability: conn.agent_availability,
            },
        );
    }

    // Apply a follow-up action queued by the previous run-loop's
    // profile-switch path. Today only one variant: re-open the
    // New Session form pre-filled. The form's profile field gets
    // normalised to the now-active profile so the user's next Enter
    // creates immediately instead of triggering another switch.
    if let Some(action) = pending {
        match action {
            PendingAfterSwitch::OpenNewSession(mut form) => {
                form.profile = active_profile.clone().unwrap_or_default();
                form.error = None;
                form.submitting = false;
                // Drop straight to the Name field so the user resumes
                // typing where they were going, not at the Profile
                // step they just resolved.
                form.field = NewSessionField::Name;
                app.overlay = Overlay::NewSession(form);
            }
        }
    }

    // One-shot probe of the daemon's PATH so the New Session form can
    // grey out first-class adapters the user hasn't installed (cursor
    // without `cursor-agent`, etc.). Older daemons return 404; that
    // path leaves `agent_availability` as `None` and the picker fails
    // open.
    match client.list_agents().await {
        Ok(list) if !list.is_empty() => {
            app.agent_availability = Some(
                list.into_iter()
                    .filter(|a| a.available)
                    .map(|a| a.name)
                    .collect(),
            );
        }
        _ => {}
    }

    let (term_tx, mut term_rx) = mpsc::unbounded_channel::<TerminalMsg>();
    let (term_tx_right, mut term_rx_right) = mpsc::unbounded_channel::<TerminalMsg>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<EventMsg>();
    let (lg_tx, mut lg_rx) = mpsc::unbounded_channel::<PtyMsg>();
    let (agent_tasks_tx, mut agent_tasks_rx) =
        mpsc::unbounded_channel::<(Uuid, Option<AgentTaskState>)>();
    // Stash cheap clones on `App` so `update_selection` can pick the
    // correct sender by side without re-threading args. The lazygit
    // sender lives here too so `refresh_lazygit_for_selection` can
    // respawn the side pane on project switches without threading a
    // `&Sender` through every handler.
    app.term_tx_left = Some(term_tx.clone());
    app.term_tx_right = Some(term_tx_right);
    app.lg_tx = Some(lg_tx.clone());
    app.agent_tasks_tx = Some(agent_tasks_tx);

    // Subscribe to the daemon's event bus.
    let _events_handle: JoinHandle<()> = client.open_event_stream(event_tx);

    // Open the terminal stream for the initial selection. The handle
    // lives on `App` (left/right slots) instead of the run-loop stack
    // so helper functions can access it through `&mut App` without
    // threading an extra `&mut Option<JoinHandle>` everywhere.
    if let Some(id) = app.selected {
        let (key_tx, key_rx) = mpsc::unbounded_channel::<TermOut>();
        // Initial connect on startup: no cached parser yet, no resume.
        let h = client.open_terminal_stream(id, term_tx.clone(), key_rx, false);
        app.term_in = Some(key_tx);
        app.term_size = (0, 0); // force first resize once we know the pane size
        app.stream_handle_left = Some(h);
    }
    // Pre-warm the agent-tasks cache for every known session in
    // parallel — non-blocking, results stream back via
    // `agent_tasks_rx`. After the first ~tens of ms, every navigation
    // becomes a pure cache hit instead of waiting on a network round
    // trip; events handle freshness from there. We fire for ALL ids
    // (including the initial selection) so the prime path is uniform.
    let prime_ids: Vec<Uuid> = app.sessions.iter().map(|s| s.id).collect();
    for id in prime_ids {
        spawn_agent_tasks_fetch(&mut app, &client, id);
    }

    let mut crossterm_events = EventStream::new();
    let mut tick = interval(TICK_INTERVAL);
    let mut last_refresh = Instant::now();

    loop {
        // Resize each PTY parser to the actual pane area before drawing.
        let size = terminal.size()?;
        let areas = ui::compute_layout(
            ratatui::layout::Rect::new(0, 0, size.width, size.height),
            app.lazygit_open(),
            app.fullscreen,
            app.tree_width,
            app.sidebar_hidden,
            app.split_open(),
            app.right_panel_visible,
            app.lazygit_width,
            app.term_split_pct,
            ui::should_show_reconnect_ui(&app),
        );
        // Cache for the next mouse event — handlers need to know which
        // pane the cursor is over, which only the layout knows.
        app.last_areas = Some(areas);
        let (term_rows, term_cols) = inner_size(areas.terminal);
        app.term.resize(term_rows, term_cols);
        // Tell the daemon (and through it tmux) about the new pane size so
        // the embedded TUI redraws into the right viewport. Without this
        // tmux clamps to its 80×24 default and you get overlapping text.
        if (term_cols, term_rows) != app.term_size
            && term_cols > 0
            && term_rows > 0
            && let Some(tx) = app.term_in.as_ref()
            && tx
                .send(TermOut::Resize {
                    cols: term_cols,
                    rows: term_rows,
                })
                .is_ok()
        {
            app.term_size = (term_cols, term_rows);
        }
        // Mirror the resize plumbing for the right split when active —
        // each side has its own parser, its own remembered size, and its
        // own input channel.
        if let Some(right_area) = areas.terminal_right
            && let Some(slot) = app.split_right.as_mut()
        {
            let (r_rows, r_cols) = inner_size(right_area);
            slot.term.resize(r_rows, r_cols);
            if (r_cols, r_rows) != slot.term_size
                && r_cols > 0
                && r_rows > 0
                && let Some(tx) = slot.term_in.as_ref()
                && tx
                    .send(TermOut::Resize {
                        cols: r_cols,
                        rows: r_rows,
                    })
                    .is_ok()
            {
                slot.term_size = (r_cols, r_rows);
            }
        }
        if let Some(lg) = app.lazygit.as_mut() {
            let (rows, cols) = inner_size(areas.lazygit.unwrap_or(areas.terminal));
            lg.resize(rows, cols);
        }

        terminal.draw(|f| ui::draw(f, &app))?;
        if app.should_quit {
            // Honor a pending profile switch over a plain quit so the
            // wrapper in `commands::terminal::run` can reconnect to
            // the new server without re-entering the OS shell.
            if let Some(name) = app.pending_switch_profile.take() {
                let then = app.pending_after_switch.take();
                return Ok(RunOutcome::SwitchProfile { name, then });
            }
            return Ok(RunOutcome::Quit);
        }

        // If lazygit exited on its own, drop it and revert focus.
        if let Some(lg) = app.lazygit.as_ref()
            && lg.finished()
        {
            app.lazygit = None;
            app.lazygit_cwd = None;
            if app.focus == Focus::Lazygit {
                app.focus = Focus::Tree;
            }
            app.status_msg = Some("lazygit exited".into());
        }

        tokio::select! {
            biased;

            maybe_input = crossterm_events.next() => {
                if let Some(Ok(ev)) = maybe_input {
                    handle_crossterm(&mut app, ev, &client, &lg_tx).await;
                }
            }

            Some(msg) = term_rx.recv() => {
                handle_terminal_msg(&mut app, msg, Side::Left);
                // Drain whatever else is already queued before yielding
                // back to the draw step. A chatty agent (claude code mid-
                // token-stream, a noisy `cargo build`, etc.) used to land
                // one WS frame per parser feed and trigger one ratatui
                // diff-render per chunk. Coalescing collapses bursts into
                // a single redraw — parser-feeds are cheap, the diff
                // render is the expensive part — without affecting input
                // responsiveness because `biased` keeps crossterm ahead
                // of these branches on the next iteration.
                while let Ok(more) = term_rx.try_recv() {
                    handle_terminal_msg(&mut app, more, Side::Left);
                }
                while let Ok(more) = term_rx_right.try_recv() {
                    handle_terminal_msg(&mut app, more, Side::Right);
                }
            }

            Some(msg) = term_rx_right.recv() => {
                handle_terminal_msg(&mut app, msg, Side::Right);
                while let Ok(more) = term_rx_right.try_recv() {
                    handle_terminal_msg(&mut app, more, Side::Right);
                }
                while let Ok(more) = term_rx.try_recv() {
                    handle_terminal_msg(&mut app, more, Side::Left);
                }
            }

            Some(msg) = event_rx.recv() => {
                handle_event_msg(&mut app, msg, &client).await;
            }

            Some(msg) = lg_rx.recv() => {
                handle_lazygit_msg(&mut app, msg);
            }

            Some((id, maybe_state)) = agent_tasks_rx.recv() => {
                // Always drop the in-flight marker so the next
                // navigation/event can re-fetch this id immediately.
                // On transport error the message carries `None` and
                // we deliberately leave the existing cached snapshot
                // alone — better to keep showing the last good plan
                // than to flash an empty panel.
                app.agent_tasks_inflight.remove(&id);
                if let Some(state) = maybe_state {
                    app.agent_tasks.insert(id, state);
                }
            }

            _ = tick.tick() => {
                app.tick_count = app.tick_count.wrapping_add(1);
                // Cheap O(MAX_NOTIFS) sweep — drops expired toasts so the
                // bottom-left stack drains without us having to schedule
                // a per-toast sleep future.
                app.tick_expire();
                // Fan a queued theme change out to the daemon so any open
                // dashboard tab picks it up live. Fire-and-forget — a
                // failed PUT just means surfaces re-sync on next launch
                // via the on-disk theme file.
                if let Some(name) = app.pending_pref_push.take() {
                    let c = client.clone();
                    tokio::spawn(async move {
                        let _ = c.put_preferences(None, Some(&name)).await;
                    });
                }
                // Lazygit follow-up for cross-repo navigation. We defer
                // the PTY respawn out of `update_selection` itself so
                // rapid j/k bursts can't thrash lazygit children: the
                // pending cwd keeps getting overwritten and only the
                // *settled* destination triggers a respawn here. The
                // ~100 ms tick is well below the human "instant"
                // threshold yet long enough that holding j through 50
                // sessions only spawns lazygit once at the end.
                drive_pending_lazygit(&mut app);
                if last_refresh.elapsed() >= REFRESH_INTERVAL {
                    last_refresh = Instant::now();
                    if let Ok(fresh) = client.list_sessions().await {
                        app.refresh_sessions(fresh);
                        // Pre-warm the cache for any newly-discovered
                        // sessions so the first nav to them is also a
                        // pure cache hit. Existing ids skip via the
                        // in-flight dedup or because the channel hasn't
                        // delivered a stale fetch result yet.
                        let new_ids: Vec<Uuid> = app
                            .sessions
                            .iter()
                            .filter(|s| !app.agent_tasks.contains_key(&s.id))
                            .map(|s| s.id)
                            .collect();
                        for id in new_ids {
                            spawn_agent_tasks_fetch(&mut app, &client, id);
                        }
                    }
                    // Slow-path safety net for the agent-tasks panel:
                    // events do the heavy lifting, but this catch-up
                    // makes the panel converge even if a bus.lagged
                    // dropped the relevant `agent_tasks.updated`.
                    if app.right_panel_visible
                        && let Some(id) = app.selected
                    {
                        spawn_agent_tasks_fetch(&mut app, &client, id);
                    }
                }
            }
        }
    }
}

/// Pane size in `(rows, cols)` once we strip the 1-cell border on each side.
fn inner_size(r: ratatui::layout::Rect) -> (u16, u16) {
    let rows = r.height.saturating_sub(2).max(1);
    let cols = r.width.saturating_sub(2).max(1);
    (rows, cols)
}

async fn handle_crossterm(
    app: &mut App,
    ev: CtEvent,
    client: &Client,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
) {
    match ev {
        CtEvent::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
            handle_key(app, key, client, lg_tx).await;
        }
        CtEvent::Mouse(me) => handle_mouse(app, me),
        CtEvent::Resize(_, _) => {}
        _ => {}
    }
}

/// Route mouse events. Three-layer dispatch (most specific wins):
///
/// 1. **Active text selection**: once a click-drag selection has been
///    started inside a pane, drag/up events extend or finalize it,
///    regardless of `wants_mouse`. Releasing the button copies the
///    extracted text to the host terminal's clipboard via OSC 52 and
///    drops the selection.
///
/// 2. **Inner program owns the mouse**: when the embedded TUI (claude
///    code, vim, k9s, htop, …) has turned on mouse tracking, encode the
///    event as an SGR escape sequence and forward it to the pane.
///    Holding **Shift** during left-click bypasses this and starts a
///    selection instead — matching the `xterm` / Alacritty / kitty
///    convention so users can still copy text out of an alt-screen
///    program.
///
/// 3. **Pane owns the mouse**: scroll-wheel ticks drive agentum's own
///    per-pane scrollback. Plain left-click also starts a selection
///    (no Shift needed because nothing else wants the click).
fn handle_mouse(app: &mut App, ev: crossterm::event::MouseEvent) {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
    let Some(areas) = app.last_areas else {
        return; // first frame hasn't drawn yet
    };
    let col = ev.column;
    let row = ev.row;
    let in_rect = |r: ratatui::layout::Rect| {
        r.width > 0
            && r.height > 0
            && col >= r.x
            && col < r.x + r.width
            && row >= r.y
            && row < r.y + r.height
    };
    let (side, rect) = if let Some(right_rect) = areas.terminal_right
        && in_rect(right_rect)
    {
        (Side::Right, right_rect)
    } else if in_rect(areas.terminal) {
        (Side::Left, areas.terminal)
    } else {
        return;
    };

    // Translate absolute terminal coords into 1-based pane-local
    // coords. The panel border is 1 cell, so the inner content starts
    // at `(rect.x + 1, rect.y + 1)`.
    let pane_col = col.saturating_sub(rect.x.saturating_add(1)).saturating_add(1);
    let pane_row = row.saturating_sub(rect.y.saturating_add(1)).saturating_add(1);

    // Layer 1: continue an in-progress selection. Once started it
    // captures every mouse event in the same pane until the user
    // releases the button. This is what makes Shift-click-drag feel
    // sticky even after the user lets go of Shift mid-drag.
    if let Some(sel) = app.term_selection.as_mut()
        && sel.dragging
        && sel.side == side
    {
        match ev.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                sel.cursor = (pane_col, pane_row);
                return;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                sel.dragging = false;
                let snapshot = *sel;
                app.term_selection = None;
                if !snapshot.is_empty() {
                    let text = extract_selection_text(app, snapshot);
                    if !text.is_empty() {
                        write_osc52(&text);
                        app.status_msg =
                            Some(format!("copied {} chars", text.chars().count()));
                    }
                }
                return;
            }
            _ => {}
        }
    }

    let wants_mouse = match side {
        Side::Left => app.term.wants_mouse_events(),
        Side::Right => app
            .split_right
            .as_ref()
            .is_some_and(|s| s.term.wants_mouse_events()),
    };
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);

    // Layer 2: start a fresh selection on left-click when the inner
    // program either doesn't want the mouse, or the user is asserting
    // override via Shift. This must run BEFORE the SGR forwarding
    // path so a Shift+click never reaches the embedded TUI as input.
    if matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) && (!wants_mouse || shift) {
        app.term_selection = Some(TermSelection {
            side,
            anchor: (pane_col, pane_row),
            cursor: (pane_col, pane_row),
            dragging: true,
        });
        return;
    }

    // Layer 3a: forward to the inner program when it asked for mouse
    // events and the user isn't trying to select.
    if wants_mouse {
        if let Some(seq) = encode_mouse_sgr(ev.kind, ev.modifiers, pane_col, pane_row) {
            let tx = match side {
                Side::Left => app.term_in.as_ref(),
                Side::Right => app.split_right.as_ref().and_then(|s| s.term_in.as_ref()),
            };
            if let Some(tx) = tx {
                let bytes = seq.into_bytes();
                app.io.record_out(bytes.len());
                let _ = tx.send(TermOut::Bytes(bytes));
            }
        }
        return;
    }

    // Layer 3b: local scrollback wheel — only ever ScrollUp / Down.
    let scroll_up = match ev.kind {
        MouseEventKind::ScrollUp => true,
        MouseEventKind::ScrollDown => false,
        _ => return,
    };
    let n = super::term::WHEEL_LINES_PER_TICK;
    match side {
        Side::Left => {
            if scroll_up {
                app.term.scroll_up(n);
            } else {
                app.term.scroll_down(n);
            }
        }
        Side::Right => {
            if let Some(slot) = app.split_right.as_mut() {
                if scroll_up {
                    slot.term.scroll_up(n);
                } else {
                    slot.term.scroll_down(n);
                }
            }
        }
    }
}

/// Walk a vt100 screen between the selection's ordered endpoints and
/// concatenate cell contents. Trailing whitespace per row is stripped
/// (vt100 pads short lines with blanks; users want clean copy). Rows
/// are separated by `\n` so the result drops cleanly into a shell
/// paste / editor / chat client.
fn extract_selection_text(app: &App, sel: TermSelection) -> String {
    let screen = match sel.side {
        Side::Left => app.term.screen(),
        Side::Right => match app.split_right.as_ref() {
            Some(s) => s.term.screen(),
            None => return String::new(),
        },
    };
    let (rows, cols) = screen.size();
    let ((s_col, s_row), (e_col, e_row)) = sel.ordered();
    // Convert 1-based coords to 0-based and clamp to the live screen.
    let s_row0 = s_row.saturating_sub(1).min(rows.saturating_sub(1));
    let e_row0 = e_row.saturating_sub(1).min(rows.saturating_sub(1));
    let s_col0 = s_col.saturating_sub(1).min(cols.saturating_sub(1));
    let e_col0 = e_col.saturating_sub(1).min(cols.saturating_sub(1));

    let mut out = String::new();
    for r in s_row0..=e_row0 {
        let (col_lo, col_hi) = if s_row0 == e_row0 {
            (s_col0.min(e_col0), s_col0.max(e_col0))
        } else if r == s_row0 {
            (s_col0, cols.saturating_sub(1))
        } else if r == e_row0 {
            (0, e_col0)
        } else {
            (0, cols.saturating_sub(1))
        };
        let mut line = String::new();
        for c in col_lo..=col_hi {
            if let Some(cell) = screen.cell(r, c) {
                line.push_str(&cell.contents());
            }
        }
        // vt100 pads short lines with empty-content cells; strip
        // trailing whitespace so the copy reflects what the user sees.
        let trimmed = line.trim_end();
        out.push_str(trimmed);
        if r != e_row0 {
            out.push('\n');
        }
    }
    out
}

/// Copy text to the host terminal's clipboard via OSC 52.
///
/// DISABLED in v0.6.33+ pending a proper deferred-emission rewrite.
/// The previous implementation wrote the OSC 52 sequence directly to
/// stdout from within the input handler, *mid-frame*, while ratatui
/// owned the screen. Two compounding failure modes followed:
///
///   1. Inside tmux (TERM=tmux-256color or $TMUX set), OSC 52 must be
///      wrapped in DCS passthrough (`\x1bPtmux;\x1b…\x1b\\`) or tmux
///      will swallow / partially echo the sequence as literal text.
///   2. Even on tmux-free terminals, the raw write bypasses ratatui's
///      diff renderer — the next `terminal.draw()` call only patches
///      cells ratatui *thinks* are dirty, so anything the OSC payload
///      disturbed in the actual terminal stays disturbed.
///
/// Net effect: visible text corruption every time a selection drag
/// ends in a pane. Until we plumb the OSC sequence through a
/// between-frames flush queue, we just drop it on the floor — the
/// in-buffer selection highlight still renders correctly, the user
/// just doesn't get the host-clipboard copy.
fn write_osc52(_text: &str) {}


/// Encode a crossterm mouse event as an xterm SGR sequence (DECSET 1006
/// / `\x1b[<…M|m`). xterm SGR is what every modern alt-screen TUI
/// requests, so always emitting it is the safe default.
///
/// - Press codes: 0 = left, 1 = middle, 2 = right
/// - Drag adds +32; the button code is preserved
/// - Scroll wheel: 64 = up, 65 = down (66/67 = horizontal)
/// - Modifier bits: shift +4, alt +8, ctrl +16
/// - Trailing `M` for press / motion / scroll, `m` for release
fn encode_mouse_sgr(
    kind: crossterm::event::MouseEventKind,
    mods: crossterm::event::KeyModifiers,
    col: u16,
    row: u16,
) -> Option<String> {
    use crossterm::event::{KeyModifiers, MouseEventKind};
    let (mut btn, action): (u32, char) = match kind {
        MouseEventKind::Down(b) => (button_code(b), 'M'),
        MouseEventKind::Up(b) => (button_code(b), 'm'),
        MouseEventKind::Drag(b) => (button_code(b) + 32, 'M'),
        MouseEventKind::ScrollUp => (64, 'M'),
        MouseEventKind::ScrollDown => (65, 'M'),
        MouseEventKind::ScrollLeft => (66, 'M'),
        MouseEventKind::ScrollRight => (67, 'M'),
        // Bare-move events (no button) are AnyMotion-mode-only. We
        // don't inspect the requested mode here, so drop them — most
        // apps don't ask for this and the event flood would saturate
        // the stream on trackpad pointer movement anyway.
        MouseEventKind::Moved => return None,
    };
    if mods.contains(KeyModifiers::SHIFT) {
        btn += 4;
    }
    if mods.contains(KeyModifiers::ALT) {
        btn += 8;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        btn += 16;
    }
    Some(format!("\x1b[<{btn};{col};{row}{action}"))
}

fn button_code(b: crossterm::event::MouseButton) -> u32 {
    use crossterm::event::MouseButton;
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

async fn handle_key(
    app: &mut App,
    key: KeyEvent,
    client: &Client,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
) {
    // Chord follow-up — if the previous key set a chord prefix, this
    // keystroke completes (or cancels) it. Done before the palette /
    // quit / cycle handlers so chord branches like `Ctrl-K Z` aren't
    // interpreted as the standalone meaning of `Z`.
    if let Some('K') = app.chord.take() {
        // VS Code parity: Ctrl-K Z (zen) toggles fullscreen, Ctrl-K B
        // toggles the sidebar. Anything else cancels the chord silently.
        let c = match key.code {
            KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        match c {
            Some('z') => {
                app.fullscreen = !app.fullscreen;
                if app.fullscreen && app.focus == Focus::Tree {
                    app.set_focus(Focus::Term);
                }
                app.status_msg = Some(if app.fullscreen {
                    "fullscreen on (Ctrl-K Z or Esc to exit)".into()
                } else {
                    "fullscreen off".into()
                });
            }
            Some('b') => {
                app.sidebar_hidden = !app.sidebar_hidden;
                if app.sidebar_hidden && app.focus == Focus::Tree {
                    app.set_focus(Focus::Term);
                }
                app.prefs.sidebar_hidden = app.sidebar_hidden;
                prefs::save(&app.prefs);
                app.status_msg = Some(if app.sidebar_hidden {
                    "sidebar hidden".into()
                } else {
                    "sidebar visible".into()
                });
            }
            // Lazygit width resize. `,` / `<` shrink, `.` / `>` grow.
            // Works from any focus (including the lazygit pane) because
            // the chord prefix runs ahead of pane key forwarding. Step
            // size is 4 cols — same cadence as the sidebar's `+`/`-`.
            Some(',') | Some('<') => {
                app.lazygit_width = app
                    .lazygit_width
                    .saturating_sub(4)
                    .max(ui::LAZYGIT_MIN_WIDTH);
                app.prefs.lazygit_width = app.lazygit_width;
                prefs::save(&app.prefs);
                app.status_msg = Some(format!("lazygit width: {}", app.lazygit_width));
            }
            Some('.') | Some('>') => {
                app.lazygit_width = app
                    .lazygit_width
                    .saturating_add(4)
                    .min(ui::LAZYGIT_MAX_WIDTH);
                app.prefs.lazygit_width = app.lazygit_width;
                prefs::save(&app.prefs);
                app.status_msg = Some(format!("lazygit width: {}", app.lazygit_width));
            }
            _ => {
                app.status_msg = Some("(chord cancelled)".into());
            }
        }
        return;
    }

    // Ctrl-P / Ctrl-Shift-P opens the command palette from anywhere.
    // Highest priority so it works even with a pane focused.
    // Ctrl-Shift-P is the canonical VS Code binding; Ctrl-P matches
    // VS Code's Quick Open and the palette here serves both roles.
    // (Ctrl-K is now a chord prefix — see below — and no longer aliases
    // the palette.)
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))
    {
        app.overlay = Overlay::Palette;
        app.palette = PaletteState::new();
        return;
    }

    // Ctrl-K — VS Code chord prefix. Standalone: nothing happens; the
    // next keystroke is interpreted as a chord follow-up (handled at
    // the top of this function on the next event). Cleared after the
    // very next event regardless of match, so a stray prefix never
    // sticks. Currently bound: K-Z fullscreen, K-B sidebar.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('k'))
    {
        app.chord = Some('K');
        app.status_msg = Some(
            "Ctrl-K · waiting (Z fullscreen · B sidebar · , / . lazygit width)".into(),
        );
        return;
    }

    // Ctrl-B — VS Code "toggle primary side bar". Hides just the tree
    // column; title and status bars stay so the user keeps the
    // breadcrumb. Distinct from Shift-F / Ctrl-K Z fullscreen which
    // strips everything.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('b'))
    {
        app.sidebar_hidden = !app.sidebar_hidden;
        if app.sidebar_hidden && app.focus == Focus::Tree {
            app.set_focus(Focus::Term);
        }
        app.prefs.sidebar_hidden = app.sidebar_hidden;
        prefs::save(&app.prefs);
        app.status_msg = Some(if app.sidebar_hidden {
            "sidebar hidden (Ctrl-B to reopen)".into()
        } else {
            "sidebar visible".into()
        });
        return;
    }

    // Ctrl-T — toggle the right-side agent-tasks panel (plan / todos /
    // background tasks). Mirror of Ctrl-B for the opposite edge. Hidden
    // automatically on terminals narrower than ~110 cols regardless of
    // this flag — see `ui::compute_layout`.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('t'))
    {
        app.right_panel_visible = !app.right_panel_visible;
        app.prefs.right_panel_visible = app.right_panel_visible;
        prefs::save(&app.prefs);
        app.status_msg = Some(if app.right_panel_visible {
            "agent panel on".into()
        } else {
            "agent panel off".into()
        });
        // Kick a fetch for the current selection so the panel populates
        // immediately on first toggle, even if the events stream hasn't
        // pushed an `agent_tasks.updated` for this session yet.
        if app.right_panel_visible
            && let Some(id) = app.selected
        {
            spawn_agent_tasks_fetch(app, client, id);
        }
        return;
    }

    // Ctrl-Shift-Left / Ctrl-Shift-Right resize the terminal split when
    // one's open. Runs ahead of pane forwarding so it works while a
    // terminal pane has focus (the pane otherwise eats arrow keys as
    // cursor movement). No-op (with status hint) when no split is open.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Left | KeyCode::Right)
    {
        if !app.split_open() {
            app.status_msg = Some("split closed (Ctrl-\\ to open)".into());
            return;
        }
        let pct = app
            .term_split_pct
            .clamp(ui::TERM_SPLIT_MIN_PCT, ui::TERM_SPLIT_MAX_PCT);
        let next = match key.code {
            KeyCode::Left => pct.saturating_sub(ui::TERM_SPLIT_STEP),
            KeyCode::Right => pct.saturating_add(ui::TERM_SPLIT_STEP),
            _ => pct,
        };
        app.term_split_pct = next.clamp(ui::TERM_SPLIT_MIN_PCT, ui::TERM_SPLIT_MAX_PCT);
        app.prefs.term_split_pct = app.term_split_pct;
        prefs::save(&app.prefs);
        app.status_msg = Some(format!(
            "split: {}% / {}%",
            app.term_split_pct,
            100 - app.term_split_pct
        ));
        return;
    }

    // Ctrl-, opens the Settings overlay. Mirrors VS Code's "Preferences:
    // Open Settings" binding. Runs ahead of pane forwarding so it works
    // from any focus, including inside the terminal pane.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char(','))
    {
        app.overlay = Overlay::Settings(SettingsState::new());
        app.status_msg = Some(
            "settings (Esc close · ↑↓ move · ←→ adjust · space toggle · r reset row)".into(),
        );
        return;
    }

    // Ctrl-R opens the rename prompt for the highlighted session. Only
    // active when the tree pane has focus — when a terminal is focused
    // Ctrl-R is reverse-search inside the shell and we let it forward
    // through. No-op when no session is highlighted (cursor on a group
    // row, etc.).
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('r'))
        && app.focus == Focus::Tree
    {
        if let Some(sess) = app.selected_session() {
            let id = sess.id;
            let name = sess.name.clone();
            app.overlay = Overlay::Rename(RenameState::new(id, &name));
            app.status_msg = Some("rename (Enter save · Esc cancel)".into());
        } else {
            app.status_msg = Some("no session selected to rename".into());
        }
        return;
    }

    // Ctrl-Tab — flip back to the previously selected session. Mirrors
    // VS Code's "go to last edited file" / iTerm2's "last tab". A
    // no-op when there's no prior session (first run, or the last
    // session was deleted).
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
    {
        if let Some(prev) = app.prev_selected
            && app.sessions.iter().any(|s| s.id == prev)
        {
            app.tree.select_session(prev);
            {
            let side = app.target_side();
            update_selection(app, client, side);
        }
        } else {
            app.status_msg = Some("no previous session".into());
        }
        return;
    }

    // Ctrl-Q is a universal hard-quit. Never forwarded to a pane so the
    // user always has an escape hatch — even if the WS terminal stream is
    // dead and Ctrl-C would otherwise disappear into the SIGINT pipe.
    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    // Ctrl-C only quits when no pane is focused. Inside a pane it's a real
    // SIGINT to whatever's running (claude code, shells, etc.) — otherwise
    // you could never interrupt a long-running task without killing the TUI.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        match app.focus {
            Focus::Term | Focus::Lazygit => {} // fall through to pane forwarding
            _ => {
                app.should_quit = true;
                return;
            }
        }
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl-D — delete an server when the Servers sidebar section
    // has focus. Shows a confirmation prompt to prevent accidental
    // deletion. When a terminal pane is focused, Ctrl-D still
    // forwards EOF (^D) to the running agent — standard Unix
    // behaviour — so this only intercepts when the sidebar cursor
    // is actually on an server entry.
    if ctrl
        && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D'))
        && app.focus == Focus::Tree
        && app.tree_section == TreeSection::Servers
    {
        if let Some(entry) = app.profiles.get(app.servers_cursor).cloned() {
            if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                app.status_msg =
                    Some("can't remove the active server — switch first".into());
            } else {
                app.overlay = Overlay::Confirm(PendingAction::RemoveServer {
                    name: entry.name.clone(),
                });
            }
        }
        return;
    }

    // F5 / F6 — global panel switchers, work even with a pane focused.
    // The Ctrl-Shift-] / Ctrl-Shift-[ pair was removed (it conflicted
    // with bracket typing on emulators that report shifted brackets as
    // unshifted glyphs); F5/F6 stays as the universal cycle.
    if key.code == KeyCode::F(5) {
        app.set_focus(next_focus(app.focus, app.lazygit_open(), app.split_open()));
        return;
    }
    if key.code == KeyCode::F(6) {
        app.set_focus(prev_focus(app.focus, app.lazygit_open(), app.split_open()));
        return;
    }

    // Ctrl-E — toggle focus between the session tree and the terminal.
    // Mirrors VS Code's Ctrl-Shift-E (focus Explorer), but as a single
    // chord that flips back to the pane on the second press so you can
    // ping-pong without reaching for Tab or 1/2. Works from any focus
    // and from any emulator (no Kitty-protocol dependency); lowercase
    // `e` stays unbound so this is conflict-free.
    if ctrl && matches!(key.code, KeyCode::Char('e') | KeyCode::Char('E')) {
        if app.focus == Focus::Tree {
            // Tree → terminal. Restore whichever side the user was
            // last typing in so split-pane workflows survive a round
            // trip through the sidebar.
            app.set_focus(if app.last_term_side == Side::Right && app.split_open() {
                Focus::TermRight
            } else {
                Focus::Term
            });
        } else {
            // Pane → tree. Reveal sidebar / drop fullscreen so the
            // tree we just jumped to is actually visible.
            app.set_focus(Focus::Tree);
            if app.fullscreen {
                app.fullscreen = false;
            }
            if app.sidebar_hidden {
                app.sidebar_hidden = false;
            }
        }
        return;
    }

    // Ctrl-O — open the server switcher overlay from any focus.
    // Mnemonic: "open server". Mirrors the dashboard's
    // ServerSwitcher chip in the topbar. Available from anywhere so
    // a user driving multiple agentum servers can hop without
    // releasing focus; also surfaced in the command palette.
    if ctrl && matches!(key.code, KeyCode::Char('o') | KeyCode::Char('O')) {
        open_profiles_overlay(app);
        return;
    }

    // Ctrl-G — toggle the lazygit side pane from any focus. Plain `g`
    // also toggles it but only fires when the tree is focused (otherwise
    // it gets forwarded as a literal keystroke to the running agent).
    // Ctrl-G is the global escape hatch so you can pop lazygit while
    // typing into claude code without first releasing focus.
    if ctrl && matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G')) {
        toggle_lazygit(app, lg_tx).await;
        return;
    }

    // Ctrl-\ — split the focused terminal pane horizontally. Mirrors
    // VS Code's "Split Editor". Cloning the current selection into the
    // right slot is the common use ("watch two agents in this repo
    // run side-by-side"); the user can then focus the right pane (Tab
    // or F5) and pick a different session via the palette or tree if
    // they want. No-op while lazygit is open — those two features are
    // mutually exclusive (4-column layouts get cramped).
    if ctrl && matches!(key.code, KeyCode::Char('\\')) {
        if app.lazygit_open() {
            app.status_msg = Some("close lazygit (g) before splitting".into());
        } else if app.split_open() {
            app.status_msg = Some("already split — Ctrl-W to close".into());
        } else {
            // Clone the current selection into a fresh slot. The
            // stream is opened by `update_selection` once we set the
            // tree cursor onto the right's session.
            app.split_right = Some(TermSlot {
                selected: None,
                term: TerminalPane::new(),
                term_in: None,
                term_size: (0, 0),
                term_reconnect_pending: false,
            });
            app.set_focus(Focus::TermRight);
            app.last_term_side = Side::Right;
            // Drive the right pane to the tree's current session.
            update_selection(app, client, Side::Right);
            app.status_msg = Some("split (Ctrl-W to close)".into());
        }
        return;
    }

    // Ctrl-W — close the current split. From the right pane: drops it.
    // From the left/tree/lazygit while a split is open: also drops the
    // right slot. No-op when there's nothing to close.
    if ctrl && matches!(key.code, KeyCode::Char('w')) {
        if app.split_open() {
            // Abort the right-pane stream task and drop the slot.
            if let Some(handle) = app.stream_handle_right.take() {
                handle.abort();
            }
            app.split_right = None;
            // Snap focus back to the left if we were on the right.
            if app.focus == Focus::TermRight {
                app.set_focus(Focus::Term);
            }
            app.last_term_side = Side::Left;
            app.status_msg = Some("split closed".into());
        } else {
            app.status_msg = Some("no split to close".into());
        }
        return;
    }

    // Ctrl-1 … Ctrl-9 — jump straight to the Nth project group in the tree
    // and focus the tree. Doesn't auto-select a session — user navigates
    // with arrows + Enter. Works even with a pane focused.
    if ctrl
        && let KeyCode::Char(c) = key.code
        && let Some(n) = c.to_digit(10)
        && (1..=9).contains(&n)
    {
        let n = n as usize;
        if app.tree.focus_group(n) {
            app.focus = Focus::Tree;
            let label = &app.tree.groups[n - 1].workdir;
            app.status_msg = Some(format!("project {n}: {label}"));
        } else {
            app.status_msg = Some(format!("no project {n}"));
        }
        return;
    }

    // Stateful overlays (own input fully).
    match &app.overlay {
        Overlay::Palette => {
            handle_palette_key(app, key, client, lg_tx).await;
            return;
        }
        Overlay::NewSession(_) => {
            handle_new_session_key(app, key, client).await;
            return;
        }
        Overlay::Confirm(_) => {
            handle_confirm_key(app, key, client).await;
            return;
        }
        Overlay::Errors => {
            handle_errors_key(app, key);
            return;
        }
        Overlay::Settings(_) => {
            handle_settings_key(app, key);
            return;
        }
        Overlay::Rename(_) => {
            handle_rename_key(app, key, client).await;
            return;
        }
        Overlay::Profiles(_) => {
            handle_profiles_key(app, key);
            return;
        }
        Overlay::None => {}
        // Help / cheatsheet / install: any of these dismiss it.
        _ => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter
            ) {
                app.overlay = Overlay::None;
            }
            return;
        }
    }

    // Tree-focus session lifecycle — Shift+K/D/U/S act on the highlighted
    // session. Gated to `Focus::Tree` so capital letters typed into
    // claude code (e.g. "KILL THE SERVER" in a prompt) reach the pane
    // instead of being swallowed by a confirmation overlay. From a pane,
    // use the command palette (Ctrl-P) for the same actions.
    let shift = key.modifiers.contains(KeyModifiers::SHIFT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT);
    if shift && app.focus == Focus::Tree {
        match key.code {
            KeyCode::Char('K') => {
                if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Kill {
                        id: s.id,
                        name: s.name.clone(),
                    });
                }
                return;
            }
            KeyCode::Char('D') => {
                // Shift-D is now an alias for Kill — there's no separate
                // delete verb. Same confirmation, same outcome.
                if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Kill {
                        id: s.id,
                        name: s.name.clone(),
                    });
                }
                return;
            }
            KeyCode::Char('U') => {
                if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Start {
                        id: s.id,
                        name: s.name.clone(),
                    });
                }
                return;
            }
            KeyCode::Char('S') => {
                if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Stop {
                        id: s.id,
                        name: s.name.clone(),
                    });
                }
                return;
            }
            _ => {}
        }
    }

    // 't' — spawn a plain terminal (bash shell). Works from tree focus.
    // Modifier-free only: Ctrl-T was previously firing this too, which
    // surprised users who expected Ctrl-T to passthrough (or do nothing)
    // and instead got a "shell-xxxx" notification on every chord press.
    if key.modifiers.is_empty() && key.code == KeyCode::Char('t') && app.focus == Focus::Tree {
        spawn_plain_terminal(app, client).await;
        return;
    }

    // While the lazygit pane is focused, forward raw bytes to its PTY.
    if app.focus == Focus::Lazygit {
        if let Some(lg) = app.lazygit.as_ref()
            && let Some(bytes) = key_to_bytes(&key)
        {
            if let Err(e) = lg.write(&bytes) {
                app.push_error(format!("lazygit write: {e}"));
            }
        }
        return;
    }

    // Stopped/crashed session in the focused term pane: there's no live
    // PTY to forward bytes to, and the empty-screen state used to leave
    // users guessing how to revive it. Accept `u` or `Enter` (without
    // any modifier) as a start shortcut so the prompt drawn by the
    // overlay matches what the keyboard actually does.
    if matches!(app.focus, Focus::Term | Focus::TermRight)
        && key.modifiers.is_empty()
        && matches!(key.code, KeyCode::Char('u') | KeyCode::Enter)
        && let Some(s) = app.selected_session()
        && matches!(s.status, Status::Stopped | Status::Crashed)
    {
        app.overlay = Overlay::Confirm(PendingAction::Start {
            id: s.id,
            name: s.name.clone(),
        });
        return;
    }

    // While the remote terminal pane is focused, forward raw bytes over the
    // WS so they hit the running process (claude code, shell, etc.). Surface
    // both failure modes loudly — silent swallowing was the original "frozen
    // pane" symptom.
    if matches!(app.focus, Focus::Term | Focus::TermRight) {
        // Shift-PgUp / Shift-PgDn drive scrollback on the focused pane
        // instead of being forwarded to claude. Single-keystroke fallback
        // for users without a scroll wheel; mirrors the convention used by
        // gnome-terminal, konsole, and the GNU screen "copy" mode.
        let shift_only = key.modifiers.contains(KeyModifiers::SHIFT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if shift_only && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            let lines = (app.term_size.1 as usize).saturating_sub(1).max(1);
            let target_pane: &mut TerminalPane = match app.focus {
                Focus::TermRight => match app.split_right.as_mut() {
                    Some(slot) => &mut slot.term,
                    None => &mut app.term,
                },
                _ => &mut app.term,
            };
            if matches!(key.code, KeyCode::PageUp) {
                target_pane.scroll_up(lines);
            } else {
                target_pane.scroll_down(lines);
            }
            return;
        }
        let Some(bytes) = key_to_bytes(&key) else {
            return;
        };
        // Any forwarded keystroke means the user is back to interacting
        // live — snap scrollback to the bottom on whichever pane they're
        // typing into. Matches Alacritty / kitty behaviour.
        match app.focus {
            Focus::TermRight => {
                if let Some(slot) = app.split_right.as_mut() {
                    slot.term.scroll_to_bottom();
                }
            }
            _ => app.term.scroll_to_bottom(),
        }
        // Pick the input channel for whichever pane is focused — split
        // pane (Right) routes to its own slot, the legacy single pane
        // (Left) keeps using the flat `term_in`.
        let tx_opt = match app.focus {
            Focus::TermRight => app.split_right.as_ref().and_then(|s| s.term_in.as_ref()),
            _ => app.term_in.as_ref(),
        };
        // Optimistically clear runtime activity state for whichever
        // session we're typing into. The user wouldn't be sending
        // input to a sleeping/awaiting agent unless the dot is about
        // to be wrong — and the watchdog confirms the new state on
        // its next tick anyway. Without this the dot can stay grey
        // after a single Working→Idle cycle if the daemon's
        // `agent.working` event was dropped (bus.lagged, WS hiccup,
        // pre-v0.6.30 daemon). A keypress is a strong "the agent is
        // working again" signal locally — believe it now, let
        // server-side events confirm it within 1 s.
        let typed_id = match app.focus {
            Focus::TermRight => app.split_right.as_ref().and_then(|s| s.selected),
            _ => app.selected,
        };
        if let Some(id) = typed_id {
            app.idle.remove(&id);
            app.awaiting_input.remove(&id);
        }
        let nbytes = bytes.len();
        let send_result = tx_opt.map(|tx| tx.send(TermOut::Bytes(bytes)));
        match send_result {
            Some(Ok(())) => {
                app.io.record_out(nbytes);
            }
            Some(Err(_)) => {
                app.push_error("terminal stream closed — Ctrl-E tree · Ctrl-Q quit");
            }
            None => {
                app.status_msg = Some(
                    "no terminal stream (no session selected?) — Ctrl-E tree · Ctrl-Q quit"
                        .into(),
                );
            }
        }
        return;
    }

    // Filter-input mode (entered with `/` from tree focus) absorbs every
    // keystroke until committed (Enter) or cancelled (Esc). Lives above
    // the tree-key match so chars like `/`, `n`, `q` extend the filter
    // instead of triggering their tree shortcuts.
    if app.filter_input_active {
        handle_filter_input_key(app, &key, client);
        return;
    }

    match key.code {
        // Plain `q` used to quit the app, but it's a high-collision
        // letter — easy to fat-finger after typing into the terminal,
        // and the tree-filter prompt accepts `q` as a search char so
        // there's no good "tree wants q for navigation" excuse to keep
        // it. Ctrl-Q stays as the universal hard-quit (handled
        // earlier in this function); the palette also offers an
        // explicit Quit action.
        KeyCode::Char('?') => app.overlay = Overlay::Help,
        KeyCode::Char('!') => {
            // Open the error log overlay. Always available (even when
            // empty) so the user can confirm "no errors yet" with the
            // same gesture they'd use to investigate a bumped counter.
            // `!` mnemonic for alerts; `e` was a poor pick because it
            // collided with Ctrl-E muscle memory and got typed into
            // session-name fields by mistake.
            app.errors_scroll = 0;
            app.overlay = Overlay::Errors;
        }
        KeyCode::Char('g') => toggle_lazygit(app, lg_tx).await,
        KeyCode::Char('G') => app.overlay = Overlay::LazygitCheats,
        KeyCode::Char('T') => app.cycle_theme(),
        // Shift-F toggles fullscreen (hides title/tree/status). Esc exits
        // when active. Mirrors the web dashboard's Shift+F shortcut so
        // muscle memory carries across surfaces.
        KeyCode::Char('F') => {
            app.fullscreen = !app.fullscreen;
            if app.fullscreen && app.focus == Focus::Tree {
                app.set_focus(Focus::Term);
            }
            app.status_msg = Some(if app.fullscreen {
                "fullscreen on (Shift-F or Esc to exit)".into()
            } else {
                "fullscreen off".into()
            });
        }
        KeyCode::Esc if app.fullscreen => {
            app.fullscreen = false;
            app.status_msg = Some("fullscreen off".into());
        }
        // Esc clears an active tree filter when filter-input mode isn't
        // running (that case is handled inside `handle_filter_input_key`).
        // Lets the user blow away a stale filter without remembering the
        // exact magic key they typed.
        KeyCode::Esc if !app.tree.filter_str().is_empty() => {
            app.tree.set_filter("");
            app.status_msg = Some("filter cleared".into());
        }
        // Ctrl-F starts filter-input mode. Any prior filter is cleared so
        // each press is a fresh search — mirrors VS Code / browser Find.
        // Plain `/` was previously the trigger but was eating arrow keys
        // when accidentally pressed during navigation.
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.tree.set_filter("");
            app.filter_input_active = true;
            app.status_msg = Some("⌃F search (Esc cancel · Enter keep · ↑↓ move)".into());
        }
        // Resize the sidebar tree. 4-col steps, clamped 16..=80; the
        // terminal pane keeps its 20-col floor at draw time. Works
        // regardless of focus so it's reachable from any panel.
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.tree_width = app.tree_width.saturating_add(4).min(80);
            app.prefs.tree_width = app.tree_width;
            prefs::save(&app.prefs);
            app.status_msg = Some(format!("tree width: {}", app.tree_width));
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            app.tree_width = app.tree_width.saturating_sub(4).max(16);
            app.prefs.tree_width = app.tree_width;
            prefs::save(&app.prefs);
            app.status_msg = Some(format!("tree width: {}", app.tree_width));
        }
        // 1/2/3 jump straight to a panel (Tree/Term/Lazygit).
        KeyCode::Char('1') => app.focus = Focus::Tree,
        KeyCode::Char('2') => app.focus = Focus::Term,
        KeyCode::Char('3') if app.lazygit_open() => app.focus = Focus::Lazygit,
        // Tab / Shift-Tab cycle focus. The plain `]` / `[` aliases were
        // removed — they collided with bracket typing and the user
        // didn't use them. F5 / F6 stays as the universal cycle for
        // emulators that send ambiguous Tab codes.
        KeyCode::Tab => {
            app.set_focus(next_focus(app.focus, app.lazygit_open(), app.split_open()));
        }
        KeyCode::BackTab => {
            app.set_focus(prev_focus(app.focus, app.lazygit_open(), app.split_open()));
        }
        KeyCode::Char('r') => {
            // Manual refresh aggregates across every live server
            // so a session created on a peer (via TUI on another
            // host, or via the dashboard) shows up here without
            // restart.
            refresh_all(app).await;
            app.status_msg = Some("refreshed (all servers)".into());
        }
        KeyCode::Char('j') | KeyCode::Down => {
            // Cursor moves through Servers first, then Sessions. At
            // the bottom of Servers, a `j` flips the section to
            // Sessions. Inside Sessions the original tree cursor
            // drives selection.
            match app.tree_section {
                TreeSection::Servers => {
                    if app.servers_cursor + 1 < app.profiles.len() {
                        app.servers_cursor += 1;
                    } else {
                        app.tree_section = TreeSection::Sessions;
                    }
                }
                TreeSection::Sessions => {
                    app.tree.move_cursor(1);
                    let side = app.target_side();
                    update_selection(app, client, side);
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            match app.tree_section {
                TreeSection::Servers => {
                    app.servers_cursor = app.servers_cursor.saturating_sub(1);
                }
                TreeSection::Sessions => {
                    if app.tree.cursor == 0 && !app.profiles.is_empty() {
                        // At the top of the sessions list, flip focus
                        // into the Servers header so a `k` from the
                        // first session reaches the last server.
                        app.tree_section = TreeSection::Servers;
                        app.servers_cursor = app.profiles.len().saturating_sub(1);
                    } else {
                        app.tree.move_cursor(-1);
                        let side = app.target_side();
                        update_selection(app, client, side);
                    }
                }
            }
        }
        KeyCode::Char('h') | KeyCode::Left => app.tree.collapse(),
        KeyCode::Char('l') | KeyCode::Right => app.tree.expand(),
        KeyCode::Char(' ') => {
            // Space: select the session under the cursor AND jump focus
            // into the terminal so the user can start typing immediately.
            // On a group row, this is a no-op (update_selection ignores
            // group rows). Enter is reserved for an upcoming multi-select
            // mode (see WIP note below).
            let on_leaf = matches!(app.tree.current_row(), Some(Row::Leaf { .. }));
            {
                let side = app.target_side();
                update_selection(app, client, side);
            }
            if on_leaf && app.selected.is_some() {
                app.set_focus(Focus::Term);
            }
        }
        KeyCode::Enter => {
            match app.tree_section {
                TreeSection::Servers => {
                    // Switch to the highlighted profile via the same
                    // soft-restart path the Ctrl-O overlay uses. No
                    // pending follow-up — the user's intent is just
                    // "drive that server now".
                    if let Some(entry) = app.profiles.get(app.servers_cursor) {
                        if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                            app.status_msg = Some(format!("already on @{}", entry.name));
                        } else {
                            app.pending_switch_profile = Some(entry.name.clone());
                            app.pending_after_switch = None;
                            app.should_quit = true;
                        }
                    }
                }
                TreeSection::Sessions => {
                    app.status_msg = Some(
                        "multi-select coming soon — use Space to enter the terminal".into(),
                    );
                }
            }
        }
        // Servers section actions: `a` adds, `d` removes. Only fire
        // when the cursor is actually in the Servers section so the
        // Sessions tree's existing `d` (delete-session) keybind, if
        // any, doesn't get hijacked.
        KeyCode::Char('a') if app.tree_section == TreeSection::Servers => {
            // Reuse the same overlay the Ctrl-O switcher uses; the
            // overlay's add-form handles validation + persistence.
            open_profiles_overlay(app);
            if let Overlay::Profiles(ref mut state) = app.overlay {
                state.add_form = Some(AddProfileForm::new());
            }
        }
        KeyCode::Char('d') if app.tree_section == TreeSection::Servers => {
            if let Some(entry) = app.profiles.get(app.servers_cursor).cloned() {
                if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                    app.status_msg =
                        Some("can't remove the active server — switch first".into());
                } else if let Ok(mut store) = super::profiles::Profiles::load() {
                    let _ = store.remove(&entry.name);
                    app.reload_profiles();
                    app.status_msg = Some(format!("removed `{}`", entry.name));
                }
            }
        }

        // Session lifecycle ------------------------------------------------
        KeyCode::Char('n') => {
            // Default the workdir to the selected session's workdir if any,
            // else the user's $HOME. Saves typing for "another agent in
            // the same repo" — by far the most common case.
            let workdir = app
                .selected_session()
                .map(|s| s.workdir.clone())
                .or_else(|| std::env::var("HOME").ok())
                .unwrap_or_default();
            // Seed the form's Profile field with the active one so
            // the user's first Enter creates on the current daemon.
            // `cycle_profile` from there walks the rest.
            let profile = app.active_profile.clone().unwrap_or_default();
            app.overlay =
                Overlay::NewSession(Box::new(NewSessionForm::with_profile(profile, workdir)));
        }
        KeyCode::Char('u') => {
            if let Some(s) = app.selected_session() {
                app.overlay = Overlay::Confirm(PendingAction::Start {
                    id: s.id,
                    name: s.name.clone(),
                });
            } else {
                app.status_msg = Some("no session selected".into());
            }
        }
        KeyCode::Char('s') => {
            if let Some(s) = app.selected_session() {
                app.overlay = Overlay::Confirm(PendingAction::Stop {
                    id: s.id,
                    name: s.name.clone(),
                });
            } else {
                app.status_msg = Some("no session selected".into());
            }
        }
        KeyCode::Char('K') => {
            if let Some(s) = app.selected_session() {
                app.overlay = Overlay::Confirm(PendingAction::Kill {
                    id: s.id,
                    name: s.name.clone(),
                });
            } else {
                app.status_msg = Some("no session selected".into());
            }
        }
        KeyCode::Char('D') | KeyCode::Char('x') => {
            // `x` is the discoverable vim/fzf-style alias; Shift-D is the
            // muscle-memory binding. Both route to the same Kill prompt
            // as Shift-K — there's no separate delete verb anymore.
            if let Some(s) = app.selected_session() {
                app.overlay = Overlay::Confirm(PendingAction::Kill {
                    id: s.id,
                    name: s.name.clone(),
                });
            } else {
                app.status_msg = Some("no session selected".into());
            }
        }

        _ => {}
    }
}

async fn handle_new_session_key(
    app: &mut App,
    key: KeyEvent,
    client: &Client,
) {
    let Overlay::NewSession(mut form) = std::mem::replace(&mut app.overlay, Overlay::None) else {
        return;
    };
    if form.submitting {
        app.overlay = Overlay::NewSession(form);
        return;
    }

    // Picker overlay: input goes there as long as it's open.
    if form.picker.is_some() {
        handle_dir_picker_key(&mut form, key, client).await;
        app.overlay = Overlay::NewSession(form);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            // Drop overlay, dropping the form.
            return;
        }

        // Tab/Shift-Tab moves between fields, with two exceptions:
        //   - Tool: Tab cycles through suggestions (web datalist parity)
        //   - Workdir: Tab attempts shell-style path autocompletion. If
        //     nothing to complete (no prefix, no matches, already at a
        //     full match) it falls through to next_field so Tab still
        //     advances the form when the path is empty / done.
        KeyCode::Tab => match form.field {
            NewSessionField::Profile => {
                // Cycle through configured profiles plus an empty
                // entry meaning "current connection". When only the
                // empty entry exists (no profiles defined), advance
                // to the next field so the user isn't trapped.
                let names: Vec<String> =
                    app.profiles.iter().map(|p| p.name.clone()).collect();
                if names.is_empty() {
                    form.next_field();
                } else {
                    let old_profile = form.profile.clone();
                    form.cycle_profile(&names);
                    // When the profile changes, fetch the default
                    // workdir from the newly-selected server so the
                    // user doesn't need to retype a path that may not
                    // exist on the other machine.
                    if form.profile != old_profile {
                        let target_client = if form.profile.is_empty() {
                            Some(client.clone())
                        } else {
                            app.clients
                                .get(&form.profile)
                                .and_then(|e| e.client.clone())
                        };
                        if let Some(tc) = target_client {
                            if let Ok(listing) = tc.list_dir(None).await {
                                form.workdir = listing.path;
                            }
                        }
                    }
                }
            }
            NewSessionField::Tool => {
                let avail = app.agent_availability.clone();
                form.cycle_tool(|t| match &avail {
                    // Mirrors `App::tool_available`. Inlined to sidestep
                    // the borrow conflict between the App-owned overlay
                    // (taken via `mem::replace` above) and the closure
                    // capturing `&app`. Uses the same probed-tools list
                    // as `is_probed_tool` so opencode/aider also get
                    // skipped when their binaries aren't installed.
                    Some(set) => !is_probed_tool(t) || set.contains(t),
                    None => true,
                });
            }
            NewSessionField::Workdir => {
                if !autocomplete_workdir(&mut form, client).await {
                    form.next_field();
                }
            }
            _ => form.next_field(),
        },
        KeyCode::BackTab => form.prev_field(),
        // Down/Up always navigate fields.
        KeyCode::Down => form.next_field(),
        KeyCode::Up => form.prev_field(),

        // Toggle field: space flips the focused checkbox; on text fields it
        // just types a literal space.
        KeyCode::Char(' ') if matches!(form.field, NewSessionField::UpAfter) => {
            form.up_after = !form.up_after;
        }
        KeyCode::Char(' ') if matches!(form.field, NewSessionField::Yolo) => {
            form.yolo = !form.yolo;
        }

        // Enter while on the workdir field opens the dir picker
        // (mirrors clicking the picker's chevron in the web UI).
        // Enter on up-after still flips it. Enter on YOLO submits —
        // YOLO is the last field, so the natural next move is to spawn,
        // not to keep toggling. Use Space if you need to flip YOLO.
        KeyCode::Enter if matches!(form.field, NewSessionField::Workdir) => {
            let seed = if form.workdir.trim().is_empty() {
                None
            } else {
                Some(form.workdir.trim().to_string())
            };
            let picker = open_dir_picker(seed.as_deref(), client).await;
            form.picker = Some(picker);
        }
        KeyCode::Enter if matches!(form.field, NewSessionField::UpAfter) => {
            form.up_after = !form.up_after;
        }

        KeyCode::Backspace => {
            if let Some(v) = form.field_value_mut() {
                v.pop();
            }
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(v) = form.field_value_mut() {
                v.push(c);
            }
        }
        KeyCode::Enter => {
            // Submit. The form's Profile field decides which client to
            // POST against — no more soft-restart for cross-profile
            // spawns now that every server is connected in parallel.
            // Empty profile string means "the default / loopback / --api
            // connection", same key shape as `app.clients`.
            let target_profile = form.profile.trim().to_string();
            if let Err(msg) = form.validate() {
                form.error = Some(msg.into());
                app.overlay = Overlay::NewSession(form);
                return;
            }
            // Block first-class agents the daemon can't actually launch
            // (cursor without `cursor-agent`, etc.). The web dialog
            // disables those tiles outright; the TUI text field will
                // still let the user type the name, so we bounce here
            // before sending a request the executor will fail later.
            if !app.tool_available(form.tool.trim()) {
                // Mirror `agentum_executor::binary_for` so the error
                // names the actual missing binary (cursor → cursor-agent)
                // instead of the friendly tool id.
                let bin = match form.tool.trim() {
                    "cursor" => "cursor-agent",
                    other => other,
                };
                form.error = Some(format!("{bin} not installed on the daemon"));
                app.overlay = Overlay::NewSession(form);
                return;
            }
            let model = if form.model.trim().is_empty() {
                None
            } else {
                Some(form.model.trim().to_string())
            };
            let mut flags = parse_args_field(&form.args);
            if form.yolo_active() && !flags.iter().any(|f| f == YOLO_FLAG) {
                flags.push(YOLO_FLAG.to_string());
            }
            // Route create + start to the chosen profile's client.
            // Falls back to the default `client` when the user picked
            // "(current connection)" or the profile lookup misses
            // (an unreachable peer would have a None client; we
            // surface that as an error instead of silently routing
            // to the default).
            let target_client = if target_profile.is_empty() {
                Some(client.clone())
            } else {
                match app.clients.get(&target_profile) {
                    Some(entry) => entry.client.clone(),
                    None => None,
                }
            };
            let Some(target_client) = target_client else {
                form.error =
                    Some(format!("profile `{target_profile}` is not currently reachable"));
                app.overlay = Overlay::NewSession(form);
                return;
            };
            match target_client
                .create_session(
                    form.name.trim(),
                    form.workdir.trim(),
                    form.tool.trim(),
                    model.as_deref(),
                    flags,
                )
                .await
            {
                Ok(created) => {
                    let id = created.id;
                    let name = created.name.clone();
                    // Tag the new session with its owning profile so
                    // the tree groups it under the right server
                    // header on the next refresh.
                    app.session_profile.insert(id, target_profile.clone());
                    if form.up_after {
                        if let Err(e) = target_client.start_session(id).await {
                            let msg = format!("created `{name}` (start failed)");
                            app.status_msg = Some(msg.clone());
                            app.push_error(format!("start `{name}`: {e}"));
                            push_notification(
                                app,
                                msg,
                                Some("see error log (!) for details".to_string()),
                                NotifKind::Warn,
                            );
                        } else {
                            let msg = format!("created + started `{name}`");
                            app.status_msg = Some(msg.clone());
                            push_notification(
                                app,
                                msg,
                                None,
                                NotifKind::Info,
                            );
                        }
                    } else {
                        let msg = format!("created `{name}` (idle)");
                        app.status_msg = Some(msg.clone());
                        push_notification(
                            app,
                            msg,
                            None,
                            NotifKind::Info,
                        );
                    }
                    // Aggregating refresh so the new session shows
                    // up regardless of which server it landed on
                    // (cross-profile spawn just took the soft-restart
                    // path; same-profile lands here directly).
                    refresh_all(app).await;
                    app.tree.select_session(id);
                    {
                        let side = app.target_side();
                        update_selection(app, client, side);
                    }
                    // Jump straight into the new terminal so the user
                    // can start typing — matches Space-from-tree.
                    if app.selected == Some(id) {
                        app.set_focus(Focus::Term);
                    }
                    return;
                }
                Err(e) => {
                    form.error = Some(format!("{e}"));
                    app.overlay = Overlay::NewSession(form);
                    return;
                }
            }
        }
        _ => {}
    }
    app.overlay = Overlay::NewSession(form);
}

/// Fetch the listing for `seed` (or `$HOME` if seed is empty/None).
///
/// If `seed` doesn't exist, walks up the path until an existing ancestor
/// is found and surfaces a hint about the fallback. This way typing a
/// stale workdir (project deleted, repo moved, typo) never traps the
/// user in a dead-end picker with no `parent` to back out of — they
/// land at the nearest real directory and can navigate from there.
async fn open_dir_picker(seed: Option<&str>, client: &Client) -> DirPickerState {
    let original = seed
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());
    let mut current = original.clone();
    let mut last_err: Option<String> = None;

    loop {
        let attempt = current.as_deref();
        match client.list_dir(attempt).await {
            Ok(listing) => {
                let fell_back = current != original;
                let error = if fell_back {
                    original
                        .as_deref()
                        .map(|o| format!("`{o}` not found — opened nearest parent"))
                } else {
                    None
                };
                return DirPickerState {
                    path: listing.path,
                    parent: listing.parent,
                    entries: listing
                        .dirs
                        .into_iter()
                        .map(|d| DirEntryView {
                            name: d.name,
                            path: d.path,
                        })
                        .collect(),
                    cursor: 0,
                    error,
                    loading: false,
                };
            }
            Err(e) => {
                // Capture the first error only — that's the one that
                // names the path the user actually asked for. Walking
                // up past it would just produce noisier "no such dir"
                // messages for paths the user never typed.
                last_err.get_or_insert_with(|| e.to_string());
                let next = current.as_deref().and_then(parent_path_string);
                match next {
                    Some(p) => current = Some(p),
                    None => {
                        // Already at $HOME / no parent. Bail with the
                        // error so the overlay isn't silently empty.
                        return DirPickerState {
                            path: original.unwrap_or_else(|| "~".into()),
                            parent: None,
                            entries: Vec::new(),
                            cursor: 0,
                            error: last_err,
                            loading: false,
                        };
                    }
                }
            }
        }
    }
}

/// Shell-style Tab completion for the workdir field.
///
/// Splits the current input into "directory part" + "name prefix",
/// asks the daemon for that directory's subdirs, and either:
///   - replaces the prefix with the unique match (appending `/` so the
///     user can keep tabbing into nested dirs), or
///   - extends the prefix to the longest common prefix shared by all
///     candidates (mimics bash readline behaviour).
///
/// Returns `true` when the workdir was changed — the caller uses this
/// to decide whether to swallow the Tab or fall through to next_field.
async fn autocomplete_workdir(form: &mut NewSessionForm, client: &Client) -> bool {
    let current = form.workdir.clone();
    if current.trim().is_empty() {
        return false;
    }
    // Split at the last `/` so `~/Dev/agen` becomes ("~/Dev/", "agen").
    // No slash → completion is meaningless ($HOME basenames vs cwd is
    // ambiguous), so let Tab just advance fields.
    let Some(slash_idx) = current.rfind('/') else {
        return false;
    };
    let dir_part = &current[..=slash_idx];
    let prefix = &current[slash_idx + 1..];

    // List the parent. Empty/`~/` map to $HOME; root gets passed through.
    let dir_query: Option<&str> = if dir_part == "/" {
        Some("/")
    } else {
        // Strip trailing `/` so the server doesn't double-resolve.
        let q = dir_part.trim_end_matches('/');
        if q.is_empty() { None } else { Some(q) }
    };
    let listing = match client.list_dir(dir_query).await {
        Ok(l) => l,
        Err(_) => return false,
    };
    let matches: Vec<&str> = listing
        .dirs
        .iter()
        .map(|d| d.name.as_str())
        .filter(|n| n.starts_with(prefix))
        .collect();
    if matches.is_empty() {
        return false;
    }

    let new_text = if matches.len() == 1 {
        // Unique completion: full name + trailing slash so the next Tab
        // dives straight into the dir.
        format!("{dir_part}{}/", matches[0])
    } else {
        let common = longest_common_prefix(&matches);
        if common.len() <= prefix.len() {
            // Already at the boundary of an ambiguous fork — Tab can't
            // advance. Treat as no-op so the user can keep typing or
            // open the picker with Enter.
            return false;
        }
        format!("{dir_part}{common}")
    };
    if new_text == current {
        return false;
    }
    form.workdir = new_text;
    true
}

fn longest_common_prefix(strs: &[&str]) -> String {
    let Some(first) = strs.first() else {
        return String::new();
    };
    let mut end = first.len();
    for s in &strs[1..] {
        let mut i = 0;
        for (a, b) in first.bytes().zip(s.bytes()) {
            if a != b {
                break;
            }
            i += 1;
        }
        end = end.min(i);
        if end == 0 {
            break;
        }
    }
    first[..end].to_string()
}

/// Parent of `p` as an owned String, or None at the filesystem root /
/// when `p` no longer has a meaningful parent (e.g. just `~`).
fn parent_path_string(p: &str) -> Option<String> {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "~" || trimmed == "/" {
        return None;
    }
    let parent = std::path::Path::new(trimmed).parent()?;
    let s = parent.to_string_lossy();
    if s.is_empty() { None } else { Some(s.into_owned()) }
}

async fn handle_dir_picker_key(form: &mut NewSessionForm, key: KeyEvent, client: &Client) {
    let Some(picker) = form.picker.as_mut() else {
        return;
    };

    match key.code {
        KeyCode::Esc => {
            form.picker = None;
        }
        KeyCode::Up => {
            picker.cursor = picker.cursor.saturating_sub(1);
        }
        KeyCode::Down => {
            if picker.cursor + 1 < picker.entries.len() {
                picker.cursor += 1;
            }
        }
        // Right / Enter: descend into the highlighted entry.
        KeyCode::Right | KeyCode::Enter => {
            let Some(entry) = picker.entries.get(picker.cursor).cloned() else {
                return;
            };
            let next = open_dir_picker(Some(&entry.path), client).await;
            form.picker = Some(next);
        }
        // Left / Backspace: pop up one level.
        KeyCode::Left | KeyCode::Backspace => {
            if let Some(parent) = picker.parent.clone() {
                let next = open_dir_picker(Some(&parent), client).await;
                form.picker = Some(next);
            }
        }
        // 'a' (accept): commit the *current directory* (the path being
        // listed) as the workdir, and close the picker.
        KeyCode::Char('a') | KeyCode::Char('s') => {
            form.workdir = picker.path.clone();
            form.picker = None;
        }
        _ => {}
    }
}

async fn handle_confirm_key(app: &mut App, key: KeyEvent, client: &Client) {
    let Overlay::Confirm(action) = std::mem::replace(&mut app.overlay, Overlay::None) else {
        return;
    };

    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            execute_action(app, action, client).await;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            // Cancelled — overlay already cleared via mem::replace.
        }
        _ => {
            // Other keys: keep prompt up.
            app.overlay = Overlay::Confirm(action);
        }
    }
}

async fn execute_action(app: &mut App, action: PendingAction, client: &Client) {
    // Server removal is purely local — it touches profiles.toml
    // and the app's in-memory profile list without a daemon round trip.
    if let PendingAction::RemoveServer { name } = &action {
        let label = format!("removed server `{name}`");
        match super::profiles::Profiles::load() {
            Ok(mut store) => {
                let _ = store.remove(name);
                app.reload_profiles();
                app.status_msg = Some(label.clone());
                push_notification(app, label, None, NotifKind::Info);
            }
            Err(e) => {
                app.push_error(format!("remove server `{name}`: {e}"));
            }
        }
        refresh_all(app).await;
        return;
    }

    // Lifecycle ops (start/stop/kill) target a specific session id, so
    // they have to talk to whichever server owns that session — not
    // the default. Look up the owner's client; fall back to `client`
    // for legacy / unmapped sessions so behaviour matches the
    // single-server world when nothing's tagged.
    let session_id = match &action {
        PendingAction::Start { id, .. }
        | PendingAction::Stop { id, .. }
        | PendingAction::Kill { id, .. } => *id,
        PendingAction::RemoveServer { .. } => unreachable!(),
    };
    let owner = app.client_for_session(session_id).cloned();
    let target = owner.unwrap_or_else(|| client.clone());
    let result = match &action {
        PendingAction::Start { id, .. } => target.start_session(*id).await,
        PendingAction::Stop { id, .. } => target.stop_session(*id).await,
        // "Kill" in the TUI means kill-and-remove: drop the tmux session
        // AND the store record so the entry disappears from the tree.
        // This is the only destructive verb — "delete" used to be a
        // separate action but it overlapped enough with kill (kill also
        // removed the record) that having both was just confusing UI
        // with no semantic payoff. `s`/Stop is still here for the rare
        // "I want to restart this exact session later" case.
        PendingAction::Kill { id, .. } => target.delete_session(*id, true).await,
        _ => unreachable!(),
    };
    let label = match &action {
        PendingAction::Start { name, .. } => format!("started `{name}`"),
        PendingAction::Stop { name, .. } => format!("stopped `{name}`"),
        PendingAction::Kill { name, .. } => format!("killed `{name}`"),
        PendingAction::RemoveServer { .. } => unreachable!(),
    };
    match result {
        Ok(()) => {
            // Toast user-initiated lifecycle actions. The server's event
            // bus only re-emits `session.crashed` / `watchdog.compact` /
            // (silenced) `session.started`, so without this nudge a clean
            // start/stop/kill would silently update the tree with
            // zero confirmation. Direct push lets the toast fire even
            // when the events WS is offline.
            app.status_msg = Some(label.clone());
            push_notification(
                app,
                label,
                None,
                NotifKind::Info,
            );
        }
        Err(e) => {
            app.push_error(format!("{label}: {e}"));
        }
    }
    // Aggregating refresh so peers' state changes still flow into
    // the sidebar even when the action targeted a peer-owned session.
    refresh_all(app).await;
}

/// Build the live action catalog from current app state.
pub fn palette_catalog(app: &App) -> Catalog {
    let sessions: Vec<(Uuid, String, String)> = app
        .sessions
        .iter()
        .map(|s| (s.id, s.name.clone(), s.workdir.clone()))
        .collect();
    let view = ViewState {
        sidebar_hidden: app.sidebar_hidden,
        right_panel_visible: app.right_panel_visible,
        fullscreen: app.fullscreen,
        split_open: app.split_open(),
    };
    Catalog::build(app.lazygit_open(), &sessions, app.selected, view, &app.prefs)
}

/// Build a [`ProfilesOverlay`] from the on-disk profiles file and
/// install it on `app`. Surfaces a friendly error in the overlay
/// itself when the file is unreadable or empty rather than silently
/// no-op'ing — the user just hit Ctrl-O for a reason.
pub fn open_profiles_overlay(app: &mut App) {
    let (entries, default_name, error) =
        match super::profiles::Profiles::load() {
            Ok(store) => {
                let default_name = store.default_name().map(str::to_string);
                let mut rows: Vec<ProfileEntry> = store
                    .list()
                    .into_iter()
                    .map(|(name, p, is_default)| ProfileEntry {
                        name,
                        url: p.url,
                        fingerprint: p.fingerprint,
                        is_default,
                    })
                    .collect();
                // Surface the active profile at the top of the picker
                // when it isn't the default — saves a step on the most
                // common task ("which one am I on right now?").
                if let Some(active) = &app.active_profile {
                    if let Some(idx) = rows.iter().position(|r| &r.name == active) {
                        let row = rows.remove(idx);
                        rows.insert(0, row);
                    }
                }
                (rows, default_name, None)
            }
            Err(e) => (Vec::new(), None, Some(format!("load profiles.toml: {e}"))),
        };
    let cursor = entries
        .iter()
        .position(|p| Some(&p.name) == app.active_profile.as_ref())
        .unwrap_or(0);
    app.overlay = Overlay::Profiles(ProfilesOverlay {
        entries,
        cursor,
        default_name,
        error,
        add_form: None,
    });
}

/// Profile-switcher overlay key handler. Two modes:
///
/// - **List mode** (default): Up/Down moves the cursor, Enter switches
///   to the highlighted profile (triggering a soft restart of the
///   run-loop), `a` opens the inline add form, `d` removes the
///   highlighted profile, `Esc`/`q` dismisses.
/// - **Add-form mode** (when `add_form.is_some()`): Tab cycles fields,
///   Enter submits, Esc returns to list mode.
fn handle_profiles_key(app: &mut App, key: KeyEvent) {
    let Overlay::Profiles(mut state) = std::mem::replace(&mut app.overlay, Overlay::None) else {
        return;
    };

    if let Some(mut form) = state.add_form.take() {
        // ----- add-form mode -----
        match key.code {
            KeyCode::Esc => {
                // Drop the form, return to the list.
                app.overlay = Overlay::Profiles(state);
                return;
            }
            KeyCode::Tab | KeyCode::Down => form.next_field(),
            KeyCode::BackTab | KeyCode::Up => form.prev_field(),
            KeyCode::Char(' ') if form.field == AddProfileField::SetDefault => {
                form.set_default = !form.set_default;
            }
            KeyCode::Backspace => {
                if let Some(v) = form.field_value_mut() {
                    v.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(v) = form.field_value_mut() {
                    v.push(c);
                }
            }
            KeyCode::Enter => {
                form.error = None;
                let name = form.name.trim().to_string();
                let url = form.url.trim().to_string();
                if name.is_empty() {
                    form.error = Some("name is required".into());
                    state.add_form = Some(form);
                    app.overlay = Overlay::Profiles(state);
                    return;
                }
                if url.is_empty() {
                    form.error = Some("URL is required".into());
                    state.add_form = Some(form);
                    app.overlay = Overlay::Profiles(state);
                    return;
                }
                if let Err(e) = url::Url::parse(&url) {
                    form.error = Some(format!("invalid URL: {e}"));
                    state.add_form = Some(form);
                    app.overlay = Overlay::Profiles(state);
                    return;
                }
                let fingerprint = if form.fingerprint.trim().is_empty() {
                    None
                } else {
                    match super::trust::normalize_fingerprint(form.fingerprint.trim()) {
                        Ok(fp) => Some(fp),
                        Err(e) => {
                            form.error = Some(format!("invalid fingerprint: {e}"));
                            state.add_form = Some(form);
                            app.overlay = Overlay::Profiles(state);
                            return;
                        }
                    }
                };
                match super::profiles::Profiles::load() {
                    Ok(mut store) => {
                        if let Err(e) = store.upsert(
                            name.clone(),
                            super::profiles::Profile {
                                url: url.clone(),
                                fingerprint,
                                insecure: false,
                            },
                        ) {
                            form.error = Some(format!("save failed: {e}"));
                            state.add_form = Some(form);
                            app.overlay = Overlay::Profiles(state);
                            return;
                        }
                        if form.set_default {
                            let _ = store.set_default(Some(name.clone()));
                        }
                        // Reload the list and snap the cursor onto the
                        // freshly added profile so Enter switches to
                        // it immediately.
                        open_profiles_overlay(app);
                        if let Overlay::Profiles(ref mut s) = app.overlay {
                            if let Some(idx) = s.entries.iter().position(|e| e.name == name) {
                                s.cursor = idx;
                            }
                        }
                        return;
                    }
                    Err(e) => {
                        form.error = Some(format!("open profiles.toml: {e}"));
                        state.add_form = Some(form);
                        app.overlay = Overlay::Profiles(state);
                        return;
                    }
                }
            }
            _ => {}
        }
        state.add_form = Some(form);
        app.overlay = Overlay::Profiles(state);
        return;
    }

    // ----- list mode -----
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.overlay = Overlay::None;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.cursor = state.cursor.saturating_sub(1);
            app.overlay = Overlay::Profiles(state);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.cursor + 1 < state.entries.len() {
                state.cursor += 1;
            }
            app.overlay = Overlay::Profiles(state);
        }
        KeyCode::Char('a') | KeyCode::Char('+') | KeyCode::Char('n') => {
            state.add_form = Some(AddProfileForm::new());
            app.overlay = Overlay::Profiles(state);
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            // Refuse to remove the active profile so the user doesn't
            // accidentally leave themselves without a target. They can
            // switch first, then delete.
            if let Some(entry) = state.entries.get(state.cursor) {
                if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                    state.error = Some("can't remove the active profile — switch first".into());
                    app.overlay = Overlay::Profiles(state);
                    return;
                }
                let name = entry.name.clone();
                if let Ok(mut store) = super::profiles::Profiles::load() {
                    let _ = store.remove(&name);
                }
                open_profiles_overlay(app);
                return;
            }
            app.overlay = Overlay::Profiles(state);
        }
        KeyCode::Enter => {
            if let Some(entry) = state.entries.get(state.cursor) {
                if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                    // Already on this one — just close.
                    app.overlay = Overlay::None;
                    return;
                }
                // Schedule a soft restart with the chosen profile.
                // run_loop reads `pending_switch_profile` on quit and
                // `commands::terminal::run` re-enters with the new
                // server. `pending_after_switch` stays None — the
                // overlay path doesn't carry a follow-up.
                app.pending_switch_profile = Some(entry.name.clone());
                app.pending_after_switch = None;
                app.should_quit = true;
                return;
            }
            app.overlay = Overlay::Profiles(state);
        }
        _ => {
            app.overlay = Overlay::Profiles(state);
        }
    }
}

/// Errors-overlay key handler. Treats the overlay as a vim-ish list:
/// j/k or arrow keys scroll, PgUp/PgDn page, g/G snap to ends, c clears
/// the log, and Esc/q/Enter/e dismiss. The cursor saturates at the list
/// length on render so we don't have to clamp on every keystroke.
/// Drive the Settings overlay. Boolean rows respond to `space` /
/// `enter`; numeric (TTL) rows respond to `←` / `→` (and also `space` /
/// `enter` to bump up); the `ResetAll` row fires `prefs.reset()` on
/// activation. `r` resets the focused row alone. Esc closes. Every
/// mutation calls `prefs::save` so the next launch comes up the same
/// way and mirrors the runtime fields onto `app.<field>` for hot-path
/// draw code that doesn't go through prefs.
fn handle_settings_key(app: &mut App, key: KeyEvent) {
    let Overlay::Settings(state) = app.overlay.clone() else {
        return;
    };
    let mut state = state;
    let mut changed = false;
    let row = state.current();
    match key.code {
        KeyCode::Esc => {
            app.overlay = Overlay::None;
            return;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_by(-1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_by(1);
        }
        KeyCode::Home => state.cursor = 0,
        KeyCode::End => state.cursor = SettingsRow::ROWS.len() - 1,
        KeyCode::Char('r') => {
            // Reset focused row to its default by overwriting from
            // `Prefs::default()`.
            let d = prefs::Prefs::default();
            apply_settings_row_from(&d, row, &mut app.prefs);
            mirror_layout_from_prefs(app);
            prefs::save(&app.prefs);
            changed = true;
            app.status_msg = Some(format!("reset row · {}", settings_row_short_label(row)));
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if let Some(kind) = ttl_row_kind(row) {
                let new_ms = app
                    .prefs
                    .bump_ttl(kind, -(prefs::NOTIF_TTL_STEP_MS as i64));
                app.status_msg = Some(format!("{} TTL: {} ms", kind.label(), new_ms));
                prefs::save(&app.prefs);
                changed = true;
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if let Some(kind) = ttl_row_kind(row) {
                let new_ms = app.prefs.bump_ttl(kind, prefs::NOTIF_TTL_STEP_MS as i64);
                app.status_msg = Some(format!("{} TTL: {} ms", kind.label(), new_ms));
                prefs::save(&app.prefs);
                changed = true;
            }
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            if let Some(kind) = ttl_row_kind(row) {
                let new_ms = app.prefs.bump_ttl(kind, prefs::NOTIF_TTL_STEP_MS as i64);
                app.status_msg = Some(format!("{} TTL: {} ms", kind.label(), new_ms));
            } else {
                toggle_settings_row(app, row);
            }
            prefs::save(&app.prefs);
            mirror_layout_from_prefs(app);
            changed = true;
        }
        _ => {}
    }
    // Persist the (possibly moved) cursor by stashing the new state back
    // into the overlay variant. Doing it here keeps every branch above
    // unaware of the wrap.
    let _ = changed;
    app.overlay = Overlay::Settings(state);
}

/// Map a settings row to the corresponding `SoundKind` if it represents
/// a TTL slider, else `None`. Used to disambiguate `←/→` (TTL bump) from
/// `space/enter` (boolean toggle) in `handle_settings_key`.
fn ttl_row_kind(row: SettingsRow) -> Option<prefs::SoundKind> {
    match row {
        SettingsRow::TtlInfo => Some(prefs::SoundKind::Info),
        SettingsRow::TtlWarn => Some(prefs::SoundKind::Warn),
        SettingsRow::TtlError => Some(prefs::SoundKind::Error),
        _ => None,
    }
}

/// Toggle the boolean represented by `row`. TTL rows are filtered out by
/// the caller so this match is exhaustive over the boolean / action
/// surface only. Each arm scopes its `&mut app.prefs` borrow tightly so
/// it doesn't collide with `app.set_focus` (which needs `&mut app`).
fn toggle_settings_row(app: &mut App, row: SettingsRow) {
    let msg: String = match row {
        SettingsRow::SoundMaster => {
            let on = app.prefs.toggle_sound_master();
            format!("sound master: {}", if on { "on" } else { "off" })
        }
        SettingsRow::SoundInfo => {
            let on = app.prefs.toggle_sound_kind(prefs::SoundKind::Info);
            format!("sound info: {}", if on { "on" } else { "off" })
        }
        SettingsRow::SoundWarn => {
            let on = app.prefs.toggle_sound_kind(prefs::SoundKind::Warn);
            format!("sound warn: {}", if on { "on" } else { "off" })
        }
        SettingsRow::SoundError => {
            let on = app.prefs.toggle_sound_kind(prefs::SoundKind::Error);
            format!("sound error: {}", if on { "on" } else { "off" })
        }
        SettingsRow::SidebarHidden => {
            app.prefs.sidebar_hidden = !app.prefs.sidebar_hidden;
            app.sidebar_hidden = app.prefs.sidebar_hidden;
            if app.sidebar_hidden && app.focus == Focus::Tree {
                app.set_focus(Focus::Term);
            }
            if app.sidebar_hidden {
                "sidebar hidden".into()
            } else {
                "sidebar visible".into()
            }
        }
        SettingsRow::RightPanelVisible => {
            app.prefs.right_panel_visible = !app.prefs.right_panel_visible;
            app.right_panel_visible = app.prefs.right_panel_visible;
            if app.right_panel_visible {
                "agent panel on".into()
            } else {
                "agent panel off".into()
            }
        }
        SettingsRow::ChipWorkdir => {
            app.prefs.toggle(prefs::StatusChip::Workdir);
            "chip · workdir".into()
        }
        SettingsRow::ChipTool => {
            app.prefs.toggle(prefs::StatusChip::Tool);
            "chip · tool".into()
        }
        SettingsRow::ChipConn => {
            app.prefs.toggle(prefs::StatusChip::Conn);
            "chip · connection".into()
        }
        SettingsRow::ChipLazygit => {
            app.prefs.toggle(prefs::StatusChip::Lazygit);
            "chip · lazygit".into()
        }
        SettingsRow::ChipTheme => {
            app.prefs.toggle(prefs::StatusChip::Theme);
            "chip · theme".into()
        }
        SettingsRow::ChipIo => {
            app.prefs.toggle(prefs::StatusChip::Io);
            "chip · I/O speeds".into()
        }
        SettingsRow::ChipIoTotals => {
            app.prefs.toggle(prefs::StatusChip::IoTotals);
            "chip · I/O totals".into()
        }
        SettingsRow::ChipPaletteHint => {
            app.prefs.toggle(prefs::StatusChip::PaletteHint);
            "chip · palette hint".into()
        }
        SettingsRow::ChipHelpHint => {
            app.prefs.toggle(prefs::StatusChip::HelpHint);
            "chip · help hint".into()
        }
        SettingsRow::ResetAll => {
            app.prefs.reset();
            "settings · reset to defaults".into()
        }
        SettingsRow::TtlInfo | SettingsRow::TtlWarn | SettingsRow::TtlError => {
            // Numeric rows are handled by the caller — should not reach.
            return;
        }
    };
    app.status_msg = Some(msg);
}

/// Copy a single field from `src` into `dst.<row>`. Used by the per-row
/// `r` reset key in `handle_settings_key`.
fn apply_settings_row_from(src: &prefs::Prefs, row: SettingsRow, dst: &mut prefs::Prefs) {
    match row {
        SettingsRow::SoundMaster => dst.sound_master = src.sound_master,
        SettingsRow::SoundInfo => dst.sound_info = src.sound_info,
        SettingsRow::SoundWarn => dst.sound_warn = src.sound_warn,
        SettingsRow::SoundError => dst.sound_error = src.sound_error,
        SettingsRow::TtlInfo => dst.notif_ttl_info_ms = src.notif_ttl_info_ms,
        SettingsRow::TtlWarn => dst.notif_ttl_warn_ms = src.notif_ttl_warn_ms,
        SettingsRow::TtlError => dst.notif_ttl_error_ms = src.notif_ttl_error_ms,
        SettingsRow::SidebarHidden => dst.sidebar_hidden = src.sidebar_hidden,
        SettingsRow::RightPanelVisible => dst.right_panel_visible = src.right_panel_visible,
        SettingsRow::ChipWorkdir => dst.show_workdir = src.show_workdir,
        SettingsRow::ChipTool => dst.show_tool = src.show_tool,
        SettingsRow::ChipConn => dst.show_conn = src.show_conn,
        SettingsRow::ChipLazygit => dst.show_lazygit = src.show_lazygit,
        SettingsRow::ChipTheme => dst.show_theme = src.show_theme,
        SettingsRow::ChipIo => dst.show_io = src.show_io,
        SettingsRow::ChipIoTotals => dst.show_io_totals = src.show_io_totals,
        SettingsRow::ChipPaletteHint => dst.show_palette_hint = src.show_palette_hint,
        SettingsRow::ChipHelpHint => dst.show_help_hint = src.show_help_hint,
        SettingsRow::ResetAll => *dst = src.clone(),
    }
}

/// Re-sync the runtime layout copies on `App` from `app.prefs` after a
/// settings change. Hot-path draw code reads `app.tree_width` etc.
/// directly to avoid hashing through prefs every frame.
fn mirror_layout_from_prefs(app: &mut App) {
    app.tree_width = app.prefs.tree_width;
    app.lazygit_width = app.prefs.lazygit_width;
    app.term_split_pct = app.prefs.term_split_pct;
    app.sidebar_hidden = app.prefs.sidebar_hidden;
    app.right_panel_visible = app.prefs.right_panel_visible;
}

/// Short identifier used in transient status messages.
fn settings_row_short_label(row: SettingsRow) -> &'static str {
    match row {
        SettingsRow::SoundMaster => "sound master",
        SettingsRow::SoundInfo => "sound info",
        SettingsRow::SoundWarn => "sound warn",
        SettingsRow::SoundError => "sound error",
        SettingsRow::TtlInfo => "TTL info",
        SettingsRow::TtlWarn => "TTL warn",
        SettingsRow::TtlError => "TTL error",
        SettingsRow::SidebarHidden => "sidebar",
        SettingsRow::RightPanelVisible => "agent panel",
        SettingsRow::ChipWorkdir => "chip workdir",
        SettingsRow::ChipTool => "chip tool",
        SettingsRow::ChipConn => "chip connection",
        SettingsRow::ChipLazygit => "chip lazygit",
        SettingsRow::ChipTheme => "chip theme",
        SettingsRow::ChipIo => "chip I/O speeds",
        SettingsRow::ChipIoTotals => "chip I/O totals",
        SettingsRow::ChipPaletteHint => "chip palette hint",
        SettingsRow::ChipHelpHint => "chip help hint",
        SettingsRow::ResetAll => "all",
    }
}

/// Drive the inline rename overlay. Plain typing edits the buffer;
/// Backspace deletes the last character; Enter submits via PATCH; Esc
/// cancels and closes. On success the local sessions list is refreshed
/// so the new name shows immediately without waiting for the bus
/// event. On failure the inline error is set and the overlay stays
/// open with the typed buffer preserved.
async fn handle_rename_key(app: &mut App, key: KeyEvent, client: &Client) {
    let Overlay::Rename(state) = app.overlay.clone() else {
        return;
    };
    let mut state = state;
    match key.code {
        KeyCode::Esc => {
            app.overlay = Overlay::None;
            app.status_msg = Some("rename cancelled".into());
        }
        KeyCode::Backspace => {
            state.buffer.pop();
            state.error = None;
            app.overlay = Overlay::Rename(state);
        }
        KeyCode::Enter => {
            let trimmed = state.buffer.trim();
            if trimmed.is_empty() {
                state.error = Some("name cannot be empty".into());
                app.overlay = Overlay::Rename(state);
                return;
            }
            if trimmed == state.original {
                app.overlay = Overlay::None;
                app.status_msg = Some("rename: no change".into());
                return;
            }
            let id = state.id;
            match client.rename_session(id, trimmed).await {
                Ok(updated) => {
                    app.overlay = Overlay::None;
                    app.status_msg = Some(format!("renamed → {}", updated.name));
                    if let Ok(fresh) = client.list_sessions().await {
                        app.refresh_sessions(fresh);
                        app.tree.select_session(id);
                    }
                }
                Err(e) => {
                    state.error = Some(format!("{e}"));
                    app.overlay = Overlay::Rename(state);
                }
            }
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Cap at 64 chars to match the server validation so the
            // user gets a hard stop rather than a "too long" error.
            if state.buffer.chars().count() < 64 {
                state.buffer.push(c);
                state.error = None;
            }
            app.overlay = Overlay::Rename(state);
        }
        _ => {
            // Unknown key — keep the overlay state and the buffer.
            app.overlay = Overlay::Rename(state);
        }
    }
}

fn handle_errors_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc
        | KeyCode::Char('q')
        | KeyCode::Char('!')
        | KeyCode::Enter => {
            app.overlay = Overlay::None;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.errors_scroll = app.errors_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.errors_scroll = app.errors_scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            app.errors_scroll = app.errors_scroll.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.errors_scroll = app.errors_scroll.saturating_add(10);
        }
        KeyCode::Home | KeyCode::Char('g') => {
            app.errors_scroll = 0;
        }
        KeyCode::End | KeyCode::Char('G') => {
            app.errors_scroll = usize::MAX;
        }
        KeyCode::Char('c') => {
            app.clear_errors();
        }
        _ => {}
    }
}

async fn handle_palette_key(
    app: &mut App,
    key: KeyEvent,
    client: &Client,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
) {
    match key.code {
        KeyCode::Esc => app.overlay = Overlay::None,
        KeyCode::Up => app.palette.cursor = app.palette.cursor.saturating_sub(1),
        KeyCode::Down => app.palette.cursor = app.palette.cursor.saturating_add(1),
        KeyCode::Backspace => {
            app.palette.query.pop();
            app.palette.cursor = 0;
        }
        // Ctrl-modified chars besides p/k were handled upstream, so this
        // arm only fires for plain typing.
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.palette.query.push(c);
            app.palette.cursor = 0;
        }
        KeyCode::Enter => {
            let cat = palette_catalog(app);
            let (_mode, filtered) = cat.filtered(&app.palette.query);
            let chosen = filtered.get(app.palette.cursor).cloned().cloned();
            app.overlay = Overlay::None;
            if let Some(action) = chosen {
                run_palette_action(app, action.kind, client, lg_tx).await;
            }
        }
        _ => {}
    }
}

async fn run_palette_action(
    app: &mut App,
    kind: ActionKind,
    client: &Client,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
) {
    match kind {
        ActionKind::Quit => app.should_quit = true,
        ActionKind::ToggleHelp => app.overlay = Overlay::Help,
        ActionKind::ShowErrors => {
            app.errors_scroll = 0;
            app.overlay = Overlay::Errors;
        }
        ActionKind::ToggleLazygit => toggle_lazygit(app, lg_tx).await,
        ActionKind::LazygitCheats => app.overlay = Overlay::LazygitCheats,
        ActionKind::Refresh => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                app.status_msg = Some("refreshed".into());
            }
        }
        ActionKind::SpawnTerminal => {
            spawn_plain_terminal(app, client).await;
        }
        ActionKind::FocusTree => app.focus = Focus::Tree,
        ActionKind::FocusTerm => app.focus = Focus::Term,
        ActionKind::FocusLazygit => {
            if app.lazygit_open() {
                app.focus = Focus::Lazygit;
            }
        }
        ActionKind::SetTheme(name) => app.set_theme(name),
        ActionKind::CycleTheme => app.cycle_theme(),
        ActionKind::SelectSession(id) => {
            // Move tree cursor + reopen the stream for this id.
            app.tree.select_session(id);
            {
            let side = app.target_side();
            update_selection(app, client, side);
        }
        }
        ActionKind::KillSession(id) => {
            if let Some(s) = app.sessions.iter().find(|s| s.id == id) {
                app.overlay = Overlay::Confirm(PendingAction::Kill {
                    id,
                    name: s.name.clone(),
                });
            }
        }
        ActionKind::ToggleSidebar => {
            app.sidebar_hidden = !app.sidebar_hidden;
            if app.sidebar_hidden && app.focus == Focus::Tree {
                app.set_focus(Focus::Term);
            }
            app.prefs.sidebar_hidden = app.sidebar_hidden;
            prefs::save(&app.prefs);
            app.status_msg = Some(if app.sidebar_hidden {
                "sidebar hidden".into()
            } else {
                "sidebar visible".into()
            });
        }
        ActionKind::ToggleRightPanel => {
            app.right_panel_visible = !app.right_panel_visible;
            app.prefs.right_panel_visible = app.right_panel_visible;
            prefs::save(&app.prefs);
            app.status_msg = Some(if app.right_panel_visible {
                "agent panel on".into()
            } else {
                "agent panel off".into()
            });
            // Match the Ctrl-T keybinding: kick a fetch when turning
            // the panel on so it populates immediately.
            if app.right_panel_visible
                && let Some(id) = app.selected
            {
                spawn_agent_tasks_fetch(app, client, id);
            }
        }
        ActionKind::ToggleFullscreen => {
            app.fullscreen = !app.fullscreen;
            app.status_msg = Some(if app.fullscreen {
                "fullscreen on (Esc to exit)".into()
            } else {
                "fullscreen off".into()
            });
        }
        ActionKind::ToggleSplit => {
            if app.split_open() {
                if let Some(handle) = app.stream_handle_right.take() {
                    handle.abort();
                }
                app.split_right = None;
                if app.focus == Focus::TermRight {
                    app.set_focus(Focus::Term);
                }
                app.last_term_side = Side::Left;
                app.status_msg = Some("split closed".into());
            } else if app.lazygit_open() {
                app.status_msg = Some("close lazygit before splitting".into());
            } else {
                app.split_right = Some(TermSlot {
                    selected: None,
                    term: TerminalPane::new(),
                    term_in: None,
                    term_size: (0, 0),
                    term_reconnect_pending: false,
                });
                app.set_focus(Focus::TermRight);
                app.last_term_side = Side::Right;
                update_selection(app, client, Side::Right);
                app.status_msg = Some("split open (Ctrl-W to close)".into());
            }
        }
        ActionKind::ToggleStatusChip(chip) => {
            let now_on = app.prefs.toggle(chip);
            prefs::save(&app.prefs);
            app.status_msg = Some(format!(
                "status: {} {}",
                chip.label(),
                if now_on { "on" } else { "off" }
            ));
        }
        ActionKind::ResetStatusBar => {
            app.prefs = Prefs::default();
            mirror_layout_from_prefs(app);
            prefs::save(&app.prefs);
            app.status_msg = Some("status bar reset to defaults".into());
        }
        ActionKind::OpenSettings => {
            app.overlay = Overlay::Settings(SettingsState::new());
            app.status_msg = Some(
                "settings (Esc close · ↑↓ move · ←→ adjust · space toggle · r reset row)".into(),
            );
        }
        ActionKind::OpenProfiles => {
            open_profiles_overlay(app);
            app.status_msg =
                Some("servers (Enter switch · a add · d remove · Esc close)".into());
        }
        ActionKind::ToggleSoundMaster => {
            let on = app.prefs.toggle_sound_master();
            prefs::save(&app.prefs);
            app.status_msg = Some(format!("sound master: {}", if on { "on" } else { "off" }));
        }
        ActionKind::ToggleSoundKind(kind) => {
            let on = app.prefs.toggle_sound_kind(kind);
            prefs::save(&app.prefs);
            app.status_msg = Some(format!(
                "sound {}: {}",
                kind.label(),
                if on { "on" } else { "off" }
            ));
        }
        ActionKind::BumpTtl(kind, delta) => {
            let new_ms = app.prefs.bump_ttl(kind, delta);
            prefs::save(&app.prefs);
            app.status_msg = Some(format!("{} TTL: {} ms", kind.label(), new_ms));
        }
        ActionKind::ResetAllPrefs => {
            app.prefs.reset();
            mirror_layout_from_prefs(app);
            prefs::save(&app.prefs);
            app.status_msg = Some("settings · reset to defaults".into());
        }
    }
}

/// Next-panel cycle. Inserts `TermRight` after `Term` when the split is
/// open (so the natural cycle is Tree → Term → TermRight → [Lazygit] →
/// Tree). When lazygit is open *and* split is open, both extras sit
/// between Term and the wrap.
fn next_focus(current: Focus, lazygit_open: bool, split_open: bool) -> Focus {
    match (current, split_open, lazygit_open) {
        (Focus::Tree, _, _) => Focus::Term,
        (Focus::Term, true, _) => Focus::TermRight,
        (Focus::Term, false, true) => Focus::Lazygit,
        (Focus::Term, false, false) => Focus::Tree,
        (Focus::TermRight, _, true) => Focus::Lazygit,
        (Focus::TermRight, _, false) => Focus::Tree,
        (Focus::Lazygit, _, _) => Focus::Tree,
    }
}

/// Previous-panel cycle. Mirrors `next_focus` reversed.
fn prev_focus(current: Focus, lazygit_open: bool, split_open: bool) -> Focus {
    match (current, split_open, lazygit_open) {
        (Focus::Tree, _, true) => Focus::Lazygit,
        (Focus::Tree, true, false) => Focus::TermRight,
        (Focus::Tree, false, false) => Focus::Term,
        (Focus::Lazygit, true, _) => Focus::TermRight,
        (Focus::Lazygit, false, _) => Focus::Term,
        (Focus::TermRight, _, _) => Focus::Term,
        (Focus::Term, _, _) => Focus::Tree,
    }
}

/// Toggle the lazygit side pane.
///
/// On open: confirm the binary is on PATH (else show the install overlay),
/// resolve the active session's workdir, spawn a PTY, and steal focus to it.
/// On close: drop the `LocalPty` (its `Drop` kills the child).
async fn toggle_lazygit(app: &mut App, lg_tx: &mpsc::UnboundedSender<PtyMsg>) {
    if app.lazygit.is_some() {
        app.lazygit = None;
        app.lazygit_cwd = None;
        if app.focus == Focus::Lazygit {
            app.focus = Focus::Tree;
        }
        app.status_msg = Some("closed lazygit".into());
        return;
    }
    // Mutually exclusive with the terminal split — three columns of
    // tree + left + right + lazygit gets unreadable on anything narrower
    // than ~160 cols. Symmetrical with Ctrl-\\ refusing while lazygit is
    // open.
    if app.split_open() {
        app.status_msg = Some("close the split (Ctrl-W) before opening lazygit".into());
        return;
    }

    if !extensions::is_installed(&LAZYGIT) {
        app.overlay = Overlay::LazygitInstall;
        return;
    }

    // Try the session's workdir first; if it doesn't exist locally (typical
    // when connected to a remote `agentum serve`), fall back to the user's
    // current dir and surface the substitution clearly. Without this fallback
    // lazygit silently fails with `chdir: no such file or directory` and the
    // pane just reports "lazygit exited" milliseconds after spawn.
    let local_cwd =
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (cwd, fell_back) = match app.selected_session() {
        Some(s) => {
            let p = PathBuf::from(&s.workdir);
            if p.is_dir() {
                (p, false)
            } else {
                (local_cwd.clone(), true)
            }
        }
        None => (local_cwd.clone(), false),
    };
    let args = extensions::resolve_args(&LAZYGIT, &cwd);

    // Use a placeholder size; the run-loop resizes on the next frame.
    match LocalPty::spawn(LAZYGIT.binary, &args, &cwd, 24, 80, lg_tx.clone()) {
        Ok(pty) => {
            app.lazygit = Some(pty);
            app.lazygit_cwd = Some(cwd.clone());
            app.focus = Focus::Lazygit;
            app.status_msg = Some(if fell_back {
                format!(
                    "lazygit: session workdir not local — opened in {}",
                    cwd.display()
                )
            } else {
                format!("lazygit @ {}", cwd.display())
            });
        }
        Err(e) => {
            app.push_error(format!("lazygit spawn failed: {e}"));
        }
    }
}

/// Respawn the lazygit side pane in the active session's workdir
/// when the user switches to a different project. Without this, the
/// pane stays pinned to whatever directory it was first opened in
/// and silently drifts out of sync with the highlighted agent.
///
/// No-op when lazygit isn't open, when no session is selected, when
/// the selection's workdir isn't a local directory (typical for
/// remote daemons — keep the existing pane rather than collapsing it
/// into the local cwd), or when the workdir is the same one we
/// already spawned in (lateral switch between agents in the same
/// repo, no need to thrash).
fn refresh_lazygit_for_selection(app: &mut App) {
    if app.lazygit.is_none() {
        return;
    }
    let Some(lg_tx) = app.lg_tx.clone() else {
        return;
    };
    let Some(sess) = app.selected_session() else {
        return;
    };
    let new_cwd = PathBuf::from(&sess.workdir);
    if !new_cwd.is_dir() {
        return;
    }
    if app.lazygit_cwd.as_ref() == Some(&new_cwd) {
        return;
    }

    // Drop the old child first so its `Drop` kills it before we open
    // the new master. Holding two PTY pairs simultaneously isn't
    // strictly wrong, but the doomed one keeps draining its read
    // thread into a sink we're about to replace.
    app.lazygit = None;
    let args = extensions::resolve_args(&LAZYGIT, &new_cwd);
    match LocalPty::spawn(LAZYGIT.binary, &args, &new_cwd, 24, 80, lg_tx) {
        Ok(pty) => {
            app.lazygit = Some(pty);
            app.lazygit_cwd = Some(new_cwd.clone());
            app.status_msg = Some(format!("lazygit @ {}", new_cwd.display()));
        }
        Err(e) => {
            app.lazygit_cwd = None;
            if app.focus == Focus::Lazygit {
                app.focus = Focus::Tree;
            }
            app.push_error(format!("lazygit respawn failed: {e}"));
        }
    }
}

/// Translate a crossterm key event into bytes the lazygit PTY understands.
/// Mirrors the subset agentmux/wezterm/etc. handle for embedded TUIs.
///
/// Note: shift-only keys are forwarded normally — the Kitty Keyboard
/// Protocol reports capital letters as `Char('A')` *with* the SHIFT
/// modifier set, where legacy mode reported them as `Char('A')` with no
/// modifier. Either way the codepoint is already the shifted glyph, so
/// we just extend the byte stream and let the receiving program (claude
/// code, a shell, etc.) interpret it.
fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut out = Vec::new();
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_alphabetic() {
                    out.push((lower as u8) - b'a' + 1);
                } else {
                    out.push(c as u8);
                }
            } else if alt {
                out.push(0x1b);
                out.extend(c.to_string().as_bytes());
            } else {
                out.extend(c.to_string().as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        // Ctrl/Alt + Backspace → backward-kill-word. `\x1b\x7f` is the
        // readline default binding for backward-kill-word and is what
        // bash/zsh/fish/Claude Code's input layer all recognise out of
        // the box. Without this branch the modifier was being dropped
        // and the keystroke degraded to a plain single-char delete.
        KeyCode::Backspace if alt || ctrl => out.extend(b"\x1b\x7f"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        // Ctrl/Alt + arrows → word-wise motion via Meta-b / Meta-f. These
        // are the readline default bindings so they work in shells and
        // in Claude Code without any rc-file tweaks. Vim sees them as
        // `Esc b` / `Esc f` which is also word-back/word-forward in
        // normal mode, so embedded vim still does the right thing.
        KeyCode::Left if alt || ctrl => out.extend(b"\x1bb"),
        KeyCode::Right if alt || ctrl => out.extend(b"\x1bf"),
        KeyCode::Up => out.extend(b"\x1b[A"),
        KeyCode::Down => out.extend(b"\x1b[B"),
        KeyCode::Right => out.extend(b"\x1b[C"),
        KeyCode::Left => out.extend(b"\x1b[D"),
        KeyCode::Home => out.extend(b"\x1b[H"),
        KeyCode::End => out.extend(b"\x1b[F"),
        KeyCode::PageUp => out.extend(b"\x1b[5~"),
        KeyCode::PageDown => out.extend(b"\x1b[6~"),
        // Ctrl/Alt + Delete → forward-kill-word (`\x1bd`, readline Meta-d).
        KeyCode::Delete if alt || ctrl => out.extend(b"\x1bd"),
        KeyCode::Delete => out.extend(b"\x1b[3~"),
        KeyCode::Insert => out.extend(b"\x1b[2~"),
        KeyCode::F(n) => match n {
            1 => out.extend(b"\x1bOP"),
            2 => out.extend(b"\x1bOQ"),
            3 => out.extend(b"\x1bOR"),
            4 => out.extend(b"\x1bOS"),
            5 => out.extend(b"\x1b[15~"),
            6 => out.extend(b"\x1b[17~"),
            7 => out.extend(b"\x1b[18~"),
            8 => out.extend(b"\x1b[19~"),
            9 => out.extend(b"\x1b[20~"),
            10 => out.extend(b"\x1b[21~"),
            11 => out.extend(b"\x1b[23~"),
            12 => out.extend(b"\x1b[24~"),
            _ => return None,
        },
        _ => return None,
    }
    Some(out)
}

/// Drive the tree-filter prompt while `app.filter_input_active`. Chars /
/// backspace mutate the live filter (each edit re-clamps the cursor to a
/// still-visible row); Enter commits and exits input mode (the filter
/// remains active so subsequent j/k navigate the filtered view); Esc
/// clears the filter and exits input mode in one shot.
///
/// After every edit we run `update_selection` so the right-hand terminal
/// pane keeps tracking the highlighted session — typing into the filter
/// feels like a live drill-down, not a deferred commit.
fn handle_filter_input_key(
    app: &mut App,
    key: &KeyEvent,
    client: &Client,
) {
    let mut filter = app.tree.filter_str().to_string();
    let mut changed = false;
    match key.code {
        KeyCode::Esc => {
            app.tree.set_filter("");
            app.filter_input_active = false;
            app.status_msg = Some("filter cleared".into());
            changed = true;
        }
        KeyCode::Enter => {
            app.filter_input_active = false;
            app.status_msg = Some(if filter.is_empty() {
                "filter cleared".into()
            } else {
                format!("filter: ⌕{filter}")
            });
        }
        KeyCode::Backspace => {
            if filter.pop().is_some() {
                app.tree.set_filter(&filter);
                app.status_msg = Some(format!("⌕ {filter}"));
                changed = true;
            }
        }
        // Arrow keys move the tree cursor while the search box is open —
        // matches VS Code / browser Find UX so the user can navigate
        // matches without having to commit + re-trigger search.
        KeyCode::Up => {
            app.tree.move_cursor(-1);
            changed = true;
        }
        KeyCode::Down => {
            app.tree.move_cursor(1);
            changed = true;
        }
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            filter.push(c);
            app.tree.set_filter(&filter);
            app.status_msg = Some(format!("⌕ {filter}"));
            changed = true;
        }
        _ => {}
    }
    // Drill into the now-highlighted session so the right pane previews
    // matches as the user types.
    if changed {
        {
            let side = app.target_side();
            update_selection(app, client, side);
        }
    }
}

/// Drive the focused (or specified) terminal slot to whatever the tree
/// cursor is pointing at — abort the previous stream, reset the parser,
/// open a new one. `side` decides which slot to retarget; the unselected
/// slot is left alone. Stream handle and term-tx live on `App` now, so
/// this needs no extra channel parameters.
fn update_selection(app: &mut App, client: &Client, side: Side) {
    let new_id = app.tree.current_session(&app.sessions);
    let current = match side {
        Side::Left => app.selected,
        Side::Right => app.split_right.as_ref().and_then(|s| s.selected),
    };
    if new_id == current {
        return;
    }
    // Abort the prior stream task (if any) for this side.
    let handle_slot = match side {
        Side::Left => &mut app.stream_handle_left,
        Side::Right => &mut app.stream_handle_right,
    };
    if let Some(handle) = handle_slot.take() {
        handle.abort();
    }
    // Remember where we were on the LEFT side before re-pointing it —
    // Ctrl-Tab reads `prev_selected` to flip back. Right-side history
    // isn't tracked yet (one per side would double the state for a
    // marginal feature; revisit if Ctrl-Tab on the right side becomes
    // a thing).
    if side == Side::Left && let Some(prev) = app.selected {
        app.prev_selected = Some(prev);
    }
    // Stash the current parser keyed by its session id, then either
    // restore a cached parser for the new selection (preserves chat
    // history across switches — see `parser_cache`) or install a fresh
    // one. The right slot doesn't get parser caching yet — it's an
    // opt-in side-by-side view and tracking two independent caches
    // doubles the state for marginal benefit.
    let mut restored_from_cache = false;
    match side {
        Side::Left => {
            app.term_in = None;
            if let Some(prev_id) = app.selected.take() {
                let stale = std::mem::replace(&mut app.term, TerminalPane::new());
                app.parser_cache.insert(prev_id, stale);
            }
            if let Some(new_id) = new_id
                && let Some(cached) = app.parser_cache.remove(&new_id)
            {
                app.term = cached;
                restored_from_cache = true;
            }
            app.selected = new_id;
            app.term_size = (0, 0);
        }
        Side::Right => {
            if let Some(slot) = app.split_right.as_mut() {
                slot.term_in = None;
                slot.selected = new_id;
                slot.term.reset();
                slot.term_size = (0, 0);
            }
        }
    }
    // Reset the I/O meter so the chip reflects throughput on the new
    // stream rather than carrying credit from the prior session. The
    // meter is shared across both panes (one pipe to the daemon), so a
    // selection change on either side is the right reset trigger.
    app.io.reset();
    // Open the new stream and stash the handle on App. Pick the right
    // outbound bytes channel for the side so the streams are isolated:
    // bytes from the right session feed the right slot's parser, never
    // the left's.
    if let Some(id) = new_id {
        let term_tx = match side {
            Side::Left => app.term_tx_left.clone(),
            Side::Right => app.term_tx_right.clone(),
        };
        let Some(term_tx) = term_tx else {
            // Channels weren't wired before run_loop set them — should
            // not happen in practice, but bail safely.
            app.status_msg = Some("internal: term channel not initialised".into());
            return;
        };
        let (key_tx, key_rx) = mpsc::unbounded_channel::<TermOut>();
        // Resume signal travels in the WS URL query, not as a wire
        // frame. Old daemons strip unknown query params silently and
        // proceed with the existing snapshot path — no risk of the
        // signal being typed into the agent's prompt by a daemon
        // that doesn't recognise it (the v0.6.20 regression fixed
        // this by gating on capabilities; v0.6.21 makes the signal
        // structurally impossible to misinterpret).
        let want_resume = side == Side::Left && restored_from_cache;
        // Route the WS stream through the client that owns this
        // session — so a peer-profile session is streamed from the
        // right daemon, not the default. Falls back to the passed-in
        // client when the lookup misses (legacy / unmapped sessions).
        let owner = app.client_for_session(id).cloned();
        let stream_client = owner.unwrap_or_else(|| client.clone());
        let handle = stream_client.open_terminal_stream(id, term_tx, key_rx, want_resume);
        match side {
            Side::Left => {
                app.stream_handle_left = Some(handle);
                app.term_in = Some(key_tx);
            }
            Side::Right => {
                app.stream_handle_right = Some(handle);
                if let Some(slot) = app.split_right.as_mut() {
                    slot.term_in = Some(key_tx);
                }
            }
        }
    }
    // Lazygit follow-up is debounced through the tick loop instead of
    // running inline here. Stamping the desired cwd + the keypress
    // moment on `App` lets `drive_pending_lazygit` decide later whether
    // the user has settled on a destination — holding j across 50
    // sessions in different repos used to cause 50 PTY respawns; now
    // it causes one. Right-side splits are deliberately skipped: the
    // pane tracks `app.selected` only.
    if side == Side::Left {
        request_lazygit_for_selection(app);
    }
    // Pull a fresh agent-tasks snapshot for the new selection on the
    // primary (left) side so the right-hand plan/todos panel updates
    // in-step with navigation. The fetch runs spawn-detached — the
    // result lands on `agent_tasks_rx`, not on this stack — so the
    // keystroke handler never blocks on a network round trip even
    // when the daemon is remote. The cache hit path renders
    // synchronously from `agent_tasks` (keyed by id) so the panel
    // shows last-known state immediately and updates the moment the
    // fresh snapshot arrives. Skipped for right-side splits because
    // the panel tracks `app.selected` only.
    if side == Side::Left
        && let Some(id) = new_id
    {
        spawn_agent_tasks_fetch(app, client, id);
    }
}

fn handle_terminal_msg(app: &mut App, msg: TerminalMsg, side: Side) {
    match msg {
        TerminalMsg::Connected => {
            // Snap any in-progress scrollback back to the live tail when
            // recovering from a network drop. Mid-disconnect scrollback
            // is almost always stale once the server replays a delta or
            // sends a fresh snapshot, and leaving the user staring at a
            // pre-drop screen is the most common "is it still working?"
            // confusion. Done once per reconnect cycle (gated on the
            // pending flag) so the initial connect at startup and
            // session-switch resumes don't disturb the user's
            // intentional scrollback position.
            match side {
                Side::Left => {
                    if app.term_reconnect_pending {
                        app.term.scroll_to_bottom();
                        app.term_reconnect_pending = false;
                        app.status_msg = Some("terminal stream reconnected".into());
                    }
                }
                Side::Right => {
                    if let Some(slot) = app.split_right.as_mut() {
                        if slot.term_reconnect_pending {
                            slot.term.scroll_to_bottom();
                            slot.term_reconnect_pending = false;
                            app.status_msg = Some("right-pane stream reconnected".into());
                        }
                    }
                }
            }
        }
        TerminalMsg::Bytes(b) => {
            app.io.record_in(b.len());
            match side {
                Side::Left => app.term.feed(&b),
                Side::Right => {
                    if let Some(slot) = app.split_right.as_mut() {
                        slot.term.feed(&b);
                    }
                }
            }
        }
        TerminalMsg::Reconnecting { attempt, delay_ms } => {
            // Surface the reconnect cycle in the status bar so the user
            // knows the stream is recovering (not silently dead) and
            // mark the side so the next `Connected` snaps scrollback.
            // The events stream has its own dedicated `app.conn` chip;
            // reusing that for the terminal would conflate two
            // independent streams that can drop on different schedules.
            let secs = (delay_ms as f64 / 1000.0).max(0.1);
            let label = match side {
                Side::Left => format!("terminal reconnecting (#{attempt}, in {secs:.1}s)"),
                Side::Right => {
                    format!("right pane reconnecting (#{attempt}, in {secs:.1}s)")
                }
            };
            app.status_msg = Some(label);
            match side {
                Side::Left => app.term_reconnect_pending = true,
                Side::Right => {
                    if let Some(slot) = app.split_right.as_mut() {
                        slot.term_reconnect_pending = true;
                    }
                }
            }
        }
        TerminalMsg::Error(s) => {
            app.push_error(s.trim().to_string());
        }
        TerminalMsg::Closed => {
            app.status_msg = Some(match side {
                Side::Left => "terminal stream closed".into(),
                Side::Right => "right-pane stream closed".into(),
            });
            match side {
                Side::Left => app.term_reconnect_pending = false,
                Side::Right => {
                    if let Some(slot) = app.split_right.as_mut() {
                        slot.term_reconnect_pending = false;
                    }
                }
            }
        }
    }
}

fn handle_lazygit_msg(app: &mut App, msg: PtyMsg) {
    match msg {
        PtyMsg::Bytes(b) => {
            if let Some(lg) = app.lazygit.as_mut() {
                lg.feed(&b);
            }
        }
        PtyMsg::Closed => {
            // Don't drop here; the run-loop's `finished()` poll will tidy
            // up so we keep the parser visible until the user moves on.
        }
    }
}

async fn handle_event_msg(app: &mut App, msg: EventMsg, client: &Client) {
    match msg {
        EventMsg::Connected => {
            app.conn = ConnState::Connected;
            app.was_connected = true;
        }
        EventMsg::Reconnecting { attempt, delay_ms } => {
            app.conn = ConnState::Reconnecting { attempt, delay_ms };
        }
        EventMsg::Closed => {
            if app.was_connected {
                app.conn = ConnState::Disconnected;
            }
        }
        EventMsg::Error(s) => {
            if app.was_connected {
                app.conn = ConnState::Disconnected;
            }
            app.push_error(format!("events: {s}"));
        }
        EventMsg::Raw(kind) => {
            if kind == "bus.lagged" {
                // Surface in both channels: errors overlay (audit trail)
                // and a warn toast (immediate feedback). Mirrors the
                // dashboard, which surfaces this as a toast too.
                app.push_error("event bus lagged (some updates may be missing)");
                push_notification(
                    app,
                    "event stream lagged".to_string(),
                    Some("some updates may be missing".to_string()),
                    NotifKind::Warn,
                );
            }
        }
        EventMsg::Event(ev) => apply_event(app, ev, client).await,
    }
}

/// Fire a non-blocking agent-tasks fetch for `id`. The HTTP round trip
/// runs on a detached tokio task; the result lands on
/// `app.agent_tasks_tx` and is applied by the run-loop's
/// `agent_tasks_rx` arm. Coalesces concurrent requests for the same
/// id (via `agent_tasks_inflight`) so a navigation burst plus an
/// `agent_tasks.updated` event can't fan out into duplicate fetches.
/// On transport error we still post `None` so the in-flight marker
/// gets cleared — the panel keeps showing the last good snapshot.
/// No-op if the channel hasn't been wired yet (pre-`run_loop`
/// contexts only).
fn spawn_agent_tasks_fetch(app: &mut App, client: &Client, id: Uuid) {
    let Some(tx) = app.agent_tasks_tx.clone() else {
        return;
    };
    if !app.agent_tasks_inflight.insert(id) {
        return; // already in-flight — coalesce
    }
    // Route to the owning server's client. agent_tasks for a peer
    // session has to ask that peer's daemon — the default knows
    // nothing about it. Fall back to `client` for legacy / unmapped.
    let owner = app.client_for_session(id).cloned();
    let target = owner.unwrap_or_else(|| client.clone());
    tokio::spawn(async move {
        let payload = match target.agent_tasks(id).await {
            Ok(state) => Some(state),
            Err(e) => {
                tracing::debug!(session = %id, error = %e, "agent_tasks fetch failed");
                None
            }
        };
        let _ = tx.send((id, payload));
    });
}

/// Debounce window between the last navigation and the lazygit PTY
/// respawn. 120 ms is below human "instant" perception for a settled
/// destination yet long enough to coalesce a held-j burst (xterm
/// typematic fires every ~30 ms).
const LAZYGIT_NAV_DEBOUNCE: Duration = Duration::from_millis(120);

/// Stamp the new selection's workdir as pending. The actual lazygit
/// respawn happens later in `drive_pending_lazygit` once the user
/// has stopped moving — see `LAZYGIT_NAV_DEBOUNCE`.
fn request_lazygit_for_selection(app: &mut App) {
    // No lazygit pane open → nothing to follow.
    if app.lazygit.is_none() {
        app.pending_lazygit_cwd = None;
        return;
    }
    let Some(sess) = app.selected_session() else {
        return;
    };
    app.pending_lazygit_cwd = Some(PathBuf::from(&sess.workdir));
    app.last_nav_at = Some(Instant::now());
}

/// Tick-driven catch-up for the deferred lazygit respawn. Fires the
/// PTY swap iff lazygit is open, the pending cwd differs from the
/// live one, and the most recent nav has settled
/// (>= `LAZYGIT_NAV_DEBOUNCE`). Otherwise leaves
/// `pending_lazygit_cwd` in place for the next tick.
fn drive_pending_lazygit(app: &mut App) {
    if app.lazygit.is_none() {
        // Pane was closed mid-debounce — drop the stale pending entry.
        app.pending_lazygit_cwd = None;
        app.last_nav_at = None;
        return;
    }
    let Some(pending) = app.pending_lazygit_cwd.clone() else {
        return;
    };
    if app.lazygit_cwd.as_ref() == Some(&pending) {
        // Already there — nothing to do.
        app.pending_lazygit_cwd = None;
        app.last_nav_at = None;
        return;
    }
    let settled = app
        .last_nav_at
        .is_some_and(|t| t.elapsed() >= LAZYGIT_NAV_DEBOUNCE);
    if !settled {
        return;
    }
    refresh_lazygit_for_selection(app);
    app.pending_lazygit_cwd = None;
    app.last_nav_at = None;
}

async fn apply_event(app: &mut App, ev: Event, client: &Client) {
    let name = ev.session_name.unwrap_or_else(|| "?".into());
    match ev.kind.as_str() {
        "session.crashed" | "watchdog.crashed" => {
            // Log + toast both. The errors overlay is the audit trail;
            // the toast is the in-the-moment alert.
            let reason = ev
                .payload
                .get("reason")
                .and_then(|v| v.as_str())
                .or_else(|| ev.payload.get("signature").and_then(|v| v.as_str()))
                .map(|s| format!("reason: {s}"));
            app.push_error(format!("crashed: {name}"));
            push_notification(
                app,
                format!("{name} crashed"),
                reason,
                NotifKind::Error,
            );
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.idle.remove(&id);
            }
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "session.started" => {
            // Silent — matches the dashboard, which suppresses started
            // events because the initial bus replay would spam toasts on
            // every reconnect.
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "session.stopped" => {
            push_notification(
                app,
                format!("{name} stopped"),
                None,
                NotifKind::Info,
            );
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.idle.remove(&id);
            }
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "watchdog.compact" => {
            push_notification(
                app,
                format!("auto-compacted {name}"),
                Some("watchdog detected low context and sent /compact".to_string()),
                NotifKind::Info,
            );
        }
        "agent.finished" => {
            // Always toast (and chime) — matches `agent.awaiting_input`.
            // The user wants the same audible/visible cue whether they're
            // staring at the pane or tabbed away to lazygit, the palette,
            // or another tmux window. The pane-visible suppression we
            // used to do here meant a long agent run could finish under
            // your nose with zero alert.
            push_notification(
                app,
                format!("{name} finished"),
                None,
                NotifKind::Info,
            );
            // Working→Idle: the agent is now sleeping at the prompt.
            // Mirror the watchdog's ActivityState::Idle so the sidebar
            // dot shows a muted `◌` instead of a misleading green `●`.
            // Defensive cleanup of awaiting_input in case
            // `agent.input_resolved` was missed (event-bus lag,
            // watchdog restart).
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.idle.insert(id);
            }
        }
        "agent.awaiting_input" => {
            // Awaiting input is a "you have to do something" event, so we
            // toast even when the session is selected — the user might be
            // tabbed away to lazygit / errors / palette and miss it.
            push_notification(
                app,
                format!("{name} needs input"),
                Some("agent is waiting on a permission prompt".to_string()),
                NotifKind::Warn,
            );
            if let Some(id) = ev.session_id {
                app.awaiting_input.insert(id);
                // An awaiting agent isn't sleeping — drop any stale idle
                // bit so the dot doesn't briefly flicker through `◌`
                // before it lands on `▲`.
                app.idle.remove(&id);
            }
        }
        "agent.working" => {
            // Agent just resumed work (Idle → Working). Without this the
            // sidebar dot stays grey while the agent is visibly working —
            // the bug that ships before this handler exists. No toast: a
            // quiet resume isn't notification-worthy.
            if let Some(id) = ev.session_id {
                app.idle.remove(&id);
                app.awaiting_input.remove(&id);
            }
        }
        "agent.input_resolved" => {
            // User answered the prompt (or it was dismissed). Clear the
            // yellow attention dot — no toast: the action that resolved
            // the prompt is what the user just did. Payload tells us
            // whether the agent is now working again or back at the
            // prompt, so we can flip the dot directly to green or `◌`
            // without waiting for a follow-up `agent.finished`.
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                let resolved = ev.payload.get("state").and_then(|v| v.as_str());
                match resolved {
                    Some("idle") => {
                        app.idle.insert(id);
                    }
                    Some("working") => {
                        app.idle.remove(&id);
                    }
                    // Older watchdogs (pre-v0.6.28) emit this event with
                    // no payload. Without the resolved-state hint we
                    // can't tell working from idle, so we just clear
                    // awaiting and let the next finished/working event
                    // settle the idle bit.
                    _ => {}
                }
            }
        }
        "session.created" => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "session.deleted" => {
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.idle.remove(&id);
            }
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "agent_tasks.updated" => {
            // Server tail-watcher saw new bytes in this session's
            // transcript. Refresh only if it's for a session we've
            // actually got cached (or the currently selected one) —
            // avoids a fetch storm when many agents are typing at once.
            let target = ev
                .payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .or(ev.session_id);
            let Some(id) = target else { return };
            let interesting =
                Some(id) == app.selected || app.agent_tasks.contains_key(&id);
            if interesting {
                spawn_agent_tasks_fetch(app, client, id);
            }
        }
        // Sidebar metadata events. Both refetch the sessions list so
        // the tree row label / tool chip update without manual refresh,
        // and we keep the cursor pinned to the affected session.
        "session.renamed" => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                if let Some(id) = ev.session_id {
                    app.tree.select_session(id);
                }
            }
            // No toast — the rename action itself already flashed a
            // status message. A second visible signal would be noise.
        }
        "session.tool_changed" => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                if let Some(id) = ev.session_id {
                    app.tree.select_session(id);
                }
            }
            // Surface the swap as an info toast so the user notices
            // without staring at the chip — the watchdog only fires
            // this when the foreground process really did change.
            let new_tool = ev
                .payload
                .get("tool")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(tool) = new_tool {
                push_notification(
                    app,
                    format!("{name} → {tool}"),
                    None,
                    NotifKind::Info,
                );
            }
        }
        "preferences.changed" => {
            // Dashboard (or another TUI) just persisted a different theme.
            // Adopt the new TUI palette in place — no notification, the
            // visible repaint is the feedback.
            if let Some(name) = ev.payload.get("tui_theme").and_then(|v| v.as_str()) {
                app.set_theme_by_name(name);
            }
        }
        _ => {}
    }
}

/// Map an in-memory `NotifKind` to the prefs-side `SoundKind`. Two enums
/// exist so `prefs.rs` stays free of upward dependencies; this is the
/// canonical translation point.
pub(super) fn sound_kind_of(kind: NotifKind) -> prefs::SoundKind {
    match kind {
        NotifKind::Info => prefs::SoundKind::Info,
        NotifKind::Warn => prefs::SoundKind::Warn,
        NotifKind::Error => prefs::SoundKind::Error,
    }
}

/// Append a typed toast to the bottom-left notification stack and play
/// the matching system sound when both the persisted prefs AND the CLI
/// override allow it. Evicts the oldest entry FIFO when the stack would
/// exceed `MAX_NOTIFS`. TTL is sourced from `prefs.ttl_for(kind)` so the
/// user can tune it via the Settings overlay.
fn push_notification(app: &mut App, title: String, body: Option<String>, kind: NotifKind) {
    let id = app.next_notif_id;
    app.next_notif_id = app.next_notif_id.saturating_add(1);
    let sk = sound_kind_of(kind);
    app.notifications.push(Notification {
        id,
        title,
        body,
        kind,
        created_at: Instant::now(),
        ttl: app.prefs.ttl_for(sk),
    });
    if app.notifications.len() > MAX_NOTIFS {
        let drop_n = app.notifications.len() - MAX_NOTIFS;
        app.notifications.drain(0..drop_n);
    }
    // Two layers: persisted user pref AND one-shot CLI override (script
    // use, --no-sound). CLI is OR'd on top because it's "extra silence",
    // never re-enabling.
    if !app.sound_muted_cli && app.prefs.sound_enabled_for(sk) {
        super::sound::play(kind);
    }
}

/// Pick the next free `shell-N` name by scanning existing session names.
/// Replaces the old random `shell-{uuid8}` scheme — users complained the
/// random suffix was both ugly and hard to refer back to ("which shell
/// was that?"). Sequential names are stable and collision-free since the
/// server enforces unique names anyway.
fn next_shell_name(sessions: &[Session]) -> String {
    let mut max_n = 0u32;
    for s in sessions {
        if let Some(rest) = s.name.strip_prefix("shell-") {
            if let Ok(n) = rest.parse::<u32>() {
                if n > max_n {
                    max_n = n;
                }
            }
        }
    }
    format!("shell-{}", max_n + 1)
}

/// Spawn a plain interactive shell as a session. Uses the `terminal`
/// adapter so the server picks the user's `$SHELL` (fish / zsh / bash)
/// rather than hard-coding bash — matching what the user gets when they
/// open a new tab in their host terminal. Stored as a regular session
/// so it appears in the tree and can be killed/deleted like any other
/// agent.
async fn spawn_plain_terminal(
    app: &mut App,
    client: &Client,
) {
    let name = next_shell_name(&app.sessions);
    let workdir = app
        .selected_session()
        .map(|s| s.workdir.clone())
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".into());
    match client
        .create_session(&name, &workdir, "terminal", None, vec![])
        .await
    {
        Ok(created) => {
            let id = created.id;
            if let Err(e) = client.start_session(id).await {
                app.push_error(format!("shell start failed: {e}"));
            } else {
                push_notification(
                    app,
                    format!("shell: {name}"),
                    None,
                    NotifKind::Info,
                );
            }
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                app.tree.select_session(id);
                {
            let side = app.target_side();
            update_selection(app, client, side);
        }
                app.set_focus(Focus::Term);
            }
        }
        Err(e) => {
            app.push_error(format!("shell create failed: {e}"));
        }
    }
}

pub fn status_dot(s: Status) -> (&'static str, ratatui::style::Color) {
    use ratatui::style::Color;
    match s {
        Status::Running => ("●", Color::Green),
        Status::Idle => ("○", Color::DarkGray),
        // Stopped is grey, not yellow — yellow now belongs exclusively to
        // the awaiting-input overlay (▲) so the two states never collide
        // at a glance. A stopped terminal is dormant, not blocked on you.
        Status::Stopped => ("◐", Color::DarkGray),
        Status::Crashed => ("✗", Color::Red),
    }
}

#[cfg(test)]
mod merge_dedup_tests {
    use super::*;
    use agentum_core::Status;
    use time::OffsetDateTime;

    fn sess(id: Uuid, name: &str) -> Session {
        let now = OffsetDateTime::now_utc();
        Session {
            id,
            name: name.to_string(),
            workdir: "/tmp".to_string(),
            tool: "claude".to_string(),
            model: None,
            flags: Vec::new(),
            status: Status::Idle,
            tmux_target: None,
            created_at: now,
            updated_at: now,
            last_activity_at: None,
            tokens: None,
            cost_usd: None,
            ctx: None,
            last_log: None,
            uptime_seconds: None,
            state: None,
            pinned: false,
        }
    }

    // The bug we're guarding: same daemon reached via two profiles (a
    // loopback "" key + a named "macos") returns the same session id
    // from both list_sessions calls. Without dedup, the sidebar paints
    // the session twice — once per profile group.
    #[test]
    fn dedupes_same_id_across_profiles() {
        let id = Uuid::new_v4();
        let per_profile = vec![
            ("".to_string(), vec![sess(id, "alpha")]),
            ("macos".to_string(), vec![sess(id, "alpha")]),
        ];
        let (merged, owners) = merge_sessions_dedup(per_profile, "macos");
        assert_eq!(merged.len(), 1);
        // Active profile "macos" wins the contested id.
        assert_eq!(owners.get(&id).map(String::as_str), Some("macos"));
    }

    #[test]
    fn named_profile_beats_loopback_when_neither_is_active() {
        let id = Uuid::new_v4();
        let per_profile = vec![
            ("".to_string(), vec![sess(id, "alpha")]),
            ("macos".to_string(), vec![sess(id, "alpha")]),
        ];
        // active_key = "" → loopback is active; loopback should win.
        let (_, owners) = merge_sessions_dedup(per_profile.clone(), "");
        assert_eq!(owners.get(&id).map(String::as_str), Some(""));
        // active_key = "other" (neither): named beats loopback.
        let (_, owners) = merge_sessions_dedup(per_profile, "other");
        assert_eq!(owners.get(&id).map(String::as_str), Some("macos"));
    }

    #[test]
    fn distinct_ids_all_kept() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let per_profile = vec![
            ("".to_string(), vec![sess(a, "alpha")]),
            ("macos".to_string(), vec![sess(b, "beta")]),
        ];
        let (merged, owners) = merge_sessions_dedup(per_profile, "");
        assert_eq!(merged.len(), 2);
        assert_eq!(owners.get(&a).map(String::as_str), Some(""));
        assert_eq!(owners.get(&b).map(String::as_str), Some("macos"));
    }
}
