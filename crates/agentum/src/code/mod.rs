//! The Code section: machines (agentd on each), coding threads and their events.
//!
//! The hub is the source of truth for threads: agentd runs the agents and numbers their events;
//! the hub stores them (deltas only stream, they aren't kept), keeps each thread's summary, and
//! pushes both to the browsers subscribed on `/api/ws`.
pub mod agentd;
pub mod browser;
pub mod db;
pub mod render;

use crate::state::App;
use crate::util::{bad, not_found, AppError, R};
use agentum_proto::api::*;
use agentum_proto::machine::{FsReq, GitReq, ModelInfo, Req};
use agentum_proto::{AgentKind, Event, EventKind, Input, Permission, SendMode, ThreadSpec, ThreadState};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

pub use agentd::Conn;

/// One message on one channel, serialized once for every browser.
pub type Out = Arc<(String, Arc<str>)>;

pub struct Hub {
    pub machines: RwLock<HashMap<String, Arc<Conn>>>,
    /// Every thread's summary (the sidebar is served from here).
    pub threads: RwLock<HashMap<String, ThreadSummary>>,
    /// Text streaming right now: thread → item → text.
    pub partial: Mutex<HashMap<String, BTreeMap<String, Partial>>>,
    pub limits: RwLock<Vec<AgentLimits>>,
    pub out: tokio::sync::broadcast::Sender<Out>,
    models: Mutex<HashMap<(String, AgentKind), (Instant, Vec<ModelInfo>)>>,
    html: Mutex<HashMap<(String, u64), Arc<str>>>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub {
            machines: Default::default(),
            threads: Default::default(),
            partial: Default::default(),
            limits: Default::default(),
            out: tokio::sync::broadcast::channel(4096).0,
            models: Default::default(),
            html: Default::default(),
        }
    }
}

impl Hub {
    pub fn publish(&self, ch: &str, m: &WsOut) {
        let js: Arc<str> = serde_json::to_string(m).unwrap_or_default().into();
        let _ = self.out.send(Arc::new((ch.to_string(), js)));
    }
    pub fn conn(&self, machine: &str) -> Option<Arc<Conn>> {
        self.machines.read().unwrap().get(machine).cloned()
    }
    pub fn summary(&self, id: &str) -> Option<ThreadSummary> {
        self.threads.read().unwrap().get(id).cloned()
    }
    fn set_summary(&self, t: ThreadSummary) {
        self.publish("threads", &WsOut::Thread { summary: t.clone() });
        self.threads.write().unwrap().insert(t.id.clone(), t);
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id() -> String {
    use rand::Rng;
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut r = rand::rng();
    format!("t_{}", (0..10).map(|_| A[r.random_range(0..A.len())] as char).collect::<String>())
}

fn title_of(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut t: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        t.push('…');
    }
    t
}

/// Load summaries; start the scheduler. Called once at boot.
pub async fn start(app: &App) -> anyhow::Result<()> {
    app.db.call(|c| db::migrate(c)).await?;
    let all = app.db.call(|c| db::all_threads(c)).await?;
    {
        let mut m = app.code.threads.write().unwrap();
        for mut t in all {
            // No machine is connected yet: nothing is running.
            if t.state.busy() {
                t.state = ThreadState::Interrupted;
                t.attention = db::attention_of(&t);
            }
            m.insert(t.id.clone(), t);
        }
    }
    let a = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            scheduled(&a).await;
        }
    });
    Ok(())
}

/// Send what was scheduled for now.
async fn scheduled(app: &App) {
    let t = now();
    let due: Vec<(i64, String, String)> = app
        .db
        .call(move |c| {
            let mut st = c.prepare("SELECT id, thread, body FROM code_scheduled WHERE send_at<=?1")?;
            let v = st.query_map([t], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.filter_map(Result::ok).collect();
            Ok(v)
        })
        .await
        .unwrap_or_default();
    for (id, thread, body) in due {
        let _ = app.db.call(move |c| c.execute("DELETE FROM code_scheduled WHERE id=?1", [id])).await;
        if let Ok(mut b) = serde_json::from_str::<SendBody>(&body) {
            b.send_at = None;
            if let Err(e) = send(app, &thread, b).await {
                tracing::warn!("scheduled send to {thread}: {}", e.1);
            }
        }
    }
}

/// A machine (re)connected: threads the hub thinks are running there but agentd doesn't have
/// are interrupted (agentd restarted).
pub async fn machine_online(app: &App, machine: &str) {
    let live: Vec<String> = app.code.conn(machine).map(|c| c.hello.read().unwrap().threads.iter().filter(|t| t.state.busy()).map(|t| t.thread.clone()).collect()).unwrap_or_default();
    let stale: Vec<ThreadSummary> = app.code.threads.read().unwrap().values().filter(|t| t.machine == machine && t.state.busy() && !live.contains(&t.id)).cloned().collect();
    for mut t in stale {
        t.state = ThreadState::Interrupted;
        t.attention = db::attention_of(&t);
        let t2 = t.clone();
        let _ = app.db.call(move |c| db::save_thread(c, &t2)).await;
        app.code.set_summary(t);
    }
}

fn view_of(app: &App, thread: &str, e: Event) -> EventView {
    let src = match &e.kind {
        EventKind::User { text, .. } | EventKind::Message { text, .. } => Some(text.clone()),
        _ => None,
    };
    let html = src.map(|s| {
        let key = (thread.to_string(), e.seq);
        if let Some(h) = app.code.html.lock().unwrap().get(&key) {
            return h.to_string();
        }
        let h: Arc<str> = render::markdown(&s).into();
        let mut cache = app.code.html.lock().unwrap();
        if cache.len() > 4000 {
            cache.clear();
        }
        cache.insert(key, h.clone());
        h.to_string()
    });
    EventView { event: e, html }
}

/// Events from agentd: store, update the summary, ack, push to browsers, tell plugins.
pub async fn ingest(app: &App, conn: &Conn, thread: &str, events: Vec<Event>) {
    let Some(mut t) = app.code.summary(thread) else {
        tracing::warn!("events for unknown thread {thread}");
        return;
    };
    let mut keep = vec![];
    let mut max_seq = t.last_seq;
    let mut session: Option<String> = None;
    for e in events {
        if e.seq <= max_seq {
            continue;
        }
        max_seq = e.seq;
        let ch = format!("thread:{thread}");
        if e.kind.is_delta() {
            let mut p = app.code.partial.lock().unwrap();
            let map = p.entry(thread.to_string()).or_default();
            match &e.kind {
                EventKind::Delta { item, text } | EventKind::ThinkingDelta { item, text } => {
                    let thinking = matches!(e.kind, EventKind::ThinkingDelta { .. });
                    map.entry(item.clone()).or_insert_with(|| Partial { item: item.clone(), thinking, text: String::new() }).text.push_str(text);
                }
                _ => {}
            }
            drop(p);
            app.code.publish(&ch, &WsOut::Event { thread: thread.to_string(), event: EventView { event: e, html: None } });
            continue;
        }
        let ts = e.ts;
        match &e.kind {
            EventKind::User { text, queued, .. } => {
                if t.title.is_empty() {
                    t.title = title_of(text);
                }
                t.preview = title_of(text);
                if *queued {
                    t.queued += 1;
                }
            }
            EventKind::Dequeued { .. } => t.queued = t.queued.saturating_sub(1),
            EventKind::Message { item, text } => {
                t.preview = title_of(text);
                app.code.partial.lock().unwrap().get_mut(thread).map(|m| m.remove(item));
            }
            EventKind::Thinking { item, .. } => {
                app.code.partial.lock().unwrap().get_mut(thread).map(|m| m.remove(item));
            }
            EventKind::Ask { title, .. } => t.asking = Some(title.clone()),
            EventKind::Answered { .. } => t.asking = None,
            EventKind::TurnDone { usage, cost_usd, .. } => {
                t.unread = true;
                t.asking = None;
                if let Some(c) = cost_usd {
                    t.cost_usd += c;
                }
                if let Some(u) = usage {
                    let mut u2 = u.clone();
                    if let Some(old) = &t.usage {
                        u2.context_window = u2.context_window.or(old.context_window);
                        u2.context_used = u2.context_used.or(old.context_used);
                    }
                    t.usage = Some(u2);
                }
                app.code.partial.lock().unwrap().remove(thread);
            }
            EventKind::Usage { usage } => t.usage = Some(usage.clone()),
            EventKind::RateLimits { limits } => {
                let l = AgentLimits { agent: t.agent, machine: t.machine.clone(), limits: limits.clone(), at: ts };
                {
                    let mut all = app.code.limits.write().unwrap();
                    all.retain(|x| !(x.agent == l.agent && x.machine == l.machine));
                    all.insert(0, l.clone());
                }
                app.code.publish("threads", &WsOut::Limits { limits: l });
            }
            EventKind::Session { session_id, model } => {
                session = Some(session_id.clone());
                if model.is_some() {
                    t.model = model.clone();
                }
            }
            EventKind::State { state } => {
                t.state = *state;
                if !state.busy() {
                    t.asking = None;
                }
            }
            EventKind::Title { title } => t.title = title.clone(),
            EventKind::Error { message, .. } => t.preview = title_of(message),
            _ => {}
        }
        if !matches!(e.kind, EventKind::Usage { .. } | EventKind::RateLimits { .. } | EventKind::Session { .. }) {
            t.updated_at = t.updated_at.max(ts);
        }
        keep.push(e);
    }
    t.last_seq = max_seq;
    t.attention = db::attention_of(&t);
    let id = thread.to_string();
    let (k2, t2, s2) = (keep.clone(), t.clone(), session.clone());
    let stored = app
        .db
        .call(move |c| {
            let fresh = db::insert_events(c, &id, &k2)?;
            db::save_thread(c, &t2)?;
            if let Some(s) = s2 {
                db::set_session(c, &id, &s)?;
            }
            Ok(fresh)
        })
        .await;
    match stored {
        Ok(fresh) => {
            conn.ack(thread, max_seq);
            app.code.set_summary(t.clone());
            let ch = format!("thread:{thread}");
            for e in fresh {
                let v = view_of(app, thread, e.clone());
                app.code.publish(&ch, &WsOut::Event { thread: thread.to_string(), event: v });
                crate::plugins::on_event(app, &t, &e);
            }
        }
        Err(e) => tracing::warn!("storing events of {thread}: {e:#}"),
    }
}

pub fn routes() -> Router<App> {
    Router::new()
        .route("/agentd", get(agentd::upgrade))
        .route("/api/ws", get(browser::upgrade))
        .route("/api/code/overview", get(overview))
        .route("/api/code/threads", get(list).post(create))
        .route("/api/code/threads/{id}", get(detail).patch(patch).delete(remove))
        .route("/api/code/threads/{id}/send", post(send_h))
        .route("/api/code/threads/{id}/stop", post(stop))
        .route("/api/code/threads/{id}/answer", post(answer))
        .route("/api/code/models", get(models))
        .route("/api/code/search", get(search))
        .route("/api/code/machines", get(machines_h).post(add_machine))
        .route("/api/code/machines/{id}", delete(remove_machine))
        .route("/api/code/machines/{id}/projects", post(add_project))
        .route("/api/code/machines/{id}/main", post(set_main))
        .route("/api/code/call", post(call))
        .route("/api/code/syntax.css", get(|| async { ([("content-type", "text/css"), ("cache-control", "public, max-age=86400")], render::syntax_css()) }))
}

fn offline(m: &str) -> AppError {
    AppError(StatusCode::CONFLICT, format!("{m} is offline"))
}

/// Call agentd on a machine (for the hub, plugins and the generic /api/code/call).
pub async fn machine_call(app: &App, machine: &str, req: Req) -> Result<Value, String> {
    let c = app.code.conn(machine).ok_or_else(|| format!("{machine} is offline"))?;
    let wait = match &req {
        Req::Git(GitReq::Push { .. }) | Req::Git(GitReq::PrMerge { .. }) | Req::Git(GitReq::WorktreeAdd { .. }) => Duration::from_secs(120),
        Req::Start { .. } | Req::Models { .. } => Duration::from_secs(60),
        _ => Duration::from_secs(30),
    };
    c.call(req, wait).await
}

async fn overview(State(app): State<App>) -> Json<Overview> {
    let machines = agentd::machines(&app).await;
    let mut threads: Vec<ThreadSummary> = app.code.threads.read().unwrap().values().filter(|t| !t.archived).cloned().collect();
    threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let limits = app.code.limits.read().unwrap().clone();
    Json(Overview { machines, threads, limits })
}

#[derive(serde::Deserialize)]
struct ListQ {
    #[serde(default)]
    archived: Option<String>,
}
async fn list(State(app): State<App>, Query(q): Query<ListQ>) -> Json<Vec<ThreadSummary>> {
    let arch = q.archived.is_some_and(|a| a == "1" || a == "true");
    let mut v: Vec<ThreadSummary> = app.code.threads.read().unwrap().values().filter(|t| t.archived == arch).cloned().collect();
    v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    v.truncate(300);
    Json(v)
}

async fn create(State(app): State<App>, Json(b): Json<NewThread>) -> R<Json<ThreadSummary>> {
    Ok(Json(create_thread(&app, b).await?))
}

pub async fn create_thread(app: &App, b: NewThread) -> R<ThreadSummary> {
    let parent = match &b.parent {
        Some(p) => Some(app.code.summary(p).ok_or_else(not_found)?),
        None => None,
    };
    let machine = parent.as_ref().map(|p| p.machine.clone()).unwrap_or(b.machine.clone());
    if app.code.conn(&machine).is_none() {
        return Err(offline(&machine));
    }
    let project = parent.as_ref().map(|p| p.project.clone()).unwrap_or(b.project.clone());
    if project.is_empty() {
        return Err(bad("pick a project"));
    }
    let (mut cwd, mut branch) = (parent.as_ref().map(|p| p.cwd.clone()).unwrap_or(project.clone()), parent.as_ref().and_then(|p| p.branch.clone()));
    if let Some(br) = b.worktree.as_ref().filter(|s| !s.trim().is_empty() && parent.is_none()) {
        let r = machine_call(app, &machine, Req::Git(GitReq::WorktreeAdd { repo: project.clone(), branch: br.trim().to_string(), base: None })).await.map_err(bad)?;
        cwd = r["path"].as_str().unwrap_or(&cwd).to_string();
        branch = r["branch"].as_str().map(str::to_string).or(Some(br.clone()));
    } else if branch.is_none() {
        branch = app.code.conn(&machine).and_then(|c| c.hello.read().unwrap().projects.iter().find(|p| p.path == project).and_then(|p| p.branch.clone()));
    }
    let agent = b.agent.or(parent.as_ref().map(|p| p.agent)).unwrap_or(AgentKind::ClaudeCode);
    let t = now();
    let title = match &parent {
        Some(p) => format!("Side chat · {}", if p.title.is_empty() { "thread" } else { &p.title }),
        None => title_of(&b.text),
    };
    let s = ThreadSummary {
        id: new_id(),
        machine: machine.clone(),
        agent,
        model: b.model.clone().filter(|m| !m.is_empty()),
        effort: b.effort.clone().filter(|m| !m.is_empty()),
        permission: b.permission,
        project,
        cwd: cwd.clone(),
        branch,
        title,
        state: if b.text.trim().is_empty() { ThreadState::Idle } else { ThreadState::Starting },
        attention: agentum_proto::Attention::Working,
        unread: false,
        pinned: false,
        archived: false,
        order: 0.0,
        section: None,
        parent: b.parent.clone(),
        created_at: t,
        updated_at: t,
        preview: title_of(&b.text),
        asking: None,
        queued: 0,
        usage: None,
        cost_usd: 0.0,
        last_seq: 0,
    };
    let mut s = s;
    s.attention = db::attention_of(&s);
    let s2 = s.clone();
    app.db.call(move |c| db::save_thread(c, &s2)).await?;
    app.code.set_summary(s.clone());
    if !b.text.trim().is_empty() {
        start_agent(app, &s, Input { text: b.text, images: b.images }, None).await?;
    }
    Ok(app.code.summary(&s.id).unwrap_or(s))
}

/// Start (or resume) the agent of a thread with this first input.
async fn start_agent(app: &App, s: &ThreadSummary, mut input: Input, resume: Option<String>) -> R<()> {
    crate::plugins::before_turn(app, s, &input.text).await.map_err(|e| AppError(StatusCode::CONFLICT, e))?;
    // A side chat's first message carries its parent's conversation.
    if let (Some(pid), None) = (&s.parent, &resume) {
        let pid2 = pid.clone();
        let evs = app.db.call(move |c| db::events(c, &pid2, None, 60)).await?.0;
        let mut ctx = String::new();
        for e in evs {
            match e.kind {
                EventKind::User { text, .. } => ctx += &format!("\n[user] {text}\n"),
                EventKind::Message { text, .. } => ctx += &format!("\n[agent] {text}\n"),
                _ => {}
            }
        }
        let tail: String = ctx.chars().rev().take(12_000).collect::<Vec<_>>().into_iter().rev().collect();
        input.text = format!("You are a side chat about another coding thread in this same directory. Answer questions about it; don't change files unless asked. Its conversation so far:\n{tail}\n\n---\n\n{}", input.text);
    }
    let spec = ThreadSpec { agent: Some(s.agent), model: s.model.clone(), effort: s.effort.clone(), permission: s.permission, cwd: s.cwd.clone() };
    let req = Req::Start { thread: s.id.clone(), spec, resume, input: Some(input), last_seq: s.last_seq };
    match machine_call(app, &s.machine, req).await {
        Ok(_) => {
            let id = s.id.clone();
            app.db.call(move |c| db::set_started(c, &id)).await?;
            Ok(())
        }
        Err(e) => {
            if let Some(mut t) = app.code.summary(&s.id) {
                t.state = ThreadState::Error;
                t.preview = title_of(&e);
                t.attention = db::attention_of(&t);
                let t2 = t.clone();
                let _ = app.db.call(move |c| db::save_thread(c, &t2)).await;
                app.code.set_summary(t);
            }
            Err(AppError(StatusCode::BAD_GATEWAY, e))
        }
    }
}

#[derive(serde::Deserialize)]
struct DetailQ {
    before: Option<u64>,
    limit: Option<u32>,
}
async fn detail(State(app): State<App>, Path(id): Path<String>, Query(q): Query<DetailQ>) -> R<Json<ThreadDetail>> {
    let summary = app.code.summary(&id).ok_or_else(not_found)?;
    let id2 = id.clone();
    let limit = q.limit.unwrap_or(400).clamp(20, 2000);
    let (evs, more) = app.db.call(move |c| db::events(c, &id2, q.before, limit)).await?;
    let events = evs.into_iter().map(|e| view_of(&app, &id, e)).collect();
    let partial = if q.before.is_none() { app.code.partial.lock().unwrap().get(&id).map(|m| m.values().cloned().collect()).unwrap_or_default() } else { vec![] };
    Ok(Json(ThreadDetail { summary, events, more_before: more, partial }))
}

async fn patch(State(app): State<App>, Path(id): Path<String>, Json(p): Json<PatchThread>) -> R<Json<ThreadSummary>> {
    Ok(Json(patch_thread(&app, &id, p).await?))
}

/// Rename, pin, archive, mark read, reorder, or switch a thread's model.
pub async fn patch_thread(app: &App, id: &str, p: PatchThread) -> R<ThreadSummary> {
    let id = id.to_string();
    let mut t = app.code.summary(&id).ok_or_else(not_found)?;
    let mut model_changed = false;
    if let Some(v) = p.title {
        t.title = v.trim().chars().take(200).collect();
    }
    if let Some(v) = p.pinned {
        t.pinned = v;
    }
    if let Some(v) = p.archived {
        t.archived = v;
        if v {
            if let Some(c) = app.code.conn(&t.machine) {
                c.fire(Req::Sleep { thread: id.clone() });
            }
        }
    }
    if let Some(v) = p.read {
        t.unread = !v;
    }
    if let Some(v) = p.order {
        t.order = v;
    }
    if let Some(v) = p.section {
        t.section = v;
    }
    if let Some(v) = p.model {
        model_changed = t.model.as_deref() != Some(v.as_str());
        t.model = (!v.is_empty()).then_some(v);
    }
    if let Some(v) = p.effort {
        model_changed |= t.effort.as_deref() != Some(v.as_str());
        t.effort = (!v.is_empty()).then_some(v);
    }
    if let Some(v) = p.permission {
        t.permission = v;
    }
    t.attention = db::attention_of(&t);
    let t2 = t.clone();
    app.db.call(move |c| db::save_thread(c, &t2)).await?;
    app.code.set_summary(t.clone());
    if model_changed {
        if let (Some(c), Some(m)) = (app.code.conn(&t.machine), t.model.clone()) {
            c.fire(Req::SetModel { thread: id, model: m, effort: t.effort.clone() });
        }
    }
    Ok(t)
}

async fn remove(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let t = app.code.summary(&id).ok_or_else(not_found)?;
    if let Some(c) = app.code.conn(&t.machine) {
        c.fire(Req::Sleep { thread: id.clone() });
    }
    let id2 = id.clone();
    app.db.call(move |c| db::delete_thread(c, &id2)).await?;
    app.code.threads.write().unwrap().remove(&id);
    app.code.publish("threads", &WsOut::ThreadGone { id });
    Ok(Json(json!({"ok": true})))
}

async fn send_h(State(app): State<App>, Path(id): Path<String>, Json(b): Json<SendBody>) -> R<Json<Value>> {
    send(&app, &id, b).await?;
    Ok(Json(json!({"ok": true})))
}

/// Send to a thread: now, into the running turn, after it, or later (send_at).
pub async fn send(app: &App, id: &str, b: SendBody) -> R<()> {
    let t = app.code.summary(id).ok_or_else(not_found)?;
    if b.text.trim().is_empty() && b.images.is_empty() {
        return Err(bad("nothing to send"));
    }
    if let Some(at) = b.send_at.filter(|at| *at > now() + 1000) {
        let (id2, body) = (id.to_string(), serde_json::to_string(&b).unwrap_or_default());
        app.db.call(move |c| c.execute("INSERT INTO code_scheduled(thread, body, send_at) VALUES(?1,?2,?3)", rusqlite::params![id2, body, at])).await?;
        return Ok(());
    }
    if app.code.conn(&t.machine).is_none() {
        return Err(offline(&t.machine));
    }
    if t.archived {
        let mut t2 = t.clone();
        t2.archived = false;
        let t3 = t2.clone();
        app.db.call(move |c| db::save_thread(c, &t3)).await?;
        app.code.set_summary(t2);
    }
    let input = Input { text: b.text.clone(), images: b.images.clone() };
    let id2 = id.to_string();
    if !app.db.call(move |c| db::started(c, &id2)).await? {
        return start_agent(app, &t, input, None).await;
    }
    if !t.state.busy() {
        crate::plugins::before_turn(app, &t, &input.text).await.map_err(|e| AppError(StatusCode::CONFLICT, e))?;
    }
    match machine_call(app, &t.machine, Req::Send { thread: id.to_string(), input: input.clone(), mode: b.mode }).await {
        Ok(_) => Ok(()),
        Err(e) if e.contains("unknown thread") => {
            let id3 = id.to_string();
            let session = app.db.call(move |c| db::session_of(c, &id3)).await?;
            start_agent(app, &t, input, session).await
        }
        Err(e) => Err(AppError(StatusCode::BAD_GATEWAY, e)),
    }
}

async fn stop(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let t = app.code.summary(&id).ok_or_else(not_found)?;
    machine_call(&app, &t.machine, Req::Stop { thread: id }).await.map_err(|e| AppError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(json!({"ok": true})))
}

async fn answer(State(app): State<App>, Path(id): Path<String>, Json(b): Json<AnswerBody>) -> R<Json<Value>> {
    let t = app.code.summary(&id).ok_or_else(not_found)?;
    machine_call(&app, &t.machine, Req::Answer { thread: id, ask: b.ask, option: b.option, text: b.text }).await.map_err(|e| AppError(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(json!({"ok": true})))
}

#[derive(serde::Deserialize)]
struct ModelsQ {
    machine: String,
    agent: String,
}
async fn models(State(app): State<App>, Query(q): Query<ModelsQ>) -> R<Json<Models>> {
    let agent = AgentKind::parse(&q.agent).ok_or_else(|| bad("unknown agent"))?;
    let key = (q.machine.clone(), agent);
    if let Some((at, v)) = app.code.models.lock().unwrap().get(&key) {
        if at.elapsed() < Duration::from_secs(600) {
            return Ok(Json(Models { agent, models: v.clone() }));
        }
    }
    let v = machine_call(&app, &q.machine, Req::Models { agent }).await.map_err(|e| AppError(StatusCode::BAD_GATEWAY, e))?;
    let list: Vec<ModelInfo> = serde_json::from_value(v.get("models").cloned().unwrap_or(v)).unwrap_or_default();
    app.code.models.lock().unwrap().insert(key, (Instant::now(), list.clone()));
    Ok(Json(Models { agent, models: list }))
}

#[derive(serde::Deserialize)]
struct SearchQ {
    q: String,
}
async fn search(State(app): State<App>, Query(q): Query<SearchQ>) -> R<Json<Vec<SearchHit>>> {
    let s = q.q.trim().to_string();
    if s.is_empty() {
        return Ok(Json(vec![]));
    }
    let mut hits: Vec<SearchHit> = vec![];
    // Titles first (instant, from memory), then what was said.
    let lq = s.to_lowercase();
    for t in app.code.threads.read().unwrap().values().filter(|t| t.title.to_lowercase().contains(&lq)).take(8) {
        hits.push(SearchHit { thread: t.id.clone(), title: t.title.clone(), seq: 0, excerpt: html_escape::encode_text(&t.preview).into_owned() });
    }
    let rows = app.db.call(move |c| db::search(c, &s, 40)).await?;
    for (thread, seq, excerpt) in rows {
        if let Some(t) = app.code.summary(&thread) {
            // The snippet's text is the agent's: escape it, keep only our <mark>s.
            let safe = html_escape::encode_text(&excerpt).replace("&lt;mark&gt;", "<mark>").replace("&lt;/mark&gt;", "</mark>");
            hits.push(SearchHit { thread, title: t.title, seq, excerpt: safe });
        }
    }
    Ok(Json(hits))
}

async fn machines_h(State(app): State<App>) -> Json<Vec<Machine>> {
    Json(agentd::machines(&app).await)
}

async fn add_machine(State(app): State<App>, headers: axum::http::HeaderMap, Json(b): Json<Value>) -> R<Json<Value>> {
    let name = b["name"].as_str().unwrap_or("").to_string();
    let (id, token) = agentd::create_machine(&app, &name).await.map_err(|e| bad(e.to_string()))?;
    let host = headers.get("x-forwarded-host").or(headers.get("host")).and_then(|v| v.to_str().ok()).unwrap_or("localhost:7100").to_string();
    let proto = if headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https") { "wss" } else { "ws" };
    Ok(Json(json!({"id": id, "token": token, "install": format!("agentd install --hub {proto}://{host}/agentd --token {token} --name {id}")})))
}

async fn remove_machine(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let id2 = id.clone();
    app.db.call(move |c| c.execute("DELETE FROM code_machines WHERE id=?1", [id2])).await?;
    app.code.machines.write().unwrap().remove(&id);
    Ok(Json(json!({"ok": true})))
}

async fn set_main(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    app.db.call(move |c| c.execute("UPDATE code_machines SET main=(id=?1)", [id])).await?;
    Ok(Json(json!({"ok": true})))
}

async fn add_project(State(app): State<App>, Path(id): Path<String>, Json(b): Json<Value>) -> R<Json<Value>> {
    let path = b["path"].as_str().unwrap_or("").to_string();
    let v = machine_call(&app, &id, Req::AddProject { path }).await.map_err(bad)?;
    Ok(Json(v))
}

#[derive(serde::Deserialize)]
struct CallBody {
    machine: String,
    req: Req,
}
/// A machine request from the web app (files, git, terminals): the same as plugins use.
async fn call(State(app): State<App>, Json(b): Json<CallBody>) -> R<Json<Value>> {
    if let Req::Fs(FsReq::Write { .. }) | Req::Fs(FsReq::Read { .. }) | Req::Fs(FsReq::List { .. }) | Req::Fs(FsReq::Find { .. }) | Req::Git(_) | Req::Term(_) = &b.req {
        machine_call(&app, &b.machine, b.req).await.map(Json).map_err(|e| AppError(StatusCode::BAD_GATEWAY, e))
    } else {
        Err(bad("only files, git and terminals through /api/code/call"))
    }
}

/// For the Secretary's code_status tool: open threads, one line each.
pub async fn status_text(app: &App) -> String {
    let mut v: Vec<ThreadSummary> = app.code.threads.read().unwrap().values().filter(|t| !t.archived).cloned().collect();
    if v.is_empty() {
        return "No open coding threads.".into();
    }
    v.sort_by_key(|t| (t.attention, std::cmp::Reverse(t.updated_at)));
    let rows: Vec<String> = v
        .iter()
        .take(40)
        .map(|t| {
            let state = match t.attention {
                agentum_proto::Attention::NeedsYou => "needs the user",
                agentum_proto::Attention::Working => "working",
                agentum_proto::Attention::Ready => "finished, unread",
                agentum_proto::Attention::Settled => "idle",
            };
            format!("- {} · {} · on {} · {}", t.project.rsplit('/').next().unwrap_or(""), t.title, t.machine, state)
        })
        .collect();
    format!("Open coding threads ({}):\n{}", rows.len(), rows.join("\n"))
}

/// For the Secretary's code_task tool: start a Claude Code thread in a project, main machine first.
pub async fn start_task(app: &App, project: &str, task: &str) -> anyhow::Result<String> {
    let want = project.trim().to_lowercase();
    let machines = agentd::machines(app).await;
    let mut found = None;
    for m in machines.iter().filter(|m| m.online) {
        if let Some(p) = m.projects.iter().find(|p| p.name.to_lowercase() == want).or_else(|| m.projects.iter().find(|p| !want.is_empty() && p.name.to_lowercase().contains(&want))) {
            found = Some((m.id.clone(), p.path.clone(), p.name.clone()));
            break;
        }
    }
    let Some((machine, path, name)) = found else {
        let names: Vec<String> = machines.iter().flat_map(|m| m.projects.iter().map(|p| p.name.clone())).take(40).collect();
        anyhow::bail!("No project called {project:?} on an online machine. Projects: {}", names.join(", "));
    };
    let t = create_thread(app, NewThread { machine: machine.clone(), project: path, agent: Some(AgentKind::ClaudeCode), permission: Permission::Edits, text: task.to_string(), ..Default::default() })
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
    Ok(format!("Started a coding thread on {name} ({machine}): {} (follow it in Code, /code?thread={}).", t.title, t.id))
}

#[allow(dead_code)]
fn _unused(_: SendMode) {}
