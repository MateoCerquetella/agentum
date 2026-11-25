//! The Code section: machines (agentd on each), coding threads and their events.
//!
//! The hub is the source of truth for threads: agentd runs the agents and numbers their events;
//! the hub stores them (deltas only stream, they aren't kept), keeps each thread's summary, and
//! pushes both to the browsers subscribed on `/api/ws`.
pub mod agentd;
pub mod browser;
pub mod db;
pub mod render;
pub mod surf;

use crate::state::App;
use crate::util::{bad, not_found, AppError, R};
use agentum_proto::api::*;
use agentum_proto::machine::{CommandInfo, FsReq, GitReq, ModelInfo, QueuedAction, Req, SurfReq, UsageReply};
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
    /// Writing the limits to the usage plugin's storage, one at a time.
    limits_saving: tokio::sync::Mutex<()>,
    pub out: tokio::sync::broadcast::Sender<Out>,
    models: Mutex<HashMap<(String, AgentKind), (Instant, Vec<ModelInfo>)>>,
    /// The `/` menu per machine, agent and directory (for 30 s).
    commands: Mutex<HashMap<(String, AgentKind, String), (Instant, Vec<CommandInfo>)>>,
    html: Mutex<HashMap<(String, u64), Arc<str>>>,
    /// What each thread's agent was last started with from the plugins (`ThreadSpec.instructions`).
    pub instructions: Mutex<HashMap<String, Option<String>>>,
    /// The Surf viewer's sockets relayed through agentd: sock → (machine, to the page).
    pub socks: Mutex<HashMap<String, (String, tokio::sync::mpsc::Sender<agentum_proto::machine::SockMsg>)>>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub {
            machines: Default::default(),
            threads: Default::default(),
            partial: Default::default(),
            limits: Default::default(),
            limits_saving: Default::default(),
            out: tokio::sync::broadcast::channel(4096).0,
            models: Default::default(),
            commands: Default::default(),
            html: Default::default(),
            socks: Default::default(),
            instructions: Default::default(),
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
    let text = mentions_plain(text);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut t: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        t.push('…');
    }
    t
}

/// How the composer writes an @-mentioned thread: `[@Its title](/code?thread=<id>)`.
const MENTION: &str = "](/code?thread=";

/// The threads `text` mentions, in order, each once.
fn mentioned(text: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    let mut rest = text;
    while let Some(i) = rest.find(MENTION) {
        rest = &rest[i + MENTION.len()..];
        let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        if !id.is_empty() && rest[id.len()..].starts_with(')') && !v.contains(&id) {
            v.push(id);
        }
    }
    v
}

/// `text` with its mentions as `@title` (for titles and previews).
fn mentions_plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("[@") {
        let after = &rest[i + 1..];
        match after.find(MENTION).filter(|j| !after[..*j].contains(['[', ']', '\n'])) {
            Some(j) if after[j + MENTION.len()..].contains(')') => {
                out += &rest[..i];
                out += &after[..j];
                let tail = &after[j + MENTION.len()..];
                rest = &tail[tail.find(')').unwrap() + 1..];
            }
            _ => {
                out += &rest[..i + 2];
                rest = &rest[i + 2..];
            }
        }
    }
    out + rest
}

/// What the agent should know of the threads `text` mentions: each one's recent conversation
/// (up to four threads, the last ~8k characters of each).
async fn mention_context(app: &App, from: &str, text: &str) -> R<Option<String>> {
    let mut out = String::new();
    for id in mentioned(text).into_iter().filter(|id| id != from).take(4) {
        let Some(t) = app.code.summary(&id) else { continue };
        let id2 = id.clone();
        let evs = app.db.call(move |c| db::events(c, &id2, None, 200)).await?.0;
        let mut conv = String::new();
        for e in evs {
            match e.kind {
                EventKind::User { text, .. } => conv += &format!("\n[user] {}\n", mentions_plain(&text)),
                EventKind::Message { text, .. } => conv += &format!("\n[agent] {text}\n"),
                _ => {}
            }
        }
        let n = conv.chars().count();
        let tail: String = if n > 8000 { format!("…{}", conv.chars().skip(n - 8000).collect::<String>()) } else { conv };
        let branch = t.branch.as_deref().map(|b| format!(" branch=\"{b}\"")).unwrap_or_default();
        out += &format!(
            "\n<thread id=\"{id}\" title=\"{}\" agent=\"{}\" machine=\"{}\" cwd=\"{}\"{branch} state=\"{:?}\">{tail}</thread>\n",
            t.title.replace('"', "'"),
            t.agent.label(),
            t.machine,
            t.cwd,
            t.state
        );
    }
    Ok((!out.is_empty()).then(|| format!("The person mentioned other Agentum threads (coding sessions, linked as [@title](/code?thread=<id>)). Their recent conversation, for context:\n{out}")))
}

/// Load summaries; start the scheduler. Called once at boot.
pub async fn start(app: &App) -> anyhow::Result<()> {
    app.db.call(|c| { db::migrate(c)?; db::migrate_more(c) }).await?;
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
    // Each account's last reported limits (the usage plugin keeps them), so the sidebar's meters
    // are there after a restart, before any thread reports again.
    if let Some(saved) = crate::plugins::kv_get(app, "usage", "limits").await.and_then(|v| v.as_object().cloned()) {
        let mut l: Vec<AgentLimits> = saved.into_values().filter_map(|v| serde_json::from_value(v).ok()).collect();
        // Oldest first, each put among the others: what a newer report replaced stays out.
        l.sort_by_key(|a| a.at);
        let mut all = vec![];
        for x in l {
            put_limits(&mut all, x);
        }
        *app.code.limits.write().unwrap() = all;
    }
    let a = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            scheduled(&a).await;
        }
    });
    // Each agent's limits from its provider: once the machines are back, then every 5 minutes.
    let a = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(40)).await;
        loop {
            usage_refresh(&a).await;
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
    Ok(())
}

/// A report of an agent's limits (a thread's RateLimits, or its provider asked directly), kept one
/// per account: the overview's, every page's, and the usage plugin's copy (so a restart keeps it).
/// A thread's report is its machine's account's (as the provider last said); `provider`: the
/// provider's answer, which has every window (a thread's lacks the per-model ones: those stay).
async fn remember_limits(app: &App, l: AgentLimits, provider: bool) {
    let put = merge_limits(&mut app.code.limits.write().unwrap(), l, provider, now());
    app.code.publish("threads", &WsOut::Limits { limits: put });
    // Every account's report, in the usage plugin's storage (the newest written last wins).
    let _g = app.code.limits_saving.lock().await;
    let saved: serde_json::Map<String, Value> = app.code.limits.read().unwrap().iter().map(|x| (format!("{}\t{}", x.agent.id(), x.key()), json!(x))).collect();
    crate::plugins::kv_set(app, "usage", "limits", &Value::Object(saved)).await;
}

/// [`remember_limits`]' bookkeeping: `l` joins its account's report; what was put back.
fn merge_limits(all: &mut Vec<AgentLimits>, mut l: AgentLimits, provider: bool, now: i64) -> AgentLimits {
    if l.account.is_none()
        && let Some(a) = all.iter().find(|x| x.agent == l.agent && x.account.is_some() && x.covers(&l.machine))
    {
        l.account = a.account.clone();
        l.account_label = a.account_label.clone();
    }
    if l.account.is_some() && !l.machines.contains(&l.machine) {
        l.machines.push(l.machine.clone());
    }
    let old = all.iter().find(|x| x.agent == l.agent && x.account == l.account && x.key() == l.key()).cloned();
    let put = match old {
        // A fresher report of the account is kept; it only learns where else it is signed in.
        Some(mut old) if old.at > l.at => {
            for m in l.machines {
                if !old.machines.contains(&m) {
                    old.machines.push(m);
                }
            }
            old
        }
        Some(old) => {
            for m in old.machines {
                if !l.machines.contains(&m) {
                    l.machines.push(m);
                }
            }
            if !provider {
                for w in old.limits {
                    if !l.limits.iter().any(|y| y.window == w.window) && w.resets_at.is_none_or(|r| r > now) {
                        l.limits.push(w);
                    }
                }
                l.plan = l.plan.or(old.plan);
            }
            l.account_label = l.account_label.or(old.account_label);
            l
        }
        None => l,
    };
    put_limits(all, put.clone());
    put
}

/// Which machines to ask for an agent's limits: every one whose account isn't known yet, and one
/// of each known account's (the next in turn, so each machine is asked again in time and a
/// machine signed in elsewhere since is noticed). Providers throttle their usage endpoints:
/// asking every machine of one account each time would only get it refused.
fn usage_askees(all: &[AgentLimits], agent: AgentKind, mut online: Vec<String>) -> Vec<String> {
    online.sort();
    let mut ask = vec![];
    for l in all.iter().filter(|l| l.agent == agent && l.account.is_some()) {
        let mut on: Vec<&String> = online.iter().filter(|m| l.machines.contains(m)).collect();
        on.sort();
        if let Some(i) = (!on.is_empty()).then(|| on.iter().position(|m| **m == l.machine).map(|i| (i + 1) % on.len()).unwrap_or(0)) {
            ask.push(on[i].clone());
        }
    }
    for m in &online {
        if !all.iter().any(|l| l.agent == agent && l.account.is_some() && l.machines.contains(m)) && !ask.contains(m) {
            ask.push(m.clone());
        }
    }
    ask
}

/// Each subscription agent's limits asked of its provider now (agentd's `Req::Usage`), on the
/// online machines that have it ([`usage_askees`]): machines signed in to different accounts (a
/// work organization here, a personal one there) each get theirs; one account's reports are one.
pub async fn usage_refresh(app: &App) -> Vec<AgentLimits> {
    let machines = agentd::machines(app).await;
    let known = app.code.limits.read().unwrap().clone();
    let mut asks = vec![];
    for agent in [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Cursor] {
        let online = machines
            .iter()
            .filter(|m| m.online && m.agents.iter().any(|a| a.agent == agent && a.installed))
            .filter(|m| app.code.conn(&m.id).is_some_and(|c| c.hello.read().unwrap().features.iter().any(|f| f == "usage")))
            .map(|m| m.id.clone())
            .collect();
        for id in usage_askees(&known, agent, online) {
            let Some(c) = app.code.conn(&id) else { continue };
            asks.push(async move { (agent, id, c.call(Req::Usage { agent }, Duration::from_secs(45)).await) });
        }
    }
    let mut got = vec![];
    for (agent, machine, r) in futures::future::join_all(asks).await {
        let v = match r {
            Ok(v) => v,
            Err(e) => {
                tracing::info!("{} usage on {machine}: {e}", agent.label());
                continue;
            }
        };
        let u: UsageReply = serde_json::from_value(v).unwrap_or_default();
        if u.limits.is_empty() {
            continue;
        }
        let (account, account_label) = u.account.map(|a| (Some(a.id), Some(a.label))).unwrap_or_default();
        let l = AgentLimits { agent, machine, limits: u.limits, at: now(), plan: u.plan, account, account_label, machines: vec![] };
        remember_limits(app, l.clone(), true).await;
        got.push(l);
    }
    got
}

/// POST /api/code/usage/refresh: ask now; every known report back.
async fn usage_refresh_h(State(app): State<App>) -> Json<Vec<AgentLimits>> {
    usage_refresh(&app).await;
    Json(app.code.limits.read().unwrap().clone())
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
            EventKind::RateLimits { limits, plan } => {
                let l = AgentLimits { agent: t.agent, machine: t.machine.clone(), limits: limits.clone(), at: ts, plan: plan.clone(), account: None, account_label: None, machines: vec![] };
                remember_limits(app, l, false).await;
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
    // A snoozed thread wakes, and a settled one comes back, when it works or asks.
    if t.state.busy() || t.asking.is_some() {
        t.snoozed_until = None;
        if t.archived && t.asking.is_some() {
            t.archived = false;
            t.archived_at = None;
        }
    }
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
        .route("/api/code/commands", get(commands))
        .route("/api/code/threads/{id}/queued/{seq}", post(queued))
        .route("/api/code/search", get(search))
        .route("/api/code/machines", get(machines_h).post(add_machine))
        .route("/api/code/machines/{id}", delete(remove_machine))
        .route("/api/code/machines/{id}/projects", post(add_project))
        .route("/api/code/machines/{id}/main", post(set_main))
        .route("/api/code/call", post(call))
        .merge(surf::routes())
        .route("/api/code/usage/refresh", post(usage_refresh_h))
        .route("/api/code/syntax.css", get(|| async { ([("content-type", "text/css"), ("cache-control", "public, max-age=86400")], render::syntax_css()) }))
        // Messages carry their images (data: URLs, up to six of ≤2.5 MB).
        .layer(axum::extract::DefaultBodyLimit::max(24 * 1024 * 1024))
}

fn offline(m: &str) -> AppError {
    AppError(StatusCode::CONFLICT, format!("{m} is offline"))
}

/// Call agentd on a machine (for the hub, plugins and the generic /api/code/call).
pub async fn machine_call(app: &App, machine: &str, req: Req) -> Result<Value, String> {
    let c = app.code.conn(machine).ok_or_else(|| format!("{machine} is offline"))?;
    // An agentd that doesn't know a request can't answer it, and would log the frame: a secret's value.
    if matches!(req, Req::Secrets(_)) && !c.hello.read().unwrap().features.iter().any(|f| f == "secrets") {
        return Err(format!("agentd on {machine} is too old for secrets: update it"));
    }
    let needs = match &req {
        Req::Commands { .. } => Some(("commands", "slash commands")),
        Req::Queued { .. } => Some(("queued", "steering a queued message")),
        _ => None,
    };
    if let Some((f, what)) = needs
        && !c.hello.read().unwrap().features.iter().any(|x| x == f)
    {
        return Err(format!("agentd on {machine} is too old for {what}: update it"));
    }
    let wait = match &req {
        Req::Git(GitReq::Push { .. }) | Req::Git(GitReq::PrMerge { .. }) | Req::Git(GitReq::WorktreeAdd { .. }) => Duration::from_secs(120),
        Req::Start { .. } | Req::Models { .. } => Duration::from_secs(60),
        // The first start on a machine downloads the browser's image.
        Req::Surf(SurfReq::Ensure { .. }) => Duration::from_secs(600),
        _ => Duration::from_secs(30),
    };
    c.call(req, wait).await
}

async fn overview(State(app): State<App>) -> Json<Overview> {
    let machines = agentd::machines(&app).await;
    // Open threads, and the last day's settled ones (the sidebar's Settled shelf).
    let day_ago = now() - 86_400_000;
    let mut threads: Vec<ThreadSummary> = app.code.threads.read().unwrap().values().filter(|t| !t.archived || t.archived_at.is_some_and(|a| a > day_ago)).cloned().collect();
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
    let existing = b.cwd.as_ref().map(|c| c.trim().trim_end_matches('/')).filter(|c| !c.is_empty() && parent.is_none());
    if let Some(br) = b.worktree.as_ref().filter(|s| !s.trim().is_empty() && parent.is_none()) {
        let base = b.base.clone().filter(|s| !s.trim().is_empty());
        let r = machine_call(app, &machine, Req::Git(GitReq::WorktreeAdd { repo: project.clone(), branch: br.trim().to_string(), base })).await.map_err(bad)?;
        cwd = r["path"].as_str().unwrap_or(&cwd).to_string();
        branch = r["branch"].as_str().map(str::to_string).or(Some(br.clone()));
    } else if let Some(wt) = existing {
        // One of this project's worktrees (agentd lists them; anything else is refused).
        let sp = machine_call(app, &machine, Req::Git(GitReq::StartPoints { repo: project.clone() })).await.map_err(bad)?;
        let w = sp["worktrees"].as_array().into_iter().flatten().find(|w| w["path"].as_str().map(|p| p.trim_end_matches('/')) == Some(wt)).ok_or_else(|| bad(format!("{wt} is not a worktree of {project}")))?;
        cwd = wt.to_string();
        branch = w["branch"].as_str().map(str::to_string);
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
        snoozed_until: None,
        archived_at: None,
    };
    let mut s = s;
    s.attention = db::attention_of(&s);
    // A plugin may refuse the turn (a concurrency limit): then the thread isn't made at all.
    if !b.text.trim().is_empty() {
        crate::plugins::before_turn(app, &s, &b.text).await.map_err(|e| AppError(StatusCode::CONFLICT, e))?;
    }
    let s2 = s.clone();
    app.db.call(move |c| db::save_thread(c, &s2)).await?;
    app.code.set_summary(s.clone());
    if !b.text.trim().is_empty() {
        let context = mention_context(app, &s.id, &b.text).await?;
        start_agent(app, &s, Input { text: b.text, images: b.images, context }, None).await?;
    }
    Ok(app.code.summary(&s.id).unwrap_or(s))
}

/// Start (or resume) the agent of a thread with this first input.
async fn start_agent(app: &App, s: &ThreadSummary, mut input: Input, resume: Option<String>) -> R<()> {
    if let Err(why) = crate::plugins::before_turn(app, s, &input.text).await {
        // Refused: a new thread mustn't stay "Starting".
        if let Some(mut t) = app.code.summary(&s.id).filter(|t| t.state == ThreadState::Starting) {
            t.state = ThreadState::Idle;
            t.attention = db::attention_of(&t);
            let t2 = t.clone();
            let _ = app.db.call(move |c| db::save_thread(c, &t2)).await;
            app.code.set_summary(t);
        }
        return Err(AppError(StatusCode::CONFLICT, why));
    }
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
    let instructions = crate::plugins::instructions(app, s).await;
    let spec = ThreadSpec { agent: Some(s.agent), model: s.model.clone(), effort: s.effort.clone(), permission: s.permission, cwd: s.cwd.clone(), instructions: instructions.clone() };
    let req = Req::Start { thread: s.id.clone(), spec, resume, input: Some(input), last_seq: s.last_seq };
    match machine_call(app, &s.machine, req).await {
        Ok(_) => {
            app.code.instructions.lock().unwrap().insert(s.id.clone(), instructions);
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
        t.archived_at = v.then(now);
        if v {
            if let Some(c) = app.code.conn(&t.machine) {
                c.fire(Req::Sleep { thread: id.clone() });
            }
        }
    }
    if let Some(v) = p.read {
        t.unread = !v;
    }
    if let Some(v) = p.snoozed_until {
        t.snoozed_until = v;
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
    let context = mention_context(app, id, &b.text).await?;
    let input = Input { text: b.text.clone(), images: b.images.clone(), context };
    let id2 = id.to_string();
    if !app.db.call(move |c| db::started(c, &id2)).await? {
        return start_agent(app, &t, input, None).await;
    }
    if !t.state.busy() {
        crate::plugins::before_turn(app, &t, &input.text).await.map_err(|e| AppError(StatusCode::CONFLICT, e))?;
        // The plugins have something new to tell the agent (its project's browser was started
        // since): start it again with that (agentd restarts an idle agent, resuming its session).
        let now = crate::plugins::instructions(app, &t).await;
        let last = app.code.instructions.lock().unwrap().get(id).cloned();
        if last.as_ref() != Some(&now) && (last.is_some() || now.is_some()) {
            let id3 = id.to_string();
            if let Some(session) = app.db.call(move |c| db::session_of(c, &id3)).await? {
                return start_agent(app, &t, input, Some(session)).await;
            }
        }
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
struct CommandsQ {
    #[serde(default)]
    thread: Option<String>,
    #[serde(default)]
    machine: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}
/// The composer's `/` menu: a thread's agent's commands and skills in its directory (or a new
/// thread's: machine, agent, cwd).
async fn commands(State(app): State<App>, Query(q): Query<CommandsQ>) -> R<Json<Vec<CommandInfo>>> {
    let (machine, agent, cwd) = match q.thread.filter(|t| !t.is_empty()) {
        Some(id) => {
            let t = app.code.summary(&id).ok_or_else(not_found)?;
            (t.machine, t.agent, t.cwd)
        }
        None => (
            q.machine.unwrap_or_default(),
            q.agent.as_deref().and_then(AgentKind::parse).ok_or_else(|| bad("unknown agent"))?,
            q.cwd.unwrap_or_default(),
        ),
    };
    let key = (machine.clone(), agent, cwd.clone());
    if let Some((at, v)) = app.code.commands.lock().unwrap().get(&key)
        && at.elapsed() < Duration::from_secs(30)
    {
        return Ok(Json(v.clone()));
    }
    let v = machine_call(&app, &machine, Req::Commands { agent, cwd }).await.map_err(|e| AppError(StatusCode::BAD_GATEWAY, e))?;
    let list: Vec<CommandInfo> = serde_json::from_value(v).unwrap_or_default();
    app.code.commands.lock().unwrap().insert(key, (Instant::now(), list.clone()));
    Ok(Json(list))
}

#[derive(serde::Deserialize)]
struct QueuedBody {
    action: QueuedAction,
}
/// Steer a queued message into the running turn, or drop it.
async fn queued(State(app): State<App>, Path((id, seq)): Path<(String, u64)>, Json(b): Json<QueuedBody>) -> R<Json<Value>> {
    let t = app.code.summary(&id).ok_or_else(not_found)?;
    machine_call(&app, &t.machine, Req::Queued { thread: id, seq, action: b.action }).await.map_err(|e| AppError(StatusCode::CONFLICT, e))?;
    Ok(Json(json!({"ok": true})))
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

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_proto::RateLimit;

    #[test]
    fn reads_mentions() {
        let t = "Look at [@Fix login](/code?thread=ab-12) and [@x](/code?thread=cd) and [@Fix login](/code?thread=ab-12), not [@y](/code?thread=bad id).";
        assert_eq!(mentioned(t), ["ab-12", "cd"]);
        assert_eq!(mentions_plain(t), "Look at @Fix login and @x and @Fix login, not @y.");
        assert_eq!(mentions_plain("mail me [@ home"), "mail me [@ home");
        assert_eq!(title_of("[@Fix login](/code?thread=ab) why?"), "@Fix login why?");
    }

    fn report(machine: &str, account: Option<&str>, windows: &[(&str, f64)], at: i64) -> AgentLimits {
        AgentLimits {
            agent: AgentKind::ClaudeCode,
            machine: machine.into(),
            limits: windows.iter().map(|(w, p)| RateLimit { window: w.to_string(), used_percent: *p, resets_at: None }).collect(),
            at,
            plan: None,
            account: account.map(str::to_string),
            account_label: account.map(|a| format!("{a} org")),
            machines: vec![],
        }
    }

    #[test]
    fn reports_join_their_account() {
        let mut all = vec![];
        // The provider, asked on three machines: two accounts.
        merge_limits(&mut all, report("jupiter", Some("me"), &[("5h", 10.0), ("Opus", 5.0)], 10), true, 0);
        merge_limits(&mut all, report("mars", Some("me"), &[("5h", 11.0), ("Opus", 5.0)], 11), true, 0);
        merge_limits(&mut all, report("workbox", Some("work"), &[("5h", 70.0)], 12), true, 0);
        assert_eq!(all.len(), 2);
        let me = all.iter().find(|x| x.key() == "me").unwrap();
        assert_eq!((me.machine.as_str(), me.machines.len(), me.limits[0].used_percent), ("mars", 2, 11.0));
        // A thread on jupiter reports (no account, no model windows): it is the personal account's,
        // and Opus stays.
        let put = merge_limits(&mut all, report("jupiter", None, &[("5h", 20.0)], 13), false, 0);
        assert_eq!((put.account.as_deref(), put.account_label.as_deref()), (Some("me"), Some("me org")));
        assert_eq!(put.limits.iter().map(|w| w.window.as_str()).collect::<Vec<_>>(), ["5h", "Opus"]);
        assert_eq!(all.len(), 2);
        // An older report of the account doesn't replace a newer one.
        let put = merge_limits(&mut all, report("mars", Some("me"), &[("5h", 1.0)], 5), true, 0);
        assert_eq!(put.limits[0].used_percent, 20.0);
        // A machine whose account isn't known stands alone.
        merge_limits(&mut all, report("saturn", None, &[("5h", 3.0)], 14), false, 0);
        assert_eq!(all.len(), 3);
        assert_eq!(limits_for(&all, AgentKind::ClaudeCode, "workbox").map(|x| x.limits[0].used_percent), Some(70.0));
        // Who is asked next: one machine per account (jupiter reported last: mars is next), and
        // saturn, whose account isn't known; Codex, never reported: every machine.
        let online = || vec!["jupiter".to_string(), "mars".into(), "saturn".into(), "workbox".into()];
        let mut ask = usage_askees(&all, AgentKind::ClaudeCode, online());
        ask.sort();
        assert_eq!(ask, ["mars", "saturn", "workbox"]);
        assert_eq!(usage_askees(&all, AgentKind::Codex, online()).len(), 4);
        assert_eq!(usage_askees(&all, AgentKind::ClaudeCode, vec!["jupiter".into()]), ["jupiter"]);
    }
}
