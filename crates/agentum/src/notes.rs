//! Notes and to-dos (notes table; note_type 'todo' rows are to-dos, with an
//! optional checklist in items and a due date). Also backs the To-do widget.
use crate::{state::App, util::*};
use axum::{extract::{Path, Query, State}, routing::{get, put}, Json, Router};
use rusqlite::Row;
use serde::Deserialize;
use serde_json::{json, Value};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/notes", get(list).post(create))
        .route("/api/notes/{id}", put(update).delete(remove))
        .route("/api/todos", get(todos).post(create_todo))
        .route("/api/todos/{id}", put(update).delete(remove))
}

fn row(r: &Row) -> rusqlite::Result<Value> {
    let items: String = r.get::<_, Option<String>>("items")?.unwrap_or_default();
    let items: Value = serde_json::from_str(&items).unwrap_or(json!([]));
    let done = items.as_array().map(|a| !a.is_empty() && a.iter().all(|i| i["done"].as_bool().unwrap_or(false) || i["checked"].as_bool().unwrap_or(false))).unwrap_or(false);
    Ok(json!({"id": r.get::<_, String>("id")?, "title": r.get::<_, Option<String>>("title")?, "content": r.get::<_, Option<String>>("content")?,
        "items": items, "note_type": r.get::<_, Option<String>>("note_type")?, "color": r.get::<_, Option<String>>("color")?,
        "label": r.get::<_, Option<String>>("label")?, "pinned": r.get::<_, i64>("pinned")? != 0, "archived": r.get::<_, i64>("archived")? != 0,
        "due": r.get::<_, Option<String>>("due_date")?, "done": done, "created_at": r.get::<_, Option<String>>("created_at")?, "updated_at": r.get::<_, Option<String>>("updated_at")?}))
}

pub async fn all(app: &App, archived: bool) -> anyhow::Result<Vec<Value>> {
    app.db.call(move |c| {
        let mut st = c.prepare("SELECT * FROM notes WHERE archived=?1 ORDER BY pinned DESC, sort_order, updated_at DESC")?;
        st.query_map([archived as i64], row)?.collect()
    }).await
}

#[derive(Deserialize)] pub struct LQ { archived: Option<bool> }
async fn list(State(app): State<App>, Query(q): Query<LQ>) -> R<Json<Value>> { Ok(Json(json!(all(&app, q.archived.unwrap_or(false)).await?))) }

/// Open to-dos (note_type todo, not finished, not calendar reminders).
pub async fn open_todos(app: &App) -> anyhow::Result<Vec<Value>> {
    Ok(all(app, false).await?.into_iter().filter(|n| n["note_type"] == "todo" && !n["done"].as_bool().unwrap_or(false) && !s(n, "title").starts_with("Reminder:")).collect())
}
async fn todos(State(app): State<App>) -> R<Json<Value>> { Ok(Json(json!(open_todos(&app).await?))) }

pub async fn insert(app: &App, b: &Value) -> anyhow::Result<Value> {
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let (i2, b2) = (id.clone(), b.clone());
    app.db.call(move |c| c.execute("INSERT INTO notes(id,title,content,items,note_type,color,label,pinned,due_date,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10)",
        rusqlite::params![i2, s(&b2, "title"), s(&b2, "content"), b2.get("items").map(|x| x.to_string()).unwrap_or("[]".into()),
            if s(&b2, "note_type").is_empty() { "note".to_string() } else { s(&b2, "note_type") }, s(&b2, "color"), s(&b2, "label"),
            b2["pinned"].as_bool().unwrap_or(false) as i64, b2.get("due").and_then(|d| d.as_str()).filter(|d| !d.is_empty()), now])).await?;
    app.emit(json!({"kind": "todos"}));
    get_one(app, &id).await
}
pub async fn get_one(app: &App, id: &str) -> anyhow::Result<Value> {
    let id = id.to_string();
    app.db.call(move |c| c.query_row("SELECT * FROM notes WHERE id=?1", [id], row)).await
}
async fn create(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> { Ok(Json(insert(&app, &b).await?)) }
async fn create_todo(State(app): State<App>, Json(mut b): Json<Value>) -> R<Json<Value>> {
    if s(&b, "title").trim().is_empty() { return Err(bad("title required")); }
    b["note_type"] = json!("todo");
    Ok(Json(insert(&app, &b).await?))
}

/// Patch: title, content, items, color, label, pinned, archived, due, done (done → every item checked, or a single item added and checked).
pub async fn patch(app: &App, id: &str, b: &Value) -> anyhow::Result<Value> {
    let mut n = get_one(app, id).await.map_err(|_| anyhow::anyhow!("note not found"))?;
    for k in ["title", "content", "color", "label", "due"] { if let Some(v) = b.get(k) { n[k] = v.clone(); } }
    for k in ["pinned", "archived"] { if let Some(v) = b.get(k) { n[k] = v.clone(); } }
    if let Some(v) = b.get("items") { n["items"] = v.clone(); }
    if let Some(done) = b.get("done").and_then(|d| d.as_bool()) {
        let items = n["items"].as_array().cloned().unwrap_or_default();
        n["items"] = if items.is_empty() { if done { json!([{"text": s(&n, "title"), "done": true, "checked": true}]) } else { json!([]) } }
            else { json!(items.into_iter().map(|mut i| { i["done"] = json!(done); i["checked"] = json!(done); i }).collect::<Vec<_>>()) };
    }
    let (id2, n2) = (id.to_string(), n.clone());
    app.db.call(move |c| c.execute("UPDATE notes SET title=?2,content=?3,items=?4,color=?5,label=?6,pinned=?7,archived=?8,due_date=?9,updated_at=?10 WHERE id=?1",
        rusqlite::params![id2, n2["title"].as_str(), n2["content"].as_str(), n2["items"].to_string(), n2["color"].as_str(), n2["label"].as_str(),
            n2["pinned"].as_bool().unwrap_or(false) as i64, n2["archived"].as_bool().unwrap_or(false) as i64, n2["due"].as_str().filter(|d| !d.is_empty()), chrono::Utc::now().to_rfc3339()])).await?;
    app.emit(json!({"kind": "todos"}));
    get_one(app, id).await
}
async fn update(State(app): State<App>, Path(id): Path<String>, Json(b): Json<Value>) -> R<Json<Value>> { Ok(Json(patch(&app, &id, &b).await?)) }
pub async fn delete(app: &App, id: &str) -> anyhow::Result<()> {
    let id = id.to_string();
    app.db.call(move |c| c.execute("DELETE FROM notes WHERE id=?1", [id])).await?;
    app.emit(json!({"kind": "todos"}));
    Ok(())
}
async fn remove(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> { delete(&app, &id).await?; Ok(Json(json!({"ok": true}))) }
