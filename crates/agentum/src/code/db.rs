//! The Code section's tables (in the hub's one SQLite file).
use agentum_proto::api::ThreadSummary;
use agentum_proto::{AgentKind, Attention, Event, Permission, ThreadState, Usage};
use rusqlite::{params, Connection, OptionalExtension, Row};

pub fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS code_machines(
            id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
            main INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, last_seen INTEGER, hello TEXT);
         CREATE TABLE IF NOT EXISTS code_threads(
            id TEXT PRIMARY KEY, machine TEXT NOT NULL, agent TEXT NOT NULL, model TEXT, effort TEXT,
            permission TEXT NOT NULL DEFAULT 'ask', project TEXT NOT NULL, cwd TEXT NOT NULL, branch TEXT,
            worktree INTEGER NOT NULL DEFAULT 0, title TEXT NOT NULL DEFAULT '', state TEXT NOT NULL DEFAULT 'idle',
            unread INTEGER NOT NULL DEFAULT 0, pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
            ord REAL NOT NULL DEFAULT 0, section TEXT, parent TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            preview TEXT NOT NULL DEFAULT '', asking TEXT, queued INTEGER NOT NULL DEFAULT 0, usage TEXT,
            cost REAL NOT NULL DEFAULT 0, last_seq INTEGER NOT NULL DEFAULT 0, session_id TEXT, started INTEGER NOT NULL DEFAULT 0);
         CREATE INDEX IF NOT EXISTS code_threads_updated ON code_threads(archived, updated_at DESC);
         CREATE TABLE IF NOT EXISTS code_events(
            thread TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL, kind TEXT NOT NULL, data TEXT NOT NULL,
            PRIMARY KEY(thread, seq)) WITHOUT ROWID;
         CREATE VIRTUAL TABLE IF NOT EXISTS code_fts USING fts5(text, thread UNINDEXED, seq UNINDEXED, tokenize='unicode61 remove_diacritics 2');
         CREATE TABLE IF NOT EXISTS code_scheduled(
            id INTEGER PRIMARY KEY, thread TEXT NOT NULL, body TEXT NOT NULL, send_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS plugin_kv(
            plugin TEXT NOT NULL, k TEXT NOT NULL, v TEXT NOT NULL, PRIMARY KEY(plugin, k)) WITHOUT ROWID;",
    )
}

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|j| j.as_str().map(str::to_string)).unwrap_or_default()
}
fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

pub const THREAD_COLS: &str = "id, machine, agent, model, effort, permission, project, cwd, branch, title, state, unread, pinned, archived, ord, section, parent, created_at, updated_at, preview, asking, queued, usage, cost, last_seq";

pub fn thread_of(r: &Row) -> rusqlite::Result<ThreadSummary> {
    let state: ThreadState = parse_enum(&r.get::<_, String>(10)?).unwrap_or_default();
    let unread: bool = r.get::<_, i64>(11)? != 0;
    let asking: Option<String> = r.get(20)?;
    let usage: Option<Usage> = r.get::<_, Option<String>>(22)?.and_then(|s| serde_json::from_str(&s).ok());
    Ok(ThreadSummary {
        id: r.get(0)?,
        machine: r.get(1)?,
        agent: AgentKind::parse(&r.get::<_, String>(2)?).unwrap_or(AgentKind::ClaudeCode),
        model: r.get(3)?,
        effort: r.get(4)?,
        permission: parse_enum::<Permission>(&r.get::<_, String>(5)?).unwrap_or_default(),
        project: r.get(6)?,
        cwd: r.get(7)?,
        branch: r.get(8)?,
        title: r.get(9)?,
        state,
        attention: agentum_proto::api::attention(state, unread, asking.is_some()),
        unread,
        pinned: r.get::<_, i64>(12)? != 0,
        archived: r.get::<_, i64>(13)? != 0,
        order: r.get(14)?,
        section: r.get(15)?,
        parent: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
        preview: r.get(19)?,
        asking,
        queued: r.get::<_, i64>(21)? as u32,
        usage,
        cost_usd: r.get(23)?,
        last_seq: r.get::<_, i64>(24)? as u64,
    })
}

pub fn all_threads(c: &Connection) -> rusqlite::Result<Vec<ThreadSummary>> {
    let mut st = c.prepare(&format!("SELECT {THREAD_COLS} FROM code_threads ORDER BY updated_at DESC"))?;
    let v = st.query_map([], thread_of)?.collect();
    v
}

pub fn get_thread(c: &Connection, id: &str) -> rusqlite::Result<Option<ThreadSummary>> {
    c.query_row(&format!("SELECT {THREAD_COLS} FROM code_threads WHERE id=?1"), [id], thread_of).optional()
}

/// Insert or update a thread's summary columns.
pub fn save_thread(c: &Connection, t: &ThreadSummary) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO code_threads(id, machine, agent, model, effort, permission, project, cwd, branch, title, state, unread, pinned, archived, ord, section, parent, created_at, updated_at, preview, asking, queued, usage, cost, last_seq)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)
         ON CONFLICT(id) DO UPDATE SET machine=excluded.machine, agent=excluded.agent, model=excluded.model, effort=excluded.effort, permission=excluded.permission,
           project=excluded.project, cwd=excluded.cwd, branch=excluded.branch, title=excluded.title, state=excluded.state, unread=excluded.unread, pinned=excluded.pinned,
           archived=excluded.archived, ord=excluded.ord, section=excluded.section, parent=excluded.parent, updated_at=excluded.updated_at, preview=excluded.preview,
           asking=excluded.asking, queued=excluded.queued, usage=excluded.usage, cost=excluded.cost, last_seq=excluded.last_seq",
        params![
            t.id, t.machine, t.agent.id(), t.model, t.effort, enum_str(&t.permission), t.project, t.cwd, t.branch, t.title, enum_str(&t.state),
            t.unread as i64, t.pinned as i64, t.archived as i64, t.order, t.section, t.parent, t.created_at, t.updated_at, t.preview, t.asking,
            t.queued as i64, t.usage.as_ref().map(|u| serde_json::to_string(u).unwrap_or_default()), t.cost_usd, t.last_seq as i64
        ],
    )?;
    Ok(())
}

pub fn session_of(c: &Connection, id: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT session_id FROM code_threads WHERE id=?1", [id], |r| r.get::<_, Option<String>>(0)).optional().map(Option::flatten)
}
pub fn set_session(c: &Connection, id: &str, session: &str) -> rusqlite::Result<()> {
    c.execute("UPDATE code_threads SET session_id=?2 WHERE id=?1", params![id, session]).map(|_| ())
}
pub fn set_started(c: &Connection, id: &str) -> rusqlite::Result<()> {
    c.execute("UPDATE code_threads SET started=1 WHERE id=?1", [id]).map(|_| ())
}
pub fn started(c: &Connection, id: &str) -> rusqlite::Result<bool> {
    c.query_row("SELECT started FROM code_threads WHERE id=?1", [id], |r| r.get::<_, i64>(0)).optional().map(|o| o.unwrap_or(0) != 0)
}

pub fn kind_name(e: &Event) -> String {
    serde_json::to_value(&e.kind).ok().and_then(|v| v["k"].as_str().map(str::to_string)).unwrap_or_default()
}

/// Store events (skipping ones already stored); returns the ones that were new.
pub fn insert_events(c: &mut Connection, thread: &str, events: &[Event]) -> rusqlite::Result<Vec<Event>> {
    let tx = c.transaction()?;
    let mut fresh = vec![];
    {
        let mut ins = tx.prepare_cached("INSERT OR IGNORE INTO code_events(thread, seq, ts, kind, data) VALUES(?1,?2,?3,?4,?5)")?;
        let mut fts = tx.prepare_cached("INSERT INTO code_fts(text, thread, seq) VALUES(?1,?2,?3)")?;
        for e in events {
            let data = serde_json::to_string(e).unwrap_or_default();
            if ins.execute(params![thread, e.seq as i64, e.ts, kind_name(e), data])? == 0 {
                continue;
            }
            let text = match &e.kind {
                agentum_proto::EventKind::User { text, .. } | agentum_proto::EventKind::Message { text, .. } => Some(text.as_str()),
                _ => None,
            };
            if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
                fts.execute(params![t, thread, e.seq as i64])?;
            }
            fresh.push(e.clone());
        }
    }
    tx.commit()?;
    Ok(fresh)
}

/// Events after `after` (all when 0), or the last `limit` before `before`.
pub fn events(c: &Connection, thread: &str, before: Option<u64>, limit: u32) -> rusqlite::Result<(Vec<Event>, bool)> {
    let before = before.unwrap_or(u64::MAX >> 1) as i64;
    let mut st = c.prepare_cached("SELECT data FROM code_events WHERE thread=?1 AND seq<?2 ORDER BY seq DESC LIMIT ?3")?;
    let mut v: Vec<Event> = st.query_map(params![thread, before, limit as i64 + 1], |r| r.get::<_, String>(0))?.filter_map(|s| s.ok().and_then(|s| serde_json::from_str(&s).ok())).collect();
    let more = v.len() > limit as usize;
    v.truncate(limit as usize);
    v.reverse();
    Ok((v, more))
}

pub fn events_after(c: &Connection, thread: &str, after: u64) -> rusqlite::Result<Vec<Event>> {
    let mut st = c.prepare_cached("SELECT data FROM code_events WHERE thread=?1 AND seq>?2 ORDER BY seq")?;
    let v = st.query_map(params![thread, after as i64], |r| r.get::<_, String>(0))?.filter_map(|s| s.ok().and_then(|s| serde_json::from_str(&s).ok())).collect();
    Ok(v)
}

pub fn search(c: &Connection, q: &str, limit: u32) -> rusqlite::Result<Vec<(String, u64, String)>> {
    // Every word as a prefix, quoted (no FTS syntax from the person).
    let query: String = q.split_whitespace().filter(|w| !w.is_empty()).map(|w| format!("\"{}\"*", w.replace('"', ""))).collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        return Ok(vec![]);
    }
    let mut st = c.prepare("SELECT thread, seq, snippet(code_fts, 0, '<mark>', '</mark>', '…', 12) FROM code_fts WHERE code_fts MATCH ?1 ORDER BY rank LIMIT ?2")?;
    let v = st.query_map(params![query, limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, String>(2)?)))?.filter_map(Result::ok).collect();
    Ok(v)
}

pub fn delete_thread(c: &Connection, id: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM code_events WHERE thread=?1", [id])?;
    c.execute("DELETE FROM code_fts WHERE thread=?1", [id])?;
    c.execute("DELETE FROM code_threads WHERE id=?1", [id])?;
    Ok(())
}

pub fn attention_of(t: &ThreadSummary) -> Attention {
    agentum_proto::api::attention(t.state, t.unread, t.asking.is_some())
}
