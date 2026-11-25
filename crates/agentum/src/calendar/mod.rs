//! CERQ Calendar (owner: calendar backend). CalDAV (iCloud, Google) and
//! Microsoft Graph (Outlook) sync into calendar_events, local edits written
//! back, recurrence expanded on read. Cross-module API below is a contract.
mod dav;
mod edit;
mod graph;
mod ics;
mod recur;
mod store;
mod sync;
mod widget;

use crate::{state::App, util::*};
use axum::{extract::{Path, Query, State}, routing::{get, post, put}, Json, Router};
use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use regex::Regex;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;

pub use store::done_keys;
pub use sync::{last_result as last_sync_result, sync as sync_now};
pub use widget::clean as clean_model_text;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/calendar/events", get(list_events).post(post_event))
        .route("/api/calendar/events/{id}", put(put_event).delete(del_event))
        .route("/api/calendar/calendars", get(get_calendars))
        .route("/api/calendar/sync", post(post_sync))
        .route("/api/calendar/done", get(get_done).post(post_done))
        .route("/api/calendar/widget", get(|State(a): State<App>| async move { Ok::<_, AppError>(Json(widget::widget(&a).await?)) }))
        .route("/api/calendar/summary", get(get_summary))
        .route("/api/calendar/quick-parse", post(quick_parse))
}
/// Background: periodic sync of every account; write-back of pending edits.
pub fn start(app: App) { sync::start(app) }

/// One occurrence in the user's time zone (recurrences expanded).
#[derive(Clone, Debug, Serialize)]
pub struct Occ {
    /// Unique per occurrence: the event uid, or "<uid>::<YYYY-MM-DD[THH:MM]>" for a recurring one.
    pub id: String,
    /// The stored event's uid (the series uid for recurring events).
    pub uid: String,
    pub calendar_id: String,
    /// "Calendario · iCloud" style: calendar name · account label.
    pub calendar: String,
    pub title: String,
    pub location: String,
    pub description: String,
    pub start: DateTime<Tz>,
    pub end: DateTime<Tz>,
    pub all_day: bool,
    pub recurring: bool,
    /// work | personal | money | birthday | holiday
    pub kind: String,
    pub color: String,
    pub read_only: bool,
}

/// Occurrences overlapping [from, to) (local dates), sorted by start.
/// "Calendario · iCloud" → "iCloud".
pub fn store_account(label: &str) -> String { store::account_of(label).to_string() }
pub async fn occurrences(app: &App, from: NaiveDate, to: NaiveDate) -> anyhow::Result<Vec<Occ>> { store::occurrences(app, from, to).await }
/// Who's in an event ("Name <email>", organizer first), from the last pull. Empty when the server doesn't say.
pub async fn people(app: &App, uid: &str) -> Vec<String> {
    let uid = uid.to_string();
    app.db.call(move |c| c.query_row("SELECT people FROM calendar_people WHERE uid=?1", [&uid], |r| r.get::<_, String>(0)).optional()).await
        .ok().flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
}
/// True until the first pull that reads attendees.
pub async fn people_missing(app: &App) -> bool {
    app.db.call(|c| c.query_row("SELECT count(*) FROM calendar_people", [], |r| r.get::<_, i64>(0))).await.map(|n| n == 0).unwrap_or(false)
}
/// Create: {summary, dtstart, dtend?, all_day?, description?, location?, calendar_id?, rrule?} (times ISO, local if no offset). Returns the event.
pub async fn create_event(app: &App, ev: Value) -> anyhow::Result<Value> { edit::create(app, &ev).await }
/// Patch any of the create fields. `uid` may be an occurrence id (then only that occurrence changes).
pub async fn update_event(app: &App, uid: &str, patch: Value) -> anyhow::Result<Value> { edit::update(app, uid, &patch).await }
pub async fn delete_event(app: &App, uid: &str) -> anyhow::Result<()> { edit::delete(app, uid, "").await }
/// [{id, name, account, color, source, read_only, default}]
pub async fn calendars(app: &App) -> anyhow::Result<Vec<Value>> { store::calendars_json(app).await }
/// Done marks ("<uid>|YYYY-MM-DD"). (For the chat's calendar tools.)
#[allow(dead_code)]
pub async fn is_done(app: &App, uid: &str, day: NaiveDate) -> bool {
    let k = store::done_key(uid, day);
    app.db.call(move |c| c.query_row("SELECT 1 FROM calendar_done WHERE key=?1", [k], |_| Ok(()))).await.is_ok()
}
/// A signature that changes whenever events or done marks change (for caches).
pub async fn signature(app: &App) -> String { store::signature(app).await }

// ---- handlers
fn err400(e: anyhow::Error) -> AppError {
    let m = format!("{e:#}");
    if m.contains("not found") { AppError(axum::http::StatusCode::NOT_FOUND, m) } else { bad(m) }
}

#[derive(Deserialize)] struct Range { start: String, end: String }
fn day(s: &str) -> R<NaiveDate> { NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").map_err(|_| bad(format!("not a date: {s}"))) }

async fn list_events(State(app): State<App>, Query(q): Query<Range>) -> R<Json<Value>> {
    let (from, to) = (day(&q.start)?, day(&q.end)?);
    if to <= from || (to - from).num_days() > 400 { return Err(bad("end must be after start, at most 400 days")); }
    let occ = store::one_holiday(occurrences(&app, from, to).await?);
    let done = done_keys(&app).await?;
    let events: Vec<Value> = occ.into_iter().map(|o| {
        let d = done.contains(&store::done_key(&o.uid, o.start.date_naive()));
        let mut v = serde_json::to_value(o).unwrap_or_default();
        v["done"] = json!(d);
        v
    }).collect();
    Ok(Json(json!({"events": events, "calendars": calendars(&app).await?})))
}
async fn post_event(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> { Ok(Json(create_event(&app, b).await.map_err(err400)?)) }
async fn put_event(State(app): State<App>, Path(id): Path<String>, Json(b): Json<Value>) -> R<Json<Value>> { Ok(Json(update_event(&app, &id, b).await.map_err(err400)?)) }
#[derive(Deserialize)] struct Scope { scope: Option<String> }
async fn del_event(State(app): State<App>, Path(id): Path<String>, Query(q): Query<Scope>) -> R<Json<Value>> {
    match q.scope.as_deref().filter(|s| !s.is_empty()) {
        Some(scope) => edit::delete(&app, &id, scope).await,
        None => delete_event(&app, &id).await,
    }.map_err(err400)?;
    Ok(Json(json!({"ok": true})))
}
async fn get_calendars(State(app): State<App>) -> R<Json<Value>> { Ok(Json(json!(calendars(&app).await?))) }
#[derive(Deserialize)] struct Force { force: Option<String> }
async fn post_sync(State(app): State<App>, Query(q): Query<Force>) -> R<Json<Value>> {
    Ok(Json(sync::sync(&app, matches!(q.force.as_deref(), Some("1") | Some("true"))).await))
}

async fn get_done(State(app): State<App>) -> R<Json<Value>> {
    let mut keys: Vec<String> = done_keys(&app).await?.into_iter().collect();
    keys.sort();
    Ok(Json(json!({"keys": keys})))
}
async fn post_done(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^|\s]{1,300}\|\d{4}-\d{2}-\d{2}$").unwrap());
    let raw = s(&b, "key");
    if raw.len() > 320 || !KEY.is_match(&raw) { return Err(bad("a mark is <event uid>|<YYYY-MM-DD>")); }
    // One mark per occurrence: the series uid and the local day.
    let (uid, d) = raw.rsplit_once('|').unwrap();
    let key = format!("{}|{d}", uid.split("::").next().unwrap_or(uid));
    let done = b["done"].as_bool().unwrap_or(true);
    let k2 = key.clone();
    app.db.call(move |c| if done { c.execute("INSERT OR REPLACE INTO calendar_done(key,ts) VALUES(?1,?2)", params![k2, now_ts()]) } else { c.execute("DELETE FROM calendar_done WHERE key=?1", [k2]) }).await?;
    app.emit(json!({"kind": "calendar"}));
    Ok(Json(json!({"ok": true, "done": done, "key": key})))
}
async fn get_summary(State(app): State<App>, Query(q): Query<Range>) -> R<Json<Value>> {
    Ok(Json(widget::summary(&app, day(&q.start)?, day(&q.end)?).await.map_err(err400)?))
}
async fn quick_parse(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    let text = s(&b, "text");
    if text.trim().is_empty() || text.len() > 500 { return Err(bad("text is required (at most 500 characters)")); }
    Ok(Json(widget::quick_parse(&app, text.trim()).await.map_err(err400)?))
}
