//! Cursor: `cursor-agent acp`, the Agent Client Protocol (JSON-RPC 2.0, one message per line on
//! stdio), one process per thread.
//!
//! Why ACP and not `cursor-agent -p --output-format stream-json`: print mode is a process per
//! turn that can't ask anything (a tool either runs, with `--force`, or is refused), can't switch
//! the model or the mode, and only stops by being killed. Over ACP the session stays up between
//! turns; permission prompts (`session/request_permission`) and Cursor's questions and plans
//! (`cursor/ask_question`, `cursor/create_plan`) reach the person; Plan is Cursor's own read-only
//! mode; the model switches live (`session/set_config_option`); `session/cancel` ends a turn
//! cleanly; `session/load` resumes; images go in the prompt; tool calls carry their diffs; todos
//! arrive as `plan` updates and `cursor/update_todos`. BB drives Cursor the same way.
//!
//! What ACP doesn't give (Cursor 2026.11): a turn's tokens (print mode's `result` has them; the
//! context comes as `usage_update` when Cursor sends it), steering (messages queue), reasoning
//! effort (a model id carries its own: `claude-opus-5-5[effort=medium,…]`, and only the listed
//! ids are accepted) and a system prompt: the plugins' instructions follow the first message of
//! each process (a resumed session hears them again, so changed ones arrive too).
//!
//! Cursor remembers the last model picked (over ACP or `--model`) as the machine's default for
//! new sessions, its CLI included: a thread's model is set only when it differs from the
//! session's. (`cursor-agent --model X acp` doesn't apply X to ACP sessions; it only saves it.)
use super::{Agent, DriverOut, Proc, Session, StartOpts, Tx, permission_options, tool_kind, tool_title};
use agentum_proto::machine::{AgentAccount, ModelInfo, UsageReply};
use agentum_proto::{AgentKind, AskKind, AskOption, EventKind, FileDiff, Input, Permission, PlanEntry, RateLimit, StopReason, ToolKind, Usage};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::oneshot;

#[derive(Debug, PartialEq)]
pub enum Act {
    Ev(EventKind),
    /// A JSON-RPC message for Cursor (an answer to its request).
    Write(Value),
}

#[derive(Debug, Clone)]
struct PendingAsk {
    rpc_id: Value,
    method: String,
    params: Value,
}

#[derive(Debug, Default)]
struct ToolSeen {
    output: String,
    diffs: Vec<FileDiff>,
    done: bool,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn refuse(id: &Value, what: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("agentd does not handle {what}")}})
}

/// Cursor's messages → thread events.
pub struct Translator {
    cwd: String,
    /// Names this process's items and asks (unique across the thread's processes).
    tag: String,
    pub permission: Permission,
    items: u64,
    /// The text streaming now: (item, thinking, text so far).
    open: Option<(String, bool, String)>,
    tools: HashMap<String, ToolSeen>,
    asks: HashMap<String, PendingAsk>,
    todos: Vec<(String, PlanEntry)>,
    /// `session/load` replays the conversation: the thread has it already.
    pub replaying: bool,
    /// We sent `session/cancel`.
    pub cancelling: bool,
    started: Option<Instant>,
    warned: HashSet<String>,
}

impl Translator {
    pub fn new(cwd: &str, permission: Permission, tag: &str) -> Translator {
        Translator {
            cwd: cwd.to_string(),
            tag: tag.to_string(),
            permission,
            items: 0,
            open: None,
            tools: HashMap::new(),
            asks: HashMap::new(),
            todos: vec![],
            replaying: false,
            cancelling: false,
            started: None,
            warned: HashSet::new(),
        }
    }

    fn warn_once(&mut self, what: String) {
        if self.warned.insert(what.clone()) {
            tracing::debug!("cursor: ignoring {what}");
        }
    }

    pub fn turn_started(&mut self) {
        self.cancelling = false;
        self.started = Some(Instant::now());
    }

    /// The streaming text ends: its whole message (or thinking) replaces the deltas.
    fn flush(&mut self, out: &mut Vec<Act>) {
        if let Some((item, thinking, text)) = self.open.take()
            && !text.is_empty()
        {
            out.push(Act::Ev(if thinking { EventKind::Thinking { item, text } } else { EventKind::Message { item, text } }));
        }
    }

    pub fn notification(&mut self, method: &str, p: &Value) -> Vec<Act> {
        let mut out = Vec::new();
        if self.replaying {
            return out;
        }
        match method {
            "session/update" => self.update(p.get("update").unwrap_or(&Value::Null), &mut out),
            "cursor/update_todos" => self.todos(p, &mut out),
            "cursor/task" => self.task(p, &mut out),
            other => self.warn_once(format!("notification {other}")),
        }
        out
    }

    fn update(&mut self, u: &Value, out: &mut Vec<Act>) {
        let kind = s(u, "sessionUpdate");
        match kind.as_str() {
            "agent_message_chunk" | "agent_thought_chunk" => {
                let c = u.get("content").unwrap_or(&Value::Null);
                let text = if s(c, "type") == "text" { s(c, "text") } else { String::new() };
                if text.is_empty() {
                    return;
                }
                let thinking = kind == "agent_thought_chunk";
                if self.open.as_ref().is_none_or(|o| o.1 != thinking) {
                    self.flush(out);
                    self.items += 1;
                    self.open = Some((format!("{}.{}{}", self.tag, self.items, if thinking { 't' } else { 'm' }), thinking, String::new()));
                }
                let o = self.open.as_mut().unwrap();
                o.2.push_str(&text);
                let item = o.0.clone();
                out.push(Act::Ev(if thinking { EventKind::ThinkingDelta { item, text } } else { EventKind::Delta { item, text } }));
            }
            "tool_call" | "tool_call_update" => {
                self.flush(out);
                let id = s(u, "toolCallId");
                if id.is_empty() {
                    return;
                }
                self.tool_seen(&id, u, out);
                self.tool_progress(&id, u, out);
            }
            "plan" => {
                self.flush(out);
                let entries = u.get("entries").and_then(|e| e.as_array()).into_iter().flatten().filter_map(|e| entry(&s(e, "content"), &s(e, "status"))).collect();
                out.push(Act::Ev(EventKind::Plan { entries }));
            }
            "usage_update" => {
                let n = |k: &str| u.get(k).and_then(|x| x.as_u64());
                if let (Some(used), Some(size)) = (n("used"), n("size")) {
                    out.push(Act::Ev(EventKind::Usage { usage: Usage { context_used: Some(used), context_window: Some(size), ..Default::default() } }));
                }
            }
            // Our own words echoed back, Cursor's slash commands and settings. And its title: it
            // names the whole first prompt, the plugins' instructions included (it came out as
            // "Agentum Instructions"); the hub titles the thread from its first message.
            "user_message_chunk" | "available_commands_update" | "current_mode_update" | "config_option_update" | "session_info_update" => {}
            other => self.warn_once(format!("update {other}")),
        }
    }

    /// The tool call's start, once (an update or a permission request may be the first word of it).
    fn tool_seen(&mut self, id: &str, tc: &Value, out: &mut Vec<Act>) {
        if self.tools.contains_key(id) {
            return;
        }
        self.tools.insert(id.to_string(), ToolSeen::default());
        let input = tc.get("rawInput").cloned().unwrap_or(Value::Null);
        let acp = s(tc, "kind");
        let name = input.get("_toolName").and_then(|n| n.as_str()).map(str::to_string).unwrap_or_else(|| if acp.is_empty() { "tool".into() } else { acp.clone() });
        let kind = match acp.as_str() {
            "read" => ToolKind::Read,
            "edit" | "delete" | "move" => ToolKind::Edit,
            "search" => ToolKind::Search,
            "execute" => ToolKind::Exec,
            "fetch" => ToolKind::Fetch,
            _ => tool_kind(&name),
        };
        let title = self.title_of(tc, &name, &input);
        out.push(Act::Ev(EventKind::Tool { item: id.to_string(), name, kind, title, input }));
    }

    fn title_of(&self, tc: &Value, name: &str, input: &Value) -> String {
        let t = s(tc, "title");
        let t = t.trim();
        let t = t.strip_prefix('`').and_then(|x| x.strip_suffix('`')).unwrap_or(t);
        if !t.is_empty() {
            return crate::util::truncate(t.lines().next().unwrap_or(""), 200);
        }
        let loc = tc.get("locations").and_then(|l| l.get(0)).map(|l| s(l, "path")).filter(|p| !p.is_empty());
        match loc {
            Some(p) => p.strip_prefix(&self.cwd).map(|r| r.trim_start_matches('/').to_string()).unwrap_or(p),
            None => tool_title(name, input, &self.cwd),
        }
    }

    fn tool_progress(&mut self, id: &str, u: &Value, out: &mut Vec<Act>) {
        let content = u.get("content").and_then(|c| c.as_array());
        let text: Vec<String> = content.into_iter().flatten().filter(|c| s(c, "type") == "content").map(|c| s(c.get("content").unwrap_or(&Value::Null), "text")).filter(|t| !t.is_empty()).collect();
        let text = text.join("\n");
        let diffs: Vec<FileDiff> = content
            .into_iter()
            .flatten()
            .filter(|c| s(c, "type") == "diff")
            .map(|c| FileDiff { path: s(c, "path"), unified: None, old: c.get("oldText").and_then(|o| o.as_str()).map(str::to_string), new: c.get("newText").and_then(|o| o.as_str()).map(str::to_string) })
            .collect();
        let status = s(u, "status");
        let Some(seen) = self.tools.get_mut(id) else { return };
        if !diffs.is_empty() {
            seen.diffs = diffs;
        }
        if seen.done {
            return;
        }
        if status == "completed" || status == "failed" {
            seen.done = true;
            let output = Some(text)
                .filter(|t| !t.is_empty())
                .or_else(|| match u.get("rawOutput") {
                    Some(Value::String(o)) => Some(o.clone()),
                    Some(o @ Value::Object(m)) => ["output", "stdout", "content", "error"].iter().find_map(|k| m.get(*k).and_then(|x| x.as_str()).map(str::to_string)).or_else(|| (m.len() > 1 || !m.contains_key("exitCode")).then(|| o.to_string())),
                    _ => None,
                })
                .filter(|o| !o.trim().is_empty())
                .map(|o| crate::util::truncate(&o, 32 * 1024));
            out.push(Act::Ev(EventKind::ToolDone { item: id.to_string(), ok: status == "completed", output, diffs: seen.diffs.clone() }));
        } else if !text.is_empty() && text != seen.output {
            // ACP replaces a tool's content: what is new is the part after what we showed.
            let new = text.strip_prefix(seen.output.as_str()).map(str::to_string).unwrap_or_else(|| text.clone());
            seen.output = text;
            if !new.is_empty() {
                out.push(Act::Ev(EventKind::ToolOutput { item: id.to_string(), text: new }));
            }
        }
    }

    /// `cursor/update_todos`: the todo list (merged into the last one, or replacing it).
    fn todos(&mut self, p: &Value, out: &mut Vec<Act>) {
        self.flush(out);
        if !p.get("merge").and_then(|m| m.as_bool()).unwrap_or(false) {
            self.todos.clear();
        }
        for t in p.get("todos").and_then(|t| t.as_array()).into_iter().flatten() {
            let id = s(t, "id");
            let e = entry(&s(t, "content"), &s(t, "status"));
            let at = self.todos.iter().position(|(i, _)| !id.is_empty() && *i == id);
            match (at, e) {
                (Some(i), Some(e)) => self.todos[i].1 = e,
                (Some(i), None) => {
                    self.todos.remove(i);
                }
                (None, Some(e)) => self.todos.push((id, e)),
                (None, None) => {}
            }
        }
        out.push(Act::Ev(EventKind::Plan { entries: self.todos.iter().map(|(_, e)| e.clone()).collect() }));
    }

    /// `cursor/task`: a subagent starts (its tool call may have said so already).
    fn task(&mut self, p: &Value, out: &mut Vec<Act>) {
        let id = s(p, "toolCallId");
        if id.is_empty() || self.tools.contains_key(&id) {
            return;
        }
        self.flush(out);
        self.tools.insert(id.clone(), ToolSeen::default());
        let what = Some(s(p, "description")).filter(|d| !d.is_empty()).unwrap_or_else(|| s(p, "prompt"));
        out.push(Act::Ev(EventKind::Tool { item: id, name: "task".into(), kind: ToolKind::Task, title: crate::util::truncate(what.lines().next().unwrap_or("Subagent"), 200), input: p.clone() }));
    }

    pub fn server_request(&mut self, rpc_id: Value, method: &str, p: &Value) -> Vec<Act> {
        let mut out = Vec::new();
        let id = format!("{}-{}", self.tag, match &rpc_id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        match method {
            "session/request_permission" => {
                self.flush(&mut out);
                let tc = p.get("toolCall").cloned().unwrap_or(Value::Null);
                let tool = s(&tc, "toolCallId");
                if !tool.is_empty() {
                    self.tool_seen(&tool, &tc, &mut out);
                }
                let edit = matches!(s(&tc, "kind").as_str(), "edit" | "delete" | "move");
                let auto = match self.permission {
                    Permission::Full => true,
                    Permission::Edits => edit,
                    Permission::Ask | Permission::Plan => false,
                };
                if auto && let Some(o) = pick_option(p, "allow_once").or_else(|| pick_option(p, "allow_always")) {
                    out.push(Act::Write(reply(&rpc_id, json!({"outcome": {"outcome": "selected", "optionId": o}}))));
                    return out;
                }
                let input = tc.get("rawInput").cloned().unwrap_or(Value::Null);
                let what = self.title_of(&tc, "tool", &input);
                let title = match (s(&tc, "kind").as_str(), input.get("command").and_then(|c| c.as_str())) {
                    ("execute", Some(c)) => format!("Run: {}", crate::util::truncate(c.lines().next().unwrap_or(""), 200)),
                    ("execute", None) => format!("Run: {what}"),
                    _ => what,
                };
                out.push(Act::Ev(EventKind::Ask { id: id.clone(), kind: AskKind::Permission, title, detail: None, input, options: permission_options() }));
            }
            "cursor/ask_question" => {
                self.flush(&mut out);
                let q = p.get("questions").and_then(|q| q.get(0)).cloned().unwrap_or(Value::Null);
                let options = q.get("options").and_then(|o| o.as_array()).into_iter().flatten().map(|o| AskOption { id: s(o, "id"), label: s(o, "label"), kind: "answer".into() }).collect();
                let detail = Some(s(p, "title")).filter(|t| !t.is_empty());
                out.push(Act::Ev(EventKind::Ask { id: id.clone(), kind: AskKind::Question, title: s(&q, "prompt"), detail, input: p.clone(), options }));
            }
            "cursor/create_plan" => {
                self.flush(&mut out);
                let entries: Vec<PlanEntry> = p.get("todos").and_then(|t| t.as_array()).into_iter().flatten().filter_map(|t| entry(&s(t, "content"), &s(t, "status"))).collect();
                if !entries.is_empty() {
                    out.push(Act::Ev(EventKind::Plan { entries }));
                }
                let title = Some(s(p, "name")).filter(|n| !n.is_empty()).map(|n| format!("Build this plan? {n}")).unwrap_or_else(|| "Build this plan?".into());
                let detail = Some(s(p, "overview")).filter(|o| !o.is_empty()).or_else(|| Some(s(p, "plan")).filter(|o| !o.is_empty())).map(|d| crate::util::truncate(&d, 4000));
                let options = vec![
                    AskOption { id: "accept".into(), label: "Accept".into(), kind: "answer".into() },
                    AskOption { id: "reject".into(), label: "Reject".into(), kind: "answer".into() },
                ];
                out.push(Act::Ev(EventKind::Ask { id: id.clone(), kind: AskKind::Question, title, detail, input: p.clone(), options }));
            }
            "cursor/update_todos" => {
                self.todos(p, &mut out);
                let todos: Vec<Value> = self.todos.iter().map(|(i, e)| json!({"id": i, "content": e.text, "status": todo_status(&e.status)})).collect();
                out.push(Act::Write(reply(&rpc_id, json!({"outcome": {"outcome": "accepted", "todos": todos}}))));
                return out;
            }
            "cursor/task" => {
                self.task(p, &mut out);
                out.push(Act::Write(reply(&rpc_id, json!({}))));
                return out;
            }
            "cursor/generate_image" => {
                out.push(Act::Write(reply(&rpc_id, json!({"outcome": {"outcome": "rejected", "reason": "Agentum doesn't make images."}}))));
                return out;
            }
            // The editor's files and terminals: not offered at initialize.
            other => {
                self.warn_once(format!("request {other}"));
                out.push(Act::Write(refuse(&rpc_id, other)));
                return out;
            }
        }
        self.asks.insert(id, PendingAsk { rpc_id, method: method.to_string(), params: p.clone() });
        out
    }

    /// The reply to an `Ask`, and its `Answered` event.
    pub fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<(Value, EventKind)> {
        let a = self.asks.remove(ask).ok_or_else(|| anyhow::anyhow!("no open question {ask}"))?;
        let outcome = match a.method.as_str() {
            "session/request_permission" => {
                let o = match option {
                    "allow" => pick_option(&a.params, "allow_once"),
                    "allow-always" => pick_option(&a.params, "allow_always").or_else(|| pick_option(&a.params, "allow_once")),
                    _ => pick_option(&a.params, "reject_once").or_else(|| pick_option(&a.params, "reject_always")),
                };
                match o {
                    Some(o) => json!({"outcome": "selected", "optionId": o}),
                    None => json!({"outcome": "cancelled"}),
                }
            }
            "cursor/ask_question" => {
                let q = a.params.get("questions").and_then(|q| q.get(0)).map(|q| s(q, "id")).unwrap_or_default();
                match (option, text) {
                    ("deny" | "cancel" | "cancelled", _) => json!({"outcome": "skipped", "reason": text.unwrap_or("The person declined to answer.")}),
                    ("" | "other", Some(t)) => json!({"outcome": "skipped", "reason": format!("The person answered in their own words: {t}")}),
                    (o, _) => json!({"outcome": "answered", "answers": [{"questionId": q, "selectedOptionIds": [o]}]}),
                }
            }
            "cursor/create_plan" => match option {
                "accept" => json!({"outcome": "accepted"}),
                _ => json!({"outcome": "rejected", "reason": text.unwrap_or("The person rejected the plan.")}),
            },
            _ => json!({"outcome": "cancelled"}),
        };
        Ok((reply(&a.rpc_id, json!({ "outcome": outcome })), EventKind::Answered { id: ask.to_string(), option: option.to_string(), text: text.map(str::to_string) }))
    }

    /// A stop: every open question is cancelled (ACP wants an answer to each).
    pub fn cancel_asks(&mut self) -> Vec<Act> {
        let mut out = Vec::new();
        for (id, a) in self.asks.drain() {
            out.push(Act::Write(reply(&a.rpc_id, json!({"outcome": {"outcome": "cancelled"}}))));
            out.push(Act::Ev(EventKind::Answered { id, option: "cancelled".into(), text: None }));
        }
        out
    }

    /// `session/prompt` answered: the turn is over.
    pub fn prompt_done(&mut self, res: Result<Value, Value>) -> Vec<Act> {
        let mut out = Vec::new();
        self.flush(&mut out);
        // Questions the turn left open are moot.
        for id in self.asks.drain().map(|(k, _)| k).collect::<Vec<_>>() {
            out.push(Act::Ev(EventKind::Answered { id, option: "cancelled".into(), text: None }));
        }
        let stop = match &res {
            Ok(r) => match s(r, "stopReason").as_str() {
                "cancelled" => StopReason::Interrupted,
                "max_tokens" | "max_turn_requests" => StopReason::MaxTurns,
                "refusal" => {
                    out.push(Act::Ev(EventKind::Error { message: "The model refused to go on.".into(), fatal: false }));
                    StopReason::Error
                }
                _ => StopReason::End,
            },
            Err(_) if self.cancelling => StopReason::Interrupted,
            Err(e) => {
                out.push(Act::Ev(EventKind::Error { message: error_text(e), fatal: false }));
                StopReason::Error
            }
        };
        self.cancelling = false;
        let duration_ms = self.started.take().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
        out.push(Act::Ev(EventKind::TurnDone { turn: 0, stop, usage: None, cost_usd: None, duration_ms }));
        out
    }
}

/// A plan entry from ACP's or Cursor's status (a cancelled todo leaves the list).
fn entry(text: &str, status: &str) -> Option<PlanEntry> {
    let status = match status {
        "in_progress" => "in-progress",
        "completed" => "done",
        "cancelled" => return None,
        _ => "pending",
    };
    (!text.is_empty()).then(|| PlanEntry { text: text.to_string(), status: status.into() })
}

fn todo_status(s: &str) -> &'static str {
    match s {
        "in-progress" => "in_progress",
        "done" => "completed",
        _ => "pending",
    }
}

/// The id of the permission option of this kind ("allow_once", "reject_once"…).
fn pick_option(p: &Value, kind: &str) -> Option<String> {
    p.get("options").and_then(|o| o.as_array()).into_iter().flatten().find(|o| s(o, "kind") == kind).map(|o| s(o, "optionId")).filter(|o| !o.is_empty())
}

/// A JSON-RPC error's words (Cursor puts the useful part in `data.message`).
fn error_text(e: &Value) -> String {
    let m = s(e, "message");
    let d = e.get("data").map(|d| match d {
        Value::String(t) => t.clone(),
        d => s(d, "message"),
    });
    match d.filter(|d| !d.is_empty() && *d != m) {
        Some(d) if m.is_empty() || m == "Internal error" => d,
        Some(d) => format!("{m}: {d}"),
        None if m.is_empty() => e.to_string(),
        None => m,
    }
}

/// The prompt: the images, the text, then (the first time) the plugins' instructions.
pub fn prompt_blocks(input: &Input, instructions: Option<&str>, images: bool) -> Value {
    let mut blocks = Vec::new();
    if images {
        for img in &input.images {
            if let Some((media, data)) = super::image_base64(img) {
                blocks.push(json!({"type": "image", "mimeType": media, "data": data}));
            }
        }
    }
    blocks.push(json!({"type": "text", "text": input.text}));
    if let Some(i) = instructions.filter(|i| !i.trim().is_empty()) {
        blocks.push(json!({"type": "text", "text": format!("(Instructions from Agentum, the app this session runs in, for the whole session:)\n\n{i}")}));
    }
    Value::Array(blocks)
}

/// The session's models: its `model` setting's choices (else ACP's model list), the current one
/// marked default. Ids are exact: Cursor refuses any other.
pub fn models_of(session: &Value) -> Vec<ModelInfo> {
    let opt = session.get("configOptions").and_then(|c| c.as_array()).into_iter().flatten().find(|o| s(o, "id") == "model");
    let (current, list): (String, Vec<(String, String)>) = match opt {
        Some(o) => (s(o, "currentValue"), o.get("options").and_then(|x| x.as_array()).into_iter().flatten().map(|m| (s(m, "value"), s(m, "name"))).collect()),
        None => {
            let m = session.get("models").unwrap_or(&Value::Null);
            (s(m, "currentModelId"), m.get("availableModels").and_then(|x| x.as_array()).into_iter().flatten().map(|m| (s(m, "modelId"), s(m, "name"))).collect())
        }
    };
    list.into_iter()
        .filter(|(id, _)| !id.is_empty())
        .map(|(id, name)| ModelInfo { label: if name.is_empty() { id.clone() } else { name }, efforts: vec![], default_effort: None, default: id == current, context_window: context_of(&id), id })
        .collect()
}

/// "claude-opus-5-5[context=300k,…]" → 300 000 tokens.
fn context_of(id: &str) -> Option<u64> {
    let params = id.split_once('[')?.1.trim_end_matches(']');
    let v = params.split(',').find_map(|p| p.trim().strip_prefix("context="))?.to_ascii_lowercase();
    let (num, mul) = match v.strip_suffix('k') {
        Some(n) => (n, 1_000.0),
        None => match v.strip_suffix('m') {
            Some(n) => (n, 1_000_000.0),
            None => (v.as_str(), 1.0),
        },
    };
    num.parse::<f64>().ok().map(|n| (n * mul) as u64)
}

/// The value of one of the session's settings ("mode", "model").
fn setting(session: &Value, id: &str) -> Option<String> {
    let from_options = session.get("configOptions").and_then(|c| c.as_array()).into_iter().flatten().find(|o| s(o, "id") == id).map(|o| s(o, "currentValue"));
    from_options.or_else(|| match id {
        "mode" => session.pointer("/modes/currentModeId").and_then(|m| m.as_str()).map(str::to_string),
        "model" => session.pointer("/models/currentModelId").and_then(|m| m.as_str()).map(str::to_string),
        _ => None,
    })
}

pub fn mode_of(p: Permission) -> &'static str {
    match p {
        Permission::Plan => "plan",
        Permission::Ask | Permission::Edits | Permission::Full => "agent",
    }
}

enum Waiter {
    Call(oneshot::Sender<Result<Value, Value>>),
    /// `session/prompt`: its answer ends the turn.
    Prompt,
    /// `session/load`: its answer ends the replay.
    Load(oneshot::Sender<Result<Value, Value>>),
}

type Pending = Arc<Mutex<HashMap<u64, Waiter>>>;

/// A JSON-RPC connection to one `cursor-agent acp`.
struct Rpc {
    proc: Proc,
    pending: Pending,
    next: AtomicU64,
}

impl Rpc {
    fn request(&self, method: &str, params: Value, w: impl FnOnce(u64) -> Waiter) -> anyhow::Result<u64> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.pending.lock().unwrap().insert(id, w(id));
        self.proc.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        Ok(id)
    }

    async fn wait(&self, id: u64, method: &str, rx: oneshot::Receiver<Result<Value, Value>>, timeout: Duration) -> anyhow::Result<Value> {
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => anyhow::bail!("cursor {method}: {}", error_text(&e)),
            Ok(Err(_)) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let t = self.proc.stderr_tail();
                anyhow::bail!("cursor-agent exited during {method}{}", if t.is_empty() { String::new() } else { format!(": {t}") })
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                anyhow::bail!("cursor-agent did not answer {method} in {}s", timeout.as_secs())
            }
        }
    }

    async fn call(&self, method: &str, params: Value, timeout: Duration) -> anyhow::Result<Value> {
        let (tx, rx) = oneshot::channel();
        let id = self.request(method, params, |_| Waiter::Call(tx))?;
        self.wait(id, method, rx, timeout).await
    }

    fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        self.proc.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Change a setting of the session: `session/set_config_option`, else the older
    /// `session/set_mode` / `session/set_model`.
    async fn set(&self, session: &str, options: bool, id: &str, value: &str) -> anyhow::Result<()> {
        let t = Duration::from_secs(20);
        if options {
            self.call("session/set_config_option", json!({"sessionId": session, "configId": id, "value": value}), t).await?;
        } else if id == "mode" {
            self.call("session/set_mode", json!({"sessionId": session, "modeId": value}), t).await?;
        } else {
            self.call("session/set_model", json!({"sessionId": session, "modelId": value}), t).await?;
        }
        Ok(())
    }
}

fn emit(acts: Vec<Act>, out: &Tx, writer: &tokio::sync::mpsc::UnboundedSender<String>) {
    for a in acts {
        match a {
            Act::Ev(e) => {
                let _ = out.send(DriverOut::Event(e));
            }
            Act::Write(v) => {
                let _ = writer.send(v.to_string());
            }
        }
    }
}

/// Start `cursor-agent acp` and initialize it: the connection and the agent's capabilities.
async fn connect(bin: &PathBuf, cwd: &PathBuf, env: &[(String, String)], out: Tx, tr: Arc<Mutex<Translator>>) -> anyhow::Result<(Rpc, Value)> {
    let mut cmd = Command::new(bin);
    cmd.arg("acp").current_dir(cwd).envs(env.iter().map(|(k, v)| (k, v)));
    let (proc, child, stdout) = Proc::spawn(cmd, "cursor")?;
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (p2, o2, writer) = (pending.clone(), out.clone(), proc.writer.clone());
    super::read_lines(stdout, child, proc.stderr.clone(), out, move |v| {
        let method = v.get("method").and_then(|m| m.as_str()).map(str::to_string);
        let id = v.get("id").cloned().filter(|i| !i.is_null());
        let params = v.get("params").unwrap_or(&Value::Null);
        let acts = match (method, id) {
            (None, Some(id)) => {
                let res = match v.get("error") {
                    Some(e) => Err(e.clone()),
                    None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                };
                match id.as_u64().and_then(|i| p2.lock().unwrap().remove(&i)) {
                    Some(Waiter::Call(w)) => {
                        let _ = w.send(res);
                        vec![]
                    }
                    Some(Waiter::Load(w)) => {
                        tr.lock().unwrap().replaying = false;
                        let _ = w.send(res);
                        vec![]
                    }
                    Some(Waiter::Prompt) => tr.lock().unwrap().prompt_done(res),
                    None => vec![],
                }
            }
            (Some(m), Some(id)) => tr.lock().unwrap().server_request(id, &m, params),
            (Some(m), None) => tr.lock().unwrap().notification(&m, params),
            _ => vec![],
        };
        emit(acts, &o2, &writer);
    });
    let rpc = Rpc { proc, pending, next: AtomicU64::new(0) };
    // No editor files or terminals: Cursor uses its own tools.
    let init = json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false}, "clientInfo": {"name": "agentd", "title": "Agentum", "version": env!("CARGO_PKG_VERSION")}});
    match rpc.call("initialize", init, Duration::from_secs(60)).await {
        Ok(r) => Ok((rpc, r)),
        Err(e) => {
            rpc.proc.kill();
            Err(e)
        }
    }
}

/// `session/new`. Signed in (`cursor-agent login`), Cursor needs no `authenticate`; signed out,
/// `authenticate` only hangs (it would start a sign-in on that machine): say so instead.
async fn new_session(rpc: &Rpc, cwd: &str) -> anyhow::Result<Value> {
    rpc.call("session/new", json!({"cwd": cwd, "mcpServers": []}), Duration::from_secs(60)).await.map_err(|e| {
        if e.to_string().contains("Authentication required") { anyhow::anyhow!("Cursor isn't signed in on this machine: run `cursor-agent login` there") } else { e }
    })
}

pub struct CursorAgent;

struct Live {
    rpc: Rpc,
    tr: Arc<Mutex<Translator>>,
    out: Tx,
    session: String,
    /// Settings go through `session/set_config_option`.
    options: bool,
    images: bool,
    /// The plugins' instructions, until the first message carries them.
    instructions: Option<String>,
}

#[async_trait]
impl Agent for CursorAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Cursor
    }

    async fn start(&self, opts: StartOpts, out: Tx) -> anyhow::Result<Box<dyn Session>> {
        let cwd = opts.cwd.to_string_lossy().to_string();
        let tr = Arc::new(Mutex::new(Translator::new(&cwd, opts.permission, &crate::util::random_hex(3))));
        let (rpc, init) = connect(&opts.bin, &opts.cwd, &opts.env, out.clone(), tr.clone()).await?;
        let caps = init.get("agentCapabilities").unwrap_or(&Value::Null);
        let images = caps.pointer("/promptCapabilities/image").and_then(|i| i.as_bool()).unwrap_or(false);
        let can_load = caps.get("loadSession").and_then(|l| l.as_bool()).unwrap_or(false);
        let started = async {
            if let Some(id) = opts.resume.as_ref().filter(|_| can_load) {
                tr.lock().unwrap().replaying = true;
                let (tx, rx) = oneshot::channel();
                let rid = rpc.request("session/load", json!({"sessionId": id, "cwd": cwd, "mcpServers": []}), |_| Waiter::Load(tx))?;
                match rpc.wait(rid, "session/load", rx, Duration::from_secs(120)).await {
                    Ok(r) => return Ok::<_, anyhow::Error>((id.clone(), r)),
                    Err(e) => {
                        tr.lock().unwrap().replaying = false;
                        tracing::info!("cursor: resuming {id} failed: {e:#}");
                        let _ = out.send(DriverOut::Event(EventKind::Notice { text: "Cursor couldn't resume its session: this is a new one, without the conversation so far.".into() }));
                    }
                }
            }
            let r = new_session(&rpc, &cwd).await?;
            let id = s(&r, "sessionId");
            anyhow::ensure!(!id.is_empty(), "cursor gave no session id");
            Ok((id, r))
        };
        let (session, r) = match started.await {
            Ok(x) => x,
            Err(e) => {
                rpc.proc.kill();
                return Err(e);
            }
        };
        let options = r.get("configOptions").and_then(|c| c.as_array()).is_some_and(|c| !c.is_empty());
        let mode = mode_of(opts.permission);
        if setting(&r, "mode").as_deref() != Some(mode)
            && let Err(e) = rpc.set(&session, options, "mode", mode).await
        {
            rpc.proc.kill();
            return Err(e.context(format!("setting Cursor's {mode} mode")));
        }
        let mut model = setting(&r, "model");
        if let Some(m) = opts.model.as_ref().filter(|m| model.as_ref() != Some(*m)) {
            match rpc.set(&session, options, "model", m).await {
                Ok(()) => model = Some(m.clone()),
                Err(e) => {
                    let _ = out.send(DriverOut::Event(EventKind::Notice { text: format!("Cursor kept {}: {e:#}", model.as_deref().unwrap_or("its default model")) }));
                }
            }
        }
        let _ = out.send(DriverOut::Event(EventKind::Session { session_id: session.clone(), model }));
        Ok(Box::new(Live { rpc, tr, out, session, options, images, instructions: opts.instructions }))
    }

    async fn models(&self, bin: PathBuf, cwd: PathBuf) -> anyhow::Result<Vec<ModelInfo>> {
        // A session lists them (an empty one: no turn is made).
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let tr = Arc::new(Mutex::new(Translator::new("", Permission::Ask, "m")));
        let (rpc, _) = connect(&bin, &cwd, &[], tx, tr).await?;
        let r = new_session(&rpc, &cwd.to_string_lossy()).await;
        rpc.proc.kill();
        Ok(models_of(&r?))
    }
}

#[async_trait]
impl Session for Live {
    async fn send(&mut self, input: &Input) -> anyhow::Result<()> {
        self.tr.lock().unwrap().turn_started();
        let prompt = prompt_blocks(input, self.instructions.take().as_deref(), self.images);
        self.rpc.request("session/prompt", json!({"sessionId": self.session, "prompt": prompt}), |_| Waiter::Prompt)?;
        Ok(())
    }

    async fn interrupt(&mut self) -> anyhow::Result<()> {
        let acts = {
            let mut t = self.tr.lock().unwrap();
            t.cancelling = true;
            t.cancel_asks()
        };
        emit(acts, &self.out, &self.rpc.proc.writer);
        self.rpc.notify("session/cancel", json!({"sessionId": self.session}))
    }

    async fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<()> {
        let (msg, ev) = self.tr.lock().unwrap().answer(ask, option, text)?;
        self.rpc.proc.write(&msg)?;
        let _ = self.out.send(DriverOut::Event(ev));
        Ok(())
    }

    async fn set_model(&mut self, model: &str, _effort: Option<&str>) -> anyhow::Result<bool> {
        // The effort is part of the model's id.
        self.rpc.set(&self.session, self.options, "model", model).await?;
        Ok(true)
    }

    async fn close(&mut self) {
        self.rpc.proc.kill();
    }
}

/// Who Cursor is signed in as (`cursor-agent status --format json`): the user and their team.
pub fn account_of(status: &Value) -> Option<AgentAccount> {
    let u = status.get("userInfo")?;
    let num = |k: &str| u.get(k).and_then(|x| x.as_u64().map(|n| n.to_string()).or_else(|| x.as_str().map(str::to_string))).filter(|x| !x.is_empty());
    let user = num("userId")?;
    let st = |k: &str| u.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
    let id = match num("teamId") {
        Some(t) => format!("{user}@{t}"),
        None => user,
    };
    Some(AgentAccount { id, label: st("teamName").or_else(|| st("email")).unwrap_or_else(|| "Cursor".into()) })
}

/// The dashboard's answers → windows: the plan's share of the billing month, and the on-demand
/// spend against its limit (as BB's usage tracker reads them).
pub fn dashboard_limits(usage: &Value) -> Vec<RateLimit> {
    let int = |v: Option<&Value>| v.and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())));
    let resets_at = int(usage.get("billingCycleEnd")).map(|t| t as i64);
    let mut out = Vec::new();
    if let Some(p) = usage.pointer("/planUsage/totalPercentUsed").and_then(|p| p.as_f64()) {
        out.push(RateLimit { window: "plan".into(), used_percent: p.clamp(0.0, 100.0), resets_at });
    }
    let spend = usage.get("spendLimitUsage");
    let pair = ["overall", "individual", "pooled"].iter().find_map(|k| {
        let s = spend?;
        let limit = int(s.get(format!("{k}Limit").as_str()))?;
        Some((int(s.get(format!("{k}Used").as_str())).unwrap_or(0), limit))
    });
    if let Some((used, limit)) = pair.filter(|(_, l)| *l > 0) {
        out.push(RateLimit { window: "on-demand".into(), used_percent: (used as f64 / limit as f64 * 100.0).clamp(0.0, 100.0), resets_at });
    }
    out
}

/// The CLI's own sign-in token (`~/.config/cursor/auth.json`), for the dashboard's usage. It
/// stays in this process: never logged, never sent to the hub.
fn access_token() -> Option<String> {
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| crate::util::home().join(".config"));
    [cfg.join("cursor/auth.json"), crate::util::home().join(".cursor/auth.json")].iter().find_map(|f| {
        let v: Value = serde_json::from_slice(&std::fs::read(f).ok()?).ok()?;
        v.get("accessToken").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(str::to_string)
    })
}

/// Cursor's limits now and who they are of: the account from the CLI, the plan's usage from
/// Cursor's dashboard API with the CLI's sign-in.
pub async fn usage_now(bin: PathBuf) -> anyhow::Result<UsageReply> {
    let json_of = |args: &'static [&'static str]| {
        let bin = bin.clone();
        async move {
            let mut c = Command::new(&bin);
            c.args(args).current_dir(crate::util::home());
            let o = crate::util::run(&mut c, Duration::from_secs(30)).await?;
            serde_json::from_str::<Value>(o.stdout.trim()).map_err(|_| anyhow::anyhow!("cursor-agent {}: not JSON", args[0]))
        }
    };
    let (status, about) = tokio::join!(json_of(&["status", "--format", "json"]), json_of(&["about", "--format", "json"]));
    let status = status?;
    if status.get("isAuthenticated").and_then(|a| a.as_bool()) == Some(false) {
        anyhow::bail!("Cursor isn't signed in on this machine: run `cursor-agent login` there");
    }
    let tier = about.ok().and_then(|a| a.get("subscriptionTier").and_then(|t| t.as_str()).map(str::to_string));
    let token = access_token().ok_or_else(|| anyhow::anyhow!("Cursor's sign-in token isn't readable on this machine"))?;
    let (usage, plan) = tokio::task::spawn_blocking(move || -> anyhow::Result<(Value, Value)> {
        let headers = vec![
            ("Authorization", format!("Bearer {token}")),
            ("Accept", "application/json".to_string()),
            ("Content-Type", "application/json".to_string()),
            ("Connect-Protocol-Version", "1".to_string()),
            ("x-cursor-client-type", "cli".to_string()),
            ("x-cursor-client-version", format!("cli-agentd-{}", env!("CARGO_PKG_VERSION"))),
            ("User-Agent", format!("agentd/{}", env!("CARGO_PKG_VERSION"))),
        ];
        let ask = |m: &str| -> anyhow::Result<Value> {
            let (status, body) = crate::https::post("api2.cursor.sh", &format!("/aiserver.v1.DashboardService/{m}"), &headers, b"{}")?;
            match status {
                200..=299 => serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("Cursor usage check: the answer is not JSON")),
                401 => anyhow::bail!("Cursor refused its sign-in on this machine: run `cursor-agent login` there"),
                s => anyhow::bail!("Cursor usage check: HTTP {s}"),
            }
        };
        Ok((ask("GetCurrentPeriodUsage")?, ask("GetPlanInfo").unwrap_or(Value::Null)))
    })
    .await??;
    let plan = plan.pointer("/planInfo/planName").and_then(|p| p.as_str()).map(str::to_string).or(tier);
    Ok(UsageReply { limits: dashboard_limits(&usage), plan, account: account_of(&status) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Translator, fixture: &str) -> (Vec<EventKind>, Vec<Value>) {
        let mut evs = Vec::new();
        let mut writes = Vec::new();
        for l in fixture.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(l).unwrap();
            let method = v.get("method").and_then(|m| m.as_str());
            let acts = match (method, v.get("id")) {
                (Some(m), Some(id)) => t.server_request(id.clone(), m, &v["params"]),
                (Some(m), None) => t.notification(m, &v["params"]),
                // The prompt's answer.
                (None, Some(_)) => t.prompt_done(match v.get("error") {
                    Some(e) => Err(e.clone()),
                    None => Ok(v["result"].clone()),
                }),
                _ => vec![],
            };
            for a in acts {
                match a {
                    Act::Ev(e) => evs.push(e),
                    Act::Write(w) => writes.push(w),
                }
            }
        }
        (evs, writes)
    }

    fn names(evs: &[EventKind]) -> Vec<&'static str> {
        evs.iter()
            .map(|e| match e {
                EventKind::Tool { .. } => "tool",
                EventKind::ToolOutput { .. } => "tool-output",
                EventKind::ToolDone { .. } => "tool-done",
                EventKind::Ask { .. } => "ask",
                EventKind::Delta { .. } => "delta",
                EventKind::Message { .. } => "message",
                EventKind::ThinkingDelta { .. } => "thinking-delta",
                EventKind::Thinking { .. } => "thinking",
                EventKind::Usage { .. } => "usage",
                EventKind::Plan { .. } => "plan",
                EventKind::Title { .. } => "title",
                EventKind::TurnDone { .. } => "turn-done",
                EventKind::Error { .. } => "error",
                EventKind::Answered { .. } => "answered",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_real_reply() {
        // Recorded from cursor-agent 2026.11.11 (Linux): "Reply with ok".
        let mut t = Translator::new("/tmp/w", Permission::Ask, "x");
        t.turn_started();
        let (evs, writes) = run(&mut t, include_str!("../../tests/fixtures/cursor_ok.jsonl"));
        assert!(writes.is_empty());
        assert_eq!(names(&evs), ["delta", "message", "turn-done"]);
        assert_eq!(evs[1], EventKind::Message { item: "x.1m".into(), text: "ok".into() });
        assert!(matches!(evs[2], EventKind::TurnDone { stop: StopReason::End, .. }));
    }

    #[test]
    fn a_turn_with_tools_asks_and_todos() {
        // Shapes from the ACP schema and Cursor's ACP page: a turn with tools was not recorded
        // (the recording turns ran no tools).
        let mut t = Translator::new("/r", Permission::Ask, "g");
        t.turn_started();
        // Everything but the prompt's answer: the asks are answered first.
        let (head, last) = include_str!("../../tests/fixtures/cursor_turn.jsonl").trim_end().rsplit_once('\n').unwrap();
        let (evs, writes) = run(&mut t, head);
        assert!(writes.is_empty());
        assert_eq!(names(&evs), ["thinking-delta", "thinking-delta", "thinking", "plan", "tool", "ask", "tool-output", "tool-done", "tool", "ask", "tool-done", "plan", "delta", "usage", "delta"]);
        assert_eq!(evs[2], EventKind::Thinking { item: "g.1t".into(), text: "Look first, then edit.".into() });
        let EventKind::Tool { item, kind, title, .. } = &evs[4] else { panic!() };
        assert_eq!((item.as_str(), *kind, title.as_str()), ("call_1", ToolKind::Exec, "cat notes.txt"));
        let EventKind::Ask { id, title, kind, .. } = &evs[5] else { panic!() };
        assert_eq!((id.as_str(), title.as_str(), *kind), ("g-0", "Run: cat notes.txt", AskKind::Permission));
        assert_eq!(evs[7], EventKind::ToolDone { item: "call_1".into(), ok: true, output: Some("hi\n".into()), diffs: vec![] });
        let EventKind::Tool { kind, title, .. } = &evs[8] else { panic!() };
        assert_eq!((*kind, title.as_str()), (ToolKind::Edit, "Edit notes.txt"));
        let EventKind::ToolDone { diffs, .. } = &evs[10] else { panic!() };
        assert_eq!((diffs[0].path.as_str(), diffs[0].old.as_deref(), diffs[0].new.as_deref()), ("/r/notes.txt", Some("hi\n"), Some("hi\nbye\n")));
        let EventKind::Plan { entries } = &evs[11] else { panic!() };
        assert!(entries.iter().all(|e| e.status == "done"));
        let EventKind::Usage { usage } = &evs[13] else { panic!() };
        assert_eq!((usage.context_used, usage.context_window), (Some(1240), Some(272000)));
        // Answers: Cursor's own option ids.
        let (r, ev) = t.answer("g-0", "allow-always", None).unwrap();
        assert_eq!(r, json!({"jsonrpc": "2.0", "id": 0, "result": {"outcome": {"outcome": "selected", "optionId": "allow-always"}}}));
        assert_eq!(ev, EventKind::Answered { id: "g-0".into(), option: "allow-always".into(), text: None });
        assert!(t.answer("g-0", "allow", None).is_err());
        let (r, _) = t.answer("g-1", "deny", None).unwrap();
        assert_eq!(r["result"]["outcome"]["optionId"], "reject-once");
        // The turn ends: the message is whole.
        let (end, _) = run(&mut t, last);
        assert_eq!(names(&end), ["message", "turn-done"]);
        assert_eq!(end[0], EventKind::Message { item: "g.2m".into(), text: "Done: added bye.".into() });
        assert!(matches!(end[1], EventKind::TurnDone { stop: StopReason::End, usage: None, .. }));
    }

    #[test]
    fn permissions_follow_the_thread() {
        let fixture = include_str!("../../tests/fixtures/cursor_turn.jsonl");
        // Edit freely: the edit is allowed by itself, the command asks.
        let mut t = Translator::new("/r", Permission::Edits, "e");
        let (evs, writes) = run(&mut t, fixture);
        assert_eq!(evs.iter().filter(|e| matches!(e, EventKind::Ask { .. })).count(), 1);
        assert_eq!(writes, [json!({"jsonrpc": "2.0", "id": 1, "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}}})]);
        // Full access: nothing asks.
        let mut t = Translator::new("/r", Permission::Full, "f");
        let (evs, writes) = run(&mut t, fixture);
        assert!(!evs.iter().any(|e| matches!(e, EventKind::Ask { .. })));
        assert_eq!(writes.len(), 2);
        assert_eq!(mode_of(Permission::Plan), "plan");
        assert_eq!(mode_of(Permission::Full), "agent");
    }

    #[test]
    fn questions_plans_and_stops() {
        let mut t = Translator::new("/r", Permission::Ask, "q");
        t.turn_started();
        let acts = t.server_request(json!(3), "cursor/ask_question", &json!({"toolCallId": "c1", "title": "Setup", "questions": [{"id": "db", "prompt": "Which database?", "options": [{"id": "pg", "label": "Postgres"}, {"id": "lite", "label": "SQLite"}]}]}));
        let Act::Ev(EventKind::Ask { id, kind, title, options, .. }) = &acts[0] else { panic!("{acts:?}") };
        assert_eq!((id.as_str(), *kind, title.as_str(), options.len()), ("q-3", AskKind::Question, "Which database?", 2));
        let (r, _) = t.answer("q-3", "lite", None).unwrap();
        assert_eq!(r["result"], json!({"outcome": {"outcome": "answered", "answers": [{"questionId": "db", "selectedOptionIds": ["lite"]}]}}));
        t.server_request(json!(4), "cursor/ask_question", &json!({"toolCallId": "c2", "questions": [{"id": "x", "prompt": "?", "options": []}]}));
        let (r, _) = t.answer("q-4", "other", Some("MySQL")).unwrap();
        assert_eq!(r["result"]["outcome"]["outcome"], "skipped");
        // A plan to accept: its todos show, and the answer says accepted.
        let acts = t.server_request(json!("p1"), "cursor/create_plan", &json!({"toolCallId": "c3", "name": "Auth", "plan": "# Auth\n…", "todos": [{"id": "1", "content": "Add login", "status": "pending"}]}));
        assert!(matches!(&acts[0], Act::Ev(EventKind::Plan { entries }) if entries.len() == 1));
        assert!(matches!(&acts[1], Act::Ev(EventKind::Ask { id, title, .. }) if id == "q-p1" && title == "Build this plan? Auth"));
        let (r, _) = t.answer("q-p1", "accept", None).unwrap();
        assert_eq!(r, json!({"jsonrpc": "2.0", "id": "p1", "result": {"outcome": {"outcome": "accepted"}}}));
        // A subagent, then a permission left open when the person stops the turn.
        let acts = t.notification("cursor/task", &json!({"toolCallId": "c4", "description": "Explore the repo", "prompt": "…", "subagentType": "explore"}));
        assert!(matches!(&acts[0], Act::Ev(EventKind::Tool { kind: ToolKind::Task, title, .. }) if title == "Explore the repo"));
        t.server_request(json!(9), "session/request_permission", &json!({"toolCall": {"toolCallId": "c5", "kind": "execute", "rawInput": {"command": "rm -rf build"}}, "options": [{"optionId": "allow-once", "kind": "allow_once"}]}));
        t.cancelling = true;
        let acts = t.cancel_asks();
        assert_eq!(acts[0], Act::Write(json!({"jsonrpc": "2.0", "id": 9, "result": {"outcome": {"outcome": "cancelled"}}})));
        let acts = t.prompt_done(Ok(json!({"stopReason": "cancelled"})));
        assert!(matches!(acts.last(), Some(Act::Ev(EventKind::TurnDone { stop: StopReason::Interrupted, .. }))));
        // Files and terminals of an editor aren't offered: refused, not left hanging.
        assert!(matches!(&t.server_request(json!(10), "fs/read_text_file", &json!({}))[0], Act::Write(w) if w["error"]["code"] == -32601));
    }

    #[test]
    fn failed_turns_and_replays() {
        let mut t = Translator::new("/r", Permission::Ask, "z");
        t.turn_started();
        let acts = t.prompt_done(Err(json!({"code": -32603, "message": "Internal error", "data": {"message": "You've hit your usage limit"}})));
        assert_eq!(acts[0], Act::Ev(EventKind::Error { message: "You've hit your usage limit".into(), fatal: false }));
        assert!(matches!(acts[1], Act::Ev(EventKind::TurnDone { stop: StopReason::Error, .. })));
        assert_eq!(error_text(&json!({"code": -32602, "message": "Invalid params", "data": {"message": "Invalid model value: x"}})), "Invalid params: Invalid model value: x");
        // A resumed session's history is not news.
        t.replaying = true;
        assert!(t.notification("session/update", &json!({"update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "old"}}})).is_empty());
    }

    #[test]
    fn models_and_settings_of_a_session() {
        let v: Value = serde_json::from_str(include_str!("../../tests/fixtures/cursor_session.json")).unwrap();
        let s = &v["session/new"];
        let m = models_of(s);
        assert!(m.len() > 20);
        let d = m.iter().find(|m| m.default).unwrap();
        assert_eq!(d.id, "claude-opus-5-5[context=300k,effort=medium,fast=false]");
        assert_eq!((d.label.as_str(), d.context_window), ("claude-opus-5-5", Some(300_000)));
        assert_eq!(m[0].label, "Auto Balance");
        assert_eq!(setting(s, "mode").as_deref(), Some("agent"));
        assert_eq!(context_of("x[context=1m]"), Some(1_000_000));
        assert_eq!(context_of("gemini-3-flash[]"), None);
        assert_eq!(v["initialize"]["agentCapabilities"]["promptCapabilities"]["image"], true);
        // Without configOptions, ACP's model list.
        let old = json!({"models": {"currentModelId": "b", "availableModels": [{"modelId": "a", "name": "A"}, {"modelId": "b", "name": "B"}]}});
        assert_eq!(models_of(&old).iter().filter(|m| m.default).map(|m| m.id.as_str()).collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn prompts() {
        let p = prompt_blocks(&Input { text: "hi".into(), images: vec!["data:image/png;base64,AAAA".into()], context: None }, Some("Use ```ui."), true);
        assert_eq!(p.as_array().unwrap().len(), 3);
        assert_eq!(p[0], json!({"type": "image", "mimeType": "image/png", "data": "AAAA"}));
        assert_eq!(p[1], json!({"type": "text", "text": "hi"}));
        assert!(p[2]["text"].as_str().unwrap().ends_with("Use ```ui."));
        assert_eq!(prompt_blocks(&Input::text("hi"), None, false), json!([{"type": "text", "text": "hi"}]));
    }

    #[test]
    fn accounts_and_usage() {
        // `status --format json` as 2026.11.11 prints it (values made up).
        let st = json!({"status": "authenticated", "isAuthenticated": true, "userInfo": {"email": "me@work.com", "userId": 320, "teamId": 89, "teamName": "Acme"}});
        assert_eq!(account_of(&st), Some(AgentAccount { id: "320@89".into(), label: "Acme".into() }));
        let st = json!({"isAuthenticated": true, "userInfo": {"email": "me@example.com", "userId": 7}});
        assert_eq!(account_of(&st), Some(AgentAccount { id: "7".into(), label: "me@example.com".into() }));
        assert_eq!(account_of(&json!({"isAuthenticated": false})), None);
        // The dashboard's shape, as BB's usage tracker parses it.
        let u = json!({"billingCycleEnd": "1798761600000", "planUsage": {"totalPercentUsed": 42.5}, "spendLimitUsage": {"individualLimit": 2000, "individualUsed": "500"}});
        let w = dashboard_limits(&u);
        assert_eq!(w.iter().map(|w| w.window.as_str()).collect::<Vec<_>>(), ["plan", "on-demand"]);
        assert_eq!((w[0].used_percent, w[0].resets_at), (42.5, Some(1_798_761_600_000)));
        assert_eq!(w[1].used_percent, 25.0);
        assert!(dashboard_limits(&json!({"spendLimitUsage": {"overallLimit": 0}})).is_empty());
    }
}

/// `cargo test -p agentd --lib live_cursor -- --ignored --nocapture`: two "Reply with ok" turns
/// with this machine's Cursor (the second resumes the first's session), then its models.
/// `CURSOR_MODEL` picks the model.
#[cfg(test)]
#[tokio::test]
#[ignore]
async fn live_cursor() {
    let bin = crate::machine::agent_bin(&crate::config::Config::default(), AgentKind::Cursor).expect("cursor-agent is not installed");
    let dir = std::env::temp_dir().join(format!("agentd-cursor-{}", crate::util::random_hex(3)));
    std::fs::create_dir_all(&dir).unwrap();
    let model = std::env::var("CURSOR_MODEL").ok();
    let mut session = None;
    for round in 0..2 {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let opts = StartOpts { thread: "t".into(), bin: bin.clone(), cwd: dir.clone(), model: model.clone(), effort: None, permission: Permission::Ask, resume: session.clone(), instructions: Some("Answer in lowercase.".into()), env: vec![] };
        let mut s = CursorAgent.start(opts, tx).await.expect("start");
        s.send(&Input::text("Reply with ok")).await.unwrap();
        let mut seen = vec![];
        loop {
            match tokio::time::timeout(Duration::from_secs(120), rx.recv()).await {
                Ok(Some(DriverOut::Event(e))) => {
                    if let EventKind::Session { session_id, .. } = &e {
                        session = Some(session_id.clone());
                    }
                    let done = matches!(e, EventKind::TurnDone { .. });
                    seen.push(format!("{e:?}").chars().take(140).collect::<String>());
                    if done {
                        break;
                    }
                }
                other => panic!("round {round}: {other:?} after {seen:#?}"),
            }
        }
        println!("round {round}: {seen:#?}");
        s.close().await;
    }
    let m = CursorAgent.models(bin, dir.clone()).await.unwrap();
    println!("{} models, the default {:?}", m.len(), m.iter().find(|m| m.default).map(|m| &m.id));
    let _ = std::fs::remove_dir_all(dir);
}
