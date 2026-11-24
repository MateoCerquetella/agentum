//! Today (owner: today backend): morning brief, plan for today, wrap-up,
//! week ahead, leave-by, money, deliveries, Mercury health, workspace
//! search/ask. Port of brief.py, assist.py, insights.py, workspace.py.
mod assist;
mod brief;
pub mod i18n;
mod insights;
pub(crate) mod workspace;

use crate::{state::App, util::*};
use axum::{extract::{Query, State}, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/brief", get(|State(a): State<App>| async move { Json(brief::get(&a).await) }))
        .route("/api/brief/refresh", post(|State(a): State<App>| async move { Json(brief::build(&a, false).await) }))
        .route("/api/assist", get(get_assist))
        .route("/api/assist/hide", post(hide))
        .route("/api/money", get(|State(a): State<App>| async move { Ok::<_, AppError>(Json(insights::money(&a).await?)) }))
        .route("/api/deliveries", get(|State(a): State<App>| async move { Ok::<_, AppError>(Json(insights::deliveries(&a).await?)) }))
        .route("/api/health/mercury", get(|| async { Json(insights::health()) }))
        .route("/api/workspace/today", get(|State(a): State<App>| async move { Ok::<_, AppError>(Json(workspace::today(&a).await?)) }))
        .route("/api/workspace/search", get(ws_search))
        .route("/api/workspace/status", get(|State(a): State<App>| async move { Json(crate::llm::status(&a)) }))
        .route("/api/workspace/ask", post(ws_ask))
}

/// Background: brief at CERQ_BRIEF_AT, plan/wrap-up/week/leave-by pushes.
pub fn start(app: App) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            brief::tick(&app).await;
            if let Err(e) = assist::tick(&app).await { tracing::warn!("assist: {e:#}"); }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

#[derive(Deserialize)] struct Fresh { fresh: Option<String> }
async fn get_assist(State(app): State<App>, Query(q): Query<Fresh>) -> Json<Value> { Json(assist::get(&app, q.fresh.as_deref() == Some("1")).await) }
async fn hide(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    let id = s(&b, "id");
    if id.is_empty() || id.len() > 200 { return Err(bad("id required (at most 200 characters)")); }
    assist::hide(&app, &id).await?;
    Ok(Json(json!({"ok": true})))
}
#[derive(Deserialize)] struct Q { q: Option<String> }
async fn ws_search(State(app): State<App>, Query(q): Query<Q>) -> R<Json<Value>> {
    let q = q.q.unwrap_or_default();
    if q.len() > 500 { return Err(bad("Search must be text of at most 500 characters.")); }
    Ok(Json(workspace::search(&app, &q).await?))
}
async fn ws_ask(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    workspace::ask(&app, &b).await.map(Json).map_err(|e| match e {
        workspace::AskError::Bad(m) => bad(m),
        workspace::AskError::Gone(m) => AppError(axum::http::StatusCode::NOT_FOUND, m),
        workspace::AskError::Other(e) => e.into(),
    })
}

// ---- small shared helpers
pub(crate) fn now(app: &App) -> chrono::DateTime<chrono_tz::Tz> { chrono::Utc::now().with_timezone(&app.tz()) }
/// "HH:MM" → minutes.
pub(crate) fn m(hhmm: &str) -> i64 {
    let mut it = hhmm.split(':');
    let h: i64 = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0);
    h * 60 + it.next().and_then(|x| x.trim().parse::<i64>().ok()).unwrap_or(0)
}
pub(crate) fn hm(mins: i64) -> String { format!("{:02}:{:02}", mins.min(1440) / 60, mins.rem_euclid(60)) }
pub(crate) fn dur(mins: i64) -> String { if mins >= 60 { format!("{}h{:02}", mins / 60, mins % 60) } else { format!("{mins} min") } }
pub(crate) fn hm_env(app: &App, key: &str, default: &str) -> i64 {
    let v = app.setting(key);
    if v.len() == 5 && v.as_bytes()[2] == b':' { m(&v) } else { m(default) }
}
pub(crate) fn num(app: &App, key: &str) -> Option<f64> { app.setting(key).trim().parse().ok() }
