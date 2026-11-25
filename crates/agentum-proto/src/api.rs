//! The hub's API for the Code section of the web app.
//!
//! REST (JSON) under `/api/code/…`; live updates on the browser's one WebSocket `/api/ws`:
//! the page sends [`WsIn`] (subscriptions, terminal keystrokes) and receives [`WsOut`].
use crate::machine::{AgentInfo, ModelInfo, Project, Stats};
use crate::{AgentKind, Attention, Event, Ms, Permission, RateLimit, SendMode, ThreadId, ThreadState, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<Ms>,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub stats: Stats,
    /// This machine is the default for new threads.
    #[serde(default)]
    pub main: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ThreadSummary {
    pub id: ThreadId,
    pub machine: String,
    pub agent: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub permission: Permission,
    /// The repo root (the project) and the directory the agent works in (it or a worktree).
    pub project: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub title: String,
    pub state: ThreadState,
    pub attention: Attention,
    pub unread: bool,
    pub pinned: bool,
    pub archived: bool,
    /// Manual order within its group (smaller first); 0 = by activity.
    #[serde(default)]
    pub order: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// A side chat's parent thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ThreadId>,
    pub created_at: Ms,
    pub updated_at: Ms,
    /// The last thing said, one line, for the sidebar.
    #[serde(default)]
    pub preview: String,
    /// The open question or permission prompt, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asking: Option<String>,
    /// Messages waiting for the running turn.
    #[serde(default)]
    pub queued: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub cost_usd: f64,
    pub last_seq: u64,
    /// Snoozed: hidden from the sidebar until then (it wakes early if it works or asks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<Ms>,
    /// When it was archived (settled); the sidebar's Settled shelf shows the last day's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<Ms>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Overview {
    pub machines: Vec<Machine>,
    pub threads: Vec<ThreadSummary>,
    /// Account limits per agent, newest known.
    #[serde(default)]
    pub limits: Vec<AgentLimits>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentLimits {
    pub agent: AgentKind,
    pub machine: String,
    pub limits: Vec<RateLimit>,
    pub at: Ms,
    /// The subscription ("max", "plus"…), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// A thread with its events. Assistant messages carry `html` (markdown rendered by the hub).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ThreadDetail {
    pub summary: ThreadSummary,
    pub events: Vec<EventView>,
    /// Older events exist before the first one here (load with `?before=`).
    pub more_before: bool,
    /// Text streaming right now, by item (so a page opened mid-turn sees it).
    #[serde(default)]
    pub partial: Vec<Partial>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Partial {
    pub item: String,
    pub thinking: bool,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EventView {
    #[serde(flatten)]
    pub event: Event,
    /// Rendered HTML of a Message, Thinking or User text (markdown, highlighted code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct NewThread {
    pub machine: String,
    /// The project (repo root) on that machine.
    pub project: String,
    pub agent: Option<AgentKind>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission: Permission,
    /// Work in a new git worktree on this branch (None: in the project itself).
    #[serde(default)]
    pub worktree: Option<String>,
    pub text: String,
    #[serde(default)]
    pub images: Vec<String>,
    /// A side chat of this thread (starts with its context).
    #[serde(default)]
    pub parent: Option<ThreadId>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SendBody {
    pub text: String,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub mode: SendMode,
    /// Send later (scheduled send), milliseconds since the epoch.
    #[serde(default)]
    pub send_at: Option<Ms>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AnswerBody {
    pub ask: String,
    pub option: String,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PatchThread {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub read: Option<bool>,
    #[serde(default)]
    pub order: Option<f64>,
    #[serde(default)]
    pub section: Option<Option<String>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission: Option<Permission>,
    /// Some(Some(t)): snooze until t; Some(None): wake it.
    #[serde(default)]
    pub snoozed_until: Option<Option<Ms>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Models {
    pub agent: AgentKind,
    pub models: Vec<ModelInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub thread: ThreadId,
    pub title: String,
    pub seq: u64,
    /// The matching text with the match wrapped in <mark>.
    pub excerpt: String,
}

/// What the page sends on `/api/ws`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum WsIn {
    /// Channels: "threads" (summaries), "machines", "thread:<id>" (its events),
    /// "term:<machine>:<term>" (terminal output), "plugin:<id>", "app" (personal-space events).
    Sub { ch: Vec<String> },
    Unsub { ch: Vec<String> },
    TermInput { machine: String, term: String, data: String },
    TermResize { machine: String, term: String, cols: u16, rows: u16 },
    Ping,
}

/// What the hub sends on `/api/ws`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum WsOut {
    Thread { summary: ThreadSummary },
    ThreadGone { id: ThreadId },
    Event { thread: ThreadId, event: EventView },
    Machine { machine: Machine },
    Limits { limits: AgentLimits },
    TermData { machine: String, term: String, data: String },
    TermExit { machine: String, term: String, code: Option<i32> },
    Plugin { plugin: String, payload: Value },
    /// A personal-space event (mail arrived, calendar changed…), as CERQ sent them.
    App { payload: Value },
    Pong,
}

/// The state shown for a thread, from its state and read mark.
pub fn attention(state: ThreadState, unread: bool, asking: bool) -> Attention {
    if asking || state == ThreadState::Waiting || (state == ThreadState::Error && unread) {
        Attention::NeedsYou
    } else if state.busy() {
        Attention::Working
    } else if unread {
        Attention::Ready
    } else {
        Attention::Settled
    }
}
