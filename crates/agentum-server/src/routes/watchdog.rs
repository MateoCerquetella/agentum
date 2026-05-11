//! `/api/watchdog` — cold-start fetch for the dashboard's watchdog
//! feed.
//!
//! Live updates flow over the WS at `/api/events`. This GET is what
//! the dashboard hits on first load (and as a fallback when the WS
//! is unavailable) to populate the rail with recent activity.

use agentum_core::WatchdogEvent;
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use serde::Deserialize;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/watchdog", get(list))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    /// Cap on returned events. Defaults to 50, max 500.
    #[serde(default)]
    limit: Option<i64>,
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<WatchdogEvent>>, ApiError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let events = state.store.list_watchdog_events(limit).await?;
    let projected: Vec<WatchdogEvent> = events.iter().filter_map(|e| e.to_watchdog()).collect();
    Ok(Json(projected))
}
