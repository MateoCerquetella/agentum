//! Codex: `codex app-server`, newline JSON-RPC on stdio (no "jsonrpc" field), one process per thread.
use super::{Agent, DriverOut, Proc, Session, StartOpts, Tx, permission_options, tool_title};
use agentum_proto::machine::ModelInfo;
use agentum_proto::{AgentKind, AskKind, AskOption, EventKind, FileDiff, Input, Permission, PlanEntry, RateLimit, StopReason, ToolKind, Usage};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::oneshot;

#[derive(Debug, PartialEq)]
pub enum Act {
    Ev(EventKind),
    Write(Value),
}

#[derive(Debug, Clone)]
struct PendingAsk {
    rpc_id: Value,
    method: String,
    params: Value,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn n(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_u64()).unwrap_or(0)
}

/// Codex's notifications and server requests → thread events.
#[derive(Default)]
pub struct Translator {
    cwd: String,
    /// Prefix of ask ids, so they stay unique across processes of one thread.
    generation: u32,
    asks: HashMap<String, PendingAsk>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    usage: Option<Usage>,
    /// Totals when the turn began, to give the turn its own share.
    turn_base: Usage,
    error_this_turn: bool,
    warned: HashSet<String>,
}

fn breakdown(b: &Value) -> Usage {
    Usage {
        input: n(b, "inputTokens"),
        output: n(b, "outputTokens"),
        cache_read: n(b, "cachedInputTokens"),
        cache_write: n(b, "cacheWriteInputTokens"),
        context_used: None,
        context_window: None,
    }
}

impl Translator {
    pub fn new(cwd: &str, generation: u32) -> Translator {
        Translator { cwd: cwd.to_string(), generation, ..Default::default() }
    }

    fn warn_once(&mut self, what: String) {
        if self.warned.insert(what.clone()) {
            tracing::debug!("codex: ignoring {what}");
        }
    }

    pub fn turn_started(&mut self) {
        self.error_this_turn = false;
        self.turn_base = self.usage.clone().unwrap_or_default();
    }

    pub fn notification(&mut self, method: &str, p: &Value) -> Vec<Act> {
        let mut out = Vec::new();
        let ev = |e: EventKind| Act::Ev(e);
        match method {
            "item/started" => {
                if let Some(e) = self.item_started(p.get("item").unwrap_or(&Value::Null)) {
                    out.push(ev(e));
                }
            }
            "item/completed" => {
                if let Some(e) = self.item_completed(p.get("item").unwrap_or(&Value::Null)) {
                    out.push(ev(e));
                }
            }
            "item/agentMessage/delta" | "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" | "item/commandExecution/outputDelta" => {
                let (item, text) = (s(p, "itemId"), s(p, "delta"));
                if !text.is_empty() {
                    out.push(ev(match method {
                        "item/agentMessage/delta" => EventKind::Delta { item, text },
                        "item/commandExecution/outputDelta" => EventKind::ToolOutput { item, text },
                        _ => EventKind::ThinkingDelta { item, text },
                    }));
                }
            }
            "turn/started" => {
                if let Some(id) = p.get("turn").and_then(|t| t.get("id")).and_then(|i| i.as_str()) {
                    self.turn_id = Some(id.to_string());
                }
            }
            "turn/completed" => {
                let t = p.get("turn").unwrap_or(&Value::Null);
                let stop = match s(t, "status").as_str() {
                    "completed" => StopReason::End,
                    "interrupted" => StopReason::Interrupted,
                    _ => StopReason::Error,
                };
                if stop == StopReason::Error && !self.error_this_turn {
                    let msg = t.get("error").map(|e| s(e, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "The turn failed".into());
                    out.push(ev(EventKind::Error { message: msg, fatal: false }));
                }
                let usage = self.usage.clone().map(|u| Usage {
                    input: u.input.saturating_sub(self.turn_base.input),
                    output: u.output.saturating_sub(self.turn_base.output),
                    cache_read: u.cache_read.saturating_sub(self.turn_base.cache_read),
                    cache_write: u.cache_write.saturating_sub(self.turn_base.cache_write),
                    ..u
                });
                out.push(ev(EventKind::TurnDone { turn: 0, stop, usage, cost_usd: None, duration_ms: n(t, "durationMs") }));
                self.turn_id = None;
            }
            "error" => {
                let msg = p.get("error").map(|e| s(e, "message")).unwrap_or_default();
                if p.get("willRetry").and_then(|w| w.as_bool()).unwrap_or(false) {
                    out.push(ev(EventKind::Notice { text: format!("Retrying: {msg}") }));
                } else {
                    self.error_this_turn = true;
                    out.push(ev(EventKind::Error { message: msg, fatal: false }));
                }
            }
            "thread/tokenUsage/updated" => {
                let tu = p.get("tokenUsage").unwrap_or(&Value::Null);
                let mut u = breakdown(tu.get("total").unwrap_or(&Value::Null));
                u.context_used = tu.get("last").and_then(|l| l.get("totalTokens")).and_then(|t| t.as_u64());
                u.context_window = tu.get("modelContextWindow").and_then(|w| w.as_u64());
                self.usage = Some(u.clone());
                out.push(ev(EventKind::Usage { usage: u }));
            }
            "account/rateLimits/updated" => {
                let r = p.get("rateLimits").unwrap_or(&Value::Null);
                let l = rate_limits(r);
                if !l.is_empty() {
                    out.push(ev(EventKind::RateLimits { limits: l, plan: plan_type(r) }));
                }
            }
            "thread/name/updated" => {
                if let Some(t) = p.get("threadName").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                    out.push(ev(EventKind::Title { title: t.to_string() }));
                }
            }
            "turn/plan/updated" => {
                let entries = p
                    .get("plan")
                    .and_then(|x| x.as_array())
                    .into_iter()
                    .flatten()
                    .map(|st| PlanEntry {
                        text: s(st, "step"),
                        status: match s(st, "status").as_str() {
                            "inProgress" => "in-progress".into(),
                            "completed" => "done".into(),
                            _ => "pending".into(),
                        },
                    })
                    .collect();
                out.push(ev(EventKind::Plan { entries }));
            }
            "model/rerouted" => out.push(ev(EventKind::Notice { text: format!("Codex moved this turn from {} to {}", s(p, "fromModel"), s(p, "toModel")) })),
            "thread/compacted" => out.push(ev(EventKind::Notice { text: "Context compacted".into() })),
            "serverRequest/resolved" => {
                let rid = p.get("requestId").cloned().unwrap_or(Value::Null);
                let key = self.asks.iter().find(|(_, a)| a.rpc_id == rid).map(|(k, _)| k.clone());
                if let Some(k) = key {
                    self.asks.remove(&k);
                    out.push(ev(EventKind::Answered { id: k, option: "cancelled".into(), text: None }));
                }
            }
            other => self.warn_once(format!("notification {other}")),
        }
        out
    }

    fn item_started(&mut self, it: &Value) -> Option<EventKind> {
        let id = s(it, "id");
        let (name, kind, title, input) = match s(it, "type").as_str() {
            "commandExecution" => {
                let actions: Vec<String> = it.get("commandActions").and_then(|a| a.as_array()).into_iter().flatten().map(|a| s(a, "type")).collect();
                let kind = if !actions.is_empty() && actions.iter().all(|a| a == "read") {
                    ToolKind::Read
                } else if !actions.is_empty() && actions.iter().all(|a| a == "listFiles" || a == "search" || a == "read") {
                    ToolKind::Search
                } else {
                    ToolKind::Exec
                };
                let cmd = s(it, "command");
                ("shell".to_string(), kind, crate::util::truncate(cmd.lines().next().unwrap_or(""), 200), json!({"command": cmd, "cwd": it.get("cwd")}))
            }
            "fileChange" => {
                let paths: Vec<String> = it
                    .get("changes")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        let p = s(c, "path");
                        p.strip_prefix(&self.cwd).map(|r| r.trim_start_matches('/').to_string()).unwrap_or(p)
                    })
                    .collect();
                ("apply_patch".to_string(), ToolKind::Edit, paths.join(", "), json!({"changes": it.get("changes")}))
            }
            "mcpToolCall" => {
                let name = format!("{}.{}", s(it, "server"), s(it, "tool"));
                let args = it.get("arguments").cloned().unwrap_or(Value::Null);
                let title = tool_title(&s(it, "tool"), &args, &self.cwd);
                (name, ToolKind::Other, title, args)
            }
            "dynamicToolCall" => {
                let args = it.get("arguments").cloned().unwrap_or(Value::Null);
                (s(it, "tool"), ToolKind::Other, s(it, "tool"), args)
            }
            "webSearch" => ("web_search".to_string(), ToolKind::Fetch, s(it, "query"), json!({"query": s(it, "query")})),
            "collabAgentToolCall" => {
                let prompt = s(it, "prompt");
                let title = if prompt.is_empty() { s(it, "tool") } else { crate::util::truncate(prompt.lines().next().unwrap_or(""), 200) };
                (s(it, "tool"), ToolKind::Task, title, json!({"prompt": prompt, "model": it.get("model")}))
            }
            "imageView" => ("view_image".to_string(), ToolKind::Read, s(it, "path"), json!({"path": s(it, "path")})),
            _ => return None,
        };
        Some(EventKind::Tool { item: id, name, kind, title, input })
    }

    fn item_completed(&mut self, it: &Value) -> Option<EventKind> {
        let id = s(it, "id");
        let status = s(it, "status");
        Some(match s(it, "type").as_str() {
            "agentMessage" => EventKind::Message { item: id, text: s(it, "text") },
            "reasoning" => {
                let join = |k: &str| it.get(k).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
                let text = Some(join("summary")).filter(|t| !t.is_empty()).unwrap_or_else(|| join("content"));
                if text.is_empty() {
                    return None;
                }
                EventKind::Thinking { item: id, text }
            }
            "commandExecution" => {
                let code = it.get("exitCode").and_then(|c| c.as_i64());
                let ok = status == "completed" && code.unwrap_or(0) == 0;
                let output = it.get("aggregatedOutput").and_then(|o| o.as_str()).filter(|o| !o.is_empty()).map(|o| crate::util::truncate(o, 32 * 1024));
                EventKind::ToolDone { item: id, ok, output, diffs: vec![] }
            }
            "fileChange" => {
                let diffs = it
                    .get("changes")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        let diff = s(c, "diff");
                        match c.get("kind").map(|k| s(k, "type")).as_deref() {
                            Some("add") => FileDiff { path: s(c, "path"), unified: None, old: None, new: Some(diff) },
                            Some("delete") => FileDiff { path: s(c, "path"), unified: None, old: Some(diff), new: None },
                            _ => FileDiff { path: s(c, "path"), unified: Some(diff), old: None, new: None },
                        }
                    })
                    .collect();
                EventKind::ToolDone { item: id, ok: status == "completed", output: (status == "declined").then(|| "Declined".to_string()), diffs }
            }
            "mcpToolCall" => {
                let err = it.get("error").filter(|e| !e.is_null());
                let output = match err {
                    Some(e) => Some(e.get("message").and_then(|m| m.as_str()).map(str::to_string).unwrap_or_else(|| e.to_string())),
                    None => it.get("result").filter(|r| !r.is_null()).map(|r| {
                        let texts: Vec<&str> = r.get("content").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.get("text").and_then(|t| t.as_str())).collect();
                        if texts.is_empty() { r.to_string() } else { texts.join("\n") }
                    }),
                };
                EventKind::ToolDone { item: id, ok: err.is_none() && status != "failed", output: output.map(|o| crate::util::truncate(&o, 32 * 1024)), diffs: vec![] }
            }
            "dynamicToolCall" => {
                let texts: Vec<&str> = it.get("contentItems").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.get("text").and_then(|t| t.as_str())).collect();
                EventKind::ToolDone {
                    item: id,
                    ok: it.get("success").and_then(|x| x.as_bool()).unwrap_or(status != "failed"),
                    output: (!texts.is_empty()).then(|| texts.join("\n")),
                    diffs: vec![],
                }
            }
            "webSearch" | "collabAgentToolCall" | "imageView" => EventKind::ToolDone { item: id, ok: status != "failed", output: None, diffs: vec![] },
            "contextCompaction" => EventKind::Notice { text: "Context compacted".into() },
            _ => return None,
        })
    }

    pub fn server_request(&mut self, rpc_id: Value, method: &str, p: &Value) -> Vec<Act> {
        let id = format!("{}-{}", self.generation, match &rpc_id {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        let ask = |kind, title: String, detail: Option<String>, options| Act::Ev(EventKind::Ask { id: id.clone(), kind, title, detail, input: p.clone(), options });
        let reason = p.get("reason").and_then(|r| r.as_str()).map(str::to_string);
        let act = match method {
            "item/commandExecution/requestApproval" | "execCommandApproval" => {
                let cmd = match p.get("command") {
                    Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" "),
                    Some(Value::String(c)) => c.clone(),
                    _ => String::new(),
                };
                ask(AskKind::Permission, format!("Run: {}", crate::util::truncate(&cmd, 200)), reason, permission_options())
            }
            "item/fileChange/requestApproval" | "applyPatchApproval" => {
                ask(AskKind::Permission, "Apply file changes".into(), reason.or_else(|| p.get("grantRoot").and_then(|g| g.as_str()).map(|g| format!("Write access to {g}"))), permission_options())
            }
            "item/permissions/requestApproval" => ask(AskKind::Permission, "Grant more permissions".into(), reason, permission_options()),
            "item/tool/requestUserInput" => {
                let q = p.get("questions").and_then(|q| q.get(0)).cloned().unwrap_or(Value::Null);
                let options = q
                    .get("options")
                    .and_then(|o| o.as_array())
                    .into_iter()
                    .flatten()
                    .map(|o| AskOption { id: s(o, "label"), label: s(o, "label"), kind: "answer".into() })
                    .collect();
                let header = s(&q, "header");
                ask(AskKind::Question, s(&q, "question"), (!header.is_empty()).then_some(header), options)
            }
            "mcpServer/elicitation/request" => return vec![Act::Write(json!({"id": rpc_id, "result": {"action": "decline"}}))],
            other => {
                self.warn_once(format!("server request {other}"));
                return vec![Act::Write(json!({"id": rpc_id, "error": {"code": -32601, "message": format!("agentd does not handle {other}")}}))];
            }
        };
        self.asks.insert(id.clone(), PendingAsk { rpc_id, method: method.to_string(), params: p.clone() });
        vec![act]
    }

    pub fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<(Value, EventKind)> {
        let a = self.asks.remove(ask).ok_or_else(|| anyhow::anyhow!("no open question {ask}"))?;
        let result = match a.method.as_str() {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => json!({"decision": match option {
                "allow" => "accept",
                "allow-always" => "acceptForSession",
                _ => "decline",
            }}),
            "execCommandApproval" | "applyPatchApproval" => json!({"decision": match option {
                "allow" => "approved",
                "allow-always" => "approved_for_session",
                _ => "denied",
            }}),
            "item/permissions/requestApproval" => match option {
                "allow" => json!({"permissions": a.params.get("permissions"), "scope": "turn"}),
                "allow-always" => json!({"permissions": a.params.get("permissions"), "scope": "session"}),
                _ => json!({"permissions": {}}),
            },
            "item/tool/requestUserInput" => {
                let chosen = if option.is_empty() || option == "other" { text.unwrap_or("").to_string() } else { option.to_string() };
                let mut answers = serde_json::Map::new();
                for q in a.params.get("questions").and_then(|q| q.as_array()).into_iter().flatten() {
                    answers.insert(s(q, "id"), json!({"answers": [chosen]}));
                }
                json!({"answers": answers})
            }
            _ => json!({}),
        };
        Ok((json!({"id": a.rpc_id, "result": result}), EventKind::Answered { id: ask.to_string(), option: option.to_string(), text: text.map(str::to_string) }))
    }
}

/// The account's plan in a rate-limit snapshot ("plus", "pro"…).
pub fn plan_type(r: &Value) -> Option<String> {
    r.get("planType").and_then(|p| p.as_str()).filter(|p| !p.is_empty()).map(str::to_string)
}

/// Codex's rate-limit snapshot → windows.
pub fn rate_limits(r: &Value) -> Vec<RateLimit> {
    let mut out = Vec::new();
    for k in ["primary", "secondary"] {
        let Some(w) = r.get(k).filter(|w| !w.is_null()) else { continue };
        let Some(used) = w.get("usedPercent").and_then(|u| u.as_f64()) else { continue };
        let window = match w.get("windowDurationMins").and_then(|m| m.as_u64()) {
            Some(300) => "5h".to_string(),
            Some(10080) => "weekly".to_string(),
            Some(m) if m % 1440 == 0 => format!("{}d", m / 1440),
            Some(m) if m % 60 == 0 => format!("{}h", m / 60),
            Some(m) => format!("{m}m"),
            None => k.to_string(),
        };
        out.push(RateLimit { window, used_percent: used, resets_at: w.get("resetsAt").and_then(|r| r.as_i64()).map(|r| r * 1000) });
    }
    out
}

pub fn policy(p: Permission) -> (&'static str, &'static str) {
    match p {
        Permission::Ask | Permission::Edits => ("on-request", "workspace-write"),
        Permission::Plan => ("untrusted", "read-only"),
        Permission::Full => ("never", "danger-full-access"),
    }
}

pub fn input_items(input: &Input) -> Value {
    let mut items = vec![json!({"type": "text", "text": input.text})];
    for img in &input.images {
        if img.starts_with("data:") || img.starts_with("http") {
            items.push(json!({"type": "image", "url": img}));
        } else {
            items.push(json!({"type": "localImage", "path": img}));
        }
    }
    Value::Array(items)
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// A JSON-RPC connection to one app-server.
struct Rpc {
    proc: Proc,
    pending: Pending,
    next: AtomicU64,
}

impl Rpc {
    async fn call(&self, method: &str, params: Value, timeout: Duration) -> anyhow::Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.proc.write(&json!({"id": id, "method": method, "params": params}))?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => anyhow::bail!("codex {method}: {e}"),
            Ok(Err(_)) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let t = self.proc.stderr_tail();
                anyhow::bail!("codex exited during {method}{}", if t.is_empty() { String::new() } else { format!(": {t}") })
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                anyhow::bail!("codex did not answer {method} in {}s", timeout.as_secs())
            }
        }
    }
}

static GEN: AtomicU32 = AtomicU32::new(0);

async fn connect(bin: &PathBuf, cwd: &PathBuf, out: Tx, tr: Arc<Mutex<Translator>>) -> anyhow::Result<Rpc> {
    let mut cmd = Command::new(bin);
    cmd.arg("app-server").current_dir(cwd);
    let (proc, child, stdout) = Proc::spawn(cmd, "codex")?;
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (p2, o2, writer) = (pending.clone(), out.clone(), proc.writer.clone());
    super::read_lines(stdout, child, proc.stderr.clone(), out, move |v| {
        let method = v.get("method").and_then(|m| m.as_str()).map(str::to_string);
        let id = v.get("id").cloned();
        match (method, id) {
            (None, Some(id)) => {
                let res = match v.get("error") {
                    Some(e) => Err(e.get("message").and_then(|m| m.as_str()).map(str::to_string).unwrap_or_else(|| e.to_string())),
                    None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                };
                if let Some(id) = id.as_u64()
                    && let Some(w) = p2.lock().unwrap().remove(&id)
                {
                    let _ = w.send(res);
                }
            }
            (Some(m), Some(id)) => {
                let acts = tr.lock().unwrap().server_request(id, &m, v.get("params").unwrap_or(&Value::Null));
                emit(acts, &o2, &writer);
            }
            (Some(m), None) => {
                let acts = tr.lock().unwrap().notification(&m, v.get("params").unwrap_or(&Value::Null));
                emit(acts, &o2, &writer);
            }
            _ => {}
        }
    });
    let rpc = Rpc { proc, pending, next: AtomicU64::new(0) };
    let r = rpc
        .call("initialize", json!({"clientInfo": {"name": "agentd", "title": "Agentum", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {"experimentalApi": true}}), Duration::from_secs(60))
        .await;
    if let Err(e) = r {
        rpc.proc.kill();
        return Err(e);
    }
    rpc.proc.write(&json!({"method": "initialized"}))?;
    Ok(rpc)
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

pub struct CodexAgent;

/// "ollama/<model>": Codex's built-in local provider with that model (your own model, no quota).
/// Anything else is a model of the account's provider.
fn provider_of(model: &str) -> (Option<&'static str>, &str) {
    match model.split_once('/') {
        Some(("ollama", m)) if !m.is_empty() => (Some("ollama"), m),
        Some(("lmstudio", m)) if !m.is_empty() => (Some("lmstudio"), m),
        _ => (None, model),
    }
}

fn set_model(p: &mut Value, model: &str, with_provider: bool) {
    let (provider, m) = provider_of(model);
    p["model"] = json!(m);
    if let (Some(pv), true) = (provider, with_provider) {
        p["modelProvider"] = json!(pv);
    }
}

/// The models of the local Ollama (OLLAMA_HOST or 127.0.0.1:11434), as "ollama/<name>".
async fn ollama_models() -> Vec<ModelInfo> {
    let host = std::env::var("OLLAMA_HOST").ok().map(|h| h.trim_start_matches("http://").trim_end_matches('/').to_string()).filter(|h| !h.is_empty()).unwrap_or_else(|| "127.0.0.1:11434".into());
    let Ok(Ok(r)) = tokio::time::timeout(Duration::from_secs(3), crate::http::request(&host, "GET", "/api/tags", "", None)).await else { return vec![] };
    let Ok(v) = r.json() else { return vec![] };
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["name"].as_str())
        .filter(|n| !n.contains("embed"))
        .map(|n| ModelInfo { id: format!("ollama/{n}"), label: format!("{n} (Ollama, this machine)"), efforts: vec![], default_effort: None, default: false, context_window: None })
        .collect()
}

struct Live {
    rpc: Rpc,
    tr: Arc<Mutex<Translator>>,
    out: Tx,
    thread_id: String,
    model: Option<String>,
    effort: Option<String>,
}

#[async_trait]
impl Agent for CodexAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    async fn start(&self, opts: StartOpts, out: Tx) -> anyhow::Result<Box<dyn Session>> {
        let generation = GEN.fetch_add(1, Ordering::Relaxed);
        let tr = Arc::new(Mutex::new(Translator::new(&opts.cwd.to_string_lossy(), generation)));
        let rpc = connect(&opts.bin, &opts.cwd, out.clone(), tr.clone()).await?;
        let (approval, sandbox) = policy(opts.permission);
        let cwd = opts.cwd.to_string_lossy().to_string();
        let res = match &opts.resume {
            Some(id) => {
                let mut p = json!({"threadId": id, "cwd": cwd, "approvalPolicy": approval, "sandbox": sandbox, "excludeTurns": true});
                if let Some(m) = &opts.model {
                    set_model(&mut p, m, true);
                }
                if let Some(i) = &opts.instructions {
                    p["developerInstructions"] = json!(i);
                }
                rpc.call("thread/resume", p, Duration::from_secs(60)).await
            }
            None => {
                let mut p = json!({"cwd": cwd, "approvalPolicy": approval, "sandbox": sandbox});
                if let Some(m) = &opts.model {
                    set_model(&mut p, m, true);
                }
                if let Some(i) = &opts.instructions {
                    p["developerInstructions"] = json!(i);
                }
                rpc.call("thread/start", p, Duration::from_secs(60)).await
            }
        };
        let res = match res {
            Ok(r) => r,
            Err(e) => {
                rpc.proc.kill();
                return Err(e);
            }
        };
        // The account's limits now, so a refused turn can say which and until when (and the
        // usage meters know before the first turn).
        if let Ok(r) = rpc.call("account/rateLimits/read", Value::Null, Duration::from_secs(10)).await {
            let r = r.get("rateLimits").unwrap_or(&Value::Null);
            let l = rate_limits(r);
            if !l.is_empty() {
                let _ = out.send(DriverOut::Event(EventKind::RateLimits { limits: l, plan: plan_type(r) }));
            }
        }
        let thread_id = res.get("thread").map(|t| s(t, "id")).unwrap_or_default();
        anyhow::ensure!(!thread_id.is_empty(), "codex gave no thread id");
        let model = res.get("model").and_then(|m| m.as_str()).map(str::to_string);
        tr.lock().unwrap().thread_id = Some(thread_id.clone());
        let _ = out.send(DriverOut::Event(EventKind::Session { session_id: thread_id.clone(), model: model.clone() }));
        Ok(Box::new(Live { rpc, tr, out, thread_id, model: opts.model.or(model), effort: opts.effort }))
    }

    async fn models(&self, bin: PathBuf, cwd: PathBuf) -> anyhow::Result<Vec<ModelInfo>> {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let tr = Arc::new(Mutex::new(Translator::new("", 0)));
        let rpc = connect(&bin, &cwd, tx, tr).await?;
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let r = rpc.call("model/list", json!({"cursor": cursor}), Duration::from_secs(30)).await;
            let r = match r {
                Ok(r) => r,
                Err(e) => {
                    rpc.proc.kill();
                    return Err(e);
                }
            };
            models.extend(models_from_list(&r));
            cursor = r.get("nextCursor").and_then(|c| c.as_str()).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        rpc.proc.kill();
        models.extend(ollama_models().await);
        Ok(models)
    }
}

pub fn models_from_list(r: &Value) -> Vec<ModelInfo> {
    r.get("data")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .filter(|m| !m.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false))
        .map(|m| ModelInfo {
            id: s(m, "id"),
            label: m.get("displayName").and_then(|d| d.as_str()).map(str::to_string).unwrap_or_else(|| s(m, "id")),
            efforts: m.get("supportedReasoningEfforts").and_then(|e| e.as_array()).into_iter().flatten().map(|e| s(e, "reasoningEffort")).collect(),
            default_effort: m.get("defaultReasoningEffort").and_then(|e| e.as_str()).map(str::to_string),
            default: m.get("isDefault").and_then(|d| d.as_bool()).unwrap_or(false),
            context_window: None,
        })
        .collect()
}

#[async_trait]
impl Session for Live {
    async fn send(&mut self, input: &Input) -> anyhow::Result<()> {
        self.tr.lock().unwrap().turn_started();
        let mut p = json!({"threadId": self.thread_id, "input": input_items(input)});
        if let Some(m) = &self.model {
            // The provider is the thread's; a turn only names the model.
            set_model(&mut p, m, false);
        }
        if let Some(e) = &self.effort {
            p["effort"] = json!(e);
        }
        let r = self.rpc.call("turn/start", p, Duration::from_secs(60)).await?;
        if let Some(id) = r.get("turn").and_then(|t| t.get("id")).and_then(|i| i.as_str()) {
            self.tr.lock().unwrap().turn_id = Some(id.to_string());
        }
        Ok(())
    }

    async fn steer(&mut self, input: &Input) -> anyhow::Result<bool> {
        let Some(turn) = self.tr.lock().unwrap().turn_id.clone() else { return Ok(false) };
        let p = json!({"threadId": self.thread_id, "expectedTurnId": turn, "input": input_items(input)});
        match self.rpc.call("turn/steer", p, Duration::from_secs(30)).await {
            Ok(_) => Ok(true),
            Err(e) => {
                tracing::info!("codex steer failed, queueing instead: {e}");
                Ok(false)
            }
        }
    }

    async fn interrupt(&mut self) -> anyhow::Result<()> {
        let Some(turn) = self.tr.lock().unwrap().turn_id.clone() else { return Ok(()) };
        self.rpc.call("turn/interrupt", json!({"threadId": self.thread_id, "turnId": turn}), Duration::from_secs(30)).await?;
        Ok(())
    }

    async fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<()> {
        let (msg, ev) = self.tr.lock().unwrap().answer(ask, option, text)?;
        self.rpc.proc.write(&msg)?;
        let _ = self.out.send(DriverOut::Event(ev));
        Ok(())
    }

    async fn set_model(&mut self, model: &str, effort: Option<&str>) -> anyhow::Result<bool> {
        // turn/start carries them: the next turn uses them.
        self.model = Some(model.to_string());
        if effort.is_some() {
            self.effort = effort.map(str::to_string);
        }
        Ok(true)
    }

    async fn close(&mut self) {
        self.rpc.proc.kill();
    }
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

    #[test]
    fn real_usage_limit_failure() {
        // Recorded on this machine: the account was over its weekly limit.
        let mut t = Translator::new("/tmp/agentd-rec/proj", 0);
        let (evs, _) = run(&mut t, include_str!("../../tests/fixtures/codex_limit.jsonl"));
        let limits = evs.iter().find_map(|e| if let EventKind::RateLimits { limits, .. } = e { Some(limits.clone()) } else { None }).unwrap();
        assert_eq!(limits[0].window, "5h");
        assert_eq!(limits[1].window, "weekly");
        assert_eq!(limits[1].used_percent, 100.0);
        assert_eq!(limits[1].resets_at, Some(1795746141000));
        let errors: Vec<&EventKind> = evs.iter().filter(|e| matches!(e, EventKind::Error { .. })).collect();
        assert_eq!(errors.len(), 1, "one error, not one per notification: {errors:?}");
        assert!(matches!(errors[0], EventKind::Error { message, .. } if message.contains("usage limit")));
        assert!(matches!(evs.last(), Some(EventKind::TurnDone { stop: StopReason::Error, duration_ms: 2008, .. })));
    }

    #[test]
    fn a_turn_with_a_command_approval_and_a_patch() {
        // Shapes from `codex app-server generate-json-schema` (0.172): a real turn could not be
        // recorded while the account was rate limited.
        let mut t = Translator::new("/r", 3);
        t.turn_started();
        let (evs, _) = run(&mut t, include_str!("../../tests/fixtures/codex_turn.jsonl"));
        let kinds: Vec<&str> = evs
            .iter()
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
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["thinking-delta", "thinking", "plan", "tool", "ask", "tool-output", "tool-done", "tool", "ask", "tool-done", "delta", "delta", "message", "usage", "title", "turn-done"]
        );
        let EventKind::Ask { id, title, .. } = &evs[4] else { panic!() };
        assert_eq!(id, "3-0");
        assert_eq!(title, "Run: touch codex.txt");
        let (reply, answered) = t.answer("3-0", "allow-always", None).unwrap();
        assert_eq!(reply, json!({"id": 0, "result": {"decision": "acceptForSession"}}));
        assert_eq!(answered, EventKind::Answered { id: "3-0".into(), option: "allow-always".into(), text: None });
        let (reply, _) = t.answer("3-1", "deny", None).unwrap();
        assert_eq!(reply["result"]["decision"], "decline");
        let EventKind::ToolDone { diffs, ok, .. } = &evs[9] else { panic!() };
        assert!(*ok);
        assert_eq!(diffs[0].path, "/r/notes.txt");
        assert!(diffs[0].unified.as_ref().unwrap().contains("+bye"));
        let EventKind::TurnDone { usage, stop, .. } = evs.last().unwrap() else { panic!() };
        assert_eq!(*stop, StopReason::End);
        let u = usage.as_ref().unwrap();
        assert_eq!((u.input, u.output, u.context_used, u.context_window), (1200, 40, Some(1240), Some(272000)));
    }

    #[test]
    fn questions_and_resolved_requests() {
        let mut t = Translator::new("/r", 1);
        let acts = t.server_request(json!(7), "item/tool/requestUserInput", &json!({"itemId":"i","threadId":"t","turnId":"u","isBlocking":true,"questions":[{"id":"q1","header":"Pick","question":"Which?","options":[{"label":"A","description":""},{"label":"B","description":""}]}]}));
        let Act::Ev(EventKind::Ask { kind, options, .. }) = &acts[0] else { panic!() };
        assert_eq!(*kind, AskKind::Question);
        assert_eq!(options.len(), 2);
        let (r, _) = t.answer("1-7", "B", None).unwrap();
        assert_eq!(r, json!({"id": 7, "result": {"answers": {"q1": {"answers": ["B"]}}}}));
        t.server_request(json!(8), "item/fileChange/requestApproval", &json!({"itemId":"i","threadId":"t","turnId":"u","startedAtMs":1}));
        let r = t.notification("serverRequest/resolved", &json!({"requestId": 8, "threadId": "t"}));
        assert_eq!(r, vec![Act::Ev(EventKind::Answered { id: "1-8".into(), option: "cancelled".into(), text: None })]);
        // Unknown server requests are refused, not left hanging.
        assert!(matches!(&t.server_request(json!(9), "attestation/generate", &json!({}))[0], Act::Write(w) if w["error"]["code"] == -32601));
    }

    #[test]
    fn models_from_model_list() {
        let r: Value = serde_json::from_str(include_str!("../../tests/fixtures/codex_models.json")).unwrap();
        let m = models_from_list(&r);
        assert!(m.len() > 3);
        let d = m.iter().find(|m| m.default).unwrap();
        assert_eq!(d.id, "gpt-6.2-sol");
        assert!(d.efforts.contains(&"high".to_string()));
        assert_eq!(d.default_effort.as_deref(), Some("low"));
    }
}
