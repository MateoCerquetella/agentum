//! The agents behind one trait. Each driver runs its CLI and translates what it says into
//! [`agentum_proto::EventKind`]s; the translation itself is a plain struct fed one JSON message
//! at a time, so it is tested from recorded lines.
use agentum_proto::machine::ModelInfo;
use agentum_proto::{AgentKind, EventKind, Input, Permission, ToolKind};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::mpsc;

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;

/// What a running agent tells its thread.
#[derive(Debug)]
pub enum DriverOut {
    Event(EventKind),
    /// The process ended (with what it said on stderr, when it failed).
    Exited(Option<String>),
}

pub type Tx = mpsc::UnboundedSender<DriverOut>;

/// How to start (or resume) an agent for one thread.
#[derive(Clone, Debug)]
pub struct StartOpts {
    pub thread: String,
    pub bin: PathBuf,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Permission,
    /// The agent's own session id to resume.
    pub resume: Option<String>,
    /// Added to the agent's system prompt (the plugins' instructions).
    pub instructions: Option<String>,
    /// Extra environment for the agent's process (the project's browser: AGENTUM_SURF_CDP).
    pub env: Vec<(String, String)>,
}

/// One live agent session (a process, or a session on a shared server).
#[async_trait]
pub trait Session: Send {
    /// Start a turn with this input.
    async fn send(&mut self, input: &Input) -> anyhow::Result<()>;
    /// Add to the running turn. `Ok(false)`: this agent can't, queue it instead.
    async fn steer(&mut self, _input: &Input) -> anyhow::Result<bool> {
        Ok(false)
    }
    /// Cut the running turn short (it still ends with a TurnDone).
    async fn interrupt(&mut self) -> anyhow::Result<()>;
    /// Answer an `Ask` (the driver emits `Answered`).
    async fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<()>;
    /// Switch the model. `Ok(false)`: it takes effect after a restart (the thread resumes the session).
    async fn set_model(&mut self, model: &str, effort: Option<&str>) -> anyhow::Result<bool>;
    /// End the session's process.
    async fn close(&mut self);
}

#[async_trait]
pub trait Agent: Send + Sync {
    fn kind(&self) -> AgentKind;
    async fn start(&self, opts: StartOpts, out: Tx) -> anyhow::Result<Box<dyn Session>>;
    async fn models(&self, bin: PathBuf, cwd: PathBuf) -> anyhow::Result<Vec<ModelInfo>>;
}

/// The four drivers.
pub struct Agents {
    pub claude: claude::ClaudeAgent,
    pub codex: codex::CodexAgent,
    pub opencode: opencode::OpencodeAgent,
    pub cursor: cursor::CursorAgent,
}

impl Agents {
    pub fn new(idle_minutes: u64) -> Agents {
        Agents { claude: claude::ClaudeAgent, codex: codex::CodexAgent, opencode: opencode::OpencodeAgent::new(idle_minutes), cursor: cursor::CursorAgent }
    }
    pub fn get(&self, kind: AgentKind) -> &dyn Agent {
        match kind {
            AgentKind::ClaudeCode => &self.claude,
            AgentKind::Codex => &self.codex,
            AgentKind::Opencode => &self.opencode,
            AgentKind::Cursor => &self.cursor,
        }
    }
}

/// A child process speaking newline-delimited JSON on stdio.
pub struct Proc {
    pub pid: Option<u32>,
    writer: mpsc::UnboundedSender<String>,
    pub stderr: Arc<Mutex<VecDeque<String>>>,
}

impl Proc {
    /// Spawn `cmd` (in its own process group, so closing it ends its children too).
    pub fn spawn(mut cmd: Command, name: &'static str) -> anyhow::Result<(Proc, Child, ChildStdout)> {
        use std::process::Stdio;
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).process_group(0);
        let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("starting {name}: {e}"))?;
        let pid = child.id();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (writer, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(mut line) = rx.recv().await {
                line.push('\n');
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let t = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!("{name} stderr: {l}");
                let mut t = t.lock().unwrap();
                if t.len() >= 30 {
                    t.pop_front();
                }
                t.push_back(l);
            }
        });
        Ok((Proc { pid, writer, stderr: tail }, child, stdout))
    }

    pub fn write(&self, v: &serde_json::Value) -> anyhow::Result<()> {
        self.writer.send(v.to_string()).map_err(|_| anyhow::anyhow!("the agent process has exited"))
    }

    pub fn stderr_tail(&self) -> String {
        tail_text(&self.stderr)
    }

    /// SIGTERM the process group, then SIGKILL after a grace period.
    pub fn kill(&self) {
        if let Some(pid) = self.pid {
            kill_group(pid);
        }
    }
}

pub fn tail_text(t: &Mutex<VecDeque<String>>) -> String {
    let t = t.lock().unwrap();
    // The last few lines that aren't log noise.
    let lines: Vec<&str> = t.iter().map(|s| s.as_str()).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(6)..].join("\n")
}

pub fn kill_group(pid: u32) {
    let pgid = pid as i32;
    // SAFETY: plain syscalls on a process group we created.
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    });
}

/// Read JSON lines from `stdout`, handing each to `f`; at EOF wait for the child and report.
pub fn read_lines(
    stdout: ChildStdout,
    mut child: Child,
    tail: Arc<Mutex<VecDeque<String>>>,
    out: Tx,
    mut f: impl FnMut(serde_json::Value) + Send + 'static,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::with_capacity(64 * 1024, stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(l)) => {
                    let l = l.trim();
                    if l.is_empty() {
                        continue;
                    }
                    match serde_json::from_str(l) {
                        Ok(v) => f(v),
                        Err(_) => tracing::debug!("agent said (not JSON): {}", crate::util::truncate(l, 300)),
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("reading agent output: {e}");
                    break;
                }
            }
        }
        let status = child.wait().await.ok();
        // Give the stderr reader a moment to catch the last lines.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let err = match status {
            Some(s) if s.success() => None,
            Some(s) => {
                let t = tail_text(&tail);
                Some(if t.is_empty() { format!("the agent exited ({s})") } else { t })
            }
            None => Some("the agent exited".into()),
        };
        let _ = out.send(DriverOut::Exited(err));
    });
}

/// A data: URL's media type and base64 payload, or a file on this machine read as one.
pub fn image_base64(src: &str) -> Option<(String, String)> {
    use base64::Engine;
    if let Some(rest) = src.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        let media = meta.split(';').next().unwrap_or("image/png").to_string();
        return Some((media, data.to_string()));
    }
    let bytes = std::fs::read(src).ok()?;
    let ext = std::path::Path::new(src).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let media = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    };
    Some((media.to_string(), base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// The standard options of a permission prompt.
pub fn permission_options() -> Vec<agentum_proto::AskOption> {
    use agentum_proto::AskOption;
    vec![
        AskOption { id: "allow".into(), label: "Allow".into(), kind: "allow".into() },
        AskOption { id: "allow-always".into(), label: "Always allow".into(), kind: "allow-always".into() },
        AskOption { id: "deny".into(), label: "Deny".into(), kind: "deny".into() },
    ]
}

/// A tool's kind from its name (the names the agents use).
pub fn tool_kind(name: &str) -> ToolKind {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "read" | "notebookread" | "view" | "imageview" | "read_file" => ToolKind::Read,
        "glob" | "grep" | "ls" | "list" | "toolsearch" | "lsp" | "codesearch" | "find" => ToolKind::Search,
        "edit" | "write" | "multiedit" | "notebookedit" | "patch" | "apply_patch" | "multi_edit" => ToolKind::Edit,
        "bash" | "bashoutput" | "killshell" | "powershell" | "shell" | "monitor" | "exec" => ToolKind::Exec,
        "webfetch" | "websearch" | "fetch" | "web_search" => ToolKind::Fetch,
        "task" | "agent" => ToolKind::Task,
        _ => ToolKind::Other,
    }
}

/// A short title for a tool call from its input.
pub fn tool_title(name: &str, input: &serde_json::Value, cwd: &str) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let rel = |p: String| -> String {
        if !cwd.is_empty()
            && let Some(r) = p.strip_prefix(cwd)
        {
            return r.trim_start_matches('/').to_string();
        }
        p
    };
    let t = match tool_kind(name) {
        ToolKind::Exec => s("command").or_else(|| s("cmd")).or_else(|| s("description")),
        ToolKind::Read | ToolKind::Edit => {
            s("file_path").or_else(|| s("filePath")).or_else(|| s("path")).or_else(|| s("notebook_path")).map(rel)
        }
        ToolKind::Search => s("pattern").or_else(|| s("query")).or_else(|| s("path").map(rel)),
        ToolKind::Fetch => s("url").or_else(|| s("query")),
        ToolKind::Task => s("description").or_else(|| s("prompt")),
        ToolKind::Other => s("description").or_else(|| s("skill")).or_else(|| s("title")),
    };
    let t = t.unwrap_or_else(|| name.to_string());
    let first = t.lines().next().unwrap_or("").to_string();
    crate::util::truncate(&first, 200)
}

/// A unified diff from Claude's `structuredPatch` hunks.
pub fn unified_from_hunks(path: &str, hunks: &[serde_json::Value]) -> Option<String> {
    if hunks.is_empty() {
        return None;
    }
    let mut s = format!("--- a/{path}\n+++ b/{path}\n");
    for h in hunks {
        let n = |k: &str| h.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        s.push_str(&format!("@@ -{},{} +{},{} @@\n", n("oldStart"), n("oldLines"), n("newStart"), n("newLines")));
        for l in h.get("lines").and_then(|v| v.as_array()).into_iter().flatten() {
            if let Some(l) = l.as_str() {
                s.push_str(l);
                s.push('\n');
            }
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kinds_and_titles() {
        assert_eq!(tool_kind("Bash"), ToolKind::Exec);
        assert_eq!(tool_kind("MultiEdit"), ToolKind::Edit);
        assert_eq!(tool_kind("Grep"), ToolKind::Search);
        assert_eq!(tool_kind("WebFetch"), ToolKind::Fetch);
        assert_eq!(tool_kind("Agent"), ToolKind::Task);
        assert_eq!(tool_title("Read", &json!({"file_path": "/r/src/a.rs"}), "/r"), "src/a.rs");
        assert_eq!(tool_title("Bash", &json!({"command": "ls -la\necho"}), "/r"), "ls -la");
    }

    #[test]
    fn hunks() {
        let u = unified_from_hunks("a.txt", &[json!({"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-a","+b"]})]).unwrap();
        assert_eq!(u, "--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n-a\n+b\n");
    }

    #[test]
    fn data_urls() {
        let (m, d) = image_base64("data:image/jpeg;base64,AAAA").unwrap();
        assert_eq!((m.as_str(), d.as_str()), ("image/jpeg", "AAAA"));
    }
}
