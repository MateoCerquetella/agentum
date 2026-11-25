//! OpenCode: one `opencode serve` per agentd (started on first use, stopped when no session has
//! used it for `idle_minutes`), its v2 HTTP API and its event stream (`/api/event`).
use super::{Agent, DriverOut, Session, StartOpts, Tx, permission_options, tool_kind, tool_title};
use crate::http::{self, enc};
use agentum_proto::machine::ModelInfo;
use agentum_proto::{AgentKind, AskKind, AskOption, EventKind, FileDiff, Input, Permission, PlanEntry, StopReason, Usage};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn n(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_f64()).map(|f| f as u64).unwrap_or(0)
}
fn tokens(t: &Value) -> Usage {
    let cache = t.get("cache").unwrap_or(&Value::Null);
    Usage { input: n(t, "input"), output: n(t, "output") + n(t, "reasoning"), cache_read: n(cache, "read"), cache_write: n(cache, "write"), context_used: None, context_window: None }
}

#[derive(Debug, Clone)]
enum Pending {
    Permission,
    Form { fields: Vec<Value> },
}

/// A reply to send for an answered question: (method, path, body).
pub type Call = (&'static str, String, Value);

/// OpenCode's events of one session → thread events.
#[derive(Default)]
pub struct Translator {
    pub session: String,
    cwd: String,
    pub context_window: Option<u64>,
    names: HashMap<String, String>,
    inputs: HashMap<String, Value>,
    patches: HashMap<String, Vec<FileDiff>>,
    asks: HashMap<String, Pending>,
    answered: HashSet<String>,
    usage: Usage,
    cost: f64,
    base: (Usage, f64),
    ctx: Option<u64>,
    running: bool,
}

impl Translator {
    pub fn new(session: &str, cwd: &str) -> Translator {
        Translator { session: session.to_string(), cwd: cwd.to_string(), ..Default::default() }
    }

    /// The session this event belongs to.
    pub fn session_of(ev: &Value) -> Option<String> {
        let d = ev.get("data")?;
        d.get("sessionID").or_else(|| d.get("form").and_then(|f| f.get("sessionID"))).and_then(|s| s.as_str()).map(str::to_string)
    }

    pub fn event(&mut self, ev: &Value) -> Vec<EventKind> {
        let d = ev.get("data").unwrap_or(&Value::Null);
        let mut out = Vec::new();
        let msg = s(d, "assistantMessageID");
        let ord = d.get("ordinal").and_then(|o| o.as_u64()).unwrap_or(0);
        match s(ev, "type").as_str() {
            "session.execution.started" => {
                self.running = true;
                self.base = (self.usage.clone(), self.cost);
            }
            "session.text.delta" => {
                let text = s(d, "delta");
                if !text.is_empty() {
                    out.push(EventKind::Delta { item: format!("{msg}:{ord}m"), text });
                }
            }
            "session.text.ended" => {
                let text = s(d, "text");
                if !text.is_empty() {
                    out.push(EventKind::Message { item: format!("{msg}:{ord}m"), text });
                }
            }
            "session.reasoning.delta" => {
                let text = s(d, "delta");
                if !text.is_empty() {
                    out.push(EventKind::ThinkingDelta { item: format!("{msg}:{ord}t"), text });
                }
            }
            "session.reasoning.ended" => {
                let text = s(d, "text");
                if !text.is_empty() {
                    out.push(EventKind::Thinking { item: format!("{msg}:{ord}t"), text });
                }
            }
            "session.tool.input.started" => {
                self.names.insert(s(d, "id"), s(d, "name"));
            }
            "session.tool.called" => {
                let id = s(d, "id");
                let name = self.names.get(&id).cloned().unwrap_or_else(|| s(d, "name"));
                let input = d.get("input").cloned().unwrap_or(Value::Null);
                self.inputs.insert(id.clone(), input.clone());
                if name == "todowrite" {
                    out.push(EventKind::Plan { entries: todos(&input) });
                }
                let title = tool_title(&name, &input, &self.cwd);
                out.push(EventKind::Tool { item: id, kind: tool_kind(&name), name, title, input });
            }
            "session.tool.success" | "session.tool.failed" => {
                let id = s(d, "id");
                let ok = s(ev, "type") == "session.tool.success";
                let text: Vec<String> = d.get("content").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.get("text").and_then(|t| t.as_str()).map(str::to_string)).collect();
                let mut output = text.join("\n");
                if !ok && let Some(e) = d.get("error") {
                    let m = s(e, "message");
                    output = if output.is_empty() { m } else { format!("{m}\n{output}") };
                }
                let name = self.names.remove(&id).unwrap_or_default();
                let input = self.inputs.remove(&id).unwrap_or(Value::Null);
                let diffs = if ok { self.patches.remove(&id).unwrap_or_else(|| diffs_from_input(&name, &input)) } else { vec![] };
                self.patches.remove(&id);
                out.push(EventKind::ToolDone { item: id, ok, output: (!output.is_empty()).then(|| crate::util::truncate(&output, 32 * 1024)), diffs });
            }
            "permission.asked" => {
                let id = s(d, "id");
                let action = s(d, "action");
                let res: Vec<String> = d.get("resources").and_then(|r| r.as_array()).into_iter().flatten().filter_map(|r| r.as_str().map(str::to_string)).collect();
                let files = d.get("metadata").and_then(|m| m.get("files")).and_then(|f| f.as_array()).cloned().unwrap_or_default();
                let diffs: Vec<FileDiff> = files.iter().map(|f| FileDiff { path: s(f, "file"), unified: f.get("patch").and_then(|p| p.as_str()).map(str::to_string), old: None, new: None }).collect();
                let detail = diffs.iter().filter_map(|d| d.unified.clone()).collect::<Vec<_>>().join("\n");
                if let Some(tool) = d.get("source").and_then(|s| s.get("id")).and_then(|i| i.as_str())
                    && !diffs.is_empty()
                {
                    self.patches.insert(tool.to_string(), diffs);
                }
                self.asks.insert(id.clone(), Pending::Permission);
                let title = if res.is_empty() { action.clone() } else { format!("{action}: {}", crate::util::truncate(&res.join(" "), 200)) };
                out.push(EventKind::Ask {
                    id,
                    kind: AskKind::Permission,
                    title,
                    detail: (!detail.is_empty()).then(|| crate::util::truncate(&detail, 16 * 1024)),
                    input: d.clone(),
                    options: permission_options(),
                });
            }
            "permission.replied" => {
                let id = s(d, "requestID");
                if self.asks.remove(&id).is_some() && !self.answered.remove(&id) {
                    let option = match s(d, "reply").as_str() {
                        "once" => "allow",
                        "always" => "allow-always",
                        _ => "deny",
                    };
                    out.push(EventKind::Answered { id, option: option.into(), text: None });
                }
            }
            "form.created" => {
                let f = d.get("form").unwrap_or(&Value::Null);
                let id = s(f, "id");
                let fields: Vec<Value> = f.get("fields").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                let first = fields.iter().find(|f| !f.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false)).cloned().unwrap_or(Value::Null);
                let options = match s(&first, "type").as_str() {
                    "boolean" => vec![AskOption { id: "yes".into(), label: "Yes".into(), kind: "answer".into() }, AskOption { id: "no".into(), label: "No".into(), kind: "answer".into() }],
                    _ => first.get("options").and_then(|o| o.as_array()).into_iter().flatten().map(|o| AskOption { id: s(o, "value"), label: s(o, "label"), kind: "answer".into() }).collect(),
                };
                let title = Some(s(f, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| s(&first, "title"));
                let detail = Some(s(&first, "title")).filter(|t| !t.is_empty() && *t != title);
                self.asks.insert(id.clone(), Pending::Form { fields });
                out.push(EventKind::Ask { id, kind: AskKind::Question, title, detail, input: f.clone(), options });
            }
            "form.replied" | "form.cancelled" => {
                let id = d.get("formID").or_else(|| d.get("id")).or_else(|| d.get("form").and_then(|f| f.get("id"))).and_then(|i| i.as_str()).unwrap_or("").to_string();
                if self.asks.remove(&id).is_some() && !self.answered.remove(&id) {
                    out.push(EventKind::Answered { id, option: if s(ev, "type") == "form.cancelled" { "cancelled".into() } else { "answered".into() }, text: None });
                }
            }
            "session.renamed" => {
                let t = s(d, "title");
                if !t.is_empty() {
                    out.push(EventKind::Title { title: t });
                }
            }
            "session.step.ended" => {
                let t = tokens(d.get("tokens").unwrap_or(&Value::Null));
                self.ctx = Some(t.input + t.output + t.cache_read + t.cache_write);
            }
            "session.usage.updated" => {
                self.usage = tokens(d.get("tokens").unwrap_or(&Value::Null));
                self.cost = d.get("cost").and_then(|c| c.as_f64()).unwrap_or(self.cost);
                out.push(EventKind::Usage { usage: Usage { context_used: self.ctx, context_window: self.context_window, ..self.usage.clone() } });
            }
            "session.retry.scheduled" => {
                let e = d.get("error").map(|e| s(e, "message")).unwrap_or_default();
                out.push(EventKind::Notice { text: format!("Retrying (attempt {}): {e}", n(d, "attempt")) });
            }
            "session.compaction.ended" => out.push(EventKind::Notice { text: "Context compacted".into() }),
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                let stop = match s(ev, "type").as_str() {
                    "session.execution.succeeded" => StopReason::End,
                    "session.execution.interrupted" => StopReason::Interrupted,
                    _ => StopReason::Error,
                };
                if stop == StopReason::Error {
                    let m = d.get("error").map(|e| s(e, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "OpenCode failed".into());
                    out.push(EventKind::Error { message: m, fatal: false });
                }
                let (bu, bc) = &self.base;
                let u = &self.usage;
                let usage = Usage {
                    input: u.input.saturating_sub(bu.input),
                    output: u.output.saturating_sub(bu.output),
                    cache_read: u.cache_read.saturating_sub(bu.cache_read),
                    cache_write: u.cache_write.saturating_sub(bu.cache_write),
                    context_used: self.ctx,
                    context_window: self.context_window,
                };
                let cost = (self.cost - bc).max(0.0);
                self.running = false;
                out.push(EventKind::TurnDone { turn: 0, stop, usage: Some(usage), cost_usd: Some(cost), duration_ms: 0 });
            }
            _ => {}
        }
        out
    }

    /// The HTTP call answering an `Ask`, and its `Answered` event.
    pub fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<(Call, EventKind)> {
        let p = self.asks.get(ask).cloned().ok_or_else(|| anyhow::anyhow!("no open question {ask}"))?;
        self.answered.insert(ask.to_string());
        let sid = &self.session;
        let call = match p {
            Pending::Permission => {
                let decision = match option {
                    "allow" => "once",
                    "allow-always" => "always",
                    _ => "reject",
                };
                ("POST", format!("/api/session/{sid}/permission/{ask}/reply"), json!({"decision": decision, "message": text}))
            }
            Pending::Form { fields } => {
                let chosen = if option.is_empty() || option == "other" { text.unwrap_or("").to_string() } else { option.to_string() };
                let mut answer = serde_json::Map::new();
                for (i, f) in fields.iter().enumerate() {
                    let v = match s(f, "type").as_str() {
                        "boolean" => json!(chosen == "yes" || chosen == "true"),
                        "multiselect" => json!([chosen]),
                        "number" | "integer" => chosen.parse::<f64>().map(|x| json!(x)).unwrap_or(json!(0)),
                        _ if i == 0 => json!(chosen),
                        _ => json!(text.unwrap_or("")),
                    };
                    answer.insert(s(f, "key"), v);
                }
                ("POST", format!("/api/session/{sid}/form/{ask}/reply"), json!({"answer": answer}))
            }
        };
        Ok((call, EventKind::Answered { id: ask.to_string(), option: option.to_string(), text: text.map(str::to_string) }))
    }

    /// The reply failed: the question is still open.
    pub fn unanswer(&mut self, ask: &str) {
        self.answered.remove(ask);
    }

    /// The reply went through.
    pub fn settle(&mut self, ask: &str) {
        self.asks.remove(ask);
        self.answered.remove(ask);
    }
}

fn todos(input: &Value) -> Vec<PlanEntry> {
    input
        .get("todos")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .map(|t| PlanEntry {
            text: s(t, "content"),
            status: match s(t, "status").as_str() {
                "in_progress" | "in-progress" => "in-progress".into(),
                "completed" | "done" => "done".into(),
                _ => "pending".into(),
            },
        })
        .collect()
}

fn diffs_from_input(name: &str, i: &Value) -> Vec<FileDiff> {
    let st = |ks: &[&str]| ks.iter().find_map(|k| i.get(*k).and_then(|v| v.as_str()).map(str::to_string));
    let path = st(&["path", "filePath", "file_path"]).unwrap_or_default();
    match name {
        "write" => vec![FileDiff { path, unified: None, old: None, new: st(&["content"]) }],
        "edit" => vec![FileDiff { path, unified: None, old: st(&["oldString", "old_string", "old"]), new: st(&["newString", "new_string", "new"]) }],
        _ => vec![],
    }
}

/// Session permissions for a thread's permission level.
pub fn ruleset(p: Permission) -> Value {
    let r = |action: &str, effect: &str| json!({"action": action, "resource": "*", "effect": effect});
    Value::Array(match p {
        Permission::Ask => vec![r("*", "allow"), r("edit", "ask"), r("shell", "ask"), r("webfetch", "ask")],
        Permission::Edits => vec![r("*", "allow"), r("shell", "ask"), r("webfetch", "ask")],
        Permission::Plan => vec![r("*", "allow"), r("edit", "deny"), r("shell", "ask")],
        Permission::Full => vec![r("*", "allow")],
    })
}

/// "provider/model" → a model ref.
pub fn model_ref(model: &str, effort: Option<&str>) -> Value {
    let (provider, id) = model.split_once('/').unwrap_or(("opencode", model));
    let mut r = json!({"id": id, "providerID": provider});
    if let Some(e) = effort {
        r["variant"] = json!(e);
    }
    r
}

pub fn models_from_list(list: &Value, default: Option<&str>) -> Vec<ModelInfo> {
    list.get("data")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .filter(|m| m.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true))
        .map(|m| {
            let id = format!("{}/{}", s(m, "providerID"), m.get("modelID").and_then(|x| x.as_str()).unwrap_or_else(|| m.get("id").and_then(|x| x.as_str()).unwrap_or("")));
            ModelInfo {
                label: m.get("name").and_then(|x| x.as_str()).map(str::to_string).unwrap_or_else(|| id.clone()),
                efforts: m.get("variants").and_then(|v| v.as_array()).into_iter().flatten().map(|v| s(v, "id")).collect(),
                default_effort: None,
                default: default == Some(id.as_str()),
                context_window: m.get("limit").and_then(|l| l.get("context")).and_then(|c| c.as_u64()),
                id,
            }
        })
        .collect()
}

type Route = (Arc<Mutex<Translator>>, Tx);

/// The shared `opencode serve`.
pub struct Server {
    pub addr: String,
    auth: String,
    child: Mutex<Option<Child>>,
    pid: Option<u32>,
    routes: Mutex<HashMap<String, Route>>,
    last_used: Mutex<Instant>,
    dead: std::sync::atomic::AtomicBool,
}

impl Server {
    pub async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
        let r = http::request(&self.addr, method, path, &self.auth, body).await?;
        if !r.ok() {
            anyhow::bail!("opencode {method} {path}: HTTP {} {}", r.status, crate::util::truncate(&r.text(), 400));
        }
        r.json()
    }

    fn alive(&self) -> bool {
        !self.dead.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    fn kill(&self) {
        self.dead.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(pid) = self.pid {
            super::kill_group(pid);
        }
        self.child.lock().unwrap().take();
        for (_, (_, tx)) in self.routes.lock().unwrap().drain() {
            let _ = tx.send(DriverOut::Exited(Some("the OpenCode server stopped".into())));
        }
    }
}

pub struct OpencodeAgent {
    server: tokio::sync::Mutex<Option<Arc<Server>>>,
    idle: Duration,
}

impl OpencodeAgent {
    pub fn new(idle_minutes: u64) -> OpencodeAgent {
        OpencodeAgent { server: tokio::sync::Mutex::new(None), idle: Duration::from_secs(idle_minutes.max(1) * 60) }
    }

    async fn server(&self, bin: &PathBuf) -> anyhow::Result<Arc<Server>> {
        let mut g = self.server.lock().await;
        if let Some(s) = g.as_ref()
            && s.alive()
        {
            s.touch();
            return Ok(s.clone());
        }
        let s = start_server(bin).await?;
        // Stop it when no session has used it for a while.
        let (weak, idle) = (Arc::downgrade(&s), self.idle);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(s) = weak.upgrade() else { return };
                if !s.alive() {
                    return;
                }
                let unused = s.routes.lock().unwrap().is_empty() && s.last_used.lock().unwrap().elapsed() >= idle;
                if unused {
                    tracing::info!("stopping the idle OpenCode server");
                    s.kill();
                    return;
                }
            }
        });
        *g = Some(s.clone());
        Ok(s)
    }
}

async fn start_server(bin: &PathBuf) -> anyhow::Result<Arc<Server>> {
    use std::process::Stdio;
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0")?;
        l.local_addr()?.port()
    };
    let password = crate::util::random_hex(16);
    let mut cmd = Command::new(bin);
    cmd.args(["serve", "--port", &port.to_string(), "--hostname", "127.0.0.1"])
        .env("OPENCODE_SERVER_PASSWORD", &password)
        .current_dir(crate::util::home())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("starting opencode serve: {e}"))?;
    for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)].into_iter().flatten() {
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(pipe).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!("opencode serve: {l}");
            }
        });
    }
    let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("opencode:{password}")));
    let server = Arc::new(Server {
        addr: format!("127.0.0.1:{port}"),
        auth,
        pid: child.id(),
        child: Mutex::new(Some(child)),
        routes: Mutex::new(HashMap::new()),
        last_used: Mutex::new(Instant::now()),
        dead: false.into(),
    });
    let t0 = Instant::now();
    loop {
        if let Ok(r) = http::request(&server.addr, "GET", "/api/info", &server.auth, None).await
            && r.ok()
        {
            break;
        }
        let exited = server.child.lock().unwrap().as_mut().map(|c| c.try_wait().ok().flatten().is_some()).unwrap_or(true);
        if exited || t0.elapsed() > Duration::from_secs(60) {
            server.kill();
            anyhow::bail!("opencode serve did not come up");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tracing::info!("opencode serve on {} (in {:?})", server.addr, t0.elapsed());
    // One event stream for every session.
    let weak = Arc::downgrade(&server);
    tokio::spawn(async move {
        loop {
            let Some(s) = weak.upgrade() else { return };
            if !s.alive() {
                return;
            }
            let (addr, auth) = (s.addr.clone(), s.auth.clone());
            drop(s);
            match http::sse(&addr, "/api/event", &auth).await {
                Ok(mut stream) => loop {
                    match stream.next().await {
                        Ok(Some(data)) => {
                            let Ok(ev) = serde_json::from_str::<Value>(&data) else { continue };
                            let Some(sid) = Translator::session_of(&ev) else { continue };
                            let Some(s) = weak.upgrade() else { return };
                            let route = s.routes.lock().unwrap().get(&sid).cloned();
                            if let Some((tr, tx)) = route {
                                s.touch();
                                for e in tr.lock().unwrap().event(&ev) {
                                    let _ = tx.send(DriverOut::Event(e));
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!("opencode event stream: {e}");
                            break;
                        }
                    }
                },
                Err(e) => tracing::warn!("opencode event stream: {e}"),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            // Did the server die?
            let Some(s) = weak.upgrade() else { return };
            let exited = s.child.lock().unwrap().as_mut().map(|c| c.try_wait().ok().flatten().is_some()).unwrap_or(true);
            if exited {
                tracing::warn!("opencode serve exited");
                s.kill();
                return;
            }
        }
    });
    Ok(server)
}

struct Live {
    server: Arc<Server>,
    sid: String,
    tr: Arc<Mutex<Translator>>,
    out: Tx,
}

#[async_trait]
impl Agent for OpencodeAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Opencode
    }

    async fn start(&self, opts: StartOpts, out: Tx) -> anyhow::Result<Box<dyn Session>> {
        let server = self.server(&opts.bin).await?;
        let cwd = opts.cwd.to_string_lossy().to_string();
        let mut sid = None;
        if let Some(id) = &opts.resume {
            match server.call("GET", &format!("/api/session/{id}"), None).await {
                Ok(_) => sid = Some(id.clone()),
                Err(e) => {
                    tracing::info!("opencode session {id} is gone ({e}); starting a new one");
                    let _ = out.send(DriverOut::Event(EventKind::Notice { text: "The OpenCode session was gone; this is a new one".into() }));
                }
            }
        }
        let sid = match sid {
            Some(s) => s,
            None => {
                let mut body = json!({"location": {"directory": cwd}, "permissions": ruleset(opts.permission)});
                if let Some(m) = &opts.model {
                    body["model"] = model_ref(m, opts.effort.as_deref());
                }
                if opts.permission == Permission::Plan {
                    body["agent"] = json!("plan");
                }
                let r = match server.call("POST", "/api/session", Some(&body)).await {
                    Ok(r) => r,
                    Err(e) if opts.permission == Permission::Plan => {
                        tracing::info!("opencode: no plan agent? ({e})");
                        body.as_object_mut().unwrap().remove("agent");
                        server.call("POST", "/api/session", Some(&body)).await?
                    }
                    Err(e) => return Err(e),
                };
                r.get("data").map(|d| s(d, "id")).filter(|i| !i.is_empty()).ok_or_else(|| anyhow::anyhow!("opencode gave no session id"))?
            }
        };
        let info = server.call("GET", &format!("/api/session/{sid}"), None).await.unwrap_or(Value::Null);
        let model = info.get("data").and_then(|d| d.get("model")).map(|m| format!("{}/{}", s(m, "providerID"), s(m, "id"))).filter(|m| m != "/");
        let mut tr = Translator::new(&sid, &cwd);
        // The context window of its model, for Usage.
        if let Some(m) = &model
            && let Ok(list) = server.call("GET", &format!("/api/model?{}={}", enc("location[directory]"), enc(&cwd)), None).await
        {
            tr.context_window = models_from_list(&list, None).into_iter().find(|x| &x.id == m).and_then(|x| x.context_window);
        }
        let tr = Arc::new(Mutex::new(tr));
        server.routes.lock().unwrap().insert(sid.clone(), (tr.clone(), out.clone()));
        let _ = out.send(DriverOut::Event(EventKind::Session { session_id: sid.clone(), model }));
        Ok(Box::new(Live { server, sid, tr, out }))
    }

    async fn models(&self, bin: PathBuf, cwd: PathBuf) -> anyhow::Result<Vec<ModelInfo>> {
        let server = self.server(&bin).await?;
        let loc = format!("{}={}", enc("location[directory]"), enc(&cwd.to_string_lossy()));
        let list = server.call("GET", &format!("/api/model?{loc}"), None).await?;
        let def = server.call("GET", &format!("/api/model/default?{loc}"), None).await.ok();
        let def = def.as_ref().and_then(|d| d.get("data")).filter(|d| !d.is_null()).map(|m| format!("{}/{}", s(m, "providerID"), m.get("modelID").and_then(|x| x.as_str()).unwrap_or("")));
        Ok(models_from_list(&list, def.as_deref()))
    }
}

fn prompt_body(input: &Input) -> Value {
    let mut b = json!({"text": input.text});
    let files: Vec<Value> = input
        .images
        .iter()
        .map(|i| {
            if i.starts_with("data:") || i.contains("://") {
                json!({"uri": i})
            } else {
                json!({"uri": format!("file://{i}")})
            }
        })
        .collect();
    if !files.is_empty() {
        b["files"] = Value::Array(files);
    }
    b
}

#[async_trait]
impl Session for Live {
    async fn send(&mut self, input: &Input) -> anyhow::Result<()> {
        self.server.touch();
        self.server.call("POST", &format!("/api/session/{}/prompt", self.sid), Some(&prompt_body(input))).await?;
        Ok(())
    }

    async fn steer(&mut self, input: &Input) -> anyhow::Result<bool> {
        let mut b = prompt_body(input);
        b["delivery"] = json!("steer");
        self.server.call("POST", &format!("/api/session/{}/prompt", self.sid), Some(&b)).await?;
        Ok(true)
    }

    async fn interrupt(&mut self) -> anyhow::Result<()> {
        self.server.call("POST", &format!("/api/session/{}/interrupt", self.sid), None).await?;
        Ok(())
    }

    async fn answer(&mut self, ask: &str, option: &str, text: Option<&str>) -> anyhow::Result<()> {
        let ((method, path, body), ev) = self.tr.lock().unwrap().answer(ask, option, text)?;
        if let Err(e) = self.server.call(method, &path, Some(&body)).await {
            self.tr.lock().unwrap().unanswer(ask);
            return Err(e);
        }
        self.tr.lock().unwrap().settle(ask);
        let _ = self.out.send(DriverOut::Event(ev));
        Ok(())
    }

    async fn set_model(&mut self, model: &str, effort: Option<&str>) -> anyhow::Result<bool> {
        self.server.call("POST", &format!("/api/session/{}/model", self.sid), Some(&json!({"model": model_ref(model, effort)}))).await?;
        Ok(true)
    }

    async fn close(&mut self) {
        // The server is shared: just stop listening for this session.
        let running = self.tr.lock().unwrap().running;
        if running {
            let _ = self.server.call("POST", &format!("/api/session/{}/interrupt", self.sid), None).await;
        }
        self.server.routes.lock().unwrap().remove(&self.sid);
        self.server.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Translator, fixture: &str) -> Vec<EventKind> {
        let mut evs = Vec::new();
        for l in fixture.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(l).unwrap();
            if let Some(e) = v.get("_sse") {
                if Translator::session_of(e).as_deref() == Some(t.session.as_str()) {
                    evs.extend(t.event(e));
                }
            }
        }
        evs
    }

    #[test]
    fn a_real_turn_with_two_permissions() {
        let mut t = Translator::new("ses_df0146f98ffepkWzyLnczOlF6j", "/tmp/agentd-rec/proj");
        t.context_window = Some(1_048_576);
        let evs = run(&mut t, include_str!("../../tests/fixtures/opencode_turn.jsonl"));
        let title = evs.iter().find_map(|e| if let EventKind::Title { title } = e { Some(title.clone()) } else { None });
        assert_eq!(title.as_deref(), Some("Create oc.txt with hi"));
        let tools: Vec<(String, agentum_proto::ToolKind, String)> = evs.iter().filter_map(|e| if let EventKind::Tool { name, kind, title, .. } = e { Some((name.clone(), *kind, title.clone())) } else { None }).collect();
        assert_eq!(tools, vec![("shell".into(), agentum_proto::ToolKind::Exec, "ls".into()), ("write".into(), agentum_proto::ToolKind::Edit, "oc.txt".into())]);
        let asks: Vec<String> = evs.iter().filter_map(|e| if let EventKind::Ask { title, .. } = e { Some(title.clone()) } else { None }).collect();
        assert_eq!(asks, vec!["shell: ls", "edit: oc.txt"]);
        // Replies came from the recorder (not through us): they show as answered.
        let answered = evs.iter().filter(|e| matches!(e, EventKind::Answered { option, .. } if option == "allow")).count();
        assert_eq!(answered, 2);
        let write_done = evs.iter().find_map(|e| if let EventKind::ToolDone { item, diffs, ok: true, .. } = e { (item == "chatcmpl-tool-96a53fbc83743412").then(|| diffs.clone()) } else { None }).unwrap();
        assert!(write_done[0].unified.as_ref().unwrap().contains("+hi"));
        let ls_out = evs.iter().find_map(|e| if let EventKind::ToolDone { item, output, .. } = e { (item == "chatcmpl-tool-9981bc0c70778409").then(|| output.clone()) } else { None }).unwrap();
        assert!(ls_out.unwrap().contains("notes.txt"));
        assert!(evs.iter().any(|e| matches!(e, EventKind::Message { text, .. } if text == "done")));
        assert!(evs.iter().any(|e| matches!(e, EventKind::Thinking { .. })));
        let EventKind::TurnDone { stop, usage, cost_usd, .. } = evs.last().unwrap() else { panic!("{:?}", evs.last()) };
        assert_eq!(*stop, StopReason::End);
        assert_eq!(*cost_usd, Some(0.0));
        let u = usage.as_ref().unwrap();
        assert_eq!(u.input, 10909);
        assert_eq!(u.context_used, Some(817 + 3 + 10752));
    }

    #[test]
    fn a_real_interrupt() {
        let mut t = Translator::new("ses_df0146f98ffepkWzyLnczOlF6j", "/tmp/agentd-rec/proj");
        let evs = run(&mut t, include_str!("../../tests/fixtures/opencode_interrupt.jsonl"));
        assert!(matches!(evs.last(), Some(EventKind::TurnDone { stop: StopReason::Interrupted, .. })));
        assert!(!evs.iter().any(|e| matches!(e, EventKind::Error { .. })));
    }

    #[test]
    fn answers() {
        let mut t = Translator::new("ses_1", "/r");
        t.event(&json!({"type": "permission.asked", "data": {"id": "per_1", "sessionID": "ses_1", "action": "shell", "resources": ["rm x"]}}));
        let ((m, path, body), ev) = t.answer("per_1", "allow-always", None).unwrap();
        assert_eq!((m, path.as_str()), ("POST", "/api/session/ses_1/permission/per_1/reply"));
        assert_eq!(body["decision"], "always");
        assert!(matches!(ev, EventKind::Answered { .. }));
        t.settle("per_1");
        // Our own answer's echo is not a second Answered.
        assert!(t.event(&json!({"type": "permission.replied", "data": {"sessionID": "ses_1", "requestID": "per_1", "reply": "always"}})).is_empty());
        let a = t.event(&json!({"type": "form.created", "data": {"form": {"id": "frm_1", "sessionID": "ses_1", "title": "Pick one", "fields": [{"key": "choice", "type": "string", "options": [{"value": "a", "label": "A"}, {"value": "b", "label": "B"}]}]}}}));
        assert!(matches!(&a[0], EventKind::Ask { kind: AskKind::Question, options, .. } if options.len() == 2));
        let ((_, path, body), _) = t.answer("frm_1", "b", None).unwrap();
        assert_eq!(path, "/api/session/ses_1/form/frm_1/reply");
        assert_eq!(body, json!({"answer": {"choice": "b"}}));
    }

    #[test]
    fn permission_rules_and_models() {
        assert_eq!(ruleset(Permission::Full), json!([{"action": "*", "resource": "*", "effect": "allow"}]));
        assert_eq!(model_ref("opencode-go/hy4-flash-free", Some("high")), json!({"id": "hy4-flash-free", "providerID": "opencode-go", "variant": "high"}));
        let list = json!({"data": [{"id": "m", "modelID": "m", "providerID": "p", "name": "M", "variants": [{"id": "low"}], "limit": {"context": 1000}}]});
        let m = models_from_list(&list, Some("p/m"));
        assert_eq!(m[0].id, "p/m");
        assert!(m[0].default);
        assert_eq!(m[0].context_window, Some(1000));
        assert_eq!(m[0].efforts, vec!["low"]);
    }
}
