//! The machine protocol: hub ↔ agentd.
//!
//! agentd opens one WebSocket to the hub (`wss://<hub>/agentd`, header
//! `Authorization: Bearer <machine token>`) and keeps it open, reconnecting with backoff.
//! Every frame is one JSON text message: [`FromAgentd`] or [`ToAgentd`].
//!
//! Thread events are delivered at least once: agentd numbers them per thread and keeps every
//! event until the hub acks its `seq` (cumulatively). After a reconnect agentd resends from the
//! hub's last ack, and the hub ignores what it already has.
use crate::{AgentKind, Event, Input, Ms, SendMode, ThreadId, ThreadSpec, ThreadState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum FromAgentd {
    /// The first frame after connecting.
    Hello(Hello),
    /// New events of one thread, in order.
    Events { thread: ThreadId, events: Vec<Event> },
    /// The answer to a [`ToAgentd::Req`].
    Reply { id: u64, #[serde(flatten)] result: Reply },
    /// Terminal output, base64 of the raw bytes.
    TermData { term: String, data: String },
    TermExit { term: String, code: Option<i32> },
    /// The machine changed: agents installed or signed in, projects added.
    Update(Hello),
    Pong { ts: Ms },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Reply {
    Ok(Value),
    Err(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Hello {
    pub protocol: u32,
    /// The machine's name, as the hub knows it (the token decides; this is informative).
    pub machine: String,
    pub version: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub home: String,
    pub agents: Vec<AgentInfo>,
    pub projects: Vec<Project>,
    /// Threads agentd has a live process or unacked events for.
    pub threads: Vec<LiveThread>,
    /// Memory and load, for the machine list.
    #[serde(default)]
    pub stats: Stats,
    /// Requests this agentd understands beyond the first protocol ("secrets").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub cpus: u32,
    pub load1: f32,
    pub mem_total_mb: u64,
    pub mem_avail_mb: u64,
    /// agentd's own memory.
    pub rss_mb: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentInfo {
    pub agent: AgentKind,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Project {
    /// The repo root on that machine.
    pub path: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LiveThread {
    pub thread: ThreadId,
    pub state: ThreadState,
    pub last_seq: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ToAgentd {
    Req { id: u64, req: Req },
    /// Everything of `thread` up to `seq` is stored; agentd may forget it.
    Ack { thread: ThreadId, seq: u64 },
    Ping { ts: Ms },
}

/// A model the agent offers, for the model picker.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Req {
    /// Start a thread (or resume one: `resume` is the agent's session id) and send `input`.
    Start { thread: ThreadId, spec: ThreadSpec, #[serde(default)] resume: Option<String>, #[serde(default)] input: Option<Input>, #[serde(default)] last_seq: u64 },
    Send { thread: ThreadId, input: Input, #[serde(default)] mode: SendMode },
    Stop { thread: ThreadId },
    Answer { thread: ThreadId, ask: String, option: String, #[serde(default)] text: Option<String> },
    SetModel { thread: ThreadId, model: String, #[serde(default)] effort: Option<String> },
    /// End the agent process (it is resumed on the next send).
    Sleep { thread: ThreadId },
    Models { agent: AgentKind },
    AddProject { path: String },
    Fs(FsReq),
    Git(GitReq),
    Term(TermReq),
    /// Only to an agentd whose [`Hello::features`] has "secrets" (an older one would log the frame).
    Secrets(SecretsReq),
}

/// A directory's `.env`. Values go in, never out: agentd answers with key names only. Its Debug
/// and Display leave the value out.
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "secrets", rename_all = "kebab-case")]
pub enum SecretsReq {
    /// `{keys: [..], exists, ignored, tracked}`: the keys in `<dir>/.env`, whether git ignores it and
    /// whether it is committed.
    Keys { dir: String },
    /// Add or replace `key` (the file is rewritten atomically, mode 600). If git doesn't ignore
    /// `.env`, it is added to `<dir>/.gitignore`: the answer says `gitignore_added`.
    Set { dir: String, key: String, value: String },
    Delete { dir: String, key: String },
}

impl std::fmt::Debug for SecretsReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretsReq({self})")
    }
}

impl std::fmt::Display for SecretsReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretsReq::Keys { dir } => write!(f, "keys of {dir}/.env"),
            SecretsReq::Set { dir, key, .. } => write!(f, "set {key} in {dir}/.env"),
            SecretsReq::Delete { dir, key } => write!(f, "delete {key} from {dir}/.env"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "fs", rename_all = "kebab-case")]
pub enum FsReq {
    /// Entries of a directory: [{name, dir, size}] (hidden and ignored files included, marked).
    List { path: String },
    /// A text file (up to 2 MB) and its sha256.
    Read { path: String },
    /// Write a text file; `expect_sha` refuses to overwrite a file changed meanwhile.
    Write { path: String, content: String, #[serde(default)] expect_sha: Option<String> },
    /// Paths under `root` whose name matches `query` (fuzzy), for quick open.
    Find { root: String, query: String, #[serde(default)] limit: Option<u32> },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "git", rename_all = "kebab-case")]
pub enum GitReq {
    /// Branch, upstream, ahead/behind and changed files.
    Status { cwd: String },
    /// A unified diff: `target` "uncommitted" (default), "staged", "branch" (vs the base) or a commit.
    Diff { cwd: String, #[serde(default)] target: Option<String>, #[serde(default)] path: Option<String> },
    Commit { cwd: String, message: String, #[serde(default)] all: bool },
    Push { cwd: String },
    Log { cwd: String, #[serde(default)] limit: Option<u32>, #[serde(default)] path: Option<String> },
    Show { cwd: String, sha: String },
    Branches { cwd: String },
    /// The GitHub pull request of the current branch (`gh pr view --json`), or null.
    Pr { cwd: String },
    /// Merge the current branch's PR (`gh pr merge`).
    PrMerge { cwd: String, #[serde(default)] method: Option<String> },
    /// A new worktree for a thread: returns {path, branch}.
    WorktreeAdd { repo: String, branch: String, #[serde(default)] base: Option<String> },
    WorktreeRemove { path: String },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "do", rename_all = "kebab-case")]
pub enum TermReq {
    /// A shell (or `cmd`) in a PTY. Output streams as [`FromAgentd::TermData`].
    Open { term: String, cwd: String, cols: u16, rows: u16, #[serde(default)] cmd: Option<String> },
    /// Keystrokes, base64.
    Input { term: String, data: String },
    Resize { term: String, cols: u16, rows: u16 },
    Close { term: String },
    /// The scrollback so far (base64), to repaint a reopened terminal.
    Replay { term: String },
    /// Terminals open on this machine: [{term, cwd, cmd}].
    List,
}
