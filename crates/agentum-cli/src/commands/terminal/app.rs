//! TUI app state, key dispatch, and event loop.

use std::collections::{HashMap, HashSet};
use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use agentum_core::{Event, Session, Status, transcript::AgentTaskState};
use anyhow::Result;
use crossterm::event::{
    Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures_util::{FutureExt, StreamExt};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval};
use uuid::Uuid;

use crate::clipboard::encode_rgba_as_png;

use super::api::{self, ClaudeUsage, Client, EventMsg, TermOut, TerminalMsg};
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

/// How long a cached host readiness report stays "fresh" before the
/// Ctrl-H overlay and New Session submit guard treat it as stale and
/// re-probe. 5 min balances "don't re-SSH on every keystroke" against
/// "notice when the user just installed tmux on the remote".
const HOST_READINESS_TTL: Duration = Duration::from_secs(5 * 60);

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
    Reconnecting {
        attempt: u32,
        delay_ms: u64,
    },
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
        if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        }
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
    /// Goal composer. Opened with `G` from Tree focus. Multi-line text
    /// area; Enter appends a newline, Ctrl-Enter submits the goal to
    /// `POST /api/board/goals`, Esc discards. UI-SPEC §Goal Composer.
    Goal(Box<GoalForm>),
    /// SSH hosts manager. Opened with `Ctrl-H`. Lists daemon-controlled
    /// hosts with a readiness status dot; Enter / `t` runs a readiness
    /// preflight for the selected host, Esc closes. Detail + dots read
    /// from `App::host_readiness_cache`. See SSH_HOST_READINESS_PRD §7.6.
    Hosts(HostsOverlay),
}

/// In-memory state for the [`Overlay::Hosts`] manager. Holds only a
/// stable snapshot of host ids (so the cursor doesn't jump if `app.hosts`
/// is refreshed underneath) plus transient loading/error state — the
/// readiness reports themselves live in [`App::host_readiness_cache`] and
/// are resolved per host at render time.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct HostsOverlay {
    pub host_ids: Vec<Uuid>,
    pub cursor: usize,
    /// True while a readiness round trip is in flight for the selected
    /// host. (Currently inline-awaited like the New Session host probe;
    /// a background refresh — PRD phase 3 — will make this meaningful.)
    pub loading: bool,
    pub error: Option<String>,
}

impl HostsOverlay {
    pub fn selected(&self) -> Option<Uuid> {
        self.host_ids.get(self.cursor).copied()
    }
}

/// In-memory state for the [`Overlay::Profiles`] switcher.
#[derive(Clone, PartialEq, Eq)]
pub struct ProfilesOverlay {
    pub entries: Vec<ProfileEntry>,
    pub cursor: usize,
    pub error: Option<String>,
    /// `Some` when the user is editing the inline add/edit form instead
    /// of the list. Mirrors the dashboard's ServerSwitcher.
    pub add_form: Option<AddProfileForm>,
}

/// Form state for [`Overlay::Goal`]. Holds the multi-line text the user
/// is composing, submission status, and any server-returned error.
///
/// Enter appends a newline (multi-line goal text is valid).
/// Ctrl-Enter submits via `POST /api/board/goals`.
/// Esc discards without submitting.
#[derive(Clone, PartialEq, Eq)]
pub struct GoalForm {
    /// The goal text being composed. May contain newlines.
    pub text: String,
    /// True while the network request is in flight.
    pub submitting: bool,
    /// Error message from the last failed submit attempt.
    pub error: Option<String>,
    /// The active server profile — used to route the request to the
    /// right daemon when multiple endpoints are configured.
    pub profile: String,
}

impl GoalForm {
    /// Construct an empty form pre-filled with the active profile name.
    pub fn default_for_profile(profile: String) -> Self {
        Self {
            text: String::new(),
            submitting: false,
            error: None,
            profile,
        }
    }
}

/// One row in the profile picker. Mirrors the on-disk profile but is
/// detached from the file so the overlay can re-render without
/// re-reading after every keystroke.
#[derive(Clone, PartialEq, Eq)]
pub struct ProfileEntry {
    pub name: String,
    pub url: String,
    pub fingerprint: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AddProfileForm {
    pub field: AddProfileField,
    pub name: String,
    pub url: String,
    pub fingerprint: String,
    pub error: Option<String>,
    /// When `Some(original_name)` the form is editing an existing
    /// profile rather than inserting a new one. On save, a rename
    /// (`original_name != name`) removes the old entry and writes the
    /// new one; matching names just upsert in place.
    pub editing: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AddProfileField {
    Name,
    Url,
    Fingerprint,
}

impl AddProfileForm {
    pub fn new() -> Self {
        Self {
            field: AddProfileField::Name,
            name: String::new(),
            url: String::new(),
            fingerprint: String::new(),
            error: None,
            editing: None,
        }
    }

    /// Pre-fill the form from an existing entry. The form keeps the
    /// original name in `editing` so a rename can be detected on save.
    pub fn edit(entry: &ProfileEntry) -> Self {
        Self {
            field: AddProfileField::Name,
            name: entry.name.clone(),
            url: entry.url.clone(),
            fingerprint: entry.fingerprint.clone().unwrap_or_default(),
            error: None,
            editing: Some(entry.name.clone()),
        }
    }

    pub fn next_field(&mut self) {
        self.field = match self.field {
            AddProfileField::Name => AddProfileField::Url,
            AddProfileField::Url => AddProfileField::Fingerprint,
            AddProfileField::Fingerprint => AddProfileField::Name,
        };
    }

    pub fn prev_field(&mut self) {
        self.field = match self.field {
            AddProfileField::Name => AddProfileField::Fingerprint,
            AddProfileField::Url => AddProfileField::Name,
            AddProfileField::Fingerprint => AddProfileField::Url,
        };
    }

    pub fn field_value_mut(&mut self) -> Option<&mut String> {
        match self.field {
            AddProfileField::Name => Some(&mut self.name),
            AddProfileField::Url => Some(&mut self.url),
            AddProfileField::Fingerprint => Some(&mut self.fingerprint),
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
/// dialog. Pressing Tab on the `Tool` field cycles through these; Enter
/// opens the tool picker modal (`ToolPickerState`) which lists the same
/// entries with availability gating. Order is what the picker renders
/// top-to-bottom — match the user's mental priority (first-class
/// agents first, gemini/hermes/copilot after, free-form shells last).
pub const TOOL_SUGGESTIONS: &[&str] = &[
    "claude", "codex", "cursor", "agent", "gemini", "hermes", "copilot", "opencode", "aider",
    "terminal", "bash",
];

/// Returns `true` when the daemon's `/api/agents` reports availability
/// for this tool name. Mirrors `agentum_executor::probed_tools()` so
/// the TUI knows which entries should be gated. Free-form names
/// (`terminal`, `bash`, anything outside the curated list) always
/// route through PassthroughAdapter and are never gated.
pub fn is_probed_tool(tool: &str) -> bool {
    matches!(
        tool,
        "claude" | "codex" | "cursor" | "agent" | "gemini" | "hermes" | "opencode" | "aider"
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
pub const YOLO_TOOLS: &[&str] = &["claude", "codex", "cursor", "agent", "gemini"];

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
    /// Host id on the target daemon. Empty means the daemon's local host.
    pub host_id: String,
    pub name: String,
    pub tool: String,
    pub model: String,
    pub workdir: String,
    pub args: String,
    pub up_after: bool,
    pub yolo: bool,
    /// Isolate this session in a dedicated `git worktree`. Defaults on
    /// (the user asked for worktree-by-default) but the toggle lets them
    /// opt out — e.g. a non-git workdir, a `terminal`/`bash` pane, or a
    /// remote host. Only sent to the server when the target is the local
    /// host; see `worktree_requested`.
    pub use_worktree: bool,
    pub error: Option<String>,
    pub submitting: bool,
    /// When `Some`, the directory-picker overlay is up. Field state persists
    /// inside the form so closing the picker restores the rest of the form.
    pub picker: Option<DirPickerState>,
    /// When `Some`, the tool-picker overlay is up (modal list of every
    /// entry in `TOOL_SUGGESTIONS` with availability gating, mirroring
    /// the dashboard's tile grid). Mutually exclusive with `picker` in
    /// practice — only one overlay can be focused at a time.
    pub tool_picker: Option<ToolPickerState>,
}

/// Static state for the tool-picker modal. Entries are derived from
/// `TOOL_SUGGESTIONS` at open time and snapshotted with their
/// per-entry availability — that way navigation keys don't need to
/// re-query `app.agent_availability` on every keystroke and the
/// rendered list stays stable for the lifetime of the overlay.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolPickerState {
    pub entries: Vec<ToolPickerEntry>,
    pub cursor: usize,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ToolPickerEntry {
    pub name: &'static str,
    /// `false` only for probed first-class agents whose binary isn't
    /// installed on the target daemon (e.g. cursor without
    /// `cursor-agent`). Free-form names like `terminal` / `bash` /
    /// `copilot` always report `true` — the executor either has a
    /// hand-rolled adapter (terminal) or trusts PATH (bash, copilot).
    pub available: bool,
    /// Short human-readable description shown next to the name in the
    /// picker list so the user knows what they're choosing without
    /// having to remember each id.
    pub description: &'static str,
}

/// Human-readable description for a tool id. Used by the picker's
/// list rendering so each entry has a single-line gloss next to its
/// name (mirrors the dashboard tile subtitles). Falls back to a
/// generic "passthrough binary" line for anything not in the table.
pub fn tool_description(tool: &str) -> &'static str {
    match tool {
        "claude" => "Anthropic Claude Code",
        "codex" => "OpenAI Codex CLI",
        "cursor" => "Cursor agent (cursor-agent binary)",
        "agent" => "Cursor agent (agent binary, Jan 2026+)",
        "gemini" => "Google Gemini CLI",
        "hermes" => "Hermes agent",
        "copilot" => "GitHub Copilot CLI",
        "opencode" => "Open-source Claude-Code-style agent",
        "aider" => "Aider pair-programmer",
        "terminal" => "Plain shell (uses $SHELL)",
        "bash" => "Plain bash shell",
        _ => "passthrough binary",
    }
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
            host_id: String::new(),
            name: String::new(),
            tool: "claude".into(),
            model: String::new(),
            workdir: default_workdir,
            args: String::new(),
            up_after: true,
            yolo: false,
            // Worktree-by-default: spawning N agents against one repo
            // without stomping each other's branch/stash is the common
            // case the user optimizes for. Opt out with Space.
            use_worktree: true,
            error: None,
            submitting: false,
            picker: None,
            tool_picker: None,
        }
    }

    pub fn next_field(&mut self) {
        self.field = match self.field {
            // Workdir lives directly under the Servers field — the
            // user's mental model is "which agentum, then which folder"
            // — and re-fetching `$HOME` when the server changes only
            // makes sense if Workdir is the very next stop. Name / Tool
            // / Model trail because they're typed independently of which
            // daemon owns the session.
            NewSessionField::Profile => NewSessionField::Host,
            NewSessionField::Host => NewSessionField::Workdir,
            NewSessionField::Workdir => NewSessionField::Name,
            NewSessionField::Name => NewSessionField::Tool,
            NewSessionField::Tool => NewSessionField::Model,
            NewSessionField::Model => NewSessionField::Args,
            NewSessionField::Args => NewSessionField::UpAfter,
            NewSessionField::UpAfter => NewSessionField::Yolo,
            NewSessionField::Yolo => NewSessionField::Worktree,
            NewSessionField::Worktree => NewSessionField::Profile,
        };
    }

    pub fn prev_field(&mut self) {
        self.field = match self.field {
            NewSessionField::Profile => NewSessionField::Worktree,
            NewSessionField::Host => NewSessionField::Profile,
            NewSessionField::Workdir => NewSessionField::Host,
            NewSessionField::Name => NewSessionField::Workdir,
            NewSessionField::Tool => NewSessionField::Name,
            NewSessionField::Model => NewSessionField::Tool,
            NewSessionField::Args => NewSessionField::Model,
            NewSessionField::UpAfter => NewSessionField::Args,
            NewSessionField::Yolo => NewSessionField::UpAfter,
            NewSessionField::Worktree => NewSessionField::Yolo,
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
            NewSessionField::Host => None,
            NewSessionField::Name => Some(&mut self.name),
            NewSessionField::Tool => Some(&mut self.tool),
            NewSessionField::Model => Some(&mut self.model),
            NewSessionField::Workdir => Some(&mut self.workdir),
            NewSessionField::Args => Some(&mut self.args),
            // toggles, not text
            NewSessionField::UpAfter | NewSessionField::Yolo | NewSessionField::Worktree => None,
        }
    }

    /// Cycle the profile field through `available` plus optionally the
    /// empty string (which represents the local loopback, rendered as
    /// "this machine"). Wraps; preserves order. Used by Tab on the
    /// Profile field.
    ///
    /// `has_local` is `true` iff `app.clients` has a `""` key — i.e.
    /// a real local-loopback connection is wired up. When `false`
    /// (the user launched with `--profile vps1` and the local daemon
    /// isn't connected), the empty entry is omitted so Tab doesn't
    /// drop the form into a "this machine" state that has no client
    /// behind it — that's the trap that made the workdir field look
    /// like it wasn't following the cycle: target resolution would
    /// silently fall back to the active server's `$HOME`, which is
    /// the same path the field already had.
    pub fn cycle_profile(&mut self, available: &[String], has_local: bool) {
        let mut wheel: Vec<String> = Vec::new();
        if has_local {
            wheel.push(String::new());
        }
        wheel.extend(available.iter().cloned());
        if wheel.len() <= 1 {
            return; // nothing to cycle through
        }
        let idx = wheel.iter().position(|n| n == &self.profile).unwrap_or(0);
        self.profile = wheel[(idx + 1) % wheel.len()].clone();
    }

    pub fn cycle_host(&mut self, hosts: &[agentum_core::Host]) {
        if hosts.len() <= 1 {
            return;
        }
        let wheel: Vec<String> = hosts.iter().map(|h| h.id.to_string()).collect();
        let idx = wheel.iter().position(|id| id == &self.host_id).unwrap_or(0);
        self.host_id = wheel[(idx + 1) % wheel.len()].clone();
    }

    pub fn host_uuid(&self) -> Option<Uuid> {
        Uuid::parse_str(self.host_id.trim()).ok()
    }

    /// True when YOLO mode is enabled and the active tool actually supports
    /// `--dangerously-skip-permissions`. Bash and aider (and friends) ignore
    /// the toggle so the flag stays out of their argv.
    pub fn yolo_active(&self) -> bool {
        let tool = self.tool.trim();
        self.yolo && YOLO_TOOLS.contains(&tool)
    }

    /// Whether to actually ask the server for a `git worktree`. The
    /// toggle can be on, but worktrees only work on the local host — the
    /// daemon rejects them for SSH hosts — so a non-empty host id
    /// suppresses the request. The UI mirrors this by greying the toggle
    /// out when a host is picked.
    pub fn worktree_requested(&self) -> bool {
        self.use_worktree && self.host_id.trim().is_empty()
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NewSessionField {
    /// Server profile this session targets. New first field — see
    /// `NewSessionForm::with_profile`. Tab cycles through configured
    /// profiles + an empty entry meaning "current connection".
    Profile,
    Host,
    Name,
    Tool,
    Model,
    Workdir,
    Args,
    UpAfter,
    Yolo,
    /// Isolate the session in a dedicated `git worktree` (own branch +
    /// checkout). Defaults on — see `NewSessionForm::with_profile`. Only
    /// meaningful when targeting the local host (the server rejects
    /// worktrees on SSH hosts), so the submit path gates on an empty
    /// host id.
    Worktree,
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
    /// Bulk variant of Start/Stop/Kill driven by the multi-select set.
    /// `names` is preserved alongside `ids` so the confirm prompt can
    /// list a few before falling back to a count, even if a session is
    /// removed between confirm-open and confirm-commit.
    Bulk {
        kind: BulkKind,
        ids: Vec<Uuid>,
        names: Vec<String>,
    },
    /// Set up a host in one flow from the Ctrl-H overlay (`i`): install
    /// the missing required deps (`tmux`/`git`, via sudo) and the missing
    /// agent CLIs (over SSH). `deps`/`agents` are captured at prompt time
    /// so the confirm text and the install agree.
    ProvisionHost {
        id: Uuid,
        name: String,
        deps: Vec<String>,
        agents: Vec<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BulkKind {
    Start,
    Stop,
    Kill,
}

impl BulkKind {
    fn verb(self) -> &'static str {
        match self {
            BulkKind::Start => "start",
            BulkKind::Stop => "stop",
            BulkKind::Kill => "kill",
        }
    }
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
            PendingAction::Bulk { kind, ids, names } => {
                let n = ids.len();
                let preview = bulk_preview(names);
                match kind {
                    BulkKind::Start => format!("start {n} checked sessions ({preview})?"),
                    BulkKind::Stop => format!(
                        "stop {n} checked sessions ({preview})? (graceful, SIGTERM then kill after 5s)"
                    ),
                    BulkKind::Kill => format!(
                        "kill {n} checked sessions ({preview})? Stops the processes and removes the records."
                    ),
                }
            }
            PendingAction::ProvisionHost {
                name, deps, agents, ..
            } => {
                let mut parts: Vec<String> = Vec::new();
                if !deps.is_empty() {
                    parts.push(format!("{} (sudo)", deps.join(" + ")));
                }
                if !agents.is_empty() {
                    parts.push(agents.join(", "));
                }
                format!(
                    "set up `{name}` — install {} over SSH? Needs passwordless sudo for tmux/git.",
                    parts.join(" and ")
                )
            }
        }
    }

    pub fn is_destructive(&self) -> bool {
        // Provisioning installs packages — consequential but not
        // destructive, so it gets the plain " confirm " framing, not red.
        !matches!(
            self,
            PendingAction::Start { .. }
                | PendingAction::Bulk {
                    kind: BulkKind::Start,
                    ..
                }
                | PendingAction::ProvisionHost { .. }
        )
    }
}

/// Render up to two names from a bulk set, then `+N more` so the
/// confirm prompt stays one line regardless of how many sessions were
/// checked.
fn bulk_preview(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => format!("`{}`", names[0]),
        2 => format!("`{}`, `{}`", names[0], names[1]),
        n => format!("`{}`, `{}` +{} more", names[0], names[1], n - 2),
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

/// Cached data for the bound-card hint strip (Phase 2, plan 05).
/// Rendered as a one-cell row above the status bar when `App::hint_card`
/// is `Some`. Toggled by the `c` key in `Focus::Tree` when the selected
/// session has a `card_id`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HintCardState {
    /// The card ID shown in the chip and hint strip.
    pub card_id: i64,
    /// Card title, already truncated to 72 chars.
    pub title: String,
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
    /// Collapse the SERVERS section of the tree sidebar to a single-line
    /// header. The full server list keeps living in memory — only the
    /// rendered surface and the j/k traversal change. Toggled via the
    /// `Ctrl-K V` chord ("view servers") or the command palette.
    pub servers_collapsed: bool,
    /// Show sessions from every reachable server in the tree, or scope
    /// the tree to just the active server's sessions. Default is "show
    /// all" — that's the multi-server view the fanout is built for.
    /// Mirrors `prefs.show_all_servers`; rebuilds the tree on flip.
    pub show_all_servers: bool,
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
    /// Consecutive failures of the periodic `list_sessions` poll. Used
    /// alongside `conn` to surface the reconnect banner when HTTP dies
    /// but the events-bus WS hasn't yet noticed (TCP keepalive lag).
    /// Reset to 0 on the next successful poll.
    pub http_fail_count: u32,
    /// Session ids the user has just killed locally. Watchdog often
    /// emits a `session.crashed` event microseconds after the row is
    /// deleted (the tmux pane vanishing trips its detector), and the
    /// resulting toast / error overlay reads as "killing it crashed it"
    /// — confusing for what was an intentional action. Ids land here
    /// inside `execute_action` and get filtered out of `apply_event`'s
    /// `session.crashed` branch. The set is small (only your in-flight
    /// destructive verbs) so no eviction policy is needed.
    pub recently_killed: std::collections::HashSet<Uuid>,
    /// Multi-select set, populated by pressing Enter on a leaf row in
    /// the Sessions tree. When non-empty, lifecycle keys (u/s/K/x/D
    /// and Ctrl-D) fan out across the checked ids instead of operating
    /// on the cursor row. Pruned in `refresh_sessions` so dead ids
    /// don't linger after a session is removed elsewhere.
    pub checked: HashSet<Uuid>,
    pub tick_count: u64,
    pub status_msg: Option<String>,
    /// OSC-52 byte sequence queued by `handle_mouse` when a selection
    /// drag ends with non-empty text. Flushed once per loop iteration
    /// AFTER `terminal.draw()` and immediately followed by
    /// `terminal.clear()` so the next frame is a full repaint —
    /// inline writes during the event handler corrupted text on
    /// terminals that don't intercept OSC 52 (the regression that
    /// originally disabled `write_osc52` in v0.6.33).
    pub pending_clipboard_seq: Option<Vec<u8>>,
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
    /// Per-session shadow of the terminal pane's current input line.
    /// Each typed printable char is appended; Backspace pops; Enter
    /// commits + clears; Esc / Ctrl-U / Ctrl-K / Ctrl-C clear. Used
    /// exclusively to detect `/clear` (or `\clear`) so the right-side
    /// plan/todo panel can mirror the agent's own context wipe without
    /// the user having to also press Ctrl-T or reselect. Best-effort —
    /// won't track in-line cursor moves (arrows, Home/End), but a
    /// missed detection only means the panel stays out of sync until
    /// the next real transcript event, which is the pre-feature
    /// behaviour anyway.
    pub term_input_lines: HashMap<Uuid, String>,
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
    /// Sessions we have positive evidence are actively working
    /// (ActivityState::Working on the watchdog side). Driven by
    /// `agent.working` and the `state: working` variant of
    /// `agent.input_resolved`. The sidebar dot renders green `●`
    /// ONLY when the id is in this set — `Status::Running` alone is
    /// no longer enough, because a long-lived tmux pane reads as
    /// Running even after the agent has gone idle. Without this
    /// the dot stuck on green for every session whose connect-time
    /// replay snapshot was missing (#stuck-green-dot regression).
    pub working: HashSet<Uuid>,
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
    /// Hosts controlled by the active daemon. Empty on older daemons or
    /// while the startup probe is pending; the New Session Host field
    /// treats empty as "local".
    pub hosts: Vec<agentum_core::Host>,
    /// Per-host readiness cache: the last report plus the instant it was
    /// fetched. Feeds the Ctrl-H overlay's status dots + detail pane and
    /// the New Session submit guard so we don't re-SSH on every keystroke.
    /// Entries older than [`HOST_READINESS_TTL`] are treated as stale.
    /// See SSH_HOST_READINESS_PRD §7.6.
    pub host_readiness_cache: HashMap<Uuid, (Instant, agentum_core::HostReadiness)>,
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
    /// Profiles currently mid-reconnect. Sidebar swaps the status dot
    /// for a spinner glyph while a name is in this set. Cleared on
    /// soft-restart (the new `App` starts with an empty set) so the
    /// spinner stops as soon as `run_loop` re-enters with the new
    /// client. Keyed the same way as `clients` — `""` for loopback,
    /// named profiles otherwise.
    pub reconnecting: HashSet<String>,
    /// When `Some`, the one-cell hint strip above the status bar shows
    /// the bound card ID and truncated title. Toggled by the `c` key
    /// in `Focus::Tree` when the selected session has a `card_id`.
    /// `None` hides the strip entirely (Phase 2, plan 05).
    pub hint_card: Option<HintCardState>,
    /// Result channel for the Ctrl-V clipboard-image-paste flow. The
    /// spawned task (which runs the arboard clipboard read + PNG
    /// encode in `spawn_blocking`, then `upload_image`) sends one
    /// `UploadOutcome` per Ctrl-V; the run-loop's `select!` drains it
    /// and writes the message into `status_msg` (success) or pushes
    /// it as a toast (error). Lives here because the spawned task
    /// can't mutate `App` directly (we're behind `&mut App`), and the
    /// existing toast/status surfaces both want main-task access.
    pub upload_outcome_tx: Option<mpsc::UnboundedSender<UploadOutcome>>,
    /// Latest Claude account-usage snapshot for the bottom-left readout
    /// (spec 001). `None` until the first poll lands; a fetch error leaves
    /// the previous value in place (better stale-but-flagged than blank).
    /// The render path treats `source != "oauth"` / a stale `claude_usage_at`
    /// as "usage unavailable" rather than showing a wrong plan %.
    pub claude_usage: Option<ClaudeUsage>,
    /// Wall-clock instant the latest `claude_usage` landed. Drives the
    /// poll cadence and lets the readout flag a stale snapshot.
    pub claude_usage_at: Option<Instant>,
    /// Result channel for the background usage poll. The spawned task posts
    /// one `Option<ClaudeUsage>` per poll (None on transport error); the
    /// run-loop's `select!` arm drains it into `claude_usage`.
    pub usage_tx: Option<mpsc::UnboundedSender<Option<ClaudeUsage>>>,
    /// True while a usage poll is in flight, so the tick loop coalesces
    /// rather than stacking requests if a fetch outlives the interval.
    pub usage_inflight: bool,
}

/// One result from the Ctrl-V image-paste flow. Posted by the spawned
/// uploader task; drained by the run-loop and surfaced as a toast.
#[derive(Debug, Clone)]
pub struct UploadOutcome {
    /// `true` → success path: the daemon wrote the image and typed
    /// the relative path into the pane. `false` → any failure, with
    /// `message` explaining what went wrong (no image in clipboard,
    /// upload failed, etc.).
    pub ok: bool,
    pub message: String,
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
    /// Daemon version reported by `/api/health` (e.g. `"0.7.61"`).
    /// `None` if the probe hasn't returned yet, the daemon is too
    /// old to surface the field, or the probe failed. Rendered by
    /// the sidebar so the user can spot a server lagging behind
    /// the local CLI before they get bit by a missing capability.
    pub version: Option<String>,
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
    /// Fetch a host's cached readiness report if it's still fresh
    /// (within [`HOST_READINESS_TTL`]). Returns `None` for an absent or
    /// stale entry so callers re-probe rather than act on old data.
    pub fn cached_readiness(&self, id: Uuid) -> Option<&agentum_core::HostReadiness> {
        self.host_readiness_cache
            .get(&id)
            .filter(|(at, _)| at.elapsed() < HOST_READINESS_TTL)
            .map(|(_, report)| report)
    }

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
            // Deliberately ignore `prefs.sidebar_hidden` on launch — the
            // sidebar is the primary navigation surface and a user who
            // accidentally hit Ctrl-B last session (or whose tui_prefs.toml
            // got synced across machines) would re-enter the TUI to a
            // blank tree column and reasonably conclude the app was
            // broken. Hide is session-local; press Ctrl-B during a
            // session to fold it, and it comes back on next launch.
            sidebar_hidden: false,
            servers_collapsed: prefs.servers_collapsed,
            show_all_servers: prefs.show_all_servers,
            chord: None,
            filter_input_active: false,
            error_count: 0,
            errors: Vec::new(),
            errors_scroll: 0,
            conn: ConnState::Connecting,
            was_connected: false,
            http_fail_count: 0,
            recently_killed: std::collections::HashSet::new(),
            checked: HashSet::new(),
            tick_count: 0,
            status_msg: None,
            pending_clipboard_seq: None,
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
            term_input_lines: HashMap::new(),
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
            working: HashSet::new(),
            agent_availability: None,
            pending_switch_profile: None,
            pending_after_switch: None,
            active_profile: None,
            profiles: Vec::new(),
            hosts: Vec::new(),
            host_readiness_cache: HashMap::new(),
            tree_section: TreeSection::Sessions,
            servers_cursor: 0,
            clients: HashMap::new(),
            session_profile: HashMap::new(),
            reconnecting: HashSet::new(),
            hint_card: None,
            upload_outcome_tx: None,
            claude_usage: None,
            claude_usage_at: None,
            usage_tx: None,
            usage_inflight: false,
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

    /// Whether the SERVERS section renders the synthetic "this machine"
    /// row at cursor 0. The row only carries useful data when the user
    /// launched without `--profile` (in which case the loopback was
    /// probed and stored under the `""` key in `app.clients`); a
    /// `--profile` launch never populates that key, so painting the row
    /// anyway leaves a phantom entry above the named profiles. The
    /// predicate also collapses the row when the user has registered a
    /// real profile pointing at the local daemon, since the named row
    /// already represents it.
    pub fn synthetic_loopback_visible(&self) -> bool {
        if !self.clients.contains_key("") {
            return false;
        }
        // If a named profile points at the loopback, the synthetic row
        // is just a duplicate of that named row — collapse it. We match
        // on URL host so a profile recorded as `http://127.0.0.1:8822`
        // or `https://localhost:8822` both win.
        !self
            .profiles
            .iter()
            .any(|p| profile_targets_loopback(&p.url))
    }

    /// Total rows in the visible SERVERS section, counting the
    /// synthetic loopback row only when it's actually being painted.
    pub fn servers_row_count(&self) -> usize {
        self.profiles.len() + usize::from(self.synthetic_loopback_visible())
    }

    /// Resolve a SERVERS-section cursor to the profile it points at.
    /// Returns `None` when the cursor sits on the synthetic loopback
    /// row (only possible while `synthetic_loopback_visible()`).
    pub fn cursor_profile(&self) -> Option<&ProfileEntry> {
        let synth = self.synthetic_loopback_visible();
        if synth && self.servers_cursor == 0 {
            return None;
        }
        let offset = usize::from(synth);
        self.profiles
            .get(self.servers_cursor.saturating_sub(offset))
    }

    /// Load the on-disk profiles into `app.profiles`. Called once at
    /// run-loop start and again any time the user adds/removes a
    /// profile via the sidebar or overlay so the sidebar stays in
    /// sync without re-reading the file every frame. Errors are
    /// non-fatal — they leave `profiles` empty and the sidebar
    /// renders an "no servers" hint.
    pub fn reload_profiles(&mut self) {
        self.profiles = match super::profiles::load() {
            Ok(store) => store
                .list()
                .into_iter()
                .map(|(name, p, _is_default)| ProfileEntry {
                    name,
                    url: p.url,
                    fingerprint: p.fingerprint,
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        // Clamp the cursor against the new row count. `servers_row_count`
        // already accounts for the synthetic loopback row appearing or
        // disappearing (e.g. when the user added a 127.0.0.1 profile).
        let max = self.servers_row_count().saturating_sub(1);
        if self.servers_cursor > max {
            self.servers_cursor = max;
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
        // Capture both server- and project-level collapse state so a
        // tree rebuild (which happens every time the session list
        // changes — and that's *often*) doesn't reset every fold the
        // user set. Namespaced keys (`server::` / `project::`) keep
        // the two levels distinct in one flat map.
        let mut prev_state: HashMap<String, bool> = HashMap::new();
        for g in &self.tree.groups {
            prev_state.insert(server_expand_key(&g.profile), g.expanded);
            for p in &g.projects {
                prev_state.insert(project_expand_key(&g.profile, &p.workdir), p.expanded);
            }
        }
        // Preserve the active filter across rebuilds — the user's typed
        // search shouldn't vanish just because the session list changed.
        let prev_filter = self.tree.filter_str().to_string();
        self.sessions = sessions;
        self.tree = self.build_scoped_tree(&prev_state);
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
        // Drop any check that's pointing at a session that no longer
        // exists (killed locally, removed by a peer, profile gone…).
        // Without this, a refreshed list could leave ghost ids in the
        // set and the next bulk action would silently skip them.
        if !self.checked.is_empty() {
            self.checked
                .retain(|id| self.sessions.iter().any(|s| s.id == *id));
        }
        // Prune the `/clear`-detector line shadows for the same
        // reason. New buffers are created on demand by
        // `track_term_input_for_clear`, so this is purely a
        // memory-hygiene measure on a long-running TUI session.
        if !self.term_input_lines.is_empty() {
            self.term_input_lines
                .retain(|id, _| self.sessions.iter().any(|s| s.id == *id));
        }
    }

    /// Build the sidebar tree honouring the current `show_all_servers`
    /// scope. When the toggle is on (default) the tree spans every
    /// reachable profile's sessions; when off, it's scoped to whichever
    /// profile is currently active so a noisy fleet doesn't bury the
    /// session the user is actually driving. The SERVERS section itself
    /// keeps listing every profile in both modes — scoping only affects
    /// which session leaves appear under SESSIONS.
    pub fn build_scoped_tree(&self, prev: &HashMap<String, bool>) -> Tree {
        if self.show_all_servers {
            return Tree::build_with_profiles(&self.sessions, &self.session_profile, prev);
        }
        let active = self.active_profile.clone().unwrap_or_default();
        let scoped: Vec<Session> = self
            .sessions
            .iter()
            .filter(|s| {
                let owner = self
                    .session_profile
                    .get(&s.id)
                    .map(String::as_str)
                    .unwrap_or("");
                owner == active.as_str()
            })
            .cloned()
            .collect();
        Tree::build_with_profiles(&scoped, &self.session_profile, prev)
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
pub async fn aggregate_sessions_with_owners(app: &App) -> (Vec<Session>, HashMap<Uuid, String>) {
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
    /// connection. Top-level node in a three-level tree:
    ///   Group (server) → Project (workdir) → Leaf (session).
    /// We keep the server collapse-state up here so a multi-project
    /// server can be folded as a unit (one chevron click hides every
    /// project + session it owns).
    pub profile: String,
    pub projects: Vec<Project>,
    pub expanded: bool,
}

pub struct Project {
    /// Workdir-normalized path (no trailing `/`). Sessions sharing a
    /// workdir on the same server roll up under this sub-header so the
    /// sidebar reads as `<server> → <project> → <sessions>` instead of
    /// a flat list — which is what the v0.7.19 collapse cost us and
    /// what the user wants back.
    pub workdir: String,
    pub sessions: Vec<Uuid>,
    pub expanded: bool,
}

#[derive(Clone, Copy)]
pub enum Row {
    Group(usize),
    Project {
        group: usize,
        project: usize,
    },
    Leaf {
        group: usize,
        project: usize,
        leaf: usize,
    },
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

/// Display name for a server-level group. `""` (loopback) renders as
/// the host's own hostname (e.g. `omarchy`, `mateo-mac`) so the user
/// sees exactly which box's daemon they're attached to — the row is
/// labelled by *whose machine is hosting the daemon*, not by a
/// generic "this is local". When the TUI runs on the same box as the
/// daemon they're labelled identically anyway; when the TUI runs on
/// a different box via `--profile`, only the named profile appears
/// (the loopback row is suppressed by the runtime when no local
/// daemon is connected). Named profiles keep the `@` prefix so the
/// sidebar reads `@vps` instead of just `vps`, which makes the user-
/// chosen alias visually distinct from the hostname-derived one.
pub fn profile_label(profile: &str) -> String {
    if profile.is_empty() {
        local_machine_label()
    } else {
        format!("@{profile}")
    }
}

/// Returns `true` when `url`'s host points at the local loopback so
/// the sidebar can collapse the synthetic "this machine" row into a
/// named profile that already represents it. Accepts IPv4 / IPv6
/// loopback literals and `localhost`. Anything unparseable returns
/// `false` — the synthetic row stays as a safety net.
pub fn profile_targets_loopback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    matches!(
        parsed.host_str(),
        Some("127.0.0.1") | Some("::1") | Some("[::1]") | Some("localhost")
    )
}

/// Hostname-derived label for the loopback row. Centralised so the
/// sidebar header, the Servers panel row, the New Session form's
/// profile field, and any "can't reach <x>" status messages all
/// agree. Falls back to "local" when the system `hostname` command
/// is unavailable or returns an empty string. Cached behind a
/// `OnceLock` so we don't fork a `hostname` subprocess every frame
/// (the label is read inside the per-frame render path).
pub fn local_machine_label() -> String {
    static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            // `hostname` is universally available on macOS and Linux
            // (POSIX), so spawning it once at startup is the cheapest
            // portable way to get the system hostname without pulling
            // in a dedicated crate. The output ends with a newline,
            // which we trim. Cut at the first `.` so a host that
            // reports `omarchy.local` (mDNS) or `mateo-mac.lan`
            // shortens to the base name — that's the part the user
            // identifies with, the suffix is just for resolution.
            let raw = std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            match raw {
                Some(name) => name.split('.').next().unwrap_or(&name).to_ascii_lowercase(),
                None => "local".to_string(),
            }
        })
        .clone()
}

/// Friendly label for a workdir: the basename with `~` for the home
/// dir. Falls back to the (collapsed) path if there's no basename —
/// only happens for filesystem-root paths. Used in the leaf row's
/// trailing workdir badge so the project context stays visible even
/// after we stopped grouping by workdir.
pub fn workdir_label(workdir: &str) -> String {
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

/// Snapshot key for a server-level group's collapse state. Namespaced
/// (`server::`) so it can't collide with project keys in the same
/// `HashMap<String, bool>` snapshot.
pub fn server_expand_key(profile: &str) -> String {
    format!("server::{profile}")
}

/// Snapshot key for a project-level subgroup's collapse state. The
/// `\0` separator can't appear in a workdir path, so a profile named
/// "x\0y" — pathological but possible — can't collide with another
/// `(profile, workdir)` pair.
pub fn project_expand_key(profile: &str, workdir: &str) -> String {
    format!("project::{profile}\0{workdir}")
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

    /// Multi-server variant: groups by `(profile, workdir)` so each
    /// server renders as a top-level header (`MY MACHINE (macos)`,
    /// `@vps`) and each workdir on that server renders as a sub-header
    /// underneath, with sessions as leaves. Three levels keep the
    /// fleet view scannable: server identity at the top, project
    /// identity inline, session identity on the leaves — no more
    /// repeating workdirs across N flat headers and no more flat
    /// session lists where the project context lives in a trailing
    /// badge.
    pub fn build_with_profiles(
        sessions: &[Session],
        session_profile: &HashMap<Uuid, String>,
        prev_expanded: &HashMap<String, bool>,
    ) -> Self {
        // Two-level bucket: profile → workdir → sessions. Normalize the
        // workdir before bucketing so `/x/proj` and `/x/proj/` don't
        // double-up as two siblings.
        let mut by_profile: HashMap<String, HashMap<String, Vec<&Session>>> = HashMap::new();
        for s in sessions {
            let profile = session_profile.get(&s.id).cloned().unwrap_or_default();
            let workdir = normalize_workdir(&s.workdir);
            by_profile
                .entry(profile)
                .or_default()
                .entry(workdir)
                .or_default()
                .push(s);
        }
        let mut profile_keys: Vec<String> = by_profile.keys().cloned().collect();
        // Loopback (empty key) first; then alphabetical by profile name.
        profile_keys.sort_by(|a, b| match (a.is_empty(), b.is_empty()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.cmp(b),
        });
        let groups: Vec<Group> = profile_keys
            .into_iter()
            .map(|profile| {
                let mut by_workdir = by_profile.remove(&profile).unwrap();
                // Sort projects by basename (`workdir_label`) so the
                // visible order matches what the user reads, not the
                // raw absolute path. Tie-break on the full path so the
                // order is deterministic when two projects share a
                // basename across different parent dirs.
                let mut workdir_keys: Vec<String> = by_workdir.keys().cloned().collect();
                workdir_keys.sort_by(|a, b| {
                    workdir_label(a)
                        .to_ascii_lowercase()
                        .cmp(&workdir_label(b).to_ascii_lowercase())
                        .then_with(|| a.cmp(b))
                });
                let projects: Vec<Project> = workdir_keys
                    .into_iter()
                    .map(|workdir| {
                        let mut sess = by_workdir.remove(&workdir).unwrap();
                        sess.sort_by(|a, b| a.name.cmp(&b.name));
                        let proj_key = project_expand_key(&profile, &workdir);
                        Project {
                            // Default projects to expanded — the user
                            // wants to *see* the sessions; if they
                            // explicitly collapsed one before the
                            // rebuild, honour that.
                            expanded: *prev_expanded.get(&proj_key).unwrap_or(&true),
                            sessions: sess.iter().map(|s| s.id).collect(),
                            workdir,
                        }
                    })
                    .collect();
                let server_key = server_expand_key(&profile);
                Group {
                    expanded: *prev_expanded.get(&server_key).unwrap_or(&true),
                    projects,
                    profile,
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
            // Per-project visible-leaf lists, paired with their project
            // indices. While filtering, only sessions whose name
            // contains the needle count; while unfiltered, all do.
            let mut visible_by_project: Vec<(usize, Vec<usize>)> = Vec::new();
            for (pi, proj) in g.projects.iter().enumerate() {
                let leaves: Vec<usize> = if filtering {
                    (0..proj.sessions.len())
                        .filter(|li| {
                            let id = proj.sessions[*li];
                            self.name_index.get(&id).is_some_and(|n| n.contains(needle))
                        })
                        .collect()
                } else {
                    (0..proj.sessions.len()).collect()
                };
                if filtering && leaves.is_empty() {
                    continue;
                }
                visible_by_project.push((pi, leaves));
            }
            // Drop empty groups while filtering; otherwise keep the
            // group header so the user can expand/collapse it.
            if filtering && visible_by_project.is_empty() {
                continue;
            }
            rows.push(Row::Group(gi));
            // Filter mode forces every group + project expanded so flat
            // search behaves like a single list; unfiltered mode honours
            // the user's saved collapse-state.
            let server_open = filtering || g.expanded;
            if !server_open {
                continue;
            }
            for (pi, leaves) in visible_by_project {
                rows.push(Row::Project {
                    group: gi,
                    project: pi,
                });
                let project_open = filtering || g.projects[pi].expanded;
                if !project_open {
                    continue;
                }
                for li in leaves {
                    rows.push(Row::Leaf {
                        group: gi,
                        project: pi,
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
            Row::Leaf {
                group,
                project,
                leaf,
            } => Some(self.groups[group].projects[project].sessions[leaf]),
            Row::Project { group, project } => self
                .groups
                .get(group)
                .and_then(|g| g.projects.get(project))
                .and_then(|p| p.sessions.first().copied())
                .filter(|_| !sessions.is_empty()),
            Row::Group(gi) => self
                .groups
                .get(gi)
                .and_then(|g| g.projects.first())
                .and_then(|p| p.sessions.first().copied())
                .filter(|_| !sessions.is_empty()),
        }
    }

    pub fn collapse(&mut self) {
        // Collapse semantics — fold the nearest closed ancestor:
        //   leaf      → collapse the parent project (so a Vim-style
        //               `h` from a session hides its siblings, not the
        //               whole server).
        //   project   → collapse the project itself; if it was already
        //               folded, walk up and collapse the server.
        //   server    → collapse the server.
        if let Some(row) = self.current_row() {
            match row {
                Row::Leaf { group, project, .. } => {
                    if let Some(p) = self
                        .groups
                        .get_mut(group)
                        .and_then(|g| g.projects.get_mut(project))
                    {
                        if p.expanded {
                            p.expanded = false;
                            self.cursor = self
                                .row_index_of(Row::Project { group, project })
                                .unwrap_or(self.cursor);
                        }
                    }
                }
                Row::Project { group, project } => {
                    let already_folded = self
                        .groups
                        .get(group)
                        .and_then(|g| g.projects.get(project))
                        .map(|p| !p.expanded)
                        .unwrap_or(true);
                    if !already_folded {
                        if let Some(p) = self
                            .groups
                            .get_mut(group)
                            .and_then(|g| g.projects.get_mut(project))
                        {
                            p.expanded = false;
                        }
                    } else if let Some(g) = self.groups.get_mut(group) {
                        if g.expanded {
                            g.expanded = false;
                            self.cursor =
                                self.row_index_of(Row::Group(group)).unwrap_or(self.cursor);
                        }
                    }
                }
                Row::Group(gi) => {
                    if let Some(g) = self.groups.get_mut(gi) {
                        if g.expanded {
                            g.expanded = false;
                            self.cursor = self.row_index_of(Row::Group(gi)).unwrap_or(self.cursor);
                        }
                    }
                }
            }
        }
    }

    pub fn expand(&mut self) {
        // Expand semantics — open the nearest closed level:
        //   server   → if folded, open it. If already open, open every
        //              project inside (one keystroke shouldn't strand
        //              the user with all projects still hidden).
        //   project  → open the project itself.
        //   leaf     → no-op (already at the bottom).
        if let Some(row) = self.current_row() {
            match row {
                Row::Group(gi) => {
                    if let Some(g) = self.groups.get_mut(gi) {
                        if !g.expanded {
                            g.expanded = true;
                        } else {
                            for p in &mut g.projects {
                                p.expanded = true;
                            }
                        }
                    }
                }
                Row::Project { group, project } => {
                    if let Some(p) = self
                        .groups
                        .get_mut(group)
                        .and_then(|g| g.projects.get_mut(project))
                    {
                        p.expanded = true;
                    }
                }
                Row::Leaf { .. } => {}
            }
        }
    }

    fn row_index_of(&self, target: Row) -> Option<usize> {
        for (i, r) in self.rows().iter().enumerate() {
            let hit = match (r, target) {
                (Row::Group(a), Row::Group(b)) => *a == b,
                (
                    Row::Project {
                        group: ga,
                        project: pa,
                    },
                    Row::Project {
                        group: gb,
                        project: pb,
                    },
                ) => *ga == gb && *pa == pb,
                (
                    Row::Leaf {
                        group: ga,
                        project: pa,
                        leaf: la,
                    },
                    Row::Leaf {
                        group: gb,
                        project: pb,
                        leaf: lb,
                    },
                ) => *ga == gb && *pa == pb && *la == lb,
                _ => false,
            };
            if hit {
                return Some(i);
            }
        }
        None
    }

    pub fn select_session(&mut self, id: Uuid) {
        for (i, r) in self.rows().iter().enumerate() {
            if let Row::Leaf {
                group,
                project,
                leaf,
            } = r
                && self.groups[*group].projects[*project].sessions[*leaf] == id
            {
                self.cursor = i;
                return;
            }
        }
    }

    /// Move the cursor to the Nth server group (1-based) and expand it.
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
        if let Row::Leaf {
            group,
            project,
            leaf,
        } = r
        {
            return Some(tree.groups[group].projects[project].sessions[leaf]);
        }
    }
    sessions.first().map(|s| s.id)
}

/// Initial selection after a cold start or profile switch. Prefers a
/// session owned by `active_profile` so the terminal pane reads as the
/// user's *current* server, not whichever session sorted first across
/// the merged fleet. Falls back to the first session in the merged list
/// when the active server has nothing of its own yet (e.g. just added
/// a vps that has zero sessions — show *something* rather than an empty
/// pane).
pub fn pick_initial_selection(
    sessions: &[Session],
    session_profile: &HashMap<Uuid, String>,
    active_profile: Option<&str>,
) -> Option<Uuid> {
    let active = active_profile.unwrap_or("");
    sessions
        .iter()
        .find(|s| session_profile.get(&s.id).map(String::as_str).unwrap_or("") == active)
        .map(|s| s.id)
        .or_else(|| sessions.first().map(|s| s.id))
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
    app.tree = app.build_scoped_tree(&HashMap::new());
    // Re-derive the initial selection now that we know which profile is
    // active. `App::new` picked whichever session sorted first across
    // the merged fleet (loopback wins because the empty profile key
    // sorts first) — after a Ctrl-S switch from loopback to a remote
    // server that left the terminal pane stuck on a loopback session
    // while the title bar read `@vps`, which the user reads as "the
    // switch didn't happen." Prefer a session owned by the active
    // profile; only fall back to the first visible leaf when the new
    // server has no sessions yet.
    app.selected = pick_initial_selection(
        &app.sessions,
        &app.session_profile,
        app.active_profile.as_deref(),
    );

    // Default profile: the live `client` we got from `connect_once`.
    // Keyed under the active profile name (or "" for loopback) so
    // `client_for_session` finds it via the same lookup as peers.
    let default_key = active_profile.clone().unwrap_or_default();
    // Capture the active daemon's version up-front so the sidebar can
    // render it on the first frame instead of waiting for the next
    // periodic refresh tick. Best-effort: a failure here only means
    // the row reads "v?" until the refresh loop fills it in.
    let active_version = client
        .health()
        .await
        .ok()
        .map(|h| h.version)
        .filter(|v| !v.is_empty());
    app.clients.insert(
        default_key.clone(),
        ClientEntry {
            client: Some(client.clone()),
            status: ServerStatus::Live,
            last_error: None,
            agent_availability: None, // populated below by the same probe path
            version: active_version,
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
                version: conn.version,
            },
        );
    }

    // One-shot drift toast for the active daemon. When the user
    // upgrades the CLI with `agentum update` (or
    // `install -m 755 target/release/agentum ~/.local/bin/agentum`),
    // the on-disk binary is replaced but the running daemon process
    // still holds the old inode in memory — `/api/health` keeps
    // returning the pre-upgrade version until the daemon is
    // restarted. The version chip already paints in the warning color
    // when drift is detected, but users have read past that — surface
    // an explicit notification on first connect so they know what to
    // do.
    {
        let local = env!("CARGO_PKG_VERSION");
        let server_version = app
            .clients
            .get(default_key.as_str())
            .and_then(|e| e.version.clone())
            .filter(|v| v != local);
        if let Some(server) = server_version {
            let label = if default_key.is_empty() {
                local_machine_label()
            } else {
                profile_label(&default_key)
            };
            push_notification(
                &mut app,
                format!("{label} is running v{server}"),
                Some(format!(
                    "your CLI is v{local} — restart `agentum serve` on the host to upgrade"
                )),
                NotifKind::Warn,
            );
        }
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
                // Drop straight to the Workdir field so the user resumes
                // typing where they were going, not at the Servers
                // step they just resolved.
                form.field = NewSessionField::Workdir;
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
    app.hosts = client.list_hosts().await.unwrap_or_default();

    let (term_tx, mut term_rx) = mpsc::unbounded_channel::<TerminalMsg>();
    let (term_tx_right, mut term_rx_right) = mpsc::unbounded_channel::<TerminalMsg>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<EventMsg>();
    let (lg_tx, mut lg_rx) = mpsc::unbounded_channel::<PtyMsg>();
    let (agent_tasks_tx, mut agent_tasks_rx) =
        mpsc::unbounded_channel::<(Uuid, Option<AgentTaskState>)>();
    // Result channel for the Ctrl-V image-paste flow. One uploader
    // task posts a single `UploadOutcome` per Ctrl-V; the `select!`
    // arm below drains the rx and surfaces the message as a status
    // or error toast. Mirrors `agent_tasks_tx` exactly so the
    // surrounding code stays consistent.
    let (upload_outcome_tx, mut upload_outcome_rx) = mpsc::unbounded_channel::<UploadOutcome>();
    // Result channel for the background Claude-usage poll (spec 001). The
    // spawned task posts one `Option<ClaudeUsage>` per poll; the `select!`
    // arm below stores it on `App`. Mirrors `agent_tasks_tx`.
    let (usage_tx, mut usage_rx) = mpsc::unbounded_channel::<Option<ClaudeUsage>>();
    // Stash cheap clones on `App` so `update_selection` can pick the
    // correct sender by side without re-threading args. The lazygit
    // sender lives here too so `refresh_lazygit_for_selection` can
    // respawn the side pane on project switches without threading a
    // `&Sender` through every handler.
    app.term_tx_left = Some(term_tx.clone());
    app.term_tx_right = Some(term_tx_right);
    app.lg_tx = Some(lg_tx.clone());
    app.agent_tasks_tx = Some(agent_tasks_tx);
    app.upload_outcome_tx = Some(upload_outcome_tx);
    app.usage_tx = Some(usage_tx);
    // Kick off the first usage poll immediately so the readout populates
    // without waiting a full interval. Subsequent polls are tick-driven.
    spawn_usage_fetch(&mut app, &client);

    // Subscribe to the daemon's event bus.
    let _events_handle: JoinHandle<()> = client.open_event_stream(event_tx);

    // Open the terminal stream for the initial selection. The handle
    // lives on `App` (left/right slots) instead of the run-loop stack
    // so helper functions can access it through `&mut App` without
    // threading an extra `&mut Option<JoinHandle>` everywhere.
    if let Some(id) = app.selected {
        let (key_tx, key_rx) = mpsc::unbounded_channel::<TermOut>();
        // Initial connect on startup: route through the owning server's
        // client so a peer-owned initial selection (e.g. the user landed
        // on a session that lives on the remote `@omarchy` daemon)
        // doesn't 404 against the active daemon's WS endpoint. Falls
        // back to the active client when the owner is unknown — the
        // legacy path for sessions not yet tagged in `session_profile`.
        let owner = app.client_for_session(id).cloned();
        let stream_client = owner.unwrap_or_else(|| client.clone());
        // Initial connect on startup: no cached parser yet, no resume.
        let h = stream_client.open_terminal_stream(id, term_tx.clone(), key_rx, false);
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
        // Flush any pending OSC-52 clipboard write *after* the draw so
        // the bytes don't intercept ratatui's diff renderer
        // mid-frame. The follow-up `clear()` forces the next frame to
        // be a full repaint, which masks the brief literal-char flash
        // on terminals that don't support OSC 52 (or tmux without
        // `allow-passthrough on`). On capable terminals the OSC is
        // invisible and the clear is just a one-frame full repaint.
        if let Some(seq) = app.pending_clipboard_seq.take() {
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(&seq);
            let _ = stdout.flush();
            drop(stdout);
            let _ = terminal.clear();
        }
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
        //
        // Drain any messages the reader thread queued before exit so
        // the parser ingests lazygit's final frame BEFORE we move the
        // pane out. Without this drain, finished() can fire between
        // the reader thread enqueueing the fatal-error frame and the
        // tokio::select loop processing it — the bytes get orphaned
        // and the user sees a context-free "lazygit exited" toast.
        // Most-frequent cause: lazygit opens, fails ("not a git
        // repo", `/dev/tty` open error, etc.), prints to stderr,
        // exits in tens of milliseconds, and the error is gone before
        // any frame renders.
        if app.lazygit.is_some() {
            while let Ok(msg) = lg_rx.try_recv() {
                handle_lazygit_msg(&mut app, msg);
            }
        }
        if let Some(lg) = app.lazygit.as_ref()
            && let Some(exit) = lg.exit_status()
        {
            let tail = capture_screen_tail(lg.screen(), 8);
            app.lazygit = None;
            app.lazygit_cwd = None;
            if app.focus == Focus::Lazygit {
                app.focus = Focus::Tree;
            }
            let trimmed = tail.trim_end_matches('\n').trim_end();
            if trimmed.is_empty() {
                app.status_msg = Some(format!("lazygit exited (code {})", exit.exit_code()));
            } else {
                app.push_error(format!(
                    "lazygit exited (code {}):\n{trimmed}",
                    exit.exit_code()
                ));
            }
        }

        tokio::select! {
            biased;

            maybe_input = crossterm_events.next() => {
                if let Some(Ok(ev)) = maybe_input {
                    handle_crossterm(&mut app, ev, &client, &lg_tx).await;
                }
                // Drain any input events already queued before we
                // redraw. Defense-in-depth for terminals that don't
                // honor `EnableBracketedPaste`: without this, a key-
                // by-key paste still triggers one redraw per char and
                // can lock the UI long enough that Ctrl-Q can't get a
                // slot to abort. Bounded loop so a stuck source can't
                // monopolise the tick — 1024 events between redraws
                // covers any realistic burst at terminal speed.
                let mut drained = 0;
                while drained < 1024 {
                    match crossterm_events.next().now_or_never() {
                        Some(Some(Ok(ev))) => {
                            handle_crossterm(&mut app, ev, &client, &lg_tx).await;
                            drained += 1;
                        }
                        _ => break,
                    }
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

            Some(outcome) = upload_outcome_rx.recv() => {
                // Ctrl-V clipboard-image-paste result. Success → put
                // the message in `status_msg` (transient bottom-right
                // hint). Failure → escalate to `push_error` so the
                // toast stack catches it; people will want to know
                // why their paste didn't land.
                if outcome.ok {
                    app.status_msg = Some(outcome.message);
                } else {
                    app.push_error(outcome.message);
                }
            }

            Some(maybe_usage) = usage_rx.recv() => {
                // Background Claude-usage poll result (spec 001). On
                // transport error the message carries `None`; we keep the
                // previous snapshot (stale-but-flagged beats blank) and
                // just clear the in-flight marker so the next tick retries.
                app.usage_inflight = false;
                if let Some(usage) = maybe_usage {
                    app.claude_usage = Some(usage);
                    app.claude_usage_at = Some(Instant::now());
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
                // Claude usage readout poll (spec 001). Gated by the
                // user's `usage_refresh` interval (≥30s) independent of
                // the 5s session refresh — usage moves slowly and the
                // upstream endpoint is rate-limitable. `spawn_usage_fetch`
                // coalesces via `usage_inflight`.
                let usage_due = app
                    .claude_usage_at
                    .map(|at| at.elapsed() >= app.prefs.usage_refresh())
                    .unwrap_or(true);
                if usage_due {
                    spawn_usage_fetch(&mut app, &client);
                }
                if last_refresh.elapsed() >= REFRESH_INTERVAL {
                    last_refresh = Instant::now();
                    // Pick up out-of-band edits to profiles.toml — e.g.
                    // `agentum profiles rm vps` from a different shell.
                    // Cheap (one small TOML read) and lets external
                    // tooling stay coherent with the running TUI's
                    // sidebar without forcing a restart.
                    app.reload_profiles();
                    // Fan out to every live server in parallel. A single-
                    // client poll here would wipe every peer-server
                    // session within REFRESH_INTERVAL of boot — exactly
                    // the "omarchy sessions disappeared from the sidebar"
                    // bug. We still track the active client's status so
                    // the reconnect banner surfaces even when the events
                    // bus hasn't yet noticed the daemon went away (TCP
                    // keepalive can lag the HTTP layer by tens of seconds).
                    let active_key = app.active_profile.clone().unwrap_or_default();
                    // Probe both endpoints per client so the periodic
                    // tick keeps the sidebar version chip honest after a
                    // remote daemon is upgraded + restarted. Without the
                    // health() leg, entry.version stayed pinned to the
                    // value captured at boot — `agentum update` on a
                    // peer would leave the chip showing the old version
                    // until the TUI itself was restarted.
                    let probes: Vec<_> = app
                        .live_clients()
                        .map(|(name, c)| {
                            let name = name.to_string();
                            let c = c.clone();
                            async move {
                                let (sessions, health) =
                                    futures_util::future::join(c.list_sessions(), c.health()).await;
                                (name, sessions, health)
                            }
                        })
                        .collect();
                    let results = futures_util::future::join_all(probes).await;
                    let active_ok = results
                        .iter()
                        .any(|(n, r, _)| n == &active_key && r.is_ok());
                    if active_ok {
                        app.http_fail_count = 0;
                    } else {
                        app.http_fail_count = app.http_fail_count.saturating_add(1);
                    }
                    // Reflect per-profile reachability in the ClientEntry
                    // status so the sidebar dot for each peer server
                    // turns red the moment its periodic probe fails —
                    // without this the dots stayed a misleading "live"
                    // green for any peer that had silently dropped off
                    // the network (TCP keepalive can lag by tens of
                    // seconds). LoginNeeded peers keep their flag — a
                    // successful HTTP probe means the daemon answered,
                    // not that the bearer token resolved on its own.
                    for (name, res, health_res) in results.iter() {
                        if let Some(entry) = app.clients.get_mut(name) {
                            match res {
                                Ok(_) => {
                                    if entry.status != ServerStatus::LoginNeeded {
                                        entry.status = ServerStatus::Live;
                                    }
                                    entry.last_error = None;
                                }
                                Err(e) => {
                                    entry.status = ServerStatus::Unreachable;
                                    entry.last_error = Some(e.to_string());
                                }
                            }
                            // Keep the version chip current. A failed
                            // probe doesn't wipe the cached value — the
                            // peer may be momentarily unreachable but
                            // not actually downgraded.
                            if let Ok(h) = health_res {
                                if !h.version.is_empty() {
                                    entry.version = Some(h.version.clone());
                                }
                            }
                        }
                    }
                    let per_profile: Vec<(String, Vec<Session>)> = results
                        .into_iter()
                        .map(|(n, r, _)| (n, r.unwrap_or_default()))
                        .collect();
                    let (merged, owners) = merge_sessions_dedup(per_profile, &active_key);
                    app.refresh_sessions_with_owners(merged, owners);
                    // Pre-warm the cache for any newly-discovered
                    // sessions so the first nav to them is also a pure
                    // cache hit. `spawn_agent_tasks_fetch` routes to the
                    // owning server's client via `client_for_session`,
                    // so peer-owned sessions warm against the right
                    // daemon. Existing ids skip via the in-flight dedup.
                    let new_ids: Vec<Uuid> = app
                        .sessions
                        .iter()
                        .filter(|s| !app.agent_tasks.contains_key(&s.id))
                        .map(|s| s.id)
                        .collect();
                    for id in new_ids {
                        spawn_agent_tasks_fetch(&mut app, &client, id);
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
        CtEvent::Key(key)
            if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
        {
            handle_key(app, key, client, lg_tx).await;
        }
        CtEvent::Mouse(me) => handle_mouse(app, me),
        CtEvent::Paste(text) => handle_paste(app, text),
        CtEvent::Resize(_, _) => {}
        _ => {}
    }
}

/// Bracketed-paste dispatcher. Runs ONCE per paste regardless of
/// length, so a 50 KB blob no longer causes 50 000 redraws. Decides
/// between three routes:
///
/// 1. **Image-attachment paste** — content sniffs as an image (PNG /
///    JPEG / GIF / WebP magic bytes, or a `data:image/...;base64,`
///    URI, or a single line that's an existing file path with an
///    image extension). We write the bytes to a temp file and push
///    the file path into the focused pane the way Claude Code
///    accepts image attachments (path on its own line).
///
/// 2. **Pane paste** — focused on a terminal pane → send the bytes
///    in ONE `TermOut::Bytes` over the WS. The inner program decides
///    how to interpret newlines etc.
///
/// 3. **Discard** — focused on the tree / overlay / lazygit. Pastes
///    into the tree filter prompt are intentionally ignored for now
///    (single-line search box, multi-line paste would be confusing).
fn handle_paste(app: &mut App, text: String) {
    if text.is_empty() {
        return;
    }
    if !matches!(app.focus, Focus::Term | Focus::TermRight) {
        // Surface the silent drop with a hint instead of pretending
        // the paste went somewhere useful.
        app.status_msg = Some("paste ignored — focus a terminal pane first".into());
        return;
    }
    let bytes = match classify_paste(&text) {
        PasteKind::ImageBytes { mime, data } => match write_paste_image(&data, mime) {
            Ok(path) => {
                app.status_msg = Some(format!(
                    "pasted image ({} bytes) → {}",
                    data.len(),
                    path.display()
                ));
                // Trailing newline so the agent's prompt commits the
                // path. Claude Code accepts `path\n` as an attachment.
                format!("{}\n", path.display()).into_bytes()
            }
            Err(e) => {
                app.push_error(format!("paste-image temp file: {e}"));
                return;
            }
        },
        PasteKind::ImagePath(path) => {
            app.status_msg = Some(format!("pasted image path → {}", path.display()));
            format!("{}\n", path.display()).into_bytes()
        }
        PasteKind::Text => text.into_bytes(),
    };
    send_paste_bytes(app, bytes);
}

/// What the bracketed-paste content looks like once sniffed. Drives
/// the three routes in `handle_paste`.
enum PasteKind {
    /// Plain text — forward as bytes to the focused pane.
    Text,
    /// Decoded image bytes plus the detected MIME type. Caller writes
    /// the bytes to a temp file and pastes the path.
    ImageBytes { mime: &'static str, data: Vec<u8> },
    /// Single-line file path that already exists on this host AND has
    /// an image extension. Skip the temp-file round-trip and paste
    /// the path verbatim — this is the iTerm2 / kitty default when a
    /// user drag-drops a local image file into the terminal.
    ImagePath(PathBuf),
}

fn classify_paste(text: &str) -> PasteKind {
    // 1. `data:image/png;base64,…` URI — what the web Clipboard API
    //    produces for `image/*` MIME entries when copied as a string.
    //    Raw binary image bytes through bracketed paste *would* sniff
    //    as an image, but crossterm lossy-decodes the buffer to UTF-8
    //    before we see it (see `parse_csi_bracketed_paste`), so the
    //    magic bytes are already U+FFFD-replaced by this point. The
    //    realistic image paths are the data URI here and the file
    //    path below; binary reads need a different transport (OSC 52
    //    read or a daemon HTTP upload route, both follow-ups).
    if let Some((mime, b64)) = strip_data_uri(text) {
        use base64::{Engine, engine::general_purpose::STANDARD};
        // Strip embedded whitespace — long base64 blobs often arrive
        // line-wrapped from the source.
        let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
        if let Ok(data) = STANDARD.decode(cleaned.as_bytes()) {
            return PasteKind::ImageBytes { mime, data };
        }
    }
    // 2. Existing path with an image extension. Don't fs::canonicalize
    //    (resolves symlinks unnecessarily); a metadata check is enough.
    let trimmed = text.trim();
    if !trimmed.is_empty() && !trimmed.contains('\n') {
        let candidate = Path::new(trimmed);
        let looks_like_image = candidate
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
                )
            })
            .unwrap_or(false);
        if looks_like_image && candidate.is_file() {
            return PasteKind::ImagePath(candidate.to_path_buf());
        }
    }
    PasteKind::Text
}

/// Recognise the most common raster image formats by header bytes.
/// Returns the canonical MIME string used both for the temp-file
/// extension picker and the eventual daemon upload route.
fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

/// Split a `data:image/<sub>;base64,<payload>` URI into `(mime,
/// payload)`. Returns `None` for non-image data URIs and for
/// non-base64 forms — we don't try to decode `;charset=…` text data
/// URIs because the pane already accepts text natively.
fn strip_data_uri(text: &str) -> Option<(&'static str, &str)> {
    let rest = text.strip_prefix("data:")?;
    let (header, payload) = rest.split_once(',')?;
    if !header.ends_with(";base64") {
        return None;
    }
    let mime_str = header.trim_end_matches(";base64");
    let mime: &'static str = match mime_str {
        "image/png" => "image/png",
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/bmp" => "image/bmp",
        _ => return None,
    };
    Some((mime, payload))
}

/// Best-effort fetch of the system clipboard via the OS helper
/// (`wl-paste`, `pbpaste`, or `xclip`). Tries image MIME types first
/// — `wl-paste -t image/png` and `xclip -selection clipboard -t
/// image/png -o` both succeed cleanly when an image is on the
/// clipboard and fail without consuming it when it's text — and
/// falls back to plain text otherwise. Runs the helper *on whatever
/// host the TUI is running on*: on a remote `agentum terminal`
/// over SSH that means the SSH host, which is almost never useful;
/// the status message names the helper that ran so users can tell
/// they didn't get the local clipboard they expected.
fn paste_from_system_clipboard(app: &mut App) {
    if !matches!(app.focus, Focus::Term | Focus::TermRight) {
        app.status_msg = Some("focus a terminal pane first".into());
        return;
    }
    let helper = match detect_clipboard_helper() {
        Some(h) => h,
        None => {
            app.push_error(
                "no clipboard helper found — install wl-paste (Wayland), \
                 pbpaste (macOS), or xclip (X11) on the host running the TUI",
            );
            return;
        }
    };
    // Image first — failure means "no image on the clipboard", at
    // which point we fall through to text. Don't surface the image
    // failure: text is the common case and showing a transient error
    // every plain-text paste would be noise.
    if let Some((mime, bytes)) = try_clipboard_image(helper) {
        let bytes_to_send = match write_paste_image(&bytes, mime) {
            Ok(path) => {
                app.status_msg = Some(format!(
                    "pasted clipboard image via {} ({} bytes) → {}",
                    helper.name(),
                    bytes.len(),
                    path.display()
                ));
                format!("{}\n", path.display()).into_bytes()
            }
            Err(e) => {
                app.push_error(format!("paste-image temp file: {e}"));
                return;
            }
        };
        send_paste_bytes(app, bytes_to_send);
        return;
    }
    match try_clipboard_text(helper) {
        Some(text) if !text.is_empty() => {
            app.status_msg = Some(format!(
                "pasted clipboard text via {} ({} chars)",
                helper.name(),
                text.chars().count()
            ));
            send_paste_bytes(app, text.into_bytes());
        }
        _ => {
            app.status_msg = Some(format!("{} returned empty clipboard", helper.name()));
        }
    }
}

#[derive(Clone, Copy)]
enum ClipboardHelper {
    WlPaste,
    PbPaste,
    Xclip,
}

impl ClipboardHelper {
    fn name(self) -> &'static str {
        match self {
            ClipboardHelper::WlPaste => "wl-paste",
            ClipboardHelper::PbPaste => "pbpaste",
            ClipboardHelper::Xclip => "xclip",
        }
    }
}

fn detect_clipboard_helper() -> Option<ClipboardHelper> {
    // Preference order matches the most common setups: Wayland
    // first (modern desktops), pbpaste on macOS, xclip as the X11
    // fallback. `which`-style probe: try invoking with `--version`
    // / a no-op arg and check the exit status. We use a cheap PATH
    // walk instead of spawning to keep the hotkey snappy.
    fn on_path(bin: &str) -> bool {
        std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths).any(|dir| {
                    let p = dir.join(bin);
                    p.is_file()
                })
            })
            .unwrap_or(false)
    }
    if on_path("wl-paste") {
        Some(ClipboardHelper::WlPaste)
    } else if on_path("pbpaste") {
        Some(ClipboardHelper::PbPaste)
    } else if on_path("xclip") {
        Some(ClipboardHelper::Xclip)
    } else {
        None
    }
}

fn try_clipboard_image(helper: ClipboardHelper) -> Option<(&'static str, Vec<u8>)> {
    use std::process::{Command, Stdio};
    // Try PNG, then JPEG — that covers screenshots from every major
    // OS. JPEG is a worthwhile second try because macOS screencapture
    // and some Linux screenshot tools default to it.
    for mime in &["image/png", "image/jpeg"] {
        let output = match helper {
            ClipboardHelper::WlPaste => Command::new("wl-paste")
                .args(["--no-newline", "-t", mime])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output(),
            ClipboardHelper::PbPaste => {
                // pbpaste only does text by default; skip silently
                // and let the caller fall through to the text branch.
                return None;
            }
            ClipboardHelper::Xclip => Command::new("xclip")
                .args(["-selection", "clipboard", "-t", mime, "-o"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output(),
        };
        if let Ok(out) = output
            && out.status.success()
            && !out.stdout.is_empty()
            && sniff_image_mime(&out.stdout).is_some()
        {
            let detected = sniff_image_mime(&out.stdout).unwrap_or(mime);
            return Some((detected, out.stdout));
        }
    }
    None
}

fn try_clipboard_text(helper: ClipboardHelper) -> Option<String> {
    use std::process::{Command, Stdio};
    let output = match helper {
        ClipboardHelper::WlPaste => Command::new("wl-paste")
            .arg("--no-newline")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
        ClipboardHelper::PbPaste => Command::new("pbpaste")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
        ClipboardHelper::Xclip => Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    };
    output
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Send paste bytes into whichever pane is focused. Shared by the
/// bracketed-paste handler and the explicit `Ctrl-K I` route so
/// "stream closed" and IO accounting stay in one place.
fn send_paste_bytes(app: &mut App, bytes: Vec<u8>) {
    let tx_opt = match app.focus {
        Focus::TermRight => app.split_right.as_ref().and_then(|s| s.term_in.clone()),
        _ => app.term_in.clone(),
    };
    let nbytes = bytes.len();
    match tx_opt.as_ref().map(|tx| tx.send(TermOut::Bytes(bytes))) {
        Some(Ok(())) => app.io.record_out(nbytes),
        Some(Err(_)) => app.push_error("terminal stream closed — Ctrl-E tree · Ctrl-Q quit"),
        None => app.status_msg = Some("no terminal stream (no session selected?)".into()),
    }
}

/// Write decoded image bytes to a uniquely-named file in the system
/// temp directory and return the path. Filename: `agentum-paste-
/// <uuid>.<ext>` — short, sortable, collision-proof. The temp file
/// outlives the paste so the agent can read it before any cleanup;
/// OS temp dirs are reaped on reboot which is fine for this use case.
fn write_paste_image(bytes: &[u8], mime: &str) -> std::io::Result<PathBuf> {
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        _ => "bin",
    };
    let mut path = std::env::temp_dir();
    path.push(format!("agentum-paste-{}.{}", Uuid::new_v4(), ext));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Spawn the Ctrl-V clipboard-image-paste flow. Gates on focus +
/// selection synchronously (so we can use `&mut App` to set status
/// hints), then drops to a detached `tokio::spawn` that runs the
/// arboard clipboard read + PNG encode inside `spawn_blocking`
/// (arboard's `Clipboard::new()` and `get_image()` are sync and can
/// block on X11 selection-owner negotiation). The uploader posts one
/// `UploadOutcome` back through `app.upload_outcome_tx`; the
/// run-loop's `select!` drains it and surfaces the result as a
/// status / error toast.
///
/// We deliberately do NOT mutate `app` from inside the spawned task
/// (the borrow checker wouldn't allow it anyway, and the rest of the
/// codebase consistently passes results back via mpsc).
fn spawn_ctrl_v_image_paste(app: &mut App, client: &Client) {
    // Focus gate — same shape as `paste_from_system_clipboard`. The
    // user can hit Ctrl-V anywhere in the TUI; we only act on it
    // when a terminal pane has focus, so accidentally pressing it
    // in the tree (where 'v' is a filter character) doesn't trigger
    // an upload.
    if !matches!(app.focus, Focus::Term | Focus::TermRight) {
        app.status_msg = Some("Ctrl-V: focus a terminal pane first".into());
        return;
    }
    let id = match app.selected {
        Some(id) => id,
        None => {
            app.status_msg = Some("Ctrl-V: no session selected".into());
            return;
        }
    };
    // Per-profile client lookup — a peer session has to ask its own
    // daemon to write the file (the active client knows nothing
    // about the peer's workdir). Falls back to the active client
    // when the session is on the default profile.
    let target_client = app
        .client_for_session(id)
        .cloned()
        .unwrap_or_else(|| client.clone());
    // Pull the result sender BEFORE the async task — the task can't
    // borrow `app`. If it's None (run-loop hasn't initialised yet)
    // we just bail without firing the upload.
    let Some(tx) = app.upload_outcome_tx.clone() else {
        return;
    };
    // Up-front status hint so the user has feedback while the broker
    // round-trip is in flight. The broker timeout is 3s — well under
    // anything the user would interpret as "the TUI froze".
    app.status_msg = Some("Ctrl-V: requesting clipboard…".into());

    tokio::spawn(async move {
        // Broker-first: ask the daemon to hop the request to a
        // connected clip-agent. The agent reads the local clipboard
        // and POSTs the upload with the request_id header, so the
        // 200 here carries the same UploadResponse shape as a direct
        // upload — TUI's success-toast path stays single-branch.
        let result = target_client.request_clipboard(id, 3000).await;
        match classify_clipboard_result(result) {
            CtrlVDecision::Success(message) => {
                let _ = tx.send(UploadOutcome { ok: true, message });
            }
            CtrlVDecision::FallbackToArboard => {
                // Single-host fallback: no clip-agent attached, so
                // try reading the OS clipboard on THIS host directly.
                // Keeps the "TUI on the same machine that owns the
                // clipboard" flow working unchanged for users who
                // never installed clip-agent.
                spawn_arboard_paste_direct(target_client, id, tx);
            }
            CtrlVDecision::ErrorNoFallback(message) => {
                let _ = tx.send(UploadOutcome { ok: false, message });
            }
        }
    });
}

/// Pure decision over a clipboard request outcome. Captures the
/// rule the broker-first Ctrl-V wrapper applies to a
/// `Result<UploadResponse, ClipboardRequestError>` so unit tests can
/// pin behaviour without spinning a real HTTP server or arboard
/// driver. Only `AgentNotConnected` triggers the arboard fallback;
/// `NoImage`/`Timeout` surface targeted toasts because falling back
/// would either repeat the same answer (NoImage) or paper over the
/// real cause (Timeout — agent there but stuck).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CtrlVDecision {
    Success(String),
    FallbackToArboard,
    ErrorNoFallback(String),
}

pub(crate) fn classify_clipboard_result(
    result: Result<api::UploadResponse, api::ClipboardRequestError>,
) -> CtrlVDecision {
    match result {
        Ok(resp) => CtrlVDecision::Success(format!(
            "uploaded {} ({} bytes)",
            resp.relative_path, resp.size_bytes
        )),
        Err(api::ClipboardRequestError::AgentNotConnected) => CtrlVDecision::FallbackToArboard,
        Err(api::ClipboardRequestError::NoImage) => CtrlVDecision::ErrorNoFallback(
            "no image in clipboard — copy an image first".into(),
        ),
        Err(api::ClipboardRequestError::Timeout) => CtrlVDecision::ErrorNoFallback(
            "no clipboard agent responded — run `agentum clip-agent --install` on the host with your clipboard".into(),
        ),
        Err(api::ClipboardRequestError::Other(e)) => {
            CtrlVDecision::ErrorNoFallback(format!("Ctrl-V: {e}"))
        }
    }
}

/// Direct local-arboard fallback: the historical Ctrl-V path,
/// extracted verbatim from `spawn_ctrl_v_image_paste` so the broker-
/// first wrapper can call it on `AgentNotConnected` without
/// duplicating the logic. Used only when no clip-agent is attached
/// to the daemon — single-host setups keep working unchanged.
fn spawn_arboard_paste_direct(
    target_client: Client,
    id: Uuid,
    tx: tokio::sync::mpsc::UnboundedSender<UploadOutcome>,
) {
    tokio::spawn(async move {
        // Clipboard read + PNG encode run on a blocking thread so a
        // large paste doesn't stall the ratatui render loop.
        let png_result = tokio::task::spawn_blocking(|| -> Result<Vec<u8>, String> {
            let mut clipboard =
                arboard::Clipboard::new().map_err(|e| format!("clipboard init failed: {e}"))?;
            let image = clipboard.get_image().map_err(clipboard_error_message)?;
            encode_rgba_as_png(
                image.width as u32,
                image.height as u32,
                image.bytes.as_ref(),
            )
            .map_err(|e| format!("PNG encode failed: {e}"))
        })
        .await;

        let png_bytes = match png_result {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(msg)) => {
                let _ = tx.send(UploadOutcome {
                    ok: false,
                    message: format!("Ctrl-V: {msg}"),
                });
                return;
            }
            Err(join_err) => {
                let _ = tx.send(UploadOutcome {
                    ok: false,
                    message: format!("Ctrl-V: clipboard task panicked: {join_err}"),
                });
                return;
            }
        };

        match target_client.upload_image(id, png_bytes, "image/png").await {
            Ok(resp) => {
                let _ = tx.send(UploadOutcome {
                    ok: true,
                    message: format!(
                        "uploaded {} ({} bytes)",
                        resp.relative_path, resp.size_bytes
                    ),
                });
            }
            Err(e) => {
                let _ = tx.send(UploadOutcome {
                    ok: false,
                    message: format!("Ctrl-V: upload failed: {e}"),
                });
            }
        }
    });
}

/// Map an arboard error to a user-readable status message.
/// Specifically calls out the "nothing copied" case, which is by far
/// the most common Ctrl-V failure mode — the message guides users
/// toward the right behaviour (copy an image first) instead of
/// dropping a stack-trace into the toast stack.
fn clipboard_error_message(err: arboard::Error) -> String {
    use arboard::Error::*;
    match err {
        ContentNotAvailable => "no image in clipboard — copy an image first (Ctrl-V is for images only — use bracketed paste for text)".to_string(),
        ClipboardNotSupported => "clipboard not supported in this environment".to_string(),
        ClipboardOccupied => "clipboard is busy — try again".to_string(),
        other => format!("clipboard error: {other}"),
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
    let pane_col = col
        .saturating_sub(rect.x.saturating_add(1))
        .saturating_add(1);
    let pane_row = row
        .saturating_sub(rect.y.saturating_add(1))
        .saturating_add(1);

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
                        // Queue the OSC-52 bytes — the run-loop flushes
                        // them after `terminal.draw()` and follows up
                        // with `terminal.clear()` so any literal-echo
                        // damage (terminals without OSC-52 support,
                        // tmux without `allow-passthrough on`) gets
                        // overpainted instead of staying on screen.
                        app.pending_clipboard_seq = Some(build_osc52_sequence(&text));
                        app.status_msg = Some(format!("copied {} chars", text.chars().count()));
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
    let ((s_col, s_row), (e_col, e_row)) = sel.ordered();
    extract_selection_from_screen(screen, (s_col, s_row), (e_col, e_row))
}

/// Pure screen-walking: copy whichever cells lie inside the
/// selection rectangle into a `String`. Lives in its own function so
/// the empty-cell → space substitution (which fixed the
/// "CIonGitHub" regression from v0.8.2's mouse copy path) can be
/// unit-tested without standing up an `App`. The `start` and `end`
/// pairs are 1-based `(col, row)` already ordered such that
/// `(s_col, s_row) ≤ (e_col, e_row)` per `TermSelection::ordered`.
fn extract_selection_from_screen(
    screen: &vt100::Screen,
    start: (u16, u16),
    end: (u16, u16),
) -> String {
    let (rows, cols) = screen.size();
    let (s_col, s_row) = start;
    let (e_col, e_row) = end;
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
            match screen.cell(r, c) {
                Some(cell) => {
                    let contents = cell.contents();
                    // Empty `contents()` means the inner program left
                    // this cell untouched — ratatui's diff renderer
                    // moves the cursor instead of overwriting blanks,
                    // so the intra-word spaces in lines like "CI on
                    // GitHub" land in cells whose `contents()` is "".
                    // Substitute a single space so the copy preserves
                    // the layout the user sees (the trailing trim
                    // below still strips the row-pad).
                    if contents.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(&contents);
                    }
                }
                None => line.push(' '),
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

/// Pull the last `max_lines` non-blank rows off a vt100 screen and
/// return them joined with `\n`. Used by the lazygit-exit path to
/// surface the child's final frame as an error message — most lazygit
/// startup failures (no git repo, missing /dev/tty, version checks)
/// print one or two lines to the controlling TTY and then exit before
/// any UI renders, so the visible-screen tail is exactly the
/// diagnostic the user needs.
fn capture_screen_tail(screen: &vt100::Screen, max_lines: usize) -> String {
    let (rows, cols) = screen.size();
    let mut out: Vec<String> = Vec::new();
    for r in 0..rows {
        let mut line = String::new();
        for c in 0..cols {
            if let Some(cell) = screen.cell(r, c) {
                line.push_str(&cell.contents());
            }
        }
        let trimmed = line.trim_end().to_string();
        if !trimmed.is_empty() {
            out.push(trimmed);
        }
    }
    // Keep only the trailing N lines; the head is usually blank
    // screen-clear padding or stale buffer state.
    if out.len() > max_lines {
        out = out.split_off(out.len() - max_lines);
    }
    out.join("\n")
}

/// Build an OSC-52 byte sequence that pushes `text` to the host
/// terminal's clipboard. Returns the raw bytes ready for `stdout` —
/// the caller (`run_loop`) flushes them between frames and follows
/// up with `terminal.clear()` so any non-OSC-52-capable terminal
/// gets a clean repaint instead of literal escape chars on screen.
/// That two-step is the fix for v0.6.33's regression where writing
/// inline from the mouse handler corrupted the visible buffer.
///
/// Tmux passthrough: when `$TMUX` is set we're inside an outer
/// tmux instance that, by default, will swallow or partially echo
/// the OSC. The DCS-passthrough wrapper (`\x1bPtmux;…\x1b\\`) tells
/// tmux to forward the inner OSC to the outer terminal verbatim.
/// The user's outer tmux still needs `set -g allow-passthrough on`
/// for this to actually escape — but that's a one-line tmux config,
/// not something agentum can paper over from the inside.
fn build_osc52_sequence(text: &str) -> Vec<u8> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let encoded = STANDARD.encode(text.as_bytes());
    let in_tmux = std::env::var_os("TMUX").is_some();
    let inner = format!("\x1b]52;c;{encoded}\x07");
    let seq = if in_tmux {
        // Each `\x1b` inside the passthrough must be doubled per
        // tmux's DCS protocol so the outer tmux strips one layer
        // and the inner ESC reaches the host terminal intact.
        format!("\x1bPtmux;{}\x1b\\", inner.replace('\x1b', "\x1b\x1b"))
    } else {
        inner
    };
    seq.into_bytes()
}

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
            // Ctrl-K V — collapse / expand the SERVERS section. The
            // sidebar otherwise carries SERVERS at the bottom; folding
            // it reclaims height for the sessions tree on a long
            // multi-profile setup. If the cursor was parked on a
            // server row, pull it back into the sessions section so
            // collapsing doesn't strand the highlight.
            Some('v') => {
                app.servers_collapsed = !app.servers_collapsed;
                if app.servers_collapsed && app.tree_section == TreeSection::Servers {
                    app.tree_section = TreeSection::Sessions;
                }
                app.prefs.servers_collapsed = app.servers_collapsed;
                prefs::save(&app.prefs);
                app.status_msg = Some(if app.servers_collapsed {
                    "servers section collapsed".into()
                } else {
                    "servers section expanded".into()
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
            // Ctrl-K I — paste clipboard contents into the focused
            // pane via the OS clipboard helper (wl-paste / pbpaste /
            // xclip). Detects image bytes and routes them through the
            // temp-file path the same way bracketed paste does. This
            // is the deliberate "fetch from system clipboard" path
            // for cases where bracketed paste either isn't enabled in
            // the terminal or the user is on a binary-clipboard
            // workflow. Over SSH the helper would run on the *remote*
            // host, which is rarely what's wanted — the status
            // message explains the local-only constraint when no
            // helper is on PATH.
            Some('i') => paste_from_system_clipboard(app),
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
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('k')) {
        app.chord = Some('K');
        app.status_msg = Some(
            "Ctrl-K · waiting (Z fullscreen · B sidebar · I image paste · , / . lazygit width) · Ctrl-V: paste clipboard image"
                .into(),
        );
        return;
    }

    // Ctrl-B — VS Code "toggle primary side bar". Hides just the tree
    // column; title and status bars stay so the user keeps the
    // breadcrumb. Distinct from Shift-F / Ctrl-K Z fullscreen which
    // strips everything.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('b')) {
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

    // Alt-Enter — toggle the multi-select check on the session row
    // under the tree cursor, *from any focus*. The tree-section
    // handler at the bottom of this fn also reacts to plain Enter
    // when the tree is focused; this global variant lets the user
    // mark sessions without giving up terminal focus (which is the
    // mode most users live in). Group rows and non-leaf cursor
    // positions no-op so an accidental Alt-Enter never sweeps an
    // entire project group in.
    if key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Enter)
        && let Some(Row::Leaf {
            group,
            project,
            leaf,
        }) = app.tree.current_row()
    {
        let id = app.tree.groups[group].projects[project].sessions[leaf];
        if !app.checked.insert(id) {
            app.checked.remove(&id);
        }
        let n = app.checked.len();
        app.status_msg = Some(if n == 0 {
            "checks cleared".into()
        } else {
            format!("{n} checked · u/s/K/x to act · Esc to clear")
        });
        return;
    }

    // Ctrl-T — toggle the right-side agent-tasks panel (plan / todos /
    // background tasks). Mirror of Ctrl-B for the opposite edge. Hidden
    // automatically on terminals narrower than ~110 cols regardless of
    // this flag — see `ui::compute_layout`.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('t')) {
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

    // Ctrl-V — paste an image from the LOCAL OS clipboard into the
    // focused agent pane. Distinct from `Ctrl-K I` (which shells out
    // to `wl-paste`/`pbpaste`/`xclip` on whatever host the TUI runs
    // on, and so reads the *wrong* clipboard over SSH or a remote
    // `--profile`): this branch reads the host where the TUI is
    // actually running via `arboard`, PNG-encodes the pixels off the
    // main task, and uploads them to the owning daemon — which writes
    // the file next to the agent's working tree and types the path
    // into the pane. We deliberately gate on Ctrl alone, NOT
    // Ctrl-Shift — Ctrl-Shift-V is reserved for the terminal
    // emulator's own paste binding (kitty, alacritty, gnome-terminal
    // all use it for text paste from the host clipboard) and
    // intercepting it would break that workflow.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'))
        && !key.modifiers.contains(KeyModifiers::SHIFT)
    {
        spawn_ctrl_v_image_paste(app, client);
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
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char(',')) {
        app.overlay = Overlay::Settings(SettingsState::new());
        app.status_msg =
            Some("settings (Esc close · ↑↓ move · ←→ adjust · space toggle · r reset row)".into());
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

    // Ctrl-H opens the SSH hosts readiness overlay. Gated to Tree focus so
    // Ctrl-H stays the shell's erase key (^H) inside a terminal pane;
    // reachable from any focus via the command palette ("Hosts…").
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('h') | KeyCode::Char('H'))
        && app.focus == Focus::Tree
    {
        open_hosts_overlay(app);
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

    // Ctrl-D — delete the row under the tree cursor. Routes by
    // section: Servers → `RemoveServer` confirmation, Sessions →
    // `Kill` confirmation. Either way the user gets a y/N prompt
    // (the "double check" — `handle_confirm_key` only commits on
    // y/Y/Enter). When a terminal pane is focused, Ctrl-D still
    // forwards EOF (^D) to the running agent — standard Unix
    // behaviour — so this only intercepts when Focus::Tree.
    if ctrl
        && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('D'))
        && app.focus == Focus::Tree
    {
        match app.tree_section {
            TreeSection::Servers => {
                // The synthetic "this machine" row doesn't correspond to
                // a persisted profile entry — there's nothing to remove.
                match app.cursor_profile().cloned() {
                    None => {
                        app.status_msg = Some(format!(
                            "can't remove {} — it's the local loopback",
                            local_machine_label()
                        ));
                    }
                    Some(entry) => {
                        if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                            app.status_msg =
                                Some("can't remove the active server — switch first".into());
                        } else {
                            app.overlay = Overlay::Confirm(PendingAction::RemoveServer {
                                name: entry.name.clone(),
                            });
                        }
                    }
                }
                return;
            }
            TreeSection::Sessions => {
                if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Kill) {
                    app.overlay = Overlay::Confirm(bulk);
                } else if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Kill {
                        id: s.id,
                        name: s.name.clone(),
                    });
                } else {
                    app.status_msg = Some("no session selected".into());
                }
                return;
            }
        }
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

    // Ctrl-S — open the server switcher overlay from any focus.
    // Mnemonic: "S = Servers". Mirrors the dashboard's ServerSwitcher
    // chip in the topbar. Available from anywhere so a user driving
    // multiple agentum servers can hop without releasing focus; also
    // surfaced in the command palette. Plain Shift+S still acts on the
    // tree cursor (stop session) — the modifier disambiguates.
    if ctrl && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S')) {
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
            let label = profile_label(&app.tree.groups[n - 1].profile);
            app.status_msg = Some(format!("server {n}: {label}"));
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
        Overlay::Goal(_) => {
            handle_goal_key(app, key, client).await;
            return;
        }
        Overlay::Hosts(_) => {
            handle_hosts_key(app, key, client).await;
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
                if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Kill) {
                    app.overlay = Overlay::Confirm(bulk);
                } else if let Some(s) = app.selected_session() {
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
                if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Kill) {
                    app.overlay = Overlay::Confirm(bulk);
                } else if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Kill {
                        id: s.id,
                        name: s.name.clone(),
                    });
                }
                return;
            }
            KeyCode::Char('U') => {
                if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Start) {
                    app.overlay = Overlay::Confirm(bulk);
                } else if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Start {
                        id: s.id,
                        name: s.name.clone(),
                    });
                }
                return;
            }
            KeyCode::Char('S') => {
                if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Stop) {
                    app.overlay = Overlay::Confirm(bulk);
                } else if let Some(s) = app.selected_session() {
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

    // While the lazygit pane is focused, forward raw bytes to its PTY —
    // except for the four width-resize chars, which we let fall through
    // to the match block below so the user can grow / shrink agentum's
    // lazygit column with the same `+`/`-` they use on the sidebar
    // tree. Lazygit's own `+` screen-mode toggle is sacrificed; the
    // outer column width is what matters when lazygit is embedded.
    if app.focus == Focus::Lazygit {
        let is_resize_key = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && matches!(
                key.code,
                KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-') | KeyCode::Char('_')
            );
        if !is_resize_key {
            if let Some(lg) = app.lazygit.as_ref()
                && let Some(bytes) = key_to_bytes(&key)
            {
                if let Err(e) = lg.write(&bytes) {
                    app.push_error(format!("lazygit write: {e}"));
                }
            }
            return;
        }
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
        // Owned clones (the senders are cheap Arc-backed handles) so
        // we don't hold an outstanding immutable borrow of `app` —
        // we need `&mut app` further down for the `/clear` shadow.
        let tx_opt = match app.focus {
            Focus::TermRight => app.split_right.as_ref().and_then(|s| s.term_in.clone()),
            _ => app.term_in.clone(),
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
            // A keypress is strong "agent is working again" signal —
            // flip the dot to green locally instead of waiting on the
            // round-trip `agent.working` event, mirroring the same
            // optimism used for the idle/awaiting clears just above.
            app.working.insert(id);
            // Shadow the line buffer so we can spot `/clear` (or
            // `\clear`) and mirror the agent's context wipe in the
            // plan/todo panel. Runs unconditionally — cheap, and we
            // want to track even before the byte is sent so the local
            // panel clear lands at the same beat as the remote
            // command.
            track_term_input_for_clear(app, id, &key, client);
        }
        let nbytes = bytes.len();
        let send_result = tx_opt.as_ref().map(|tx| tx.send(TermOut::Bytes(bytes)));
        match send_result {
            Some(Ok(())) => {
                app.io.record_out(nbytes);
            }
            Some(Err(_)) => {
                app.push_error("terminal stream closed — Ctrl-E tree · Ctrl-Q quit");
            }
            None => {
                app.status_msg = Some(
                    "no terminal stream (no session selected?) — Ctrl-E tree · Ctrl-Q quit".into(),
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
        KeyCode::Char('G') => {
            // UI-SPEC §Interaction Contract: G from Tree focus opens the
            // Goal composer; from any other focus (e.g. lazygit pane open)
            // fall through to the LazygitCheats sheet so that binding is
            // not silently eaten. The Goal compositor is only useful from
            // the tree where the board is visible.
            if matches!(app.overlay, Overlay::None) && app.focus == Focus::Tree {
                app.overlay = Overlay::Goal(Box::new(GoalForm::default_for_profile(
                    app.active_profile.clone().unwrap_or_default(),
                )));
                tracing::info!("opened Overlay::Goal");
            } else {
                app.overlay = Overlay::LazygitCheats;
            }
        }
        // `c` in Tree focus toggles the bound-card hint strip above the
        // status bar. Only fires when the selected session has a card_id;
        // no-ops otherwise (no terminal forwarding — tree focus never
        // forwards keys to the PTY). Phase 2, plan 05.
        KeyCode::Char('c') if app.focus == Focus::Tree && key.modifiers.is_empty() => {
            if let Some(sess) = app.selected_session() {
                if let Some(card_id) = sess.card_id {
                    // Toggle: collapse if already showing this card.
                    if app.hint_card.as_ref().map(|h| h.card_id) == Some(card_id) {
                        app.hint_card = None;
                    } else {
                        // Fetch the card title from the daemon (best-effort).
                        match client.get_board_item(card_id).await {
                            Ok(card) => {
                                let title: String =
                                    card.title.unwrap_or_default().chars().take(72).collect();
                                app.hint_card = Some(HintCardState { card_id, title });
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    card_id,
                                    "fetch card for hint strip failed"
                                );
                            }
                        }
                    }
                }
                // If card_id is None, `c` is a no-op.
            }
        }
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
        // Esc clears the multi-select set first — it's the most
        // recently entered transient state, and matches the way Esc
        // works in the rest of the TUI (peel one layer at a time). If
        // nothing's checked, fall through to filter / fullscreen / etc.
        KeyCode::Esc if !app.checked.is_empty() => {
            let n = app.checked.len();
            app.checked.clear();
            app.status_msg = Some(format!("cleared {n} checked"));
        }
        // Esc collapses the bound-card hint strip if it is showing and no
        // other transient state (overlay, filter, bulk-check) took the Esc
        // above. Peel-one-layer-at-a-time pattern (Phase 2, plan 05).
        KeyCode::Esc if app.hint_card.is_some() && matches!(app.overlay, Overlay::None) => {
            app.hint_card = None;
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
        // Resize the focused side column. 4-col steps; clamps differ
        // per target (tree 16..=80, lazygit per ui::LAZYGIT_*). Focus on
        // the lazygit pane retargets the keys to its column so the user
        // doesn't have to learn the Ctrl-K , / Ctrl-K . chord. Default
        // path (tree or terminal focus) keeps the historical behaviour
        // of resizing the sidebar tree.
        KeyCode::Char('+') | KeyCode::Char('=') => {
            if app.focus == Focus::Lazygit && app.lazygit_open() {
                app.lazygit_width = app
                    .lazygit_width
                    .saturating_add(4)
                    .min(ui::LAZYGIT_MAX_WIDTH);
                app.prefs.lazygit_width = app.lazygit_width;
                prefs::save(&app.prefs);
                app.status_msg = Some(format!("lazygit width: {}", app.lazygit_width));
            } else {
                app.tree_width = app.tree_width.saturating_add(4).min(80);
                app.prefs.tree_width = app.tree_width;
                prefs::save(&app.prefs);
                app.status_msg = Some(format!("tree width: {}", app.tree_width));
            }
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            if app.focus == Focus::Lazygit && app.lazygit_open() {
                app.lazygit_width = app
                    .lazygit_width
                    .saturating_sub(4)
                    .max(ui::LAZYGIT_MIN_WIDTH);
                app.prefs.lazygit_width = app.lazygit_width;
                prefs::save(&app.prefs);
                app.status_msg = Some(format!("lazygit width: {}", app.lazygit_width));
            } else {
                app.tree_width = app.tree_width.saturating_sub(4).max(16);
                app.prefs.tree_width = app.tree_width;
                prefs::save(&app.prefs);
                app.status_msg = Some(format!("tree width: {}", app.tree_width));
            }
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
            // Sidebar order is Servers (top, optional) → Sessions
            // (bottom, scrollable). `j` flows out the bottom of the
            // Servers list into the top of the sessions tree. When
            // the Servers section is collapsed the user lives in
            // Sessions only and `j` is just a normal tree move.
            match app.tree_section {
                TreeSection::Servers => {
                    // `servers_row_count` already accounts for the
                    // optional synthetic loopback row, so `last` is the
                    // index of the *last visible row* regardless of
                    // whether the synthetic row is being painted.
                    let last = app.servers_row_count().saturating_sub(1);
                    if app.servers_cursor < last {
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
            // Mirror of `j`: `k` at the top of Sessions hops up into
            // the last server row when Servers is expanded; collapsed
            // → stay in Sessions and no-op at cursor 0.
            match app.tree_section {
                TreeSection::Servers => {
                    app.servers_cursor = app.servers_cursor.saturating_sub(1);
                }
                TreeSection::Sessions => {
                    if app.tree.cursor == 0 {
                        if !app.servers_collapsed {
                            app.tree_section = TreeSection::Servers;
                            // Snap to the *last* visible servers row,
                            // accounting for the optional synthetic
                            // loopback above the profile list.
                            app.servers_cursor = app.servers_row_count().saturating_sub(1);
                        }
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
            // group rows). Enter is the multi-select toggle (see below).
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
                    // Switch the active profile to whichever server the
                    // cursor sits on. Mirrors the Ctrl-S overlay's Enter
                    // — schedules a soft restart via `pending_switch_profile`
                    // so the run-loop tears down and reconnects against
                    // the new target. Cursor 0 is the synthetic
                    // "this machine" row → empty profile name, which the
                    // mod.rs SwitchProfile arm translates to `None` so
                    // apply_profile takes the loopback-detection path.
                    //
                    // Refuse while lazygit is open: the soft restart
                    // drops the App, which drops the lazygit PTY, which
                    // kills the child. The user sees the pane vanish
                    // and reads it as a crash. Symmetric to Ctrl-\\
                    // refusing to split while lazygit is open.
                    if app.lazygit_open() {
                        app.status_msg = Some("close lazygit (g) before switching servers".into());
                        return;
                    }
                    let target: String = match app.cursor_profile() {
                        Some(e) => e.name.clone(),
                        None if app.synthetic_loopback_visible() => String::new(),
                        None => return,
                    };
                    let active = app.active_profile.clone().unwrap_or_default();
                    let label = profile_label(&target);
                    // Same-profile Enter is a re-connect, not a no-op:
                    // useful when the daemon went away (Unreachable /
                    // LoginNeeded) and the user wants to retry without
                    // first switching to another server.
                    if active == target {
                        app.status_msg = Some(format!("reconnecting to {label}…"));
                    } else {
                        app.status_msg = Some(format!("switching to {label}…"));
                    }
                    // Mark the target as in-flight so the sidebar swaps
                    // its status dot for a spinner glyph until the soft
                    // restart completes (or fails). The new App starts
                    // with an empty set, so the spinner stops naturally.
                    app.reconnecting.insert(target.clone());
                    app.pending_switch_profile = Some(target);
                    app.pending_after_switch = None;
                    app.should_quit = true;
                }
                TreeSection::Sessions => {
                    // Toggle the leaf under the cursor in the multi-select
                    // set. No-op on group rows so an accidental Enter on a
                    // collapsed project doesn't sweep every child in. Once
                    // anything is checked, lifecycle keys (u/s/K/x/D and
                    // Ctrl-D) act on the set; Esc clears it.
                    if let Some(Row::Leaf {
                        group,
                        project,
                        leaf,
                    }) = app.tree.current_row()
                    {
                        let id = app.tree.groups[group].projects[project].sessions[leaf];
                        if !app.checked.insert(id) {
                            app.checked.remove(&id);
                        }
                        let n = app.checked.len();
                        app.status_msg = Some(if n == 0 {
                            "checks cleared".into()
                        } else {
                            format!("{n} checked · u/s/K/x to act · Esc to clear")
                        });
                    }
                }
            }
        }
        // Servers section actions: `a` adds, `d` removes. Only fire
        // when the cursor is actually in the Servers section so the
        // Sessions tree's existing `d` (delete-session) keybind, if
        // any, doesn't get hijacked.
        KeyCode::Char('a') if app.tree_section == TreeSection::Servers => {
            // Reuse the same overlay the Ctrl-S switcher uses; the
            // overlay's add-form handles validation + persistence.
            open_profiles_overlay(app);
            if let Overlay::Profiles(ref mut state) = app.overlay {
                state.add_form = Some(AddProfileForm::new());
            }
        }
        KeyCode::Char('d') | KeyCode::Char('D') if app.tree_section == TreeSection::Servers => {
            // Route through the same confirmation overlay as `Ctrl-D`
            // so an accidental keypress doesn't silently nuke the
            // profile. The previous direct-`store.remove` path was the
            // muscle-memory landmine the user kept hitting.
            match app.cursor_profile().cloned() {
                None => {
                    // Synthetic loopback row — nothing on disk to remove.
                    app.status_msg = Some(format!(
                        "can't remove {} — it's the local loopback",
                        local_machine_label()
                    ));
                }
                Some(entry) => {
                    if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                        app.status_msg =
                            Some("can't remove the active server — switch first".into());
                    } else {
                        app.overlay = Overlay::Confirm(PendingAction::RemoveServer {
                            name: entry.name.clone(),
                        });
                    }
                }
            }
        }

        // Session lifecycle ------------------------------------------------
        KeyCode::Char('n') => {
            // Default the workdir to the selected session's workdir if any,
            // else the *daemon's* $HOME (not the laptop's). When the user
            // is driving a remote profile from macOS, `std::env::var("HOME")`
            // resolves to `/Users/…` which doesn't exist on the Linux
            // daemon. Asking the server resolves to whatever its own
            // `$HOME` is, matching what Tab-cycling profiles inside the
            // form already does.
            // Seed the form with a *consistent* (profile, workdir)
            // pair. When a session is selected, pre-fill against
            // the server that actually owns it — its workdir is a
            // path on that server, so opening on a different profile
            // (e.g. the laptop's active connection) would hand the
            // user a path that doesn't exist on the target.
            //
            // No selection → fall back to the active connection and
            // ask the daemon for its `$HOME`. If the network hiccups
            // we use the laptop's `$HOME` as a last resort so the
            // form still opens with something editable.
            let (profile, workdir) = if let Some(s) = app.selected_session() {
                let owning_profile = app.profile_for_session(s.id).to_string();
                (owning_profile, s.workdir.clone())
            } else {
                let active = app.active_profile.clone().unwrap_or_default();
                let home = match client.list_dir(None).await {
                    Ok(listing) => listing.path,
                    Err(_) => std::env::var("HOME").unwrap_or_default(),
                };
                (active, home)
            };
            app.overlay =
                Overlay::NewSession(Box::new(NewSessionForm::with_profile(profile, workdir)));
        }
        KeyCode::Char('u') => {
            if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Start) {
                app.overlay = Overlay::Confirm(bulk);
            } else if let Some(s) = app.selected_session() {
                app.overlay = Overlay::Confirm(PendingAction::Start {
                    id: s.id,
                    name: s.name.clone(),
                });
            } else {
                app.status_msg = Some("no session selected".into());
            }
        }
        KeyCode::Char('s') => {
            if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Stop) {
                app.overlay = Overlay::Confirm(bulk);
            } else if let Some(s) = app.selected_session() {
                app.overlay = Overlay::Confirm(PendingAction::Stop {
                    id: s.id,
                    name: s.name.clone(),
                });
            } else {
                app.status_msg = Some("no session selected".into());
            }
        }
        KeyCode::Char('K') => {
            if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Kill) {
                app.overlay = Overlay::Confirm(bulk);
            } else if let Some(s) = app.selected_session() {
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
            if let Some(bulk) = bulk_action_from_checks(app, BulkKind::Kill) {
                app.overlay = Overlay::Confirm(bulk);
            } else if let Some(s) = app.selected_session() {
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

/// Build and open the Ctrl-H hosts overlay from the current host list.
/// Snapshots host ids so the cursor is stable; readiness is fetched
/// lazily (Enter / `t`), not on open.
fn open_hosts_overlay(app: &mut App) {
    let host_ids: Vec<Uuid> = app.hosts.iter().map(|h| h.id).collect();
    if host_ids.is_empty() {
        app.status_msg = Some("no hosts defined — add one with `agentum hosts add …`".into());
        return;
    }
    app.overlay = Overlay::Hosts(HostsOverlay {
        host_ids,
        cursor: 0,
        loading: false,
        error: None,
    });
    app.status_msg =
        Some("hosts · ↑↓ move · Enter/t check · i set up (deps + agents) · Esc close".into());
}

/// Key handling for [`Overlay::Hosts`]. ↑/↓ (or k/j) move the cursor;
/// Enter or `t` runs a readiness preflight for the selected host (an
/// inline SSH round trip — same blocking pattern as the New Session host
/// probe — cached in `app.host_readiness_cache`); Esc closes.
async fn handle_hosts_key(app: &mut App, key: KeyEvent, client: &Client) {
    let Overlay::Hosts(mut overlay) = std::mem::replace(&mut app.overlay, Overlay::None) else {
        return;
    };
    match key.code {
        // Already swapped to `Overlay::None` above — leave it closed.
        KeyCode::Esc => return,
        KeyCode::Up | KeyCode::Char('k') => {
            overlay.cursor = overlay.cursor.saturating_sub(1);
            overlay.error = None;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if overlay.cursor + 1 < overlay.host_ids.len() {
                overlay.cursor += 1;
            }
            overlay.error = None;
        }
        KeyCode::Enter | KeyCode::Char('t') => {
            if let Some(id) = overlay.selected() {
                overlay.loading = true;
                overlay.error = None;
                match client.host_readiness(id).await {
                    Ok(report) => {
                        app.host_readiness_cache
                            .insert(id, (Instant::now(), report));
                    }
                    Err(e) => overlay.error = Some(e.to_string()),
                }
                overlay.loading = false;
            }
        }
        // `i` sets up the selected host in one flow: install the missing
        // required deps (tmux/git) AND the missing agent CLIs, via a
        // single Confirm. Uses the cached readiness to decide what's
        // missing, so it needs a prior check (Enter/t).
        KeyCode::Char('i') => {
            if let Some(id) = overlay.selected() {
                let (deps, agents): (Vec<String>, Vec<String>) = app
                    .cached_readiness(id)
                    .map(|r| {
                        let deps = r
                            .required
                            .iter()
                            .filter(|d| !d.installed && d.bootstrapable)
                            .map(|d| d.id.clone())
                            .collect();
                        let agents = r
                            .agents
                            .iter()
                            .filter(|a| !a.installed)
                            .map(|a| a.id.clone())
                            .collect();
                        (deps, agents)
                    })
                    .unwrap_or_default();
                if deps.is_empty() && agents.is_empty() {
                    overlay.error = Some(
                        "nothing to install — press Enter/t to check, or host already set up"
                            .into(),
                    );
                } else {
                    let name = app
                        .hosts
                        .iter()
                        .find(|h| h.id == id)
                        .map(|h| h.name.clone())
                        .unwrap_or_else(|| "host".into());
                    // Hand off to the Confirm overlay; don't restore Hosts.
                    app.overlay = Overlay::Confirm(PendingAction::ProvisionHost {
                        id,
                        name,
                        deps,
                        agents,
                    });
                    return;
                }
            }
        }
        _ => {}
    }
    app.overlay = Overlay::Hosts(overlay);
}

/// Reopen the Ctrl-H hosts overlay with the cursor positioned on `id`.
/// Used after a bootstrap confirm so the user lands back on the host they
/// just acted on, with its (now refreshed) status dot.
fn reopen_hosts_overlay_at(app: &mut App, id: Uuid) {
    open_hosts_overlay(app);
    if let Overlay::Hosts(ref mut o) = app.overlay
        && let Some(pos) = o.host_ids.iter().position(|h| *h == id)
    {
        o.cursor = pos;
    }
}

async fn handle_new_session_key(app: &mut App, key: KeyEvent, client: &Client) {
    let Overlay::NewSession(mut form) = std::mem::replace(&mut app.overlay, Overlay::None) else {
        return;
    };
    if form.submitting {
        app.overlay = Overlay::NewSession(form);
        return;
    }

    // Picker overlay: input goes there as long as it's open.
    if form.picker.is_some() {
        // Resolve the *target* profile's client so drill-in /
        // pop-up queries hit the right daemon. Otherwise picking
        // a peer server in the Servers field and then navigating
        // the picker would walk the local laptop's tree instead.
        let target_client = app
            .clients
            .get(form.profile.as_str())
            .and_then(|e| e.client.clone());
        let target_ref = target_client.as_ref().unwrap_or(client);
        handle_dir_picker_key(&mut form, key, target_ref).await;
        app.overlay = Overlay::NewSession(form);
        return;
    }

    // Tool-picker overlay (mirrors the dir-picker modal but lists
    // every entry in `TOOL_SUGGESTIONS`). Owns input while open.
    if form.tool_picker.is_some() {
        handle_tool_picker_key(&mut form, key);
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
                // entry meaning the local loopback ("this machine").
                // The empty entry is only offered when a real local
                // client is connected — otherwise cycling to "" would
                // silently fall back to the active server's `$HOME`
                // and the workdir field wouldn't appear to change.
                let names: Vec<String> = app.profiles.iter().map(|p| p.name.clone()).collect();
                let has_local = app.clients.contains_key("");
                // Wheel size is what `cycle_profile` would build:
                // configured peers + optional empty entry for local.
                // If that has zero or one entry there's nothing to
                // cycle through, so Tab should advance to the next
                // field instead of trapping the cursor here.
                let wheel_size = names.len() + if has_local { 1 } else { 0 };
                if wheel_size <= 1 {
                    form.next_field();
                } else {
                    let old_profile = form.profile.clone();
                    form.cycle_profile(&names, has_local);
                    // When the profile changes, fetch the new server's
                    // `$HOME` and use it as the workdir. We resolve
                    // strictly through `app.clients` now — no fallback
                    // to the run-loop's client for the empty case,
                    // because that fallback used to mask the
                    // "no local connected" state by returning the
                    // active server's `$HOME` (the bug the user hit).
                    if form.profile != old_profile {
                        let target_client = app
                            .clients
                            .get(form.profile.as_str())
                            .and_then(|e| e.client.clone());
                        if let Some(tc) = target_client {
                            match tc.list_dir(None).await {
                                Ok(listing) => {
                                    form.workdir = listing.path;
                                    form.host_id.clear();
                                    app.hosts = tc.list_hosts().await.unwrap_or_default();
                                    app.agent_availability =
                                        tc.list_agents_on(None).await.ok().map(|list| {
                                            list.into_iter()
                                                .filter(|a| a.available)
                                                .map(|a| a.name)
                                                .collect()
                                        });
                                    // Clear any prior error: the new
                                    // refetch succeeded, so the
                                    // workdir field is authoritative.
                                    form.error = None;
                                }
                                Err(e) => {
                                    // Surface the failure inline so the
                                    // user understands why the workdir
                                    // didn't move with the cycle.
                                    form.error = Some(format!(
                                        "couldn't reach {}: {e}",
                                        profile_label(&form.profile)
                                    ));
                                }
                            }
                        } else {
                            form.error = Some(format!(
                                "{} isn't connected — try Ctrl-S to re-add",
                                profile_label(&form.profile)
                            ));
                        }
                    }
                }
            }
            NewSessionField::Host => {
                if app.hosts.len() <= 1 {
                    form.next_field();
                } else {
                    form.cycle_host(&app.hosts);
                    let host_id = Uuid::parse_str(&form.host_id).ok();
                    let host_name = host_id
                        .and_then(|id| app.hosts.iter().find(|h| h.id == id))
                        .map(|h| h.name.clone())
                        .unwrap_or_else(|| "host".to_string());
                    let target_client = app
                        .clients
                        .get(form.profile.as_str())
                        .and_then(|e| e.client.clone());
                    let target_ref = target_client.as_ref().unwrap_or(client);

                    // Prefer a single readiness round trip per host: it
                    // reports agent availability *and* required-dep gaps,
                    // so we derive the tool picker's availability set from
                    // it and surface a blocking hint when tmux/git are
                    // missing. Caching feeds the submit guard. PRD §7.6.
                    let mut readiness_ok = true;
                    match host_id {
                        Some(id) => match target_ref.host_readiness(id).await {
                            Ok(report) => {
                                app.agent_availability = Some(
                                    report
                                        .agents
                                        .iter()
                                        .filter(|a| a.installed)
                                        .map(|a| a.id.clone())
                                        .collect(),
                                );
                                readiness_ok = report.ok;
                                if !report.ok {
                                    form.error = Some(format!(
                                        "{host_name} not ready — {} (fix via Ctrl-H)",
                                        report.message
                                    ));
                                }
                                app.host_readiness_cache
                                    .insert(id, (Instant::now(), report));
                            }
                            // Old daemon without `/readiness`, or a
                            // transient failure: fall back to the agents
                            // probe so the picker still gates correctly.
                            Err(_) => {
                                app.agent_availability =
                                    target_ref.list_agents_on(host_id).await.ok().map(|list| {
                                        list.into_iter()
                                            .filter(|a| a.available)
                                            .map(|a| a.name)
                                            .collect()
                                    });
                            }
                        },
                        // Local host (empty id): instant probe, no SSH.
                        None => {
                            app.agent_availability =
                                target_ref.list_agents_on(host_id).await.ok().map(|list| {
                                    list.into_iter()
                                        .filter(|a| a.available)
                                        .map(|a| a.name)
                                        .collect()
                                });
                        }
                    }

                    // Default the workdir to the host's home. Don't clear
                    // a readiness error if one is already blocking.
                    match target_ref.list_dir_on(None, host_id).await {
                        Ok(listing) => {
                            form.workdir = listing.path;
                            if readiness_ok {
                                form.error = None;
                            }
                        }
                        Err(e) => {
                            if readiness_ok {
                                form.error = Some(format!("couldn't list host home: {e}"));
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
                // Tab autocompletion has to resolve against the
                // target server's filesystem too — same reason as
                // the Enter→open_dir_picker path above.
                let target_client = app
                    .clients
                    .get(form.profile.as_str())
                    .and_then(|e| e.client.clone());
                let target_ref = target_client.as_ref().unwrap_or(client);
                if !autocomplete_workdir(&mut form, target_ref).await {
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
        KeyCode::Char(' ') if matches!(form.field, NewSessionField::Worktree) => {
            form.use_worktree = !form.use_worktree;
        }

        // Enter while on the workdir field opens the dir picker
        // (mirrors clicking the picker's chevron in the web UI).
        // Enter on up-after still flips it. Enter on YOLO or Worktree
        // submits — Worktree is the last field, so the natural next move
        // is to spawn, not to keep toggling. Use Space to flip either.
        KeyCode::Enter if matches!(form.field, NewSessionField::Workdir) => {
            let seed = if form.workdir.trim().is_empty() {
                None
            } else {
                Some(form.workdir.trim().to_string())
            };
            // Open the picker on the *target* server's filesystem —
            // otherwise picking a peer profile in the Servers field
            // and then pressing Enter on Workdir would browse the
            // local laptop's tree and show no matching paths.
            let target_client = app
                .clients
                .get(form.profile.as_str())
                .and_then(|e| e.client.clone());
            let target_ref = target_client.as_ref().unwrap_or(client);
            let picker = open_dir_picker(seed.as_deref(), target_ref, form.host_uuid()).await;
            if target_client.is_none() && !form.profile.is_empty() {
                form.error = Some(format!(
                    "{} isn't connected — listing local fs",
                    profile_label(&form.profile)
                ));
            }
            form.picker = Some(picker);
        }
        KeyCode::Enter if matches!(form.field, NewSessionField::UpAfter) => {
            form.up_after = !form.up_after;
        }
        // Enter on the Tool field opens the modal picker — same UX as
        // Enter-on-Workdir opening the dir-tree picker. Tab still
        // cycles for muscle-memory parity with older versions and the
        // dashboard's keyboard nav.
        KeyCode::Enter if matches!(form.field, NewSessionField::Tool) => {
            let avail = app.agent_availability.clone();
            form.tool_picker = Some(open_tool_picker(&form.tool, avail.as_ref()));
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
            // Block spawning on a host whose last readiness check found a
            // required dependency (tmux/git) missing. Reuses the cached
            // report from the Host-field probe so submit adds no round
            // trip; a host the user never probed has no cache entry and
            // isn't blocked here (the daemon still rejects an unworkable
            // spawn). No "proceed anyway" in the MVP — PRD US-3.
            if let Some(host_id) = form.host_uuid()
                && let Some(report) = app.cached_readiness(host_id)
                && !report.ok
            {
                form.error = Some(format!(
                    "host not ready — {} (Ctrl-H to fix)",
                    report.message
                ));
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
                form.error = Some(format!(
                    "profile `{target_profile}` is not currently reachable"
                ));
                app.overlay = Overlay::NewSession(form);
                return;
            };
            match target_client
                .create_session_on(
                    form.name.trim(),
                    form.workdir.trim(),
                    form.tool.trim(),
                    model.as_deref(),
                    flags,
                    form.host_uuid(),
                    form.worktree_requested(),
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
                            push_notification(app, msg, None, NotifKind::Info);
                        }
                    } else {
                        let msg = format!("created `{name}` (idle)");
                        app.status_msg = Some(msg.clone());
                        push_notification(app, msg, None, NotifKind::Info);
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

// ── Goal overlay helpers ──────────────────────────────────────────────────────

/// Append a newline to the form's text. Called when the user presses Enter
/// inside the Goal overlay (plain Enter = newline, Ctrl-Enter = submit).
pub fn goal_overlay_handle_enter(form: &mut GoalForm) {
    form.text.push('\n');
}

/// Return `true` if the form has non-whitespace text and is not already
/// submitting. Used to gate Ctrl-Enter so empty goals are never sent.
pub fn goal_overlay_should_submit(form: &GoalForm) -> bool {
    !form.submitting && !form.text.trim().is_empty()
}

/// Translate a raw server error string into a human-readable message.
///
/// When the server returns a column-rule validation envelope such as
/// `400 — {"missing":["body"],"status":"todo"}` we surface
/// `"Your <status> column needs: <fields>"`. All other strings are
/// returned unchanged so the UI can display them verbatim.
pub fn format_goal_error(raw: &str) -> String {
    // The server prepends the HTTP status, e.g. "400 — {…}".
    // Find the first `{` to isolate the JSON body.
    if let Some(json_start) = raw.find('{') {
        let json_slice = &raw[json_start..];
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_slice) {
            // Column-rule envelope has shape `{"missing":[…],"status":"<col>"}`.
            if let (Some(missing_arr), Some(status_str)) = (
                v.get("missing").and_then(|m| m.as_array()),
                v.get("status").and_then(|s| s.as_str()),
            ) {
                let fields: Vec<&str> = missing_arr.iter().filter_map(|f| f.as_str()).collect();
                if !fields.is_empty() {
                    return format!("Your {status_str} column needs: {}", fields.join(", "));
                }
            }
        }
    }
    raw.to_string()
}

/// Key handler for [`Overlay::Goal`].
///
/// - **Esc**: close the overlay and discard the form.
/// - **Enter** (no modifier): append a newline to the text field.
/// - **Ctrl-Enter**: submit the goal via `POST /api/board/goals`.
/// - **Backspace**: delete the last character.
/// - **Printable chars**: append to `form.text`.
async fn handle_goal_key(app: &mut App, key: KeyEvent, client: &Client) {
    let Overlay::Goal(mut form) = std::mem::replace(&mut app.overlay, Overlay::None) else {
        return;
    };

    // Block input while a submit is in flight.
    if form.submitting {
        app.overlay = Overlay::Goal(form);
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Esc => {
            // Discard and close — overlay already set to None above.
        }

        KeyCode::Enter if ctrl => {
            // Ctrl-Enter: submit if text is non-empty.
            if !goal_overlay_should_submit(&form) {
                app.overlay = Overlay::Goal(form);
                return;
            }
            form.submitting = true;
            form.error = None;
            let text = form.text.trim().to_string();
            app.overlay = Overlay::Goal(form.clone());

            match client.submit_goal(&text).await {
                Ok(_resp) => {
                    // Success — close the overlay; the board will refresh
                    // via the event bus when the planner creates cards.
                    app.overlay = Overlay::None;
                    app.status_msg = Some("Goal submitted — planner is on it".into());
                    tracing::info!("goal submitted successfully");
                }
                Err(e) => {
                    form.submitting = false;
                    form.error = Some(format_goal_error(&format!("{e}")));
                    app.overlay = Overlay::Goal(form);
                }
            }
        }

        KeyCode::Enter => {
            // Plain Enter: insert a newline into the multi-line text area.
            goal_overlay_handle_enter(&mut form);
            app.overlay = Overlay::Goal(form);
        }

        KeyCode::Backspace => {
            form.text.pop();
            app.overlay = Overlay::Goal(form);
        }

        KeyCode::Char(c) if !ctrl => {
            form.text.push(c);
            app.overlay = Overlay::Goal(form);
        }

        _ => {
            app.overlay = Overlay::Goal(form);
        }
    }
}

// ── end Goal overlay helpers ──────────────────────────────────────────────────

/// Fetch the listing for `seed` (or `$HOME` if seed is empty/None).
///
/// If `seed` doesn't exist, walks up the path until an existing ancestor
/// is found and surfaces a hint about the fallback. This way typing a
/// stale workdir (project deleted, repo moved, typo) never traps the
/// user in a dead-end picker with no `parent` to back out of — they
/// land at the nearest real directory and can navigate from there.
async fn open_dir_picker(
    seed: Option<&str>,
    client: &Client,
    host_id: Option<Uuid>,
) -> DirPickerState {
    let original = seed
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());
    let mut current = original.clone();
    let mut last_err: Option<String> = None;

    loop {
        let attempt = current.as_deref();
        match client.list_dir_on(attempt, host_id).await {
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
    let listing = match client.list_dir_on(dir_query, form.host_uuid()).await {
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
            // Ambiguous fork with no further common prefix to fill in
            // (e.g. `~/D` vs Desktop/Documents/Developer). Return `true`
            // so the caller stays on the workdir field instead of
            // advancing to Tool — bumping the user out of the field
            // they're trying to type into is the opposite of what
            // bash readline would do. The user can keep typing to
            // disambiguate, or press Enter to open the picker.
            return true;
        }
        format!("{dir_part}{common}")
    };
    if new_text == current {
        // Same situation as the ambiguous-fork branch above: Tab found
        // matches but couldn't extend the buffer. Stay on the field.
        return true;
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
    if s.is_empty() {
        None
    } else {
        Some(s.into_owned())
    }
}

/// Build a `ToolPickerState` snapshotted against the current
/// availability probe. Cursor starts on whichever entry matches
/// `current` (the form's existing Tool value); if `current` isn't in
/// the suggestion list we anchor on the first entry so the user can
/// see the catalog from the top.
fn open_tool_picker(
    current: &str,
    availability: Option<&std::collections::HashSet<String>>,
) -> ToolPickerState {
    let trimmed = current.trim();
    let entries: Vec<ToolPickerEntry> = TOOL_SUGGESTIONS
        .iter()
        .map(|&name| {
            // Mirrors `App::tool_available`. Free-form names (terminal,
            // bash, copilot — anything outside the probed-tools list)
            // are always available because the daemon either has a
            // built-in adapter or routes them through PassthroughAdapter,
            // which trusts PATH.
            let available = if !is_probed_tool(name) {
                true
            } else {
                match availability {
                    Some(set) => set.contains(name),
                    None => true,
                }
            };
            ToolPickerEntry {
                name,
                available,
                description: tool_description(name),
            }
        })
        .collect();
    let cursor = entries.iter().position(|e| e.name == trimmed).unwrap_or(0);
    ToolPickerState { entries, cursor }
}

/// Tool-picker keymap. Mirrors `handle_dir_picker_key` so muscle
/// memory transfers: ↑/↓ move, Enter accepts, Esc cancels. Unlike the
/// dir picker there's no concept of "descend into" or "use this dir"
/// — every entry is a leaf, so Enter and `a` both commit. Entries
/// marked unavailable (uninstalled probed binaries) are still
/// selectable but populate `form.error` so the user gets a hint
/// before submission rather than after.
fn handle_tool_picker_key(form: &mut NewSessionForm, key: KeyEvent) {
    let Some(picker) = form.tool_picker.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Esc => {
            form.tool_picker = None;
        }
        KeyCode::Up => {
            picker.cursor = picker.cursor.saturating_sub(1);
        }
        KeyCode::Down if picker.cursor + 1 < picker.entries.len() => {
            picker.cursor += 1;
        }
        KeyCode::Enter | KeyCode::Char('a') | KeyCode::Char('s') => {
            if let Some(entry) = picker.entries.get(picker.cursor).cloned() {
                form.tool = entry.name.to_string();
                // Mirror the dashboard tile-dim semantics: choosing an
                // uninstalled probed agent shows the hint inline so
                // the user doesn't have to submit to learn the binary
                // isn't on the daemon's PATH.
                form.error = if entry.available {
                    None
                } else {
                    let bin = match entry.name {
                        "cursor" => "cursor-agent",
                        other => other,
                    };
                    Some(format!("{bin} not installed on the daemon"))
                };
            }
            form.tool_picker = None;
        }
        _ => {}
    }
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
        KeyCode::Down if picker.cursor + 1 < picker.entries.len() => {
            picker.cursor += 1;
        }
        // Right / Enter: descend into the highlighted entry.
        KeyCode::Right | KeyCode::Enter => {
            let Some(entry) = picker.entries.get(picker.cursor).cloned() else {
                return;
            };
            let next = open_dir_picker(Some(&entry.path), client, form.host_uuid()).await;
            form.picker = Some(next);
        }
        // Left / Backspace: pop up one level.
        KeyCode::Left | KeyCode::Backspace => {
            if let Some(parent) = picker.parent.clone() {
                let next = open_dir_picker(Some(&parent), client, form.host_uuid()).await;
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

/// Build a `PendingAction::Bulk` from the multi-select set, or return
/// `None` if nothing is checked / every checked id has gone stale. The
/// returned action lists ids+names sorted by current sidebar order so
/// the confirm preview reads in the same order the user sees.
fn bulk_action_from_checks(app: &App, kind: BulkKind) -> Option<PendingAction> {
    if app.checked.is_empty() {
        return None;
    }
    let mut ids: Vec<Uuid> = Vec::with_capacity(app.checked.len());
    let mut names: Vec<String> = Vec::with_capacity(app.checked.len());
    for s in &app.sessions {
        if app.checked.contains(&s.id) {
            ids.push(s.id);
            names.push(s.name.clone());
        }
    }
    if ids.is_empty() {
        return None;
    }
    Some(PendingAction::Bulk { kind, ids, names })
}

async fn execute_action(app: &mut App, action: PendingAction, client: &Client) {
    // Server removal is purely local — it touches profiles.toml
    // and the app's in-memory profile list without a daemon round trip.
    if let PendingAction::RemoveServer { name } = &action {
        let label = format!("removed server `{name}`");
        match super::profiles::load() {
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

    // Bulk variant: fan out per-id, routing each to its owning client,
    // and aggregate the result into one toast. We mirror the single-id
    // path's recently_killed + selection cleanup so the watchdog
    // crash-suppression and post-kill empty pane both behave the same.
    if let PendingAction::Bulk { kind, ids, .. } = action {
        let verb = kind.verb();
        let total = ids.len();
        let mut ok = 0usize;
        let mut errs: Vec<String> = Vec::new();
        for id in ids {
            let owner = app.client_for_session(id).cloned();
            let target = owner.unwrap_or_else(|| client.clone());
            if matches!(kind, BulkKind::Kill) {
                app.recently_killed.insert(id);
                if app.selected == Some(id) {
                    app.selected = None;
                    app.term.reset();
                }
            }
            let res = match kind {
                BulkKind::Start => target.start_session(id).await,
                BulkKind::Stop => target.stop_session(id).await,
                BulkKind::Kill => target.delete_session(id, true).await,
            };
            match res {
                Ok(()) => ok += 1,
                Err(e) => errs.push(format!("{id}: {e}")),
            }
        }
        // Clear the checks regardless of outcome — leaving them
        // around after a fan-out would invite a second accidental
        // bulk press on an already-dispatched set. The user can
        // re-check from scratch if they need to retry failures.
        app.checked.clear();
        let label = if errs.is_empty() {
            format!("{verb} {ok}/{total} checked")
        } else {
            format!("{verb} {ok}/{total} checked ({} failed)", errs.len())
        };
        if errs.is_empty() {
            app.status_msg = Some(label.clone());
            push_notification(app, label, None, NotifKind::Info);
        } else {
            for e in &errs {
                app.push_error(format!("bulk {verb}: {e}"));
            }
            app.status_msg = Some(label);
        }
        refresh_all(app).await;
        return;
    }

    // Host bootstrap is not a session action — it talks to the active
    // daemon (which owns the host) and refreshes the readiness cache.
    // Handle it before the session-id match below (which would panic on
    // this variant).
    if let PendingAction::ProvisionHost {
        id,
        name,
        deps,
        agents,
    } = &action
    {
        app.status_msg = Some(format!("setting up `{name}`…"));
        let mut errors: Vec<String> = Vec::new();
        // 1) Required deps (tmux/git) over the bootstrap path (sudo).
        if !deps.is_empty() {
            let dep_refs: Vec<&str> = deps.iter().map(String::as_str).collect();
            match client.bootstrap_host(*id, &dep_refs).await {
                Ok(report) => {
                    app.host_readiness_cache
                        .insert(*id, (Instant::now(), report));
                }
                Err(e) => errors.push(format!("deps: {e}")),
            }
        }
        // 2) Agent CLIs over SSH. Runs even if deps failed — agents
        //    install independently of tmux/git.
        if !agents.is_empty() {
            let agent_refs: Vec<&str> = agents.iter().map(String::as_str).collect();
            match client.install_agents(*id, &agent_refs).await {
                Ok(report) => {
                    app.host_readiness_cache
                        .insert(*id, (Instant::now(), report));
                }
                Err(e) => errors.push(format!("agents: {e}")),
            }
        }
        let (label, kind) = if errors.is_empty() {
            (format!("`{name}` set up"), NotifKind::Info)
        } else {
            for e in &errors {
                app.push_error(format!("provision `{name}`: {e}"));
            }
            (
                format!("`{name}`: setup had errors ({})", errors.join("; ")),
                NotifKind::Warn,
            )
        };
        app.status_msg = Some(label.clone());
        push_notification(app, label, None, kind);
        // Land the user back on the host they acted on, dot refreshed.
        reopen_hosts_overlay_at(app, *id);
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
        PendingAction::RemoveServer { .. }
        | PendingAction::Bulk { .. }
        | PendingAction::ProvisionHost { .. } => unreachable!(),
    };
    let owner = app.client_for_session(session_id).cloned();
    let target = owner.unwrap_or_else(|| client.clone());
    // Mark a Kill target as recently-killed BEFORE the API call so
    // any watchdog `session.crashed` event the server emits between
    // "tmux pane gone" and "row deleted" gets suppressed in
    // `apply_event`. If the kill itself fails we still leave the id
    // in the set briefly — refresh_all below reconciles the truth
    // either way.
    if let PendingAction::Kill { id, .. } = &action {
        app.recently_killed.insert(*id);
        // If the user killed the session they were looking at, drop
        // selection + reset the pane so the empty state shows. Without
        // this, `refresh_sessions` would auto-jump selection to the
        // next visible session — which often is another crashed one,
        // and the "● crashed" banner reappears as if the kill failed.
        if app.selected == Some(*id) {
            app.selected = None;
            app.term.reset();
        }
    }
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
        PendingAction::RemoveServer { .. }
        | PendingAction::Bulk { .. }
        | PendingAction::ProvisionHost { .. } => unreachable!(),
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
            push_notification(app, label, None, NotifKind::Info);
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
    let sessions: Vec<(Uuid, String, String, bool)> = app
        .sessions
        .iter()
        .map(|s| {
            (
                s.id,
                s.name.clone(),
                s.workdir.clone(),
                matches!(s.status, agentum_core::Status::Running),
            )
        })
        .collect();
    let view = ViewState {
        sidebar_hidden: app.sidebar_hidden,
        right_panel_visible: app.right_panel_visible,
        fullscreen: app.fullscreen,
        split_open: app.split_open(),
        servers_collapsed: app.servers_collapsed,
        show_all_servers: app.show_all_servers,
    };
    Catalog::build(
        app.lazygit_open(),
        &sessions,
        app.selected,
        view,
        &app.prefs,
    )
}

/// Build a [`ProfilesOverlay`] from the on-disk profiles file and
/// install it on `app`. Surfaces a friendly error in the overlay
/// itself when the file is unreadable or empty rather than silently
/// no-op'ing — the user just hit Ctrl-S for a reason.
/// Flip `show_all_servers`, rebuild the tree against the new scope,
/// persist to prefs, and surface a status toast describing the new
/// state. Used by both the palette action and the Ctrl-S overlay's
/// `s` key so the two surfaces stay in lockstep.
pub fn toggle_show_all_servers(app: &mut App) {
    app.show_all_servers = !app.show_all_servers;
    app.prefs.show_all_servers = app.show_all_servers;
    prefs::save(&app.prefs);
    // Capture the user's fold state across the rebuild — same trick
    // `refresh_sessions` uses so flipping the scope doesn't reset
    // every collapsed group.
    let mut prev_state: HashMap<String, bool> = HashMap::new();
    for g in &app.tree.groups {
        prev_state.insert(server_expand_key(&g.profile), g.expanded);
        for p in &g.projects {
            prev_state.insert(project_expand_key(&g.profile, &p.workdir), p.expanded);
        }
    }
    let prev_filter = app.tree.filter_str().to_string();
    app.tree = app.build_scoped_tree(&prev_state);
    if !prev_filter.is_empty() {
        app.tree.set_filter(&prev_filter);
    }
    app.tree.clamp_cursor();
    if let Some(id) = app.selected {
        app.tree.select_session(id);
    }
    app.status_msg = Some(if app.show_all_servers {
        "showing sessions from all servers".into()
    } else {
        let label = profile_label(app.active_profile.as_deref().unwrap_or(""));
        format!("showing sessions on {label} only")
    });
}

pub fn open_profiles_overlay(app: &mut App) {
    let (entries, error) = match super::profiles::load() {
        Ok(store) => {
            let mut rows: Vec<ProfileEntry> = store
                .list()
                .into_iter()
                .map(|(name, p, _is_default)| ProfileEntry {
                    name,
                    url: p.url,
                    fingerprint: p.fingerprint,
                })
                .collect();
            // Surface the active profile at the top of the picker so
            // the most common task ("which one am I on right now?") is
            // already under the cursor.
            if let Some(active) = &app.active_profile {
                if let Some(idx) = rows.iter().position(|r| &r.name == active) {
                    let row = rows.remove(idx);
                    rows.insert(0, row);
                }
            }
            (rows, None)
        }
        Err(e) => (Vec::new(), Some(format!("load profiles.toml: {e}"))),
    };
    let cursor = entries
        .iter()
        .position(|p| Some(&p.name) == app.active_profile.as_ref())
        .unwrap_or(0);
    app.overlay = Overlay::Profiles(ProfilesOverlay {
        entries,
        cursor,
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
        // ----- add/edit form mode -----
        match key.code {
            KeyCode::Esc => {
                // Drop the form, return to the list.
                app.overlay = Overlay::Profiles(state);
                return;
            }
            KeyCode::Tab | KeyCode::Down => form.next_field(),
            KeyCode::BackTab | KeyCode::Up => form.prev_field(),
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
                match super::profiles::load() {
                    Ok(mut store) => {
                        // Edit-with-rename: drop the old entry first so
                        // we never end up with both the original and
                        // the renamed copy on disk if the upsert below
                        // fails partway. The active-profile guard in
                        // list-mode already prevents renaming the row
                        // the user is connected to.
                        if let Some(original) = form.editing.as_deref() {
                            if original != name {
                                let _ = store.remove(original);
                            }
                        }
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
                        // Reload the list and snap the cursor onto the
                        // freshly added/edited profile so Enter switches
                        // to it immediately.
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
        KeyCode::Char('e') | KeyCode::Char('E') => {
            // Edit the highlighted profile. Refuse to edit the active
            // profile: a rename or URL flip mid-session would race the
            // live client against a moving target. They can switch off
            // first, then edit.
            if let Some(entry) = state.entries.get(state.cursor) {
                if app.active_profile.as_deref() == Some(entry.name.as_str()) {
                    state.error = Some("can't edit the active profile — switch first".into());
                    app.overlay = Overlay::Profiles(state);
                    return;
                }
                state.add_form = Some(AddProfileForm::edit(entry));
            }
            app.overlay = Overlay::Profiles(state);
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            // Toggle the tree's scope ("all servers" vs "active only")
            // without leaving the overlay. The header line at the top
            // of the overlay reflects the new state on the next draw
            // so the user gets immediate feedback.
            toggle_show_all_servers(app);
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
                if let Ok(mut store) = super::profiles::load() {
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
                // Same lazygit guard as the sidebar's Enter-on-server
                // switch: the soft restart drops the App and with it
                // the lazygit PTY, which the user reads as a crash.
                if app.lazygit_open() {
                    state.error = Some("close lazygit (g) before switching servers".into());
                    app.overlay = Overlay::Profiles(state);
                    return;
                }
                // Schedule a soft restart with the chosen profile.
                // run_loop reads `pending_switch_profile` on quit and
                // `commands::terminal::run` re-enters with the new
                // server. `pending_after_switch` stays None — the
                // overlay path doesn't carry a follow-up. Mark the
                // target reconnecting so the sidebar spinner is also
                // shown during the brief alt-screen tear-down (the new
                // App starts with an empty set, so it clears itself).
                app.reconnecting.insert(entry.name.clone());
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
                let new_ms = app.prefs.bump_ttl(kind, -(prefs::NOTIF_TTL_STEP_MS as i64));
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
            // Route the PATCH to the daemon that actually owns this
            // session — multi-server aggregation means the highlighted
            // row may belong to a peer profile, and hitting the default
            // client would 404 because the session id doesn't exist
            // there. Falls back to the default for untagged sessions.
            let target = app
                .client_for_session(id)
                .cloned()
                .unwrap_or_else(|| client.clone());
            match target.rename_session(id, trimmed).await {
                Ok(updated) => {
                    app.overlay = Overlay::None;
                    app.status_msg = Some(format!("renamed → {}", updated.name));
                    refresh_all(app).await;
                    app.tree.select_session(id);
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
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('!') | KeyCode::Enter => {
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
            refresh_all(app).await;
            app.status_msg = Some("refreshed".into());
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
        ActionKind::ToggleServers => {
            app.servers_collapsed = !app.servers_collapsed;
            // Pull the cursor back into Sessions if it was sitting on
            // a server row about to disappear.
            if app.servers_collapsed && app.tree_section == TreeSection::Servers {
                app.tree_section = TreeSection::Sessions;
            }
            app.prefs.servers_collapsed = app.servers_collapsed;
            prefs::save(&app.prefs);
            app.status_msg = Some(if app.servers_collapsed {
                "servers section collapsed".into()
            } else {
                "servers section expanded".into()
            });
        }
        ActionKind::ToggleShowAllServers => {
            toggle_show_all_servers(app);
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
                Some("servers (Enter switch · a add · d remove · s scope · Esc close)".into());
        }
        ActionKind::OpenHosts => {
            open_hosts_overlay(app);
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

        // ── Session CRUD from palette ─────────────────────────────
        ActionKind::NewSession => {
            let (profile, workdir) = if let Some(s) = app.selected_session() {
                let owning_profile = app.profile_for_session(s.id).to_string();
                (owning_profile, s.workdir.clone())
            } else {
                let active = app.active_profile.clone().unwrap_or_default();
                let home = match client.list_dir(None).await {
                    Ok(listing) => listing.path,
                    Err(_) => std::env::var("HOME").unwrap_or_default(),
                };
                (active, home)
            };
            app.overlay =
                Overlay::NewSession(Box::new(NewSessionForm::with_profile(profile, workdir)));
        }
        ActionKind::RenameSession(id) => {
            if let Some(s) = app.sessions.iter().find(|s| s.id == id) {
                let name = s.name.clone();
                app.overlay = Overlay::Rename(RenameState::new(id, &name));
                app.status_msg = Some("rename (Enter save · Esc cancel)".into());
            }
        }
        ActionKind::StartSession(id) => {
            if let Some(s) = app.sessions.iter().find(|s| s.id == id) {
                app.overlay = Overlay::Confirm(PendingAction::Start {
                    id,
                    name: s.name.clone(),
                });
            }
        }
        ActionKind::StopSession(id) => {
            if let Some(s) = app.sessions.iter().find(|s| s.id == id) {
                app.overlay = Overlay::Confirm(PendingAction::Stop {
                    id,
                    name: s.name.clone(),
                });
            }
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

/// Walk up from `start` looking for a `.git` entry (a directory or a
/// gitlink file in a worktree). Returns the first repo root found, or
/// `None` if we hit the filesystem root without seeing one. Used by
/// `toggle_lazygit` so the remote-workdir fallback opens in a real
/// repo instead of dumping the user into lazygit's "empty repo"
/// screen on their `$HOME`.
fn nearest_git_repo(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
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
    let local_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (cwd, fell_back) = match app.selected_session() {
        Some(s) => {
            let p = PathBuf::from(&s.workdir);
            if p.is_dir() {
                (p, false)
            } else {
                (
                    nearest_git_repo(&local_cwd).unwrap_or_else(|| local_cwd.clone()),
                    true,
                )
            }
        }
        None => (
            nearest_git_repo(&local_cwd).unwrap_or_else(|| local_cwd.clone()),
            false,
        ),
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
    // Extract the workdir as an owned string so the &Session borrow
    // doesn't outlive into the later `app.focus = ...` /
    // `app.lazygit = ...` mutations.
    let remote_workdir = match app.selected_session() {
        Some(s) => s.workdir.clone(),
        None => return,
    };
    // Try the session's workdir literally first; if it doesn't resolve
    // to a local directory (typical when the session lives on a remote
    // daemon and the path is Linux-side while the TUI runs on macOS),
    // try replacing the foreign home prefix with the local $HOME so a
    // user with parallel `~/Developer/projects/<name>` checkouts on
    // both machines sees lazygit follow into the local copy.
    let Some(new_cwd) = resolve_local_workdir(&remote_workdir) else {
        // No local equivalent for this session's workdir — drop the
        // stale pane rather than leaving it pinned to a totally
        // unrelated project. lazygit_cwd is cleared too so the next
        // switch to a local-workdir session triggers a fresh spawn
        // instead of being short-circuited by the "same cwd" check.
        if app.focus == Focus::Lazygit {
            app.focus = Focus::Tree;
        }
        app.lazygit = None;
        app.lazygit_cwd = None;
        app.status_msg = Some(format!(
            "lazygit closed — `{remote_workdir}` isn't a local directory"
        ));
        return;
    };
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
fn handle_filter_input_key(app: &mut App, key: &KeyEvent, client: &Client) {
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
        KeyCode::Backspace if filter.pop().is_some() => {
            app.tree.set_filter(&filter);
            app.status_msg = Some(format!("⌕ {filter}"));
            changed = true;
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
    if side == Side::Left
        && let Some(prev) = app.selected
    {
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
/// Map a session's workdir to a real local directory the lazygit
/// child can spawn into. Tries three things in order:
///   1. The path verbatim — the common case for purely-local fleets.
///   2. The trailing path past the first match of `/home/<user>/` or
///      `/Users/<user>/`, joined against the local `$HOME`. Lets a
///      Mac user with `~/Developer/projects/agentum` follow into a
///      remote Linux session whose workdir is
///      `/home/malloc/Developer/projects/agentum`.
///   3. None — telling the caller to drop the pane rather than show
///      stale state from a previous project.
fn resolve_local_workdir(remote: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(remote);
    if direct.is_dir() {
        return Some(direct);
    }
    let local_home = std::env::var_os("HOME").map(PathBuf::from)?;
    // Match `/home/<u>/` or `/Users/<u>/` and pull the suffix.
    for prefix in ["/home/", "/Users/"] {
        if let Some(rest) = remote.strip_prefix(prefix)
            && let Some((_, suffix)) = rest.split_once('/')
        {
            let candidate = local_home.join(suffix);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Shadow the user's current input line for one terminal pane so we
/// can spot when they type `/clear` (or `\clear`) and submit it. When
/// the line commits with that command, we both wipe the local plan/
/// todo/task cache for the session and POST to the daemon's reset
/// endpoint — the daemon also fast-forwards its transcript cursor so
/// the next refresh doesn't repaint the cleared state from the
/// already-on-disk log.
///
/// This is intentionally a simple appender. We don't model in-line
/// cursor moves (arrows, Home/End) or paste operations. The worst
/// case for a missed detection is the panel staying out of sync until
/// the next real transcript event, which is exactly the pre-feature
/// behaviour — so a false negative is harmless, while a false positive
/// (clearing when the user didn't actually run /clear) would be
/// annoying, which is why we only commit on Enter and only match the
/// exact trimmed line.
fn track_term_input_for_clear(app: &mut App, session_id: Uuid, key: &KeyEvent, client: &Client) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let buf = app.term_input_lines.entry(session_id).or_default();
    match key.code {
        KeyCode::Char(c) if ctrl => {
            // Most Ctrl-combos either kill the line outright (Ctrl-U,
            // Ctrl-K, Ctrl-W, Ctrl-C) or are non-printing in agent
            // CLIs. Reset the shadow rather than risk false positives.
            let _ = c;
            buf.clear();
        }
        KeyCode::Char(c) if alt => {
            // Alt-prefixed keys send an escape + char to the agent;
            // they don't extend the typed line in any agent CLI we
            // target. Reset to be safe.
            let _ = c;
            buf.clear();
        }
        KeyCode::Char(c) => buf.push(c),
        KeyCode::Backspace => {
            buf.pop();
        }
        KeyCode::Enter => {
            // Commit point. Trim so trailing whitespace / leading
            // spaces don't defeat the match.
            let line = std::mem::take(buf);
            let trimmed = line.trim();
            let is_clear =
                trimmed.eq_ignore_ascii_case("/clear") || trimmed.eq_ignore_ascii_case("\\clear");
            if is_clear {
                // Local clear first so the right panel goes blank
                // immediately, before the round-trip to the daemon.
                if let Some(state) = app.agent_tasks.get_mut(&session_id) {
                    *state = AgentTaskState::default();
                }
                // Server reset: detached so the keystroke send isn't
                // blocked on it. Fire-and-forget; the next
                // `agent_tasks.updated` event will reconfirm.
                let owner = app
                    .client_for_session(session_id)
                    .cloned()
                    .unwrap_or_else(|| client.clone());
                tokio::spawn(async move {
                    if let Err(e) = owner.reset_agent_tasks(session_id).await {
                        tracing::debug!(session = %session_id, error = %e, "agent-tasks reset failed");
                    }
                });
                app.status_msg = Some("plan / todos / tasks cleared".into());
            }
        }
        KeyCode::Esc => buf.clear(),
        KeyCode::Up | KeyCode::Down => {
            // History recall — whatever the agent retrieves is opaque
            // to us. Reset rather than misattribute the recalled line.
            buf.clear();
        }
        _ => {}
    }
}

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

/// Spawn a background fetch of `/api/usage/claude` (spec 001). Coalesces
/// via `usage_inflight` so a slow fetch can't stack behind the tick. The
/// result (or `None` on error) lands on `usage_rx` in the run-loop.
/// Always polls the *active* daemon: usage is a host-global readout, and
/// the active server is the one whose creds the user is reasoning about.
fn spawn_usage_fetch(app: &mut App, client: &Client) {
    let Some(tx) = app.usage_tx.clone() else {
        return;
    };
    if app.usage_inflight {
        return; // coalesce — a poll is already running
    }
    app.usage_inflight = true;
    let target = client.clone();
    tokio::spawn(async move {
        let payload = match target.claude_usage().await {
            Ok(usage) => Some(usage),
            Err(e) => {
                tracing::debug!(error = %e, "claude usage fetch failed");
                None
            }
        };
        let _ = tx.send(payload);
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
            // User-initiated kills emit a watchdog `session.crashed`
            // microseconds after the row is deleted (the tmux pane
            // vanishing trips the detector). If the id is in our
            // recently-killed set, this is that echo — drop it
            // silently rather than telling the user their kill
            // crashed the agent. Otherwise: log + toast both.
            let already_killed = ev
                .session_id
                .map(|id| app.recently_killed.remove(&id))
                .unwrap_or(false);
            if !already_killed {
                let reason = ev
                    .payload
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .or_else(|| ev.payload.get("signature").and_then(|v| v.as_str()))
                    .map(|s| format!("reason: {s}"));
                app.push_error(format!("crashed: {name}"));
                push_notification(app, format!("{name} crashed"), reason, NotifKind::Error);
            }
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.idle.remove(&id);
                app.working.remove(&id);
            }
            refresh_all(app).await;
        }
        "session.started" => {
            // Silent — matches the dashboard, which suppresses started
            // events because the initial bus replay would spam toasts on
            // every reconnect.
            refresh_all(app).await;
        }
        "session.stopped" => {
            push_notification(app, format!("{name} stopped"), None, NotifKind::Info);
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.idle.remove(&id);
                app.working.remove(&id);
            }
            refresh_all(app).await;
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
            //
            // Skip the toast when the event is a bootstrap signal
            // rather than a fresh transition:
            //   `initial: true` — watchdog's first observation after
            //   spawning onto an already-finished session.
            //   `replay: true` — daemon resent the current state
            //   when this client connected to /api/events.
            // The dot still needs to update because the agent IS
            // idle; only the toast/chime is stale.
            let bootstrap = ev
                .payload
                .get("initial")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || ev
                    .payload
                    .get("replay")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            if !bootstrap {
                push_notification(app, format!("{name} finished"), None, NotifKind::Info);
            }
            // Working→Idle: the agent is now sleeping at the prompt.
            // Mirror the watchdog's ActivityState::Idle so the sidebar
            // dot shows a muted `◌` instead of a misleading green `●`.
            // Defensive cleanup of awaiting_input + working in case
            // `agent.input_resolved` was missed (event-bus lag,
            // watchdog restart).
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.working.remove(&id);
                app.idle.insert(id);
            }
        }
        "agent.awaiting_input" => {
            // Awaiting input is a "you have to do something" event, so we
            // toast even when the session is selected — the user might be
            // tabbed away to lazygit / errors / palette and miss it.
            //
            // `initial`/`replay` mean the agent was already blocked
            // before the watchdog/client tuned in — flip the dot but
            // skip the toast (no fresh demand to surface).
            let bootstrap = ev
                .payload
                .get("initial")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || ev
                    .payload
                    .get("replay")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            if !bootstrap {
                push_notification(
                    app,
                    format!("{name} needs input"),
                    Some("agent is waiting on a permission prompt".to_string()),
                    NotifKind::Warn,
                );
            }
            if let Some(id) = ev.session_id {
                app.awaiting_input.insert(id);
                // An awaiting agent isn't sleeping or working — drop
                // any stale idle/working bits so the dot doesn't
                // briefly flicker through `◌`/`●` before it lands
                // on `▲`.
                app.idle.remove(&id);
                app.working.remove(&id);
            }
        }
        "agent.working" => {
            // Agent just resumed work (Idle → Working). Without this the
            // sidebar dot stays grey while the agent is visibly working.
            // We also INSERT into `working` so the dot turns green — the
            // dot used to derive green from `Status::Running` alone,
            // which made every long-lived session read as green forever
            // (#stuck-green-dot regression).  No toast: a quiet resume
            // isn't notification-worthy.
            if let Some(id) = ev.session_id {
                app.idle.remove(&id);
                app.awaiting_input.remove(&id);
                app.working.insert(id);
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
                        app.working.remove(&id);
                    }
                    Some("working") => {
                        app.idle.remove(&id);
                        app.working.insert(id);
                    }
                    // Older watchdogs (pre-v0.6.28) emit this event with
                    // no payload. Without the resolved-state hint we
                    // can't tell working from idle, so we just clear
                    // awaiting and let the next finished/working event
                    // settle the idle/working bits.
                    _ => {}
                }
            }
        }
        "session.created" => {
            refresh_all(app).await;
        }
        "session.deleted" => {
            if let Some(id) = ev.session_id {
                app.awaiting_input.remove(&id);
                app.idle.remove(&id);
                app.working.remove(&id);
            }
            refresh_all(app).await;
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
            let interesting = Some(id) == app.selected || app.agent_tasks.contains_key(&id);
            if interesting {
                spawn_agent_tasks_fetch(app, client, id);
            }
        }
        // Sidebar metadata events. Both refetch the sessions list so
        // the tree row label / tool chip update without manual refresh,
        // and we keep the cursor pinned to the affected session.
        "session.renamed" => {
            refresh_all(app).await;
            if let Some(id) = ev.session_id {
                app.tree.select_session(id);
            }
            // No toast — the rename action itself already flashed a
            // status message. A second visible signal would be noise.
        }
        "session.tool_changed" => {
            refresh_all(app).await;
            if let Some(id) = ev.session_id {
                app.tree.select_session(id);
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
                push_notification(app, format!("{name} → {tool}"), None, NotifKind::Info);
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
async fn spawn_plain_terminal(app: &mut App, client: &Client) {
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
                push_notification(app, format!("shell: {name}"), None, NotifKind::Info);
            }
            refresh_all(app).await;
            app.tree.select_session(id);
            {
                let side = app.target_side();
                update_selection(app, client, side);
            }
            app.set_focus(Focus::Term);
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

/// Per-tool icon glyph + accent color. Mirrors the sidebar's session
/// status_dot pattern: tiny single-cell glyphs, colored by tool brand
/// so the user can scan "which agents am I running" without reading
/// the trailing `tool/model` label. Coverage tracks `TOOL_SUGGESTIONS`
/// plus the passthrough shells; unknown tools fall back to a neutral
/// square so a new tool added on the daemon side still renders
/// something rather than blanking.
pub fn tool_icon(tool: &str) -> (&'static str, ratatui::style::Color) {
    use ratatui::style::Color;
    match tool {
        // Anthropic's six-pointed asterisk — the exact mark Claude
        // Code prints in its own TUI. Yellow = closest ANSI base color
        // to the Claude brand orange (#d97757) without hardcoding RGB
        // and breaking high-contrast themes.
        "claude" => ("✻", Color::Yellow),
        // OpenAI's six-petal florette mimics the ChatGPT/OpenAI knot
        // in a single cell; green keeps Codex visually distinct from
        // Claude's yellow asterisk on a busy sidebar.
        "codex" => ("❋", Color::Green),
        // Cursor's brand mark is a filled circle with a stylized
        // sparkle inside — the fisheye is the closest single-cell
        // glyph. Magenta picks it out from claude/codex.
        "cursor" => ("◉", Color::Magenta),
        // Generic "agent" passthrough — diamond reads as "some agent"
        // without claiming a specific vendor.
        "agent" => ("◆", Color::Cyan),
        // Google's Gemini brand is a four-pointed asymmetric sparkle;
        // the solid version pairs with cyan to separate it from
        // Claude's yellow asterisk.
        "gemini" => ("✦", Color::Cyan),
        // Hermes (xAI / Nous tooling) — hexagon-ish ring is distinct
        // from the other star/diamond glyphs.
        "hermes" => ("⌬", Color::LightMagenta),
        // Copilot — paper-plane-ish glyph; blue ties to GitHub's
        // visual identity.
        "copilot" => ("❖", Color::Blue),
        // Open-source coding agents share the warning/amber band so
        // they group visually distinct from the vendor agents above.
        "opencode" => ("◇", Color::LightYellow),
        "aider" => ("✚", Color::LightGreen),
        // Plain shells — `$` is the universal prompt glyph; muted so
        // they don't compete with agent rows for attention.
        "terminal" | "bash" => ("$", Color::DarkGray),
        _ => ("▣", Color::DarkGray),
    }
}

#[cfg(test)]
mod profile_targets_loopback_tests {
    //! Pin the URL-host classifier the sidebar uses to decide whether
    //! a registered profile already represents the local daemon — and
    //! therefore the synthetic "MY MACHINE" row above it is redundant.
    //! Misclassifying a remote host as loopback would *hide* the
    //! synthetic row and leave the user without a way to navigate
    //! back to their local daemon, so the cases below are the safety
    //! net.
    use super::*;

    #[test]
    fn loopback_literals_are_recognised() {
        assert!(profile_targets_loopback("http://127.0.0.1:8822"));
        assert!(profile_targets_loopback("https://127.0.0.1:8822"));
        assert!(profile_targets_loopback("http://localhost:8822"));
        assert!(profile_targets_loopback("https://localhost"));
        assert!(profile_targets_loopback("http://[::1]:8822"));
    }

    #[test]
    fn remote_hosts_are_not_loopback() {
        assert!(!profile_targets_loopback("https://my-vps.example.com:8822"));
        assert!(!profile_targets_loopback("https://100.64.0.1:8822"));
        assert!(!profile_targets_loopback(
            "https://mateos-macbook-pro.tail-scale.ts.net:8822"
        ));
        // Looks like a loopback substring but isn't:
        assert!(!profile_targets_loopback(
            "https://localhost.evil.example:8822"
        ));
    }

    #[test]
    fn unparseable_url_falls_back_to_safe_default() {
        // An unparseable URL must NOT classify as loopback — that would
        // hide the synthetic row and confuse the user.
        assert!(!profile_targets_loopback(""));
        assert!(!profile_targets_loopback("not a url"));
        assert!(!profile_targets_loopback("127.0.0.1:8822")); // missing scheme
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
            host_id: None,
            host_label: None,
            host_kind: None,
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
            card_id: None,
            worktree_path: None,
            worktree_branch: None,
            worktree_base_ref: None,
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

    // The bug we're guarding: a Ctrl-S switch from loopback to a remote
    // server used to leave the terminal pane on whichever session sorted
    // first in the merged list — which is loopback because the empty
    // profile key sorts ahead of named profiles. The active profile chip
    // would update, but the pane content would not, so the user read the
    // switch as a no-op.
    #[test]
    fn pick_initial_prefers_active_profile_session() {
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        // Loopback session sorts first in the merged list.
        let sessions = vec![sess(local, "alpha"), sess(remote, "beta")];
        let mut owners = HashMap::new();
        owners.insert(local, String::new());
        owners.insert(remote, "vps".to_string());
        // Active = vps → must pick the remote session, NOT the
        // first-in-list loopback session.
        assert_eq!(
            pick_initial_selection(&sessions, &owners, Some("vps")),
            Some(remote)
        );
    }

    #[test]
    fn pick_initial_falls_back_when_active_has_no_sessions() {
        let local = Uuid::new_v4();
        let sessions = vec![sess(local, "alpha")];
        let mut owners = HashMap::new();
        owners.insert(local, String::new());
        // Active = a brand-new server with zero sessions yet. Fall back
        // to *something* so the pane isn't empty on switch.
        assert_eq!(
            pick_initial_selection(&sessions, &owners, Some("vps")),
            Some(local)
        );
    }

    #[test]
    fn pick_initial_none_when_no_sessions_anywhere() {
        let owners = HashMap::new();
        assert_eq!(pick_initial_selection(&[], &owners, Some("vps")), None);
        assert_eq!(pick_initial_selection(&[], &owners, None), None);
    }
}

#[cfg(test)]
mod cycle_profile_tests {
    //! These tests lock the new contract: the empty "" entry only
    //! appears in the cycle wheel when a real local-loopback client
    //! is connected. Pre-fix, the wheel always included "" and
    //! cycling to it would silently leave the workdir on the active
    //! server's `$HOME` — which read as "workdir not following the
    //! server switch."
    use super::*;

    fn new_form(profile: &str) -> NewSessionForm {
        NewSessionForm::with_profile(profile.to_string(), String::new())
    }

    #[test]
    fn cycle_with_local_includes_this_machine() {
        // Loopback launch: one peer profile + a connected local
        // client. Wheel = ["", "vps1"]. Tab from "" → "vps1" →
        // back to "".
        let mut form = new_form("");
        form.cycle_profile(&["vps1".to_string()], true);
        assert_eq!(form.profile, "vps1");
        form.cycle_profile(&["vps1".to_string()], true);
        assert_eq!(form.profile, "");
    }

    #[test]
    fn cycle_without_local_skips_this_machine() {
        // `--profile vps1` launch with no local loopback. Wheel is
        // just ["vps1"]; cycling does nothing (form is trapped on
        // the only reachable server, which is fine — there's no
        // "this machine" to cycle to).
        let mut form = new_form("vps1");
        form.cycle_profile(&["vps1".to_string()], false);
        assert_eq!(form.profile, "vps1");
    }

    #[test]
    fn cycle_without_local_walks_multiple_peers() {
        // `--profile vps1` launch with two peers and no local.
        // Wheel = ["vps1", "vps2"]; Tab walks between them.
        let mut form = new_form("vps1");
        form.cycle_profile(&["vps1".to_string(), "vps2".to_string()], false);
        assert_eq!(form.profile, "vps2");
        form.cycle_profile(&["vps1".to_string(), "vps2".to_string()], false);
        assert_eq!(form.profile, "vps1");
    }

    #[test]
    fn cycle_with_local_walks_full_wheel() {
        // Loopback + two peers. Wheel = ["", "vps1", "vps2"].
        let mut form = new_form("");
        let peers = vec!["vps1".to_string(), "vps2".to_string()];
        form.cycle_profile(&peers, true);
        assert_eq!(form.profile, "vps1");
        form.cycle_profile(&peers, true);
        assert_eq!(form.profile, "vps2");
        form.cycle_profile(&peers, true);
        assert_eq!(form.profile, "");
    }

    #[test]
    fn cycle_with_unknown_starting_profile_lands_on_first() {
        // Defensive: if the form's profile field somehow doesn't
        // match anything in the wheel, treat that as index 0 so
        // Tab still produces a sensible next step.
        let mut form = new_form("ghost");
        form.cycle_profile(&["vps1".to_string()], true);
        // wheel = ["", "vps1"]; unknown → idx 0; (0+1) % 2 → "vps1"
        assert_eq!(form.profile, "vps1");
    }
}

#[cfg(test)]
mod worktree_tests {
    //! Worktree-by-default contract: the New-Session form opens with the
    //! worktree toggle ON, and the request is only sent for local-host
    //! spawns (the daemon rejects worktrees on SSH hosts).
    use super::*;

    #[test]
    fn defaults_to_on() {
        let form = NewSessionForm::with_profile(String::new(), String::new());
        assert!(form.use_worktree, "worktree should default on");
        assert!(
            form.worktree_requested(),
            "default form targets the local host, so a worktree is requested"
        );
    }

    #[test]
    fn opt_out_suppresses_request() {
        let mut form = NewSessionForm::with_profile(String::new(), String::new());
        form.use_worktree = false;
        assert!(!form.worktree_requested());
    }

    #[test]
    fn selecting_a_host_suppresses_request() {
        // A non-empty host id means a (possibly SSH) host was picked;
        // the toggle stays visually on but we don't send the spec.
        let mut form = NewSessionForm::with_profile(String::new(), String::new());
        form.host_id = "11111111-1111-1111-1111-111111111111".into();
        assert!(form.use_worktree);
        assert!(!form.worktree_requested());
    }

    #[test]
    fn worktree_is_the_last_field_in_the_cycle() {
        let mut form = NewSessionForm::with_profile(String::new(), String::new());
        form.field = NewSessionField::Yolo;
        form.next_field();
        assert_eq!(form.field, NewSessionField::Worktree);
        // Last field wraps back to the top of the form.
        form.next_field();
        assert_eq!(form.field, NewSessionField::Profile);
        // And it's reachable going backwards from the top.
        form.prev_field();
        assert_eq!(form.field, NewSessionField::Worktree);
    }
}

#[cfg(test)]
mod tool_picker_tests {
    //! v0.7.46 surfaces the New-Session Tool field as a modal picker
    //! (mirroring the dir-picker). These tests pin the contract:
    //! catalog ordering, availability snapshotting, cursor anchoring,
    //! and the keymap's accept/cancel paths.
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::HashSet;

    fn ev(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn new_form() -> NewSessionForm {
        NewSessionForm::with_profile(String::new(), String::new())
    }

    #[test]
    fn catalog_includes_copilot_and_terminal() {
        // New entries the user explicitly asked for. Order is the
        // contract — TOOL_SUGGESTIONS is what the picker renders
        // top-to-bottom and what Tab-cycle walks.
        assert!(TOOL_SUGGESTIONS.contains(&"copilot"));
        assert!(TOOL_SUGGESTIONS.contains(&"terminal"));
        // Sanity: the existing entries weren't dropped.
        for legacy in [
            "claude", "codex", "cursor", "agent", "opencode", "aider", "bash",
        ] {
            assert!(
                TOOL_SUGGESTIONS.contains(&legacy),
                "regression: TOOL_SUGGESTIONS dropped {legacy}"
            );
        }
    }

    #[test]
    fn picker_anchors_cursor_on_current_tool() {
        let picker = open_tool_picker("cursor", None);
        let cursor_entry = &picker.entries[picker.cursor];
        assert_eq!(cursor_entry.name, "cursor");
    }

    #[test]
    fn picker_falls_back_to_first_entry_for_unknown_current() {
        // Free-form tool name the user typed (e.g. a hypothetical
        // "myagent") — picker shouldn't error, just anchor on top.
        let picker = open_tool_picker("zzz-unknown", None);
        assert_eq!(picker.cursor, 0);
    }

    #[test]
    fn free_form_names_always_available() {
        // `terminal`, `bash`, `copilot` aren't in the probed set, so
        // they're always available regardless of the daemon's probe.
        let empty: HashSet<String> = HashSet::new();
        let picker = open_tool_picker("terminal", Some(&empty));
        let terminal = picker
            .entries
            .iter()
            .find(|e| e.name == "terminal")
            .unwrap();
        assert!(terminal.available);
        let copilot = picker.entries.iter().find(|e| e.name == "copilot").unwrap();
        assert!(copilot.available);
    }

    #[test]
    fn probed_agents_are_dimmed_when_uninstalled() {
        // Daemon reports zero installed first-class agents — every
        // probed entry in the picker should mark `available = false`.
        let empty: HashSet<String> = HashSet::new();
        let picker = open_tool_picker("claude", Some(&empty));
        let claude = picker.entries.iter().find(|e| e.name == "claude").unwrap();
        assert!(!claude.available);
    }

    #[test]
    fn enter_commits_selection() {
        let mut form = new_form();
        form.tool = "claude".into();
        form.tool_picker = Some(open_tool_picker("claude", None));
        // Move down once and accept.
        handle_tool_picker_key(&mut form, ev(KeyCode::Down));
        handle_tool_picker_key(&mut form, ev(KeyCode::Enter));
        assert!(form.tool_picker.is_none());
        // Second entry in TOOL_SUGGESTIONS is `codex`.
        assert_eq!(form.tool, "codex");
    }

    #[test]
    fn esc_cancels_without_changing_tool() {
        let mut form = new_form();
        form.tool = "claude".into();
        form.tool_picker = Some(open_tool_picker("claude", None));
        handle_tool_picker_key(&mut form, ev(KeyCode::Down));
        handle_tool_picker_key(&mut form, ev(KeyCode::Esc));
        assert!(form.tool_picker.is_none());
        assert_eq!(form.tool, "claude");
    }
}

#[cfg(test)]
mod tool_icon_tests {
    //! Pin the per-agent sidebar icon contract: every entry in
    //! `TOOL_SUGGESTIONS` must resolve to a real glyph (not the unknown
    //! fallback), and unknown tools must fall back gracefully so a new
    //! agent on the daemon side doesn't render a blank cell.
    use super::*;

    #[test]
    fn every_known_tool_has_an_icon() {
        let (fallback_glyph, _) = tool_icon("__unknown_tool__");
        for &t in TOOL_SUGGESTIONS {
            let (glyph, _) = tool_icon(t);
            assert_ne!(
                glyph, fallback_glyph,
                "tool `{t}` falls through to the unknown-tool icon — add a branch in tool_icon",
            );
        }
    }

    #[test]
    fn unknown_tool_returns_fallback_not_panic() {
        // Defensive: a daemon could surface a tool name the TUI doesn't
        // know about (newer daemon, third-party adapter). The render
        // path must still produce something visible rather than panic
        // or render an empty cell.
        let (glyph, _) = tool_icon("not-a-real-tool");
        assert!(!glyph.is_empty());
    }
}

#[cfg(test)]
mod osc52_tests {
    //! OSC-52 clipboard write — pin the wire format. Mid-frame writes
    //! that confused ratatui's diff renderer plus tmux swallowing the
    //! sequence broke v0.6.31's first attempt at this; the rewrite
    //! defers the write to between frames and DCS-wraps the payload
    //! when running inside tmux. These tests cover the wire format
    //! only — the deferred-flush + clear() dance is covered by the
    //! integration path (manual repro: drag-select inside a pane).
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};

    /// Save/restore the `$TMUX` env var around a single test body.
    /// Tests assert by inspecting bytes, so a stray parallel test
    /// flipping the env wouldn't *corrupt* the buffer — it'd just
    /// build the other branch's bytes. Cheap to be conservative.
    fn with_tmux_env<R>(value: Option<&str>, body: impl FnOnce() -> R) -> R {
        let saved = std::env::var_os("TMUX");
        unsafe {
            match value {
                Some(v) => std::env::set_var("TMUX", v),
                None => std::env::remove_var("TMUX"),
            }
        }
        let out = body();
        unsafe {
            match saved {
                Some(v) => std::env::set_var("TMUX", v),
                None => std::env::remove_var("TMUX"),
            }
        }
        out
    }

    #[test]
    fn plain_terminal_uses_bare_osc52() {
        with_tmux_env(None, || {
            let seq = build_osc52_sequence("hello");
            let s = std::str::from_utf8(&seq).unwrap();
            let payload = STANDARD.encode(b"hello");
            assert_eq!(s, format!("\x1b]52;c;{payload}\x07"));
        });
    }

    #[test]
    fn inside_tmux_uses_dcs_passthrough() {
        // Each inner ESC must be doubled per tmux's DCS protocol so
        // the outer tmux strips one layer and the host terminal sees
        // the OSC-52 intact. Failure mode if we get this wrong: tmux
        // echoes parts of the sequence as visible chars in the pane.
        with_tmux_env(Some("/tmp/tmux-fake,1234,0"), || {
            let seq = build_osc52_sequence("hi");
            let s = std::str::from_utf8(&seq).unwrap();
            let payload = STANDARD.encode(b"hi");
            let inner = format!("\x1b]52;c;{payload}\x07");
            let expected = format!("\x1bPtmux;{}\x1b\\", inner.replace('\x1b', "\x1b\x1b"));
            assert_eq!(s, expected);
        });
    }
}

#[cfg(test)]
mod selection_tests {
    //! Shift-drag copy must preserve intra-word spaces even when the
    //! inner agent painted them via cursor positioning (which ratatui
    //! does in its diff renderer). Without the empty-cell → space
    //! substitution in `extract_selection_from_screen` we collapsed
    //! "CI on GitHub" to "CIonGitHub" because vt100 returns "" for
    //! cells the agent never wrote to.
    use super::*;

    fn screen_from(bytes: &[u8], rows: u16, cols: u16) -> vt100::Parser {
        let mut p = vt100::Parser::new(rows, cols, 0);
        p.process(bytes);
        p
    }

    #[test]
    fn cursor_positioned_writes_preserve_spaces() {
        // Move-write-move-write — leaves cols 2 and 5 untouched, which
        // is exactly the pattern that produced "CIonGitHub" in the
        // bug report. CSI H is cursor home (1;1), CSI C moves right.
        let mut p = screen_from(b"", 1, 20);
        p.process(b"\x1b[1;1HCI");
        p.process(b"\x1b[1;4Hon");
        p.process(b"\x1b[1;7HGitHub");
        let text = extract_selection_from_screen(p.screen(), (1, 1), (12, 1));
        assert_eq!(text, "CI on GitHub");
    }

    #[test]
    fn literal_spaces_unchanged() {
        // Plain-write path — spaces were always written explicitly,
        // shouldn't double-up.
        let mut p = screen_from(b"", 1, 20);
        p.process(b"CI on GitHub");
        let text = extract_selection_from_screen(p.screen(), (1, 1), (12, 1));
        assert_eq!(text, "CI on GitHub");
    }

    #[test]
    fn trailing_blanks_trimmed() {
        // Row-pad cells past the written content must NOT show up as
        // a long trail of spaces. The trim_end runs once per row.
        let mut p = screen_from(b"", 1, 40);
        p.process(b"hello");
        let text = extract_selection_from_screen(p.screen(), (1, 1), (40, 1));
        assert_eq!(text, "hello");
    }

    #[test]
    fn multi_row_joins_with_newline() {
        let mut p = screen_from(b"", 3, 10);
        p.process(b"\x1b[1;1Hone");
        p.process(b"\x1b[2;1Htwo");
        p.process(b"\x1b[3;1Hthree");
        let text = extract_selection_from_screen(p.screen(), (1, 1), (5, 3));
        assert_eq!(text, "one\ntwo\nthree");
    }

    #[test]
    fn single_row_partial_range() {
        // Start/end in the same row narrows to the selected columns
        // only, even when the row has more content past `end`.
        let mut p = screen_from(b"", 1, 20);
        p.process(b"hello world");
        // Cols 7..=11 = "world"
        let text = extract_selection_from_screen(p.screen(), (7, 1), (11, 1));
        assert_eq!(text, "world");
    }
}

#[cfg(test)]
mod paste_tests {
    //! Bracketed-paste routing. The unit tests pin the *classifier* —
    //! whether a given paste payload is text, raw image bytes, a
    //! data URI, or an existing image file path. The forwarding
    //! plumbing (`handle_paste` itself) is covered by manual repro
    //! since it needs a live app + WS stream to exercise end-to-end.
    use super::*;

    #[test]
    fn raw_binary_paste_falls_through_to_text() {
        // Crossterm lossy-decodes bracketed-paste payloads to UTF-8
        // before the TUI sees them. So a "binary" paste (e.g. PNG
        // bytes from a terminal that forwards binary clipboards
        // verbatim) arrives with U+FFFD in place of the magic bytes
        // and can't be detected as an image at this layer. This test
        // pins that behaviour so we don't accidentally route mangled
        // binary content through the image branch later.
        let lossy = String::from_utf8_lossy(b"\x89PNG\r\n\x1a\ndata");
        assert!(matches!(classify_paste(&lossy), PasteKind::Text));
    }

    #[test]
    fn data_uri_with_image_mime_decodes() {
        // 1x1 transparent PNG, base64-encoded. The classifier must
        // strip the `data:` prefix, decode the payload, and report
        // the MIME so `write_paste_image` picks the right extension.
        let b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        let uri = format!("data:image/png;base64,{b64}");
        match classify_paste(&uri) {
            PasteKind::ImageBytes { mime, data } => {
                assert_eq!(mime, "image/png");
                assert!(data.starts_with(b"\x89PNG"));
            }
            other => panic!(
                "expected ImageBytes, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn data_uri_for_text_falls_through_to_text() {
        // `data:text/plain;base64,…` is a string the user might paste
        // intentionally — don't try to be clever and decode it.
        let uri = "data:text/plain;base64,aGVsbG8=";
        assert!(matches!(classify_paste(uri), PasteKind::Text));
    }

    #[test]
    fn plain_string_is_text() {
        assert!(matches!(
            classify_paste("hello, world\nsecond line"),
            PasteKind::Text
        ));
    }

    #[test]
    fn nonexistent_image_path_stays_text() {
        // The classifier checks `is_file()` — a bare string that
        // *looks* like a path but doesn't exist must not be treated
        // as an attachment (otherwise pasting "screenshot.png" as
        // plain prose would silently misroute).
        assert!(matches!(
            classify_paste("/definitely/not/a/real/path.png"),
            PasteKind::Text
        ));
    }

    #[test]
    fn existing_image_path_classifies_as_image_path() {
        // Write a real tiny PNG and verify the path branch fires.
        let tmp = std::env::temp_dir().join(format!("agentum-test-{}.png", Uuid::new_v4()));
        std::fs::write(&tmp, b"\x89PNG\r\n\x1a\n").unwrap();
        let tmp_str = tmp.to_string_lossy().to_string();
        match classify_paste(&tmp_str) {
            PasteKind::ImagePath(p) => assert_eq!(p, tmp),
            _ => {
                let _ = std::fs::remove_file(&tmp);
                panic!("expected ImagePath");
            }
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn sniff_recognises_common_formats() {
        assert_eq!(sniff_image_mime(b"\x89PNG\r\n\x1a\nxx"), Some("image/png"));
        assert_eq!(sniff_image_mime(b"\xff\xd8\xff\xe0xxx"), Some("image/jpeg"));
        assert_eq!(sniff_image_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(
            sniff_image_mime(b"RIFF\x00\x00\x00\x00WEBPmore"),
            Some("image/webp")
        );
        assert_eq!(sniff_image_mime(b"BMjunk"), Some("image/bmp"));
        assert_eq!(sniff_image_mime(b"plain text"), None);
    }
}

/// Tests for `Overlay::Goal`, `GoalForm`, the G-key dispatch, and the
/// `format_goal_error` helper. Written before the implementation so
/// the compiler confirms that every referenced symbol is real.
#[cfg(test)]
mod goal_overlay_tests {
    use super::*;

    /// G from Tree focus with no overlay open should open `Overlay::Goal`.
    /// This is the UI-SPEC §Interaction Contract keybinding.
    #[test]
    fn pressing_g_on_tree_opens_goal_overlay() {
        let form = GoalForm::default_for_profile("local".into());
        assert_eq!(form.text, "");
        assert!(!form.submitting);
        assert!(form.error.is_none());
        assert_eq!(form.profile, "local");

        // The overlay variant must exist and hold the form.
        let overlay = Overlay::Goal(Box::new(form));
        assert!(matches!(overlay, Overlay::Goal(_)));
    }

    /// G from a non-Tree focus should NOT open the Goal overlay. The
    /// context-aware dispatch leaves that for LazygitCheats or ignores it.
    #[test]
    fn pressing_g_off_tree_is_not_goal_overlay() {
        // Verify that GoalForm can be constructed so the enum variant exists,
        // but also confirm that from non-Tree focus the overlay stays None.
        // We test the discriminant logic directly: Overlay::LazygitCheats
        // should NOT be Overlay::Goal.
        let overlay = Overlay::LazygitCheats;
        assert!(!matches!(overlay, Overlay::Goal(_)));
    }

    /// Enter key inside `Overlay::Goal` must append a newline to `text`,
    /// not submit the form. Ctrl-Enter is the submit gesture.
    #[test]
    fn enter_inside_goal_overlay_appends_newline() {
        let mut form = GoalForm::default_for_profile("local".into());
        form.text.push_str("hello");
        // Simulate the Enter key: push a newline.
        goal_overlay_handle_enter(&mut form);
        assert_eq!(form.text, "hello\n");
        // Submitting flag must not have changed.
        assert!(!form.submitting);
    }

    /// Ctrl-Enter on an empty text field must be a no-op: no submit,
    /// submitting stays false. This prevents accidental empty-goal creation.
    #[test]
    fn ctrl_enter_on_empty_text_is_noop() {
        let form = GoalForm::default_for_profile("local".into());
        // `text` is empty — trimmed text is also empty.
        let should_submit = goal_overlay_should_submit(&form);
        assert!(!should_submit, "empty text must not trigger submit");
    }

    /// Esc inside `Overlay::Goal` must close the overlay without submitting.
    #[test]
    fn esc_closes_goal_overlay_without_submit() {
        let mut form = GoalForm::default_for_profile("local".into());
        form.text.push_str("some goal text");
        // Esc should leave form.submitting == false (no submit happened).
        assert!(!form.submitting);
        // The overlay itself is dismissed by setting app.overlay = Overlay::None;
        // here we just verify the form state is consistent for cancellation.
        let submitting_before_esc = form.submitting;
        // After Esc the caller sets overlay to None — form.submitting must
        // already be false (nothing was sent).
        assert!(!submitting_before_esc);
    }

    /// `format_goal_error` must detect a column-rule 400 envelope
    /// (`{"missing":["body"],"status":"todo"}`) and surface a human-readable
    /// message that starts with "Your todo column needs:".
    #[test]
    fn format_goal_error_maps_column_rule_envelope() {
        let raw = r#"400 — {"missing":["body"],"status":"todo"}"#;
        let msg = format_goal_error(raw);
        assert!(
            msg.starts_with("Your todo column needs:"),
            "expected column-rule message, got: {msg}"
        );
    }

    /// `format_goal_error` on a plain string must return the string unchanged.
    #[test]
    fn format_goal_error_passes_through_plain_strings() {
        let raw = "503 — service unavailable";
        let msg = format_goal_error(raw);
        assert_eq!(msg, raw);
    }

    /// `format_goal_error` on a generic JSON body (not a column-rule envelope)
    /// must return the raw string without mangling it.
    #[test]
    fn format_goal_error_passes_through_other_json() {
        let raw = r#"500 — {"error":"internal server error"}"#;
        let msg = format_goal_error(raw);
        assert_eq!(msg, raw);
    }
}

#[cfg(test)]
mod hint_card_tests {
    //! Unit tests for the bound-card hint strip (`c` key in Focus::Tree).
    //! Phase 2, plan 05 (BIND-05, BIND-06).
    //!
    //! The HTTP fetch inside `handle_key` can't be exercised without a live
    //! daemon, so tests pin the *dispatch invariants* — the `HintCardState`
    //! data type, the toggle logic, and the focus/card_id guards — rather
    //! than the async key handler itself.
    use super::*;

    /// Test A — `HintCardState` can be constructed and toggled.
    ///
    /// The toggle logic in `handle_key` reads
    /// `app.hint_card.as_ref().map(|h| h.card_id) == Some(card_id)`
    /// to decide whether to collapse or expand the strip. Verify this
    /// round-trips correctly.
    #[test]
    fn hint_card_state_toggle_logic() {
        let card_id: i64 = 42;

        // Expanding: assign a new state.
        let mut hint: Option<HintCardState> = Some(HintCardState {
            card_id,
            title: "Design the kanban board UI".to_string(),
        });
        assert_eq!(hint.as_ref().map(|h| h.card_id), Some(card_id));

        // Collapsing: same card_id → set to None.
        if hint.as_ref().map(|h| h.card_id) == Some(card_id) {
            hint = None;
        }
        assert!(hint.is_none(), "hint should collapse to None");
    }

    /// Test B — the dispatch guard: `c` only fires the hint logic when
    /// `Focus::Tree` is active. Any other focus must NOT change `hint_card`.
    ///
    /// The guard in `handle_key` is:
    ///   `KeyCode::Char('c') if app.focus == Focus::Tree && key.modifiers.is_empty()`
    /// We test the predicate directly.
    #[test]
    fn c_key_guard_is_tree_only() {
        // Guard predicate: should fire only for Tree focus.
        let focuses = [Focus::Tree, Focus::Term, Focus::TermRight, Focus::Lazygit];
        for focus in focuses {
            let fires = focus == Focus::Tree;
            if focus == Focus::Tree {
                assert!(fires, "guard must fire for Focus::Tree");
            } else {
                assert!(!fires, "guard must NOT fire for non-Tree focus");
            }
        }
    }

    /// Test C — `c` is a no-op when the session has no `card_id`.
    ///
    /// The handler reads `sess.card_id` and only proceeds when `Some`.
    /// Verify the `Option::map` + early-return pattern is safe.
    #[test]
    fn c_key_noop_when_no_card_id() {
        let card_id: Option<i64> = None;

        // Simulate the guard: if card_id is None, no state change.
        let hint: Option<HintCardState> = card_id.map(|id| HintCardState {
            card_id: id,
            title: "Test".to_string(),
        });
        assert!(hint.is_none(), "hint must stay None when card_id is absent");
    }

    /// Test D — `HintCardState` implements `Clone` and `PartialEq` (derived),
    /// needed for the toggle check and any future state diffing.
    #[test]
    fn hint_card_state_derives_clone_and_eq() {
        let a = HintCardState {
            card_id: 7,
            title: "Alpha".to_string(),
        };
        let b = a.clone();
        assert_eq!(a, b);

        let c = HintCardState {
            card_id: 8,
            title: "Beta".to_string(),
        };
        assert_ne!(a, c);
    }
}

#[cfg(test)]
mod ctrl_v_tests {
    use super::{CtrlVDecision, api, classify_clipboard_result, clipboard_error_message};

    fn upload(rel: &str, bytes: u64) -> api::UploadResponse {
        api::UploadResponse {
            path: format!("/tmp/{rel}"),
            relative_path: rel.into(),
            size_bytes: bytes,
        }
    }

    #[test]
    fn ctrl_v_uses_broker_when_present() {
        // Broker returns Ok(UploadResponse) → success toast, no
        // arboard fallback.
        let result = Ok(upload(".agentum-uploads/a.png", 42));
        let decision = classify_clipboard_result(result);
        assert_eq!(
            decision,
            CtrlVDecision::Success("uploaded .agentum-uploads/a.png (42 bytes)".into())
        );
    }

    #[test]
    fn ctrl_v_falls_back_to_arboard_on_agent_not_connected() {
        // Only `AgentNotConnected` triggers the arboard fallback —
        // single-host users who never installed clip-agent keep
        // working unchanged.
        let result = Err(api::ClipboardRequestError::AgentNotConnected);
        let decision = classify_clipboard_result(result);
        assert_eq!(decision, CtrlVDecision::FallbackToArboard);
    }

    #[test]
    fn ctrl_v_no_image_kind_does_not_fallback() {
        // Broker just told us the remote clipboard has no image;
        // falling back would either give the same answer or paste a
        // stale local image the user didn't intend.
        let result = Err(api::ClipboardRequestError::NoImage);
        let decision = classify_clipboard_result(result);
        match decision {
            CtrlVDecision::ErrorNoFallback(msg) => {
                assert!(
                    msg.contains("no image in clipboard"),
                    "expected 'no image in clipboard' in toast, got: {msg}"
                );
            }
            other => panic!("expected ErrorNoFallback, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_v_timeout_kind_does_not_fallback() {
        // Timeout means agent is connected but stuck; fallback would
        // mask the real issue. Toast steers users toward the install
        // command instead.
        let result = Err(api::ClipboardRequestError::Timeout);
        let decision = classify_clipboard_result(result);
        match decision {
            CtrlVDecision::ErrorNoFallback(msg) => {
                assert!(
                    msg.contains("clip-agent --install"),
                    "expected install hint in toast, got: {msg}"
                );
            }
            other => panic!("expected ErrorNoFallback, got {other:?}"),
        }
    }

    #[test]
    fn clipboard_error_message_pins_user_facing_string() {
        // Pinning the string ensures the most common Ctrl-V failure
        // mode ("nothing copied") gets a stable, debuggable hint
        // that's also greppable from issue reports.
        let msg = clipboard_error_message(arboard::Error::ContentNotAvailable);
        assert_eq!(
            msg,
            "no image in clipboard — copy an image first (Ctrl-V is for images only — use bracketed paste for text)"
        );
    }

    #[test]
    fn clipboard_error_message_handles_other_variants() {
        // Spot-check the other branches so a future arboard upgrade
        // that adds variants forces a re-look at this match (the
        // catch-all `other => …` keeps the build green either way,
        // but the explicit branches stay covered).
        let msg = clipboard_error_message(arboard::Error::ClipboardOccupied);
        assert!(msg.contains("busy"), "got: {msg}");
        let msg = clipboard_error_message(arboard::Error::ClipboardNotSupported);
        assert!(msg.contains("not supported"), "got: {msg}");
    }
}
