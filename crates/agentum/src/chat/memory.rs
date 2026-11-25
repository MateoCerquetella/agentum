//! What the chat remembers about you across chats. The model adds facts with
//! memory_add and drops wrong ones with memory_forget; every chat starts with
//! them in its instructions. Settings → Memory lists them for you to edit.
use crate::{state::App, util::*};
use axum::{extract::{Path, State}, routing::{get, put}, Json, Router};
use serde_json::{json, Value};

const PROMPT_CHARS: usize = 6000;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/memories", get(list).post(add_route))
        .route("/api/memories/{id}", put(edit).delete(remove))
}

async fn list(State(app): State<App>) -> R<Json<Value>> {
    let v = app.db.call(|c| {
        let mut st = c.prepare("SELECT id, coalesce(text,''), created_at FROM memories ORDER BY created_at DESC")?;
        st.query_map([], |r| Ok(json!({"id": r.get::<_, String>(0)?, "text": r.get::<_, String>(1)?, "created_at": r.get::<_, Option<String>>(2)?})))?.collect::<Result<Vec<_>, _>>()
    }).await?;
    Ok(Json(json!(v)))
}

async fn add_route(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    add(&app, &s(&b, "text")).await.map_err(|e| bad(e.to_string()))?;
    app.emit(json!({"kind": "memories"}));
    Ok(Json(json!({"ok": true})))
}

async fn edit(State(app): State<App>, Path(id): Path<String>, Json(b): Json<Value>) -> R<Json<Value>> {
    let t = s(&b, "text").trim().chars().take(500).collect::<String>();
    if t.is_empty() { return Err(bad("empty")); }
    app.db.call(move |c| c.execute("UPDATE memories SET text=?2 WHERE id=?1", [id, t])).await?;
    app.emit(json!({"kind": "memories"}));
    Ok(Json(json!({"ok": true})))
}

async fn remove(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    app.db.call(move |c| c.execute("DELETE FROM memories WHERE id=?1", [id])).await?;
    app.emit(json!({"kind": "memories"}));
    Ok(Json(json!({"ok": true})))
}

/// Save a fact unless the same one is already there. Ok(false) when it was.
pub async fn add(app: &App, text: &str) -> anyhow::Result<bool> {
    let t = text.trim().chars().take(500).collect::<String>();
    if t.is_empty() { anyhow::bail!("empty"); }
    let id = uuid::Uuid::new_v4().to_string();
    Ok(app.db.call(move |c| {
        if c.query_row("SELECT 1 FROM memories WHERE lower(trim(text))=lower(?1)", [&t], |_| Ok(())).is_ok() { return Ok(false); }
        c.execute("INSERT INTO memories(id,text,category,created_at) VALUES(?1,?2,'fact',datetime('now'))", [id, t])?;
        Ok(true)
    }).await?)
}

/// Forget by the short id shown to the model (the first 8 characters).
pub async fn forget(app: &App, id: &str) -> anyhow::Result<String> {
    let id = id.trim().trim_matches(['[', ']']).to_string();
    if id.len() < 6 { anyhow::bail!("use the memory id from your instructions"); }
    let gone: Vec<String> = app.db.call(move |c| {
        let mut st = c.prepare("SELECT id, coalesce(text,'') FROM memories WHERE id LIKE ?1 || '%'")?;
        let rows = st.query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        if rows.len() != 1 { return Ok(vec![]); }
        c.execute("DELETE FROM memories WHERE id=?1", [&rows[0].0])?;
        Ok(vec![rows[0].1.clone()])
    }).await?;
    match gone.first() { Some(t) => { app.emit(json!({"kind": "memories"})); Ok(format!("Forgot: {t}")) } None => anyhow::bail!("no memory with that id") }
}

/// The memories for the system prompt, newest first, within a budget.
pub async fn for_prompt(app: &App) -> String {
    let rows: Vec<(String, String)> = app.db.call(|c| {
        let mut st = c.prepare("SELECT id, coalesce(text,'') FROM memories ORDER BY created_at DESC LIMIT 200")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    }).await.unwrap_or_default();
    let mut used = 0;
    let lines: Vec<String> = rows.iter().filter(|(_, t)| !t.trim().is_empty()).map_while(|(id, t)| {
        let line = format!("- [{}] {}", &id[..id.len().min(8)], t.trim());
        used += line.len();
        (used <= PROMPT_CHARS).then_some(line)
    }).collect();
    if lines.is_empty() { return String::new(); }
    format!("\n\nWhat you remember about the user from earlier chats (use it when it helps; if the user corrects one, memory_forget its [id] and memory_add the new fact):\n{}", lines.join("\n"))
}
