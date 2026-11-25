//! SQLite (`agentd.db`), owned by one dedicated thread so the runtime never blocks on it.
//!
//! Jobs are closures run on that thread; whatever is queued together runs in one transaction.
use agentum_proto::{AgentKind, Event, ThreadSpec, ThreadState};
use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::mpsc;

type Job = Box<dyn FnOnce(&Connection) + Send>;
/// Run after the transaction holding the job commits.
type After = Box<dyn FnOnce() + Send>;

enum Msg {
    Job(Job),
    After(Job, After),
}

#[derive(Clone)]
pub struct Db {
    tx: mpsc::Sender<Msg>,
}

/// A thread as agentd remembers it.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadRow {
    pub id: String,
    pub agent: AgentKind,
    pub spec: ThreadSpec,
    pub session_id: Option<String>,
    pub state: ThreadState,
    pub last_seq: u64,
    pub acked: u64,
    pub turn: u32,
}

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
CREATE TABLE IF NOT EXISTS threads (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  spec TEXT NOT NULL,
  session_id TEXT,
  state TEXT NOT NULL,
  last_seq INTEGER NOT NULL DEFAULT 0,
  acked INTEGER NOT NULL DEFAULT 0,
  turn INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
  thread TEXT NOT NULL,
  seq INTEGER NOT NULL,
  json TEXT NOT NULL,
  PRIMARY KEY (thread, seq)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
";

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}
fn enum_from<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.execute_batch(SCHEMA)?;
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::Builder::new().name("agentd-db".into()).spawn(move || run(conn, rx))?;
        Ok(Db { tx })
    }

    /// Run `f` on the database thread; errors are logged.
    pub fn exec(&self, what: &'static str, f: impl FnOnce(&Connection) -> rusqlite::Result<()> + Send + 'static) {
        let _ = self.tx.send(Msg::Job(Box::new(move |c| {
            if let Err(e) = f(c) {
                tracing::warn!("db {what}: {e}");
            }
        })));
    }

    /// Run `f` on the database thread and wait for its result.
    pub async fn call<T: Send + 'static>(&self, f: impl FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Msg::Job(Box::new(move |c| {
                let _ = tx.send(f(c));
            })))
            .map_err(|_| anyhow::anyhow!("database thread is gone"))?;
        rx.await.map_err(|_| anyhow::anyhow!("database thread is gone"))?
    }

    pub fn save_thread(&self, row: ThreadRow) {
        self.exec("save thread", move |c| {
            c.execute(
                "INSERT INTO threads (id, agent, spec, session_id, state, last_seq, acked, turn, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET agent=?2, spec=?3, session_id=?4, state=?5,
                   last_seq=MAX(last_seq, ?6), acked=MAX(acked, ?7), turn=?8, updated_at=?9",
                params![
                    row.id,
                    row.agent.id(),
                    serde_json::to_string(&row.spec).unwrap_or_default(),
                    row.session_id,
                    enum_str(&row.state),
                    row.last_seq as i64,
                    row.acked as i64,
                    row.turn,
                    crate::util::now_ms()
                ],
            )?;
            Ok(())
        });
    }

    /// Store an event; `after` runs once it is committed.
    pub fn insert_event(&self, thread: &str, ev: &Event, after: impl FnOnce() + Send + 'static) {
        let thread = thread.to_string();
        let seq = ev.seq;
        let json = serde_json::to_string(ev).unwrap_or_default();
        let job: Job = Box::new(move |c| {
            let r = c
                .execute("INSERT OR REPLACE INTO events (thread, seq, json) VALUES (?1, ?2, ?3)", params![thread, seq as i64, json])
                .and_then(|_| c.execute("UPDATE threads SET last_seq = MAX(last_seq, ?2) WHERE id = ?1", params![thread, seq as i64]));
            if let Err(e) = r {
                tracing::warn!("db insert event: {e}");
            }
        });
        if let Err(mpsc::SendError(Msg::After(_, after))) = self.tx.send(Msg::After(job, Box::new(after))) {
            // No database thread: don't hold the event back forever.
            after();
        }
    }

    pub fn ack(&self, thread: &str, seq: u64) {
        let thread = thread.to_string();
        self.exec("ack", move |c| {
            c.execute("DELETE FROM events WHERE thread = ?1 AND seq <= ?2", params![thread, seq as i64])?;
            c.execute("UPDATE threads SET acked = MAX(acked, ?2) WHERE id = ?1", params![thread, seq as i64])?;
            Ok(())
        });
    }

    pub async fn load_threads(&self) -> anyhow::Result<Vec<(ThreadRow, Vec<Event>)>> {
        self.call(|c| {
            let mut st = c.prepare("SELECT id, agent, spec, session_id, state, last_seq, acked, turn FROM threads")?;
            let rows: Vec<ThreadRow> = st
                .query_map([], |r| {
                    let agent: String = r.get(1)?;
                    let spec: String = r.get(2)?;
                    let state: String = r.get(4)?;
                    Ok(ThreadRow {
                        id: r.get(0)?,
                        agent: AgentKind::parse(&agent).unwrap_or(AgentKind::ClaudeCode),
                        spec: serde_json::from_str(&spec).unwrap_or_default(),
                        session_id: r.get(3)?,
                        state: enum_from(&state).unwrap_or_default(),
                        last_seq: r.get::<_, i64>(5)? as u64,
                        acked: r.get::<_, i64>(6)? as u64,
                        turn: r.get(7)?,
                    })
                })?
                .collect::<Result<_, _>>()?;
            let mut out = Vec::with_capacity(rows.len());
            let mut ev = c.prepare("SELECT json FROM events WHERE thread = ?1 AND seq > ?2 ORDER BY seq")?;
            for row in rows {
                let events: Vec<Event> = ev
                    .query_map(params![row.id, row.acked as i64], |r| r.get::<_, String>(0))?
                    .filter_map(|j| j.ok().and_then(|j| serde_json::from_str(&j).ok()))
                    .collect();
                out.push((row, events));
            }
            Ok(out)
        })
        .await
    }

    pub async fn kv_get(&self, k: &'static str) -> Option<String> {
        self.call(move |c| Ok(c.query_row("SELECT v FROM kv WHERE k = ?1", [k], |r| r.get::<_, String>(0)).optional()?))
            .await
            .ok()
            .flatten()
    }

    pub fn kv_set(&self, k: &'static str, v: String) {
        self.exec("kv", move |c| {
            c.execute("INSERT INTO kv (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = ?2", params![k, v])?;
            Ok(())
        });
    }

    /// Wait until everything queued so far is committed (and its after-commit work done).
    pub async fn flush(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        if self.tx.send(Msg::After(Box::new(|_| {}), Box::new(move || { let _ = tx.send(()); }))).is_ok() {
            let _ = rx.await;
        }
    }
}

fn run(conn: Connection, rx: mpsc::Receiver<Msg>) {
    let mut afters: Vec<After> = Vec::new();
    while let Ok(first) = rx.recv() {
        let _ = conn.execute_batch("BEGIN");
        let mut next = Some(first);
        let mut n = 0;
        while let Some(m) = next.take() {
            match m {
                Msg::Job(job) => job(&conn),
                Msg::After(job, after) => {
                    job(&conn);
                    afters.push(after);
                }
            }
            n += 1;
            if n < 512 {
                next = rx.try_recv().ok();
            }
        }
        if let Err(e) = conn.execute_batch("COMMIT") {
            tracing::warn!("db commit: {e}");
        }
        for a in afters.drain(..) {
            a();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_proto::EventKind;

    #[tokio::test]
    async fn threads_and_events_survive() {
        let dir = std::env::temp_dir().join(format!("agentd-db-{}", crate::util::random_hex(4)));
        let path = dir.join("agentd.db");
        {
            let db = Db::open(&path).unwrap();
            db.save_thread(ThreadRow {
                id: "t_a".into(),
                agent: AgentKind::Codex,
                spec: ThreadSpec { cwd: "/x".into(), ..Default::default() },
                session_id: Some("s1".into()),
                state: ThreadState::Running,
                last_seq: 0,
                acked: 0,
                turn: 1,
            });
            for seq in 1..=3 {
                db.insert_event("t_a", &Event { seq, ts: 1, kind: EventKind::Notice { text: format!("n{seq}") } }, || {});
            }
            db.ack("t_a", 1);
            db.flush().await;
        }
        let db = Db::open(&path).unwrap();
        let rows = db.load_threads().await.unwrap();
        assert_eq!(rows.len(), 1);
        let (row, events) = &rows[0];
        assert_eq!(row.state, ThreadState::Running);
        assert_eq!(row.session_id.as_deref(), Some("s1"));
        assert_eq!((row.last_seq, row.acked), (3, 1));
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
