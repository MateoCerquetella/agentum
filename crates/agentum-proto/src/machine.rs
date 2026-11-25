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
    /// A message from a WebSocket agentd opened for the hub ([`SurfReq::Open`]).
    Sock(SockMsg),
    /// That WebSocket closed.
    SockClose { sock: String },
}

/// One WebSocket message relayed between the hub and agentd (the Surf viewer's screencast).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SockMsg {
    pub sock: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A binary message, base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
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
    /// To a WebSocket agentd opened for the hub ([`SurfReq::Open`]).
    Sock(SockMsg),
    /// Close it.
    SockClose { sock: String },
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
    /// The projects' browsers (Surf).
    Surf(SurfReq),
    /// Only to an agentd whose [`Hello::features`] has "secrets" (an older one would log the frame).
    Secrets(SecretsReq),
    /// The agent's subscription limits now, asked of its provider: a [`UsageReply`].
    /// Only to an agentd whose [`Hello::features`] has "usage".
    Usage { agent: AgentKind },
    /// The slash commands and skills `agent` has in `cwd` (the project's and the user's): a
    /// `Vec<CommandInfo>`. Only to an agentd whose [`Hello::features`] has "commands".
    Commands { agent: AgentKind, cwd: String },
    /// A queued message (its `User` event's seq): `steer` it into the running turn now, or `drop`
    /// it. Only to an agentd whose [`Hello::features`] has "queued".
    Queued { thread: ThreadId, seq: u64, action: QueuedAction },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QueuedAction {
    Steer,
    Drop,
}

/// A slash command or a skill, for the composer's `/` menu.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CommandInfo {
    /// Typed after the slash (`review`, `frontend:lint`, `compact`).
    pub name: String,
    pub kind: CommandKind,
    /// "project", "user", "plugin" or "builtin".
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What goes after the name (`argument-hint`: "[file]").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandKind {
    Command,
    Skill,
    Builtin,
}

/// The answer to [`Req::Usage`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct UsageReply {
    #[serde(default)]
    pub limits: Vec<crate::RateLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Who the agent is signed in as on this machine (an older agentd doesn't say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AgentAccount>,
}

/// The account an agent is signed in to: ids and names only, never a credential.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentAccount {
    /// Stable across machines signed in to the same account.
    pub id: String,
    pub label: String,
}

/// The response header of the hub's `/agentd` upgrade naming the agents it knows ("claude-code,
/// codex,opencode,cursor"). An agentd reports only those (an older hub can't read the others).
pub const AGENTS_HEADER: &str = "x-agentum-agents";

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
    /// What a new thread can start from: `{default, current, dirty, branches: [..], remote: [..],
    /// worktrees: [{path, branch, locked, prunable}]}` (the repo's own checkout left out).
    StartPoints { repo: String },
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

/// Surf: a project's own browser (Steel in a Docker container on the project's machine). Its
/// DevTools (CDP) and viewer listen on the machine's loopback only; the hub shows the viewer
/// through agentd ([`SurfReq::Get`], [`SurfReq::Open`]).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "surf", rename_all = "kebab-case")]
pub enum SurfReq {
    /// Start the project's browser (or wake it, or reuse it): its [`SurfInfo`] once it answers.
    /// `idle_hours`: stop browsers unused for this long (0: never); agentd keeps the last value.
    Ensure { project: String, #[serde(default)] idle_hours: Option<f64> },
    /// The browsers on this machine: [SurfInfo].
    List,
    /// Stop the project's browser and remove its container (its profile, with sign-ins, is kept).
    Stop { project: String },
    /// GET a page of the viewer (`path`: `/v1/sessions/debug?…`): {status, body, origin}; `origin`
    /// is the address Steel writes into it (`127.0.0.1:<port>`).
    Get { id: String, path: String },
    /// Open a WebSocket to the viewer (`path`: `/v1/sessions/cast?…`) as `sock`: its messages
    /// arrive as [`FromAgentd::Sock`], the hub's go as [`ToAgentd::Sock`].
    Open { id: String, sock: String, path: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SurfInfo {
    /// Short and stable per project (the container is `agentum-surf-<id>`).
    pub id: String,
    /// The repo root.
    pub project: String,
    /// "running", "stopped" (idle: the next thread wakes it), "starting".
    pub state: String,
    /// Docker's words ("Up 2 hours").
    #[serde(default)]
    pub status: String,
    /// The viewer and Steel's API, on the machine's 127.0.0.1.
    pub api_port: u16,
    /// Chrome's DevTools, on the machine's 127.0.0.1.
    pub cdp_port: u16,
    /// What agents connect to: `ws://127.0.0.1:<api_port>/` (Playwright's `connectOverCDP`).
    #[serde(default)]
    pub cdp_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<Ms>,
}
