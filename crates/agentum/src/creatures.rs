//! The bots' creatures: what each has found, wears and was fed, and its boxes
//! (web/src/creature). Kept here so every device shows the same creatures.
//! The browser owns the rules (what a box holds, when the next one comes);
//! this only stores each bot's state, as kv "creatures" = {bot: state}.
use crate::{state::App, util::*};
use axum::{extract::{Path, State}, routing::{get, put}, Json, Router};
use serde_json::{json, Value};

pub fn routes() -> Router<App> {
    Router::new().route("/api/creatures", get(all)).route("/api/creatures/{bot}", put(save))
}

async fn all(State(app): State<App>) -> R<Json<Value>> {
    Ok(Json(app.db.kv_get("creatures").await?.unwrap_or(json!({}))))
}

async fn save(State(app): State<App>, Path(bot): Path<String>, Json(state): Json<Value>) -> R<Json<Value>> {
    if bot.is_empty() || bot.len() > 20 || !bot.chars().all(|c| c.is_ascii_lowercase()) { return Err(bad("bot must be a short lowercase id")); }
    if !state.is_object() || state.to_string().len() > 16_384 { return Err(bad("a creature is a small object")); }
    let mut all = app.db.kv_get("creatures").await?.unwrap_or(json!({}));
    if !all.is_object() { all = json!({}); }
    all[&bot] = state;
    app.db.kv_set("creatures", &all).await?;
    app.emit(json!({"kind": "creatures", "bot": bot}));
    Ok(Json(json!({"ok": true})))
}
