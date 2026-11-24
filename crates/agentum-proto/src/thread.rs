//! Threads and their events.
use crate::Ms;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// "t_" + 10 lowercase base36 characters.
pub type ThreadId = String;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Opencode,
}
impl AgentKind {
    pub const ALL: [AgentKind; 3] = [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Opencode];
    pub fn id(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Opencode => "opencode",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::Opencode => "OpenCode",
        }
    }
    pub fn parse(s: &str) -> Option<AgentKind> {
        AgentKind::ALL.into_iter().find(|a| a.id() == s)
    }
}

/// How much the agent may do without asking.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Ask before edits and commands.
    #[default]
    Ask,
    /// Edit files freely, ask before commands.
    Edits,
    /// Read and plan only.
    Plan,
    /// Do anything (no prompts).
    Full,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadState {
    /// No turn running; the agent process may be asleep (it is resumed on the next send).
    #[default]
    Idle,
    Starting,
    Running,
    /// Running, but stopped on a question or a permission prompt.
    Waiting,
    /// The last turn ended in an error.
    Error,
    /// The turn was cut short (a stop, or agentd restarted); the next send resumes the session.
    Interrupted,
}
impl ThreadState {
    pub fn busy(self) -> bool {
        matches!(self, ThreadState::Starting | ThreadState::Running | ThreadState::Waiting)
    }
}

/// Where a thread stands for the person: the sidebar's order (Needs you, Ready, Working, Settled).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Attention {
    /// Waiting on a question or a permission, or failed.
    NeedsYou,
    /// A turn finished that you haven't read.
    Ready,
    Working,
    #[default]
    Settled,
}

/// What a new thread runs with.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ThreadSpec {
    pub agent: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning effort ("low", "medium", "high", …) as the agent names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission: Permission,
    /// The directory the agent works in (a project root or a worktree).
    pub cwd: String,
}

/// What a person sends: text plus optional image attachments (data: URLs or paths on the machine).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Input {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}
impl Input {
    pub fn text(t: impl Into<String>) -> Input {
        Input { text: t.into(), images: vec![] }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SendMode {
    /// After the running turn (or now, if idle).
    #[default]
    Queue,
    /// Into the running turn, if the agent supports it (Codex turn/steer); otherwise queued.
    Steer,
    /// Stop the running turn, then send.
    Interrupt,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind {
    Read,
    Edit,
    Exec,
    Search,
    Fetch,
    Task,
    Other,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    End,
    Interrupted,
    Error,
    MaxTurns,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AskKind {
    /// May the agent use this tool? Options are allow / allow always / deny.
    Permission,
    /// The agent asks the person something; options are its answers (and free text is allowed).
    Question,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AskOption {
    pub id: String,
    pub label: String,
    /// "allow", "allow-always", "deny", or "answer" for a question's choice.
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileDiff {
    pub path: String,
    /// A unified diff of the change, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlanEntry {
    pub text: String,
    /// "pending", "in-progress", "done".
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    /// Tokens in the context window now, and its size, when the agent says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// A rate limit the agent's account is under (Claude's 5-hour and weekly windows, Codex's).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RateLimit {
    /// "5h", "weekly", "opus-weekly", …
    pub window: String,
    /// 0–100.
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Ms>,
}

/// One event in a thread. `seq` counts from 1 with no gaps, per thread, assigned by agentd.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub ts: Ms,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "k", rename_all = "kebab-case")]
pub enum EventKind {
    /// What the person sent. `queued`: it waits for the running turn to end.
    User { text: String, #[serde(default, skip_serializing_if = "Vec::is_empty")] images: Vec<String>, #[serde(default)] queued: bool },
    /// A queued message was delivered (it now starts a turn).
    Dequeued { seq_of_user: u64 },
    TurnStarted { turn: u32 },
    /// Streaming text of an assistant message; `item` names the message.
    Delta { item: String, text: String },
    /// The whole assistant message (replaces its deltas).
    Message { item: String, text: String },
    ThinkingDelta { item: String, text: String },
    Thinking { item: String, text: String },
    /// A tool call started.
    Tool { item: String, name: String, kind: ToolKind, title: String, #[serde(default)] input: Value },
    /// Streaming output of a tool (a command's stdout).
    ToolOutput { item: String, text: String },
    ToolDone {
        item: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diffs: Vec<FileDiff>,
    },
    Plan { entries: Vec<PlanEntry> },
    /// A permission prompt or a question; the turn waits until it is answered.
    Ask {
        id: String,
        kind: AskKind,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default)]
        input: Value,
        options: Vec<AskOption>,
    },
    Answered { id: String, option: String, #[serde(default, skip_serializing_if = "Option::is_none")] text: Option<String> },
    TurnDone {
        turn: u32,
        stop: StopReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
        #[serde(default)]
        duration_ms: u64,
    },
    /// The context window right now (sent when it changes).
    Usage { usage: Usage },
    RateLimits { limits: Vec<RateLimit> },
    /// The agent's own session id (for resume) and the model it is really on.
    Session { session_id: String, #[serde(default, skip_serializing_if = "Option::is_none")] model: Option<String> },
    State { state: ThreadState },
    Title { title: String },
    /// Something the person should see that isn't the agent's (a resume, a model switch).
    Notice { text: String },
    Error { message: String, #[serde(default)] fatal: bool },
}

impl EventKind {
    /// Streaming pieces, folded into their final event once it arrives (and not kept by the hub).
    pub fn is_delta(&self) -> bool {
        matches!(self, EventKind::Delta { .. } | EventKind::ThinkingDelta { .. } | EventKind::ToolOutput { .. })
    }
}
