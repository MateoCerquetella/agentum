//! TUI app state, key dispatch, and event loop.

use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use agentum_core::{Event, Session, Status};
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
use super::palette::{ActionKind, Catalog};
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
/// TTLs in milliseconds, mirroring `dashboard/src/lib/stores/events.ts`.
pub const NOTIF_TTL_INFO_MS: u64 = 6000;
pub const NOTIF_TTL_WARN_MS: u64 = 4000;
pub const NOTIF_TTL_ERROR_MS: u64 = 12000;

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
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    #[default]
    Connecting,
    Connected,
    Disconnected,
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
    /// Scrollable view of the recent error log. Opened with `e` from
    /// tree focus or via the command palette. Replaces the status-bar
    /// counter that previously was the only place the user could tell
    /// errors had happened.
    Errors,
    /// New-session form (n key on the tree).
    NewSession(Box<NewSessionForm>),
    /// Generic confirmation prompt for destructive session actions.
    Confirm(PendingAction),
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
pub const TOOL_SUGGESTIONS: &[&str] = &["claude", "codex", "opencode", "aider", "bash"];

/// Tools that accept `--dangerously-skip-permissions`. Mirrors the
/// `yoloTools` set in `dashboard/src/lib/components/NewSessionDialog.svelte`
/// so the TUI and web treat the same set of agents as YOLO-capable.
pub const YOLO_TOOLS: &[&str] = &["claude", "codex", "opencode"];

/// Flag appended when YOLO mode is on and the tool is in `YOLO_TOOLS`.
pub const YOLO_FLAG: &str = "--dangerously-skip-permissions";

/// Inline new-session form. Mirrors the web `NewSessionDialog` field-for-
/// field: name, tool (with cycle-suggestions), model, workdir (with a
/// directory-picker sub-overlay), extra args, an "up after create" toggle,
/// and a YOLO toggle that appends `--dangerously-skip-permissions` for
/// permission-skipping agents.
#[derive(Clone, PartialEq, Eq)]
pub struct NewSessionForm {
    pub field: NewSessionField,
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
    pub fn new(default_workdir: String) -> Self {
        Self {
            field: NewSessionField::Name,
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
            NewSessionField::Name => NewSessionField::Tool,
            NewSessionField::Tool => NewSessionField::Model,
            NewSessionField::Model => NewSessionField::Workdir,
            NewSessionField::Workdir => NewSessionField::Args,
            NewSessionField::Args => NewSessionField::UpAfter,
            NewSessionField::UpAfter => NewSessionField::Yolo,
            NewSessionField::Yolo => NewSessionField::Name,
        };
    }

    pub fn prev_field(&mut self) {
        self.field = match self.field {
            NewSessionField::Name => NewSessionField::Yolo,
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
            NewSessionField::Name => Some(&mut self.name),
            NewSessionField::Tool => Some(&mut self.tool),
            NewSessionField::Model => Some(&mut self.model),
            NewSessionField::Workdir => Some(&mut self.workdir),
            NewSessionField::Args => Some(&mut self.args),
            NewSessionField::UpAfter | NewSessionField::Yolo => None, // toggles, not text
        }
    }

    /// True when YOLO mode is enabled and the active tool actually supports
    /// `--dangerously-skip-permissions`. Bash and aider (and friends) ignore
    /// the toggle so the flag stays out of their argv.
    pub fn yolo_active(&self) -> bool {
        let tool = self.tool.trim();
        self.yolo && YOLO_TOOLS.iter().any(|t| *t == tool)
    }

    /// Cycle the tool field through `TOOL_SUGGESTIONS`. Triggered by
    /// pressing Tab when the Tool field has focus and isn't already on
    /// the last suggestion. Wraps to `claude` after the last entry.
    pub fn cycle_tool(&mut self) {
        let current = self.tool.trim();
        let idx = TOOL_SUGGESTIONS.iter().position(|t| *t == current);
        let next = match idx {
            Some(i) => TOOL_SUGGESTIONS[(i + 1) % TOOL_SUGGESTIONS.len()],
            None => TOOL_SUGGESTIONS[0],
        };
        self.tool = next.to_string();
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
    Delete {
        id: Uuid,
        name: String,
        running: bool,
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
            PendingAction::Delete { name, running, .. } => {
                if *running {
                    format!(
                        "delete session `{name}`? It is currently running — will be killed first."
                    )
                } else {
                    format!("delete session `{name}`?")
                }
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
    pub status_msg: Option<String>,
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
    /// When `true`, `push_notification` skips `sound::play`. Set from
    /// `--no-sound` / `AGENTUM_TUI_NO_SOUND` at startup; no runtime
    /// toggle (no key binding requested).
    pub sound_muted: bool,
}

impl App {
    pub fn new(sessions: Vec<Session>) -> Self {
        let tree = Tree::build(&sessions, &HashMap::new());
        let selected = first_visible_session(&tree, &sessions);
        Self {
            sessions,
            tree,
            selected,
            prev_selected: None,
            term: TerminalPane::new(),
            focus: Focus::Tree,
            tree_width: 32,
            fullscreen: false,
            sidebar_hidden: false,
            chord: None,
            filter_input_active: false,
            error_count: 0,
            errors: Vec::new(),
            errors_scroll: 0,
            conn: ConnState::Connecting,
            status_msg: None,
            should_quit: false,
            overlay: Overlay::None,
            palette: PaletteState::new(),
            lazygit: None,
            lazygit_cwd: None,
            lg_tx: None,
            term_in: None,
            term_size: (0, 0),
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
            sound_muted: false,
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
    }

    pub fn cycle_theme(&mut self) {
        self.theme = Theme::next(self.theme.name);
        theme::save(self.theme.name);
        self.status_msg = Some(format!("theme: {}", self.theme.name));
    }

    pub fn selected_session(&self) -> Option<&Session> {
        let id = self.selected?;
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn refresh_sessions(&mut self, sessions: Vec<Session>) {
        let prev_state: HashMap<String, bool> = self
            .tree
            .groups
            .iter()
            .map(|g| (g.workdir.clone(), g.expanded))
            .collect();
        // Preserve the active filter across rebuilds — the user's typed
        // search shouldn't vanish just because the session list changed.
        let prev_filter = self.tree.filter_str().to_string();
        self.sessions = sessions;
        self.tree = Tree::build(&self.sessions, &prev_state);
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
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.errors.push(ErrorEntry {
            at: SystemTime::now(),
            text,
        });
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
        // Normalize workdirs before grouping so `/x/proj` and `/x/proj/`
        // don't show up as two separate groups in the sidebar.
        let mut by_workdir: HashMap<String, Vec<&Session>> = HashMap::new();
        for s in sessions {
            let key = normalize_workdir(&s.workdir);
            by_workdir.entry(key).or_default().push(s);
        }
        let mut keys: Vec<String> = by_workdir.keys().cloned().collect();
        keys.sort();
        let groups: Vec<Group> = keys
            .into_iter()
            .map(|k| {
                let mut sess = by_workdir.remove(&k).unwrap();
                sess.sort_by(|a, b| a.name.cmp(&b.name));
                Group {
                    expanded: *prev_expanded.get(&k).unwrap_or(&true),
                    sessions: sess.iter().map(|s| s.id).collect(),
                    workdir: k,
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

pub async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: Client,
    sessions: Vec<Session>,
    sound_muted: bool,
) -> Result<()> {
    let mut app = App::new(sessions);
    app.sound_muted = sound_muted;

    let (term_tx, mut term_rx) = mpsc::unbounded_channel::<TerminalMsg>();
    let (term_tx_right, mut term_rx_right) = mpsc::unbounded_channel::<TerminalMsg>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<EventMsg>();
    let (lg_tx, mut lg_rx) = mpsc::unbounded_channel::<PtyMsg>();
    // Stash cheap clones on `App` so `update_selection` can pick the
    // correct sender by side without re-threading args. The lazygit
    // sender lives here too so `refresh_lazygit_for_selection` can
    // respawn the side pane on project switches without threading a
    // `&Sender` through every handler.
    app.term_tx_left = Some(term_tx.clone());
    app.term_tx_right = Some(term_tx_right);
    app.lg_tx = Some(lg_tx.clone());

    // Subscribe to the daemon's event bus.
    let _events_handle: JoinHandle<()> = client.open_event_stream(event_tx);

    // Open the terminal stream for the initial selection. The handle
    // lives on `App` (left/right slots) instead of the run-loop stack
    // so helper functions can access it through `&mut App` without
    // threading an extra `&mut Option<JoinHandle>` everywhere.
    if let Some(id) = app.selected {
        let (key_tx, key_rx) = mpsc::unbounded_channel::<TermOut>();
        let h = client.open_terminal_stream(id, term_tx.clone(), key_rx);
        app.term_in = Some(key_tx);
        app.term_size = (0, 0); // force first resize once we know the pane size
        app.stream_handle_left = Some(h);
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
        );
        // Cache for the next mouse event — handlers need to know which
        // pane the cursor is over, which only the layout knows.
        app.last_areas = Some(areas);
        let (term_rows, term_cols) = inner_size(areas.terminal);
        app.term.resize(term_rows, term_cols);
        // Tell the daemon (and through it tmux) about the new pane size so
        // the embedded TUI redraws into the right viewport. Without this
        // tmux clamps to its 80×24 default and you get overlapping text.
        if (term_cols, term_rows) != app.term_size && term_cols > 0 && term_rows > 0 {
            if let Some(tx) = app.term_in.as_ref()
                && tx
                    .send(TermOut::Resize {
                        cols: term_cols,
                        rows: term_rows,
                    })
                    .is_ok()
            {
                app.term_size = (term_cols, term_rows);
            }
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
            return Ok(());
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
            }

            Some(msg) = term_rx_right.recv() => {
                handle_terminal_msg(&mut app, msg, Side::Right);
            }

            Some(msg) = event_rx.recv() => {
                handle_event_msg(&mut app, msg, &client).await;
            }

            Some(msg) = lg_rx.recv() => {
                handle_lazygit_msg(&mut app, msg);
            }

            _ = tick.tick() => {
                // Cheap O(MAX_NOTIFS) sweep — drops expired toasts so the
                // bottom-left stack drains without us having to schedule
                // a per-toast sleep future.
                app.tick_expire();
                if last_refresh.elapsed() >= REFRESH_INTERVAL {
                    last_refresh = Instant::now();
                    if let Ok(fresh) = client.list_sessions().await {
                        app.refresh_sessions(fresh);
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

/// Route mouse events. Two-layer dispatch:
///
/// 1. If the inner program (claude code, vim, k9s, htop, …) has turned
///    on mouse tracking, encode the event as an SGR escape sequence and
///    forward it to the pane. Alt-screen TUIs all want this — the
///    program owns scroll/click semantics, same as Alacritty / kitty /
///    iTerm. Crucially, this is what unblocks scrollback inside claude
///    code: vt100's local scrollback is empty in alt-screen mode, so
///    the only way `Shift-PgUp`-style scrolling works is to let claude
///    handle it natively.
/// 2. Otherwise, scroll-wheel ticks drive agentum's own per-pane
///    scrollback for shell-style output. Click / drag / move events are
///    dropped (host terminal's Shift-click still works for native
///    selection).
fn handle_mouse(app: &mut App, ev: crossterm::event::MouseEvent) {
    use crossterm::event::MouseEventKind;
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

    // Does the inner pane want the mouse? Inspect the slot's vt100
    // mouse-protocol mode. When it's on, encode + forward; otherwise
    // we own the event.
    let wants_mouse = match side {
        Side::Left => app.term.wants_mouse_events(),
        Side::Right => app
            .split_right
            .as_ref()
            .is_some_and(|s| s.term.wants_mouse_events()),
    };

    if wants_mouse {
        // Translate absolute terminal coords into 1-based pane-local
        // coords. The panel border is 1 cell, so the inner content
        // starts at `(rect.x + 1, rect.y + 1)`. Subtract that, then add
        // 1 because xterm coords are 1-based.
        let inner_col = col.saturating_sub(rect.x.saturating_add(1)).saturating_add(1);
        let inner_row = row.saturating_sub(rect.y.saturating_add(1)).saturating_add(1);
        if let Some(seq) = encode_mouse_sgr(ev.kind, ev.modifiers, inner_col, inner_row) {
            let tx = match side {
                Side::Left => app.term_in.as_ref(),
                Side::Right => app.split_right.as_ref().and_then(|s| s.term_in.as_ref()),
            };
            if let Some(tx) = tx {
                let _ = tx.send(TermOut::Bytes(seq.into_bytes()));
            }
        }
        return;
    }

    // Local scrollback path — only ever ScrollUp / ScrollDown.
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
                app.status_msg = Some(if app.sidebar_hidden {
                    "sidebar hidden".into()
                } else {
                    "sidebar visible".into()
                });
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
        app.status_msg = Some("Ctrl-K · waiting (Z fullscreen · B sidebar)".into());
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
        app.status_msg = Some(if app.sidebar_hidden {
            "sidebar hidden (Ctrl-B to reopen)".into()
        } else {
            "sidebar visible".into()
        });
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
    // run side-by-side"); the user can then focus the right pane (one
    // Ctrl-Shift-]) and pick a different session via the palette or
    // tree if they want. No-op while lazygit is open — those two
    // features are mutually exclusive (4-column layouts get cramped).
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
                if let Some(s) = app.selected_session() {
                    app.overlay = Overlay::Confirm(PendingAction::Delete {
                        id: s.id,
                        name: s.name.clone(),
                        running: matches!(s.status, Status::Running),
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
    if !shift && key.code == KeyCode::Char('t') && app.focus == Focus::Tree {
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
        match tx_opt {
            Some(tx) => {
                if tx.send(TermOut::Bytes(bytes)).is_err() {
                    app.push_error("terminal stream closed — Ctrl-E tree · Ctrl-Q quit");
                }
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
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('?') => app.overlay = Overlay::Help,
        KeyCode::Char('e') => {
            // Open the error log overlay. Always available (even when
            // empty) so the user can confirm "no errors yet" with the
            // same gesture they'd use to investigate a bumped counter.
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
        // `/` starts filter-input mode. Any prior filter is cleared so
        // each `/` press is a fresh search — matches vim, fzf, and every
        // other "press / to filter" UI.
        KeyCode::Char('/') => {
            app.tree.set_filter("");
            app.filter_input_active = true;
            app.status_msg = Some("/ (Esc to cancel · Enter to keep)".into());
        }
        // Resize the sidebar tree. 4-col steps, clamped 16..=80; the
        // terminal pane keeps its 20-col floor at draw time. Works
        // regardless of focus so it's reachable from any panel.
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.tree_width = app.tree_width.saturating_add(4).min(80);
            app.status_msg = Some(format!("tree width: {}", app.tree_width));
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            app.tree_width = app.tree_width.saturating_sub(4).max(16);
            app.status_msg = Some(format!("tree width: {}", app.tree_width));
        }
        // 1/2/3 jump straight to a panel (Tree/Term/Lazygit).
        KeyCode::Char('1') => app.focus = Focus::Tree,
        KeyCode::Char('2') => app.focus = Focus::Term,
        KeyCode::Char('3') if app.lazygit_open() => app.focus = Focus::Lazygit,
        // [ / ] / Tab cycle focus.
        KeyCode::Char(']') | KeyCode::Tab => {
            app.set_focus(next_focus(app.focus, app.lazygit_open(), app.split_open()));
        }
        KeyCode::Char('[') | KeyCode::BackTab => {
            app.set_focus(prev_focus(app.focus, app.lazygit_open(), app.split_open()));
        }
        KeyCode::Char('r') => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                app.status_msg = Some("refreshed".into());
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.tree.move_cursor(1);
            {
            let side = app.target_side();
            update_selection(app, client, side);
        }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.tree.move_cursor(-1);
            {
            let side = app.target_side();
            update_selection(app, client, side);
        }
        }
        KeyCode::Char('h') | KeyCode::Left => app.tree.collapse(),
        KeyCode::Char('l') | KeyCode::Right => app.tree.expand(),
        KeyCode::Enter => {
            // If the cursor is on a session leaf, select it AND jump focus
            // into the terminal so the user can start typing immediately.
            // On a group row, expand/collapse (current behavior preserved
            // by update_selection which is a no-op on group rows).
            let on_leaf = matches!(app.tree.current_row(), Some(Row::Leaf { .. }));
            {
            let side = app.target_side();
            update_selection(app, client, side);
        }
            if on_leaf && app.selected.is_some() {
                app.set_focus(Focus::Term);
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
            app.overlay = Overlay::NewSession(Box::new(NewSessionForm::new(workdir)));
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
            // `x` is the more discoverable delete shortcut — vim/fzf-style
            // and unshifted so the user doesn't have to learn that capital
            // letters mean lifecycle actions. Shift-D stays bound for
            // muscle memory; both route to the same confirmation prompt.
            if let Some(s) = app.selected_session() {
                app.overlay = Overlay::Confirm(PendingAction::Delete {
                    id: s.id,
                    name: s.name.clone(),
                    running: matches!(s.status, Status::Running),
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

        // Tab/Shift-Tab moves between fields, EXCEPT on the Tool field
        // where Tab cycles through suggestions (matches web's datalist).
        KeyCode::Tab => {
            if matches!(form.field, NewSessionField::Tool) {
                form.cycle_tool();
            } else {
                form.next_field();
            }
        }
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
            // Submit.
            if let Err(msg) = form.validate() {
                form.error = Some(msg.into());
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
            match client
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
                    if form.up_after {
                        if let Err(e) = client.start_session(id).await {
                            app.status_msg = Some(format!("created `{name}` (start failed)"));
                            app.push_error(format!("start `{name}`: {e}"));
                        } else {
                            app.status_msg = Some(format!("created + started `{name}`"));
                        }
                    } else {
                        app.status_msg = Some(format!("created `{name}` (idle)"));
                    }
                    if let Ok(fresh) = client.list_sessions().await {
                        app.refresh_sessions(fresh);
                        app.tree.select_session(id);
                        {
            let side = app.target_side();
            update_selection(app, client, side);
        }
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
/// Falls back to an empty listing on transport errors so the picker
/// still opens with a visible error message.
async fn open_dir_picker(seed: Option<&str>, client: &Client) -> DirPickerState {
    let path = seed.map(|s| s.to_string());
    match client.list_dir(path.as_deref()).await {
        Ok(listing) => DirPickerState {
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
            error: None,
            loading: false,
        },
        Err(e) => DirPickerState {
            path: seed.unwrap_or("~").to_string(),
            parent: None,
            entries: Vec::new(),
            cursor: 0,
            error: Some(e.to_string()),
            loading: false,
        },
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
    let result = match &action {
        PendingAction::Start { id, .. } => client.start_session(*id).await,
        PendingAction::Stop { id, .. } => client.stop_session(*id).await,
        // "Kill" in the TUI means kill-and-remove: drop the tmux session
        // AND the store record so the entry disappears from the tree.
        // The split-brain behaviour where Kill stopped the process but
        // left a corpse in the sidebar was confusing — users expect
        // "kill terminal" to behave the same way every other multiplexer
        // does (close + remove). The legacy stop-only verb is still
        // reachable as `s`/Stop for the rare "I want to restart this
        // exact session later" case.
        PendingAction::Kill { id, .. } => client.delete_session(*id, true).await,
        PendingAction::Delete { id, running, .. } => client.delete_session(*id, *running).await,
    };
    let label = match &action {
        PendingAction::Start { name, .. } => format!("started `{name}`"),
        PendingAction::Stop { name, .. } => format!("stopped `{name}`"),
        PendingAction::Kill { name, .. } => format!("killed & removed `{name}`"),
        PendingAction::Delete { name, .. } => format!("deleted `{name}`"),
    };
    match result {
        Ok(()) => app.status_msg = Some(label),
        Err(e) => {
            app.push_error(format!("{label}: {e}"));
        }
    }
    if let Ok(fresh) = client.list_sessions().await {
        app.refresh_sessions(fresh);
    }
}

/// Build the live action catalog from current app state.
pub fn palette_catalog(app: &App) -> Catalog {
    let sessions: Vec<(Uuid, String, String)> = app
        .sessions
        .iter()
        .map(|s| (s.id, s.name.clone(), s.workdir.clone()))
        .collect();
    Catalog::build(app.lazygit_open(), &sessions, app.selected)
}

/// Errors-overlay key handler. Treats the overlay as a vim-ish list:
/// j/k or arrow keys scroll, PgUp/PgDn page, g/G snap to ends, c clears
/// the log, and Esc/q/Enter/e dismiss. The cursor saturates at the list
/// length on render so we don't have to clamp on every keystroke.
fn handle_errors_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc
        | KeyCode::Char('q')
        | KeyCode::Char('e')
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
        ActionKind::DeleteSession(id) => {
            if let Some(s) = app.sessions.iter().find(|s| s.id == id) {
                app.overlay = Overlay::Confirm(PendingAction::Delete {
                    id,
                    name: s.name.clone(),
                    running: matches!(s.status, Status::Running),
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
                format!("filter: /{filter}")
            });
        }
        KeyCode::Backspace => {
            if filter.pop().is_some() {
                app.tree.set_filter(&filter);
                app.status_msg = Some(format!("/ {filter}"));
                changed = true;
            }
        }
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            filter.push(c);
            app.tree.set_filter(&filter);
            app.status_msg = Some(format!("/ {filter}"));
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
    // Reset parser + selection + input channel for the chosen side.
    match side {
        Side::Left => {
            app.term_in = None;
            app.selected = new_id;
            app.term.reset();
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
        let handle = client.open_terminal_stream(id, term_tx, key_rx);
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
    // Keep the lazygit side pane in lock-step with whichever side
    // owns the "primary" selection. Right-side splits are deliberately
    // skipped — they're an opt-in side-by-side view, and
    // `lazygit_cwd` only tracks one repo. If a user wants lazygit on
    // the right pane's workdir, focusing it as the left pane (close
    // split with Ctrl-W) does the right thing.
    if side == Side::Left {
        refresh_lazygit_for_selection(app);
    }
}

fn handle_terminal_msg(app: &mut App, msg: TerminalMsg, side: Side) {
    match msg {
        TerminalMsg::Bytes(b) => match side {
            Side::Left => app.term.feed(&b),
            Side::Right => {
                if let Some(slot) = app.split_right.as_mut() {
                    slot.term.feed(&b);
                }
            }
        },
        TerminalMsg::Error(s) => {
            // Server-sent text frames are diagnostic — typically
            // `[input dropped: tmux exited with status 1 (stderr: …)]`
            // when `tmux send-keys` rejects the target. Route into the
            // error overlay rather than the status bar so the actual
            // pane (claude code, etc.) doesn't get garbled mid-render
            // and the message survives more than a single redraw.
            app.push_error(s.trim().to_string());
        }
        TerminalMsg::Closed => {
            app.status_msg = Some(match side {
                Side::Left => "terminal stream closed".into(),
                Side::Right => "right-pane stream closed".into(),
            });
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
        EventMsg::Connected => app.conn = ConnState::Connected,
        EventMsg::Closed => app.conn = ConnState::Disconnected,
        EventMsg::Error(s) => {
            app.conn = ConnState::Disconnected;
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
                    NOTIF_TTL_WARN_MS,
                );
            }
        }
        EventMsg::Event(ev) => apply_event(app, ev, client).await,
    }
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
                NOTIF_TTL_ERROR_MS,
            );
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
                NOTIF_TTL_INFO_MS,
            );
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
                NOTIF_TTL_INFO_MS,
            );
        }
        "session.created" | "session.deleted" => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        _ => {}
    }
}

/// Append a typed toast to the bottom-left notification stack and play
/// the matching system sound (unless `app.sound_muted`). Evicts the
/// oldest entry FIFO when the stack would exceed `MAX_NOTIFS`. Mirrors
/// the dashboard's `pushToast` shape (id, kind, title, body, ttl_ms).
fn push_notification(
    app: &mut App,
    title: String,
    body: Option<String>,
    kind: NotifKind,
    ttl_ms: u64,
) {
    let id = app.next_notif_id;
    app.next_notif_id = app.next_notif_id.saturating_add(1);
    app.notifications.push(Notification {
        id,
        title,
        body,
        kind,
        created_at: Instant::now(),
        ttl: Duration::from_millis(ttl_ms),
    });
    if app.notifications.len() > MAX_NOTIFS {
        let drop_n = app.notifications.len() - MAX_NOTIFS;
        app.notifications.drain(0..drop_n);
    }
    if !app.sound_muted {
        super::sound::play(kind);
    }
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
    let name = format!("shell-{}", Uuid::new_v4().to_string().split('-').next().unwrap_or("0"));
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
                    NOTIF_TTL_INFO_MS,
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
        Status::Stopped => ("◐", Color::Yellow),
        Status::Crashed => ("✗", Color::Red),
    }
}
