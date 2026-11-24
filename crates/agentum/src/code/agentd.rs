//! agentd's connection: `GET /agentd` (WebSocket), `Authorization: Bearer <machine token>`.
//! One per machine; the hub sends requests and acks, agentd sends events, replies and output.
use crate::state::App;
use agentum_proto::api::{Machine, WsOut};
use agentum_proto::machine::{FromAgentd, Hello, Reply, Req, ToAgentd};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub struct Conn {
    pub id: String,
    tx: mpsc::UnboundedSender<String>,
    pub hello: RwLock<Hello>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    next: AtomicU64,
}

impl Conn {
    fn send(&self, m: &ToAgentd) {
        let _ = self.tx.send(serde_json::to_string(m).unwrap_or_default());
    }
    /// A request; its reply, or an error after `wait`.
    pub async fn call(&self, req: Req, wait: Duration) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&ToAgentd::Req { id, req });
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(Reply::Ok(v))) => Ok(v),
            Ok(Ok(Reply::Err(e))) => Err(e),
            Ok(Err(_)) => Err(format!("{} disconnected", self.id)),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("{} didn't answer in {}s", self.id, wait.as_secs()))
            }
        }
    }
    /// A request whose answer nobody waits for (keystrokes).
    pub fn fire(&self, req: Req) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.send(&ToAgentd::Req { id, req });
    }
    pub fn ack(&self, thread: &str, seq: u64) {
        self.send(&ToAgentd::Ack { thread: thread.to_string(), seq });
    }
}

fn token_hash(t: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(t.as_bytes()))
}

/// A new machine and its token (shown once).
pub async fn create_machine(app: &App, name: &str) -> anyhow::Result<(String, String)> {
    let id: String = name.trim().to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    anyhow::ensure!(!id.is_empty() && id.len() <= 32, "a machine's name is letters, digits and dashes");
    let token = {
        use rand::RngCore;
        let mut b = [0u8; 32];
        rand::rng().fill_bytes(&mut b);
        format!("agm_{}", hex::encode(b))
    };
    let (id2, name2, h) = (id.clone(), name.trim().to_string(), token_hash(&token));
    let now = chrono::Utc::now().timestamp_millis();
    app.db
        .call(move |c| {
            let main = c.query_row("SELECT count(*) FROM code_machines", [], |r| r.get::<_, i64>(0))? == 0;
            c.execute(
                "INSERT INTO code_machines(id, name, token_hash, main, created_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET token_hash=excluded.token_hash",
                rusqlite::params![id2, name2, h, main as i64, now],
            )
        })
        .await?;
    Ok((id, token))
}

/// Every machine: the hub's list, with the live ones' latest hello.
pub async fn machines(app: &App) -> Vec<Machine> {
    let rows: Vec<(String, String, bool, Option<i64>, Option<String>)> = app
        .db
        .call(|c| {
            let mut st = c.prepare("SELECT id, name, main, last_seen, hello FROM code_machines ORDER BY main DESC, name")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?, r.get(4)?)))?.filter_map(Result::ok).collect();
            Ok(v)
        })
        .await
        .unwrap_or_default();
    let live = app.code.machines.read().unwrap().clone();
    rows.into_iter()
        .map(|(id, name, main, last_seen, hello)| {
            let conn = live.get(&id);
            let h: Hello = conn.map(|c| c.hello.read().unwrap().clone()).or_else(|| hello.and_then(|s| serde_json::from_str(&s).ok())).unwrap_or_default();
            machine_view(id, name, main, conn.is_some(), last_seen, h)
        })
        .collect()
}

fn machine_view(id: String, name: String, main: bool, online: bool, last_seen: Option<i64>, h: Hello) -> Machine {
    Machine { id, name, online, last_seen, hostname: h.hostname, os: h.os, arch: h.arch, version: h.version, home: h.home, agents: h.agents, projects: h.projects, stats: h.stats, main }
}

async fn publish_machine(app: &App, id: &str) {
    if let Some(m) = machines(app).await.into_iter().find(|m| m.id == id) {
        app.code.publish("machines", &WsOut::Machine { machine: m });
    }
}

pub async fn upgrade(State(app): State<App>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let token = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("").trim().to_string();
    let h = token_hash(&token);
    let id: Option<String> = app.db.call(move |c| { use rusqlite::OptionalExtension; c.query_row("SELECT id FROM code_machines WHERE token_hash=?1", [h], |r| r.get(0)).optional() }).await.ok().flatten();
    let Some(id) = id.filter(|_| token.len() > 20) else { return (StatusCode::UNAUTHORIZED, "unknown machine token").into_response() };
    ws.max_message_size(64 << 20).on_upgrade(move |s| run(app, id, s))
}

async fn run(app: App, id: String, sock: WebSocket) {
    let (mut sink, mut stream) = sock.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let conn = Arc::new(Conn { id: id.clone(), tx, hello: RwLock::new(Hello::default()), pending: Mutex::new(HashMap::new()), next: AtomicU64::new(1) });
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if sink.send(Message::Text(m.into())).await.is_err() {
                break;
            }
        }
    });
    let mut registered = false;
    let pinger = {
        let c = conn.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(20)).await;
                c.send(&ToAgentd::Ping { ts: chrono::Utc::now().timestamp_millis() });
            }
        })
    };
    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(m) = serde_json::from_str::<FromAgentd>(&text) else {
            tracing::warn!("agentd {id}: unreadable frame");
            continue;
        };
        match m {
            FromAgentd::Hello(h) | FromAgentd::Update(h) => {
                if h.protocol != agentum_proto::PROTOCOL {
                    tracing::warn!("agentd {id} speaks protocol {} (hub: {})", h.protocol, agentum_proto::PROTOCOL);
                }
                let live = h.threads.clone();
                *conn.hello.write().unwrap() = h.clone();
                let (id2, js, now) = (id.clone(), serde_json::to_string(&h).unwrap_or_default(), chrono::Utc::now().timestamp_millis());
                let _ = app.db.call(move |c| c.execute("UPDATE code_machines SET hello=?2, last_seen=?3 WHERE id=?1", rusqlite::params![id2, js, now])).await;
                if !registered {
                    registered = true;
                    if let Some(old) = app.code.machines.write().unwrap().insert(id.clone(), conn.clone()) {
                        drop(old);
                    }
                    tracing::info!("agentd {id} connected ({} {}, {} projects)", h.os, h.arch, h.projects.len());
                    // Threads agentd knows but the hub has fewer events of: ask for nothing, agentd resends unacked.
                    let _ = live;
                    super::machine_online(&app, &id).await;
                }
                publish_machine(&app, &id).await;
            }
            FromAgentd::Events { thread, events } => super::ingest(&app, &conn, &thread, events).await,
            FromAgentd::Reply { id: rid, result } => {
                if let Some(tx) = conn.pending.lock().unwrap().remove(&rid) {
                    let _ = tx.send(result);
                }
            }
            FromAgentd::TermData { term, data } => app.code.publish(&format!("term:{id}:{term}"), &WsOut::TermData { machine: id.clone(), term, data }),
            FromAgentd::TermExit { term, code } => app.code.publish(&format!("term:{id}:{term}"), &WsOut::TermExit { machine: id.clone(), term, code }),
            FromAgentd::Pong { .. } => {}
        }
    }
    pinger.abort();
    writer.abort();
    // Only forget it if a newer connection hasn't replaced it.
    {
        let mut m = app.code.machines.write().unwrap();
        if m.get(&id).is_some_and(|c| Arc::ptr_eq(c, &conn)) {
            m.remove(&id);
        }
    }
    for (_, tx) in conn.pending.lock().unwrap().drain() {
        let _ = tx.send(Reply::Err(format!("{id} disconnected")));
    }
    let (id2, now) = (id.clone(), chrono::Utc::now().timestamp_millis());
    let _ = app.db.call(move |c| c.execute("UPDATE code_machines SET last_seen=?2 WHERE id=?1", rusqlite::params![id2, now])).await;
    tracing::info!("agentd {id} disconnected");
    publish_machine(&app, &id).await;
}
