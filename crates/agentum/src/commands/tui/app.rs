//! TUI app state, key dispatch, and event loop.

use std::collections::HashMap;
use std::io::Stdout;
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
use super::term::TerminalPane;
use super::ui;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const TICK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Term,
    Input,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    #[default]
    Connecting,
    Connected,
    Disconnected,
}

pub struct App {
    pub sessions: Vec<Session>,
    pub tree: Tree,
    pub selected: Option<Uuid>,
    pub term: TerminalPane,
    pub input: String,
    pub focus: Focus,
    pub error_count: u32,
    pub conn: ConnState,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub help: bool,
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
            input: String::new(),
            focus: Focus::Tree,
            error_count: 0,
            conn: ConnState::Connecting,
            status_msg: None,
            should_quit: false,
            help: false,
        }
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

    // Subscribe to the daemon's event bus.
    let _events_handle: JoinHandle<()> = client.open_event_stream(event_tx);

    // Open the terminal stream for the initial selection.
    let mut stream_handle: Option<JoinHandle<()>> = app
        .selected
        .map(|id| client.open_terminal_stream(id, term_tx.clone()));

    let mut crossterm_events = EventStream::new();
    let mut tick = interval(TICK_INTERVAL);
    let mut last_refresh = Instant::now();

    loop {
        // Resize the vt100 parser to match the actual terminal pane area
        // (terminal width minus 32-col tree minus 2 borders, height minus
        // title (1) + status (1) + input (3) + 2 borders).
        let size = terminal.size()?;
        let cols = size.width.saturating_sub(32 + 2).max(1);
        let rows = size.height.saturating_sub(1 + 1 + 3 + 2).max(1);
        app.term.resize(rows, cols);

        terminal.draw(|f| ui::draw(f, &app))?;
        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            biased;

            maybe_input = crossterm_events.next() => {
                if let Some(Ok(ev)) = maybe_input {
                    handle_crossterm(&mut app, ev, &client, &term_tx, &mut stream_handle).await;
                }
            }

            Some(msg) = term_rx.recv() => {
                handle_terminal_msg(&mut app, msg);
            }

            Some(msg) = event_rx.recv() => {
                handle_event_msg(&mut app, msg, &client).await;
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

async fn handle_crossterm(
    app: &mut App,
    ev: CtEvent,
    client: &Client,
    term_tx: &mpsc::UnboundedSender<TerminalMsg>,
    stream_handle: &mut Option<JoinHandle<()>>,
) {
    match ev {
        CtEvent::Key(key) if key.kind == KeyEventKind::Press => {
            handle_key(app, key, client, term_tx, stream_handle).await;
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
    stream_handle: &mut Option<JoinHandle<()>>,
) {
    // Global: Ctrl-C always quits.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    if app.help {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')) {
            app.help = false;
        }
        return;
    }

    if app.focus == Focus::Input {
        match key.code {
            KeyCode::Esc => app.focus = Focus::Tree,
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            KeyCode::Enter => {
                if let Some(id) = app.selected {
                    let text = app.input.clone();
                    app.input.clear();
                    match client.send_text(id, &text, true).await {
                        Ok(()) => app.status_msg = Some("sent".into()),
                        Err(e) => {
                            app.status_msg = Some(format!("send failed: {e}"));
                            app.error_count += 1;
                        }
                    }
                } else {
                    app.status_msg = Some("no session selected".into());
                }
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('?') => app.help = true,
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Tree => Focus::Term,
                Focus::Term => Focus::Input,
                Focus::Input => Focus::Tree,
            }
        }
        KeyCode::Char('i') => app.focus = Focus::Input,
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
            update_selection(app, client, term_tx, stream_handle);
        }
        _ => {}
    }
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
    app.selected = new_id;
    app.term.reset();
    if let Some(id) = new_id {
        *stream_handle = Some(client.open_terminal_stream(id, term_tx.clone()));
    }
}

fn handle_terminal_msg(app: &mut App, msg: TerminalMsg) {
    match msg {
        TerminalMsg::Bytes(b) => app.term.feed(&b),
        TerminalMsg::Error(s) => {
            // Show as a soft notice in the pane itself.
            let line = format!("\r\n[stream] {s}\r\n");
            app.term.feed(line.as_bytes());
        }
        TerminalMsg::Closed => {
            let line = "\r\n[stream closed]\r\n";
            app.term.feed(line.as_bytes());
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
    match ev.kind.as_str() {
        "session.crashed" | "watchdog.crashed" => {
            app.error_count += 1;
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        "session.started" | "session.stopped" | "session.created" | "session.deleted" => {
            if let Ok(fresh) = client.list_sessions().await {
                app.refresh_sessions(fresh);
            }
        }
        _ => {}
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
