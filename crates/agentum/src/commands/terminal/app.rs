//! TUI app state, key dispatch, and event loop.

use std::collections::HashMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::time::Duration;

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

use super::api::{Client, EventMsg, TerminalMsg};
use super::extensions::{self, LAZYGIT};
use super::palette::{ActionKind, Catalog};
use super::pty::{LocalPty, PtyMsg};
use super::term::TerminalPane;
use super::theme::{self, Theme};
use super::ui;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const TICK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Term,
    Lazygit,
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
    /// New-session form (n key on the tree).
    NewSession(Box<NewSessionForm>),
    /// Generic confirmation prompt for destructive session actions.
    Confirm(PendingAction),
}

/// Suggested tool names. Mirrors the web's datalist on the New Session
/// dialog. Pressing Tab on the `Tool` field cycles through these.
pub const TOOL_SUGGESTIONS: &[&str] = &["claude", "codex", "opencode", "aider", "bash"];

/// Inline new-session form. Mirrors the web `NewSessionDialog` field-for-
/// field: name, tool (with cycle-suggestions), model, workdir (with a
/// directory-picker sub-overlay), extra args, and an "up after create"
/// toggle.
#[derive(Clone, PartialEq, Eq)]
pub struct NewSessionForm {
    pub field: NewSessionField,
    pub name: String,
    pub tool: String,
    pub model: String,
    pub workdir: String,
    pub args: String,
    pub up_after: bool,
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
            NewSessionField::UpAfter => NewSessionField::Name,
        };
    }

    pub fn prev_field(&mut self) {
        self.field = match self.field {
            NewSessionField::Name => NewSessionField::UpAfter,
            NewSessionField::Tool => NewSessionField::Name,
            NewSessionField::Model => NewSessionField::Tool,
            NewSessionField::Workdir => NewSessionField::Model,
            NewSessionField::Args => NewSessionField::Workdir,
            NewSessionField::UpAfter => NewSessionField::Args,
        };
    }

    pub fn field_value_mut(&mut self) -> Option<&mut String> {
        match self.field {
            NewSessionField::Name => Some(&mut self.name),
            NewSessionField::Tool => Some(&mut self.tool),
            NewSessionField::Model => Some(&mut self.model),
            NewSessionField::Workdir => Some(&mut self.workdir),
            NewSessionField::Args => Some(&mut self.args),
            NewSessionField::UpAfter => None, // toggle, not text
        }
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
                format!("kill session `{name}` immediately? (no graceful stop)")
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

/// Command-palette state. Driven by Ctrl-P / Ctrl-K. The action list is
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
    pub term: TerminalPane,
    pub focus: Focus,
    /// When `true` the title bar, tree sidebar, and status bar are hidden
    /// so the active pane (term + optional lazygit) fills the viewport.
    /// Toggled with Shift-F. Esc exits.
    pub fullscreen: bool,
    pub error_count: u32,
    pub conn: ConnState,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub overlay: Overlay,
    pub palette: PaletteState,
    /// `Some` while a lazygit child is alive in the side pane.
    pub lazygit: Option<LocalPty>,
    /// Outbound key channel for the active terminal stream. `None` while
    /// no session is selected; recreated each time the stream is reopened.
    pub term_in: Option<mpsc::UnboundedSender<Vec<u8>>>,
    pub theme: &'static Theme,
    /// Recent notifications displayed in bottom-left. Pushed on session
    /// events; capped at 3 entries. Each auto-clears after 8s.
    pub notifications: Vec<String>,
}

impl App {
    pub fn new(sessions: Vec<Session>) -> Self {
        let tree = Tree::build(&sessions, &HashMap::new());
        let selected = first_visible_session(&tree, &sessions);
        Self {
            sessions,
            tree,
            selected,
            term: TerminalPane::new(),
            focus: Focus::Tree,
            fullscreen: false,
            error_count: 0,
            conn: ConnState::Connecting,
            status_msg: None,
            should_quit: false,
            overlay: Overlay::None,
            palette: PaletteState::new(),
            lazygit: None,
            term_in: None,
            theme: theme::load(),
            notifications: Vec::new(),
        }
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
        self.sessions = sessions;
        self.tree = Tree::build(&self.sessions, &prev_state);
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
}

// ---------- Tree ----------

pub struct Tree {
    pub groups: Vec<Group>,
    pub cursor: usize, // index into the flattened visible row list
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

impl Tree {
    pub fn build(sessions: &[Session], prev_expanded: &HashMap<String, bool>) -> Self {
        let mut by_workdir: HashMap<String, Vec<&Session>> = HashMap::new();
        for s in sessions {
            by_workdir.entry(s.workdir.clone()).or_default().push(s);
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
        Self { groups, cursor: 0 }
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (gi, g) in self.groups.iter().enumerate() {
            rows.push(Row::Group(gi));
            if g.expanded {
                for li in 0..g.sessions.len() {
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
) -> Result<()> {
    let mut app = App::new(sessions);

    let (term_tx, mut term_rx) = mpsc::unbounded_channel::<TerminalMsg>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<EventMsg>();
    let (lg_tx, mut lg_rx) = mpsc::unbounded_channel::<PtyMsg>();

    // Subscribe to the daemon's event bus.
    let _events_handle: JoinHandle<()> = client.open_event_stream(event_tx);

    // Open the terminal stream for the initial selection.
    let mut stream_handle: Option<JoinHandle<()>> = if let Some(id) = app.selected {
        let (key_tx, key_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let h = client.open_terminal_stream(id, term_tx.clone(), key_rx);
        app.term_in = Some(key_tx);
        Some(h)
    } else {
        None
    };

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
        );
        let (term_rows, term_cols) = inner_size(areas.terminal);
        app.term.resize(term_rows, term_cols);
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
            if app.focus == Focus::Lazygit {
                app.focus = Focus::Tree;
            }
            app.status_msg = Some("lazygit exited".into());
        }

        tokio::select! {
            biased;

            maybe_input = crossterm_events.next() => {
                if let Some(Ok(ev)) = maybe_input {
                    handle_crossterm(&mut app, ev, &client, &term_tx, &lg_tx, &mut stream_handle).await;
                }
            }

            Some(msg) = term_rx.recv() => {
                handle_terminal_msg(&mut app, msg);
            }

            Some(msg) = event_rx.recv() => {
                handle_event_msg(&mut app, msg, &client).await;
            }

            Some(msg) = lg_rx.recv() => {
                handle_lazygit_msg(&mut app, msg);
            }

            _ = tick.tick() => {
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
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
) {
    match ev {
        CtEvent::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
            handle_key(app, key, client, term_tx, lg_tx, stream_handle).await;
        }
        CtEvent::Resize(_, _) => {}
        _ => {}
    }
}

async fn handle_key(
    app: &mut App,
    key: KeyEvent,
    client: &Client,
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
) {
    // Ctrl-P / Ctrl-K opens the command palette from anywhere. Highest
    // priority so it works even with a pane focused.
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('k'))
    {
        app.overlay = Overlay::Palette;
        app.palette = PaletteState::new();
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

    // Global panel switch — works *even with a pane focused* so you can
    // escape Term/Lazygit without first releasing focus. The binding
    // mirrors Slack/iTerm2/browser tab cycling (Cmd-Shift-]/[ on macOS,
    // which translates to Ctrl-Shift-]/[ in a TUI since Cmd never reaches
    // the app).
    //   Ctrl-Shift-]  → next panel
    //   Ctrl-Shift-[  → previous panel
    // Plain `[` / `]` remain available in non-pane focus (handled lower
    // down) so they don't get swallowed when typing into claude code.
    //
    // Why Ctrl-Shift and not plain Ctrl: Ctrl-[ in plain ASCII *is* the
    // ESC byte — terminals can't distinguish the two without the Kitty
    // Keyboard Protocol (which we push in mod.rs::run when supported).
    // Ctrl-Shift-[ doesn't have that ambiguity, so it works on more
    // emulators and survives accidentally running through nested screen
    // sessions.
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let ctrl_shift = ctrl && key.modifiers.contains(KeyModifiers::SHIFT);
    // Match both `]` and `}` because some emulators report Shift-] as the
    // shifted glyph, others as `]` with the Shift modifier set. The Kitty
    // protocol normalises this, but we accept both for portability.
    if ctrl_shift && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('}')) {
        app.focus = next_focus(app.focus, app.lazygit_open());
        return;
    }
    if ctrl_shift && matches!(key.code, KeyCode::Char('[') | KeyCode::Char('{')) {
        app.focus = prev_focus(app.focus, app.lazygit_open());
        return;
    }

    // F5 / F6 — reliable global panel switchers. Use these instead of
    // Ctrl-Shift-] / Ctrl-Shift-[ when your terminal doesn't speak the
    // Kitty Keyboard Protocol (e.g. plain xterm, cmd.exe).
    if key.code == KeyCode::F(5) {
        app.focus = next_focus(app.focus, app.lazygit_open());
        return;
    }
    if key.code == KeyCode::F(6) {
        app.focus = prev_focus(app.focus, app.lazygit_open());
        return;
    }
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
            handle_palette_key(app, key, client, term_tx, lg_tx, stream_handle).await;
            return;
        }
        Overlay::NewSession(_) => {
            handle_new_session_key(app, key, client, term_tx, stream_handle).await;
            return;
        }
        Overlay::Confirm(_) => {
            handle_confirm_key(app, key, client).await;
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
        spawn_plain_terminal(app, client, term_tx, stream_handle).await;
        return;
    }

    // While the lazygit pane is focused, forward raw bytes to its PTY.
    if app.focus == Focus::Lazygit {
        if let Some(lg) = app.lazygit.as_ref()
            && let Some(bytes) = key_to_bytes(&key)
        {
            if let Err(e) = lg.write(&bytes) {
                app.status_msg = Some(format!("lazygit write: {e}"));
                app.error_count += 1;
            }
        }
        return;
    }

    // While the remote terminal pane is focused, forward raw bytes over the
    // WS so they hit the running process (claude code, shell, etc.). Surface
    // both failure modes loudly — silent swallowing was the original "frozen
    // pane" symptom.
    if app.focus == Focus::Term {
        let Some(bytes) = key_to_bytes(&key) else {
            return;
        };
        match app.term_in.as_ref() {
            Some(tx) => {
                if tx.send(bytes).is_err() {
                    app.status_msg =
                        Some("terminal stream closed — Ctrl-G release · Ctrl-Q quit".into());
                    app.error_count += 1;
                }
            }
            None => {
                app.status_msg = Some(
                    "no terminal stream (no session selected?) — Ctrl-G release · Ctrl-Q quit"
                        .into(),
                );
            }
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('?') => app.overlay = Overlay::Help,
        KeyCode::Char('g') => toggle_lazygit(app, lg_tx).await,
        KeyCode::Char('G') => app.overlay = Overlay::LazygitCheats,
        KeyCode::Char('T') => app.cycle_theme(),
        // Shift-F toggles fullscreen (hides title/tree/status). Esc exits
        // when active. Mirrors the web dashboard's Shift+F shortcut so
        // muscle memory carries across surfaces.
        KeyCode::Char('F') => {
            app.fullscreen = !app.fullscreen;
            if app.fullscreen && app.focus == Focus::Tree {
                app.focus = Focus::Term;
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
        // 1/2/3 jump straight to a panel (Tree/Term/Lazygit).
        KeyCode::Char('1') => app.focus = Focus::Tree,
        KeyCode::Char('2') => app.focus = Focus::Term,
        KeyCode::Char('3') if app.lazygit_open() => app.focus = Focus::Lazygit,
        // [ / ] / Tab cycle focus.
        KeyCode::Char(']') | KeyCode::Tab => {
            app.focus = next_focus(app.focus, app.lazygit_open());
        }
        KeyCode::Char('[') | KeyCode::BackTab => {
            app.focus = prev_focus(app.focus, app.lazygit_open());
        }
        KeyCode::Char('r') => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                app.status_msg = Some("refreshed".into());
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.tree.move_cursor(1);
            update_selection(app, client, term_tx, stream_handle);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.tree.move_cursor(-1);
            update_selection(app, client, term_tx, stream_handle);
        }
        KeyCode::Char('h') | KeyCode::Left => app.tree.collapse(),
        KeyCode::Char('l') | KeyCode::Right => app.tree.expand(),
        KeyCode::Enter => {
            // If the cursor is on a session leaf, select it AND jump focus
            // into the terminal so the user can start typing immediately.
            // On a group row, expand/collapse (current behavior preserved
            // by update_selection which is a no-op on group rows).
            let on_leaf = matches!(app.tree.current_row(), Some(Row::Leaf { .. }));
            update_selection(app, client, term_tx, stream_handle);
            if on_leaf && app.selected.is_some() {
                app.focus = Focus::Term;
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
        KeyCode::Char('D') => {
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
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
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

        // Toggle field: space flips up_after; on text fields it just types
        // a literal space.
        KeyCode::Char(' ') if matches!(form.field, NewSessionField::UpAfter) => {
            form.up_after = !form.up_after;
        }

        // Enter while on the workdir field opens the dir picker
        // (mirrors clicking the picker's chevron in the web UI).
        // Enter on the up_after toggle flips it. Enter elsewhere submits.
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
            let flags = parse_args_field(&form.args);
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
                            app.status_msg =
                                Some(format!("created `{name}` but start failed: {e}"));
                            app.error_count += 1;
                        } else {
                            app.status_msg = Some(format!("created + started `{name}`"));
                        }
                    } else {
                        app.status_msg = Some(format!("created `{name}` (idle)"));
                    }
                    if let Ok(fresh) = client.list_sessions().await {
                        app.refresh_sessions(fresh);
                        app.tree.select_session(id);
                        update_selection(app, client, term_tx, stream_handle);
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
        PendingAction::Kill { id, .. } => client.kill_session(*id).await,
        PendingAction::Delete { id, running, .. } => client.delete_session(*id, *running).await,
    };
    let label = match &action {
        PendingAction::Start { name, .. } => format!("started `{name}`"),
        PendingAction::Stop { name, .. } => format!("stopped `{name}`"),
        PendingAction::Kill { name, .. } => format!("killed `{name}`"),
        PendingAction::Delete { name, .. } => format!("deleted `{name}`"),
    };
    match result {
        Ok(()) => app.status_msg = Some(label),
        Err(e) => {
            app.status_msg = Some(format!("{label}: {e}"));
            app.error_count += 1;
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
    Catalog::build(app.lazygit_open(), &sessions)
}

async fn handle_palette_key(
    app: &mut App,
    key: KeyEvent,
    client: &Client,
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
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
                run_palette_action(app, action.kind, client, term_tx, lg_tx, stream_handle).await;
            }
        }
        _ => {}
    }
}

async fn run_palette_action(
    app: &mut App,
    kind: ActionKind,
    client: &Client,
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    lg_tx: &mpsc::UnboundedSender<PtyMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
) {
    match kind {
        ActionKind::Quit => app.should_quit = true,
        ActionKind::ToggleHelp => app.overlay = Overlay::Help,
        ActionKind::ToggleLazygit => toggle_lazygit(app, lg_tx).await,
        ActionKind::LazygitCheats => app.overlay = Overlay::LazygitCheats,
        ActionKind::Refresh => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                app.status_msg = Some("refreshed".into());
            }
        }
        ActionKind::SpawnTerminal => {
            spawn_plain_terminal(app, client, term_tx, stream_handle).await;
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
            update_selection(app, client, term_tx, stream_handle);
        }
    }
}

fn next_focus(current: Focus, lazygit_open: bool) -> Focus {
    if lazygit_open {
        match current {
            Focus::Tree => Focus::Term,
            Focus::Term => Focus::Lazygit,
            Focus::Lazygit => Focus::Tree,
        }
    } else {
        match current {
            Focus::Tree => Focus::Term,
            Focus::Term => Focus::Tree,
            Focus::Lazygit => Focus::Tree,
        }
    }
}

fn prev_focus(current: Focus, lazygit_open: bool) -> Focus {
    if lazygit_open {
        match current {
            Focus::Tree => Focus::Lazygit,
            Focus::Lazygit => Focus::Term,
            Focus::Term => Focus::Tree,
        }
    } else {
        match current {
            Focus::Tree => Focus::Term,
            Focus::Term => Focus::Tree,
            Focus::Lazygit => Focus::Tree,
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
        if app.focus == Focus::Lazygit {
            app.focus = Focus::Tree;
        }
        app.status_msg = Some("closed lazygit".into());
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
            app.status_msg = Some(format!("lazygit spawn failed: {e}"));
            app.error_count += 1;
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
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out.extend(b"\x1b[A"),
        KeyCode::Down => out.extend(b"\x1b[B"),
        KeyCode::Right => out.extend(b"\x1b[C"),
        KeyCode::Left => out.extend(b"\x1b[D"),
        KeyCode::Home => out.extend(b"\x1b[H"),
        KeyCode::End => out.extend(b"\x1b[F"),
        KeyCode::PageUp => out.extend(b"\x1b[5~"),
        KeyCode::PageDown => out.extend(b"\x1b[6~"),
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

fn update_selection(
    app: &mut App,
    client: &Client,
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
) {
    let new_id = app.tree.current_session(&app.sessions);
    if new_id == app.selected {
        return;
    }
    if let Some(handle) = stream_handle.take() {
        handle.abort();
    }
    app.term_in = None;
    app.selected = new_id;
    app.term.reset();
    if let Some(id) = new_id {
        let (key_tx, key_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        *stream_handle = Some(client.open_terminal_stream(id, term_tx.clone(), key_rx));
        app.term_in = Some(key_tx);
    }
}

fn handle_terminal_msg(app: &mut App, msg: TerminalMsg) {
    match msg {
        TerminalMsg::Bytes(b) => app.term.feed(&b),
        TerminalMsg::Error(s) => {
            // Server-sent text frames are diagnostic — typically
            // `[input dropped: tmux exited with status 1 (stderr: …)]`
            // when `tmux send-keys` rejects the target. Surface to the
            // status bar so the actual pane (claude code, etc.) doesn't
            // get garbled mid-render. Bump error_count so it's visible
            // even when the user isn't reading the chrome line.
            app.status_msg = Some(s.trim().to_string());
            app.error_count += 1;
        }
        TerminalMsg::Closed => {
            app.status_msg = Some("terminal stream closed".into());
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
            app.error_count += 1;
            app.status_msg = Some(format!("events: {s}"));
        }
        EventMsg::Raw(kind) => {
            if kind == "bus.lagged" {
                app.error_count += 1;
            }
        }
        EventMsg::Event(ev) => apply_event(app, ev, client).await,
    }
}

async fn apply_event(app: &mut App, ev: Event, client: &Client) {
    let name = ev.session_name.unwrap_or_else(|| "?".into());
    match ev.kind.as_str() {
        "session.crashed" | "watchdog.crashed" => {
            app.error_count += 1;
            push_notification(app, format!("crashed: {name}"));
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "session.started" => {
            push_notification(app, format!("started: {name}"));
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "session.stopped" => {
            push_notification(app, format!("stopped: {name}"));
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "session.created" | "session.deleted" => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        _ => {}
    }
}

/// Push a notification onto the bottom-left display. Caps at 3 entries.
/// Each notification appears for ~8 seconds, using a simple counter
/// (decremented each frame by the UI). The notification text is shown in the
/// status line on the far left, giving instant feedback for session lifecycle
/// events (crashes, stops, starts, etc.).
fn push_notification(app: &mut App, text: String) {
    app.notifications.push(text);
    if app.notifications.len() > 3 {
        app.notifications.remove(0);
    }
    app.status_msg = Some(app.notifications.last().cloned().unwrap_or_default());
}

/// Spawn a plain bash terminal as a session. Uses the passthrough adapter
/// so the server picks up `bash` from PATH. Stored as a regular session so
/// it appears in the tree and can be killed/deleted like any other agent.
async fn spawn_plain_terminal(
    app: &mut App,
    client: &Client,
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
) {
    let name = format!("shell-{}", Uuid::new_v4().to_string().split('-').next().unwrap_or("0"));
    let workdir = app
        .selected_session()
        .map(|s| s.workdir.clone())
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".into());
    match client
        .create_session(&name, &workdir, "bash", None, vec![])
        .await
    {
        Ok(created) => {
            let id = created.id;
            if let Err(e) = client.start_session(id).await {
                app.status_msg = Some(format!("shell start failed: {e}"));
                app.error_count += 1;
            } else {
                push_notification(app, format!("shell: {name}"));
            }
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
                app.tree.select_session(id);
                update_selection(app, client, term_tx, stream_handle);
                app.focus = Focus::Term;
            }
        }
        Err(e) => {
            app.status_msg = Some(format!("shell create failed: {e}"));
            app.error_count += 1;
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
