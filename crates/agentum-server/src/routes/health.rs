use agentum_core::Status;
use axum::Json;
use axum::extract::State;
use axum::routing::get;
use axum::{Router, response::IntoResponse};
use serde::Serialize;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/health", get(health))
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
    uptime_seconds: u64,
    sessions_running: i64,
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    let running = state
        .store
        .count_by_status(Status::Running)
        .await
        .unwrap_or(0);

    Json(Health {
        status: "ok",
        version: state.version,
        uptime_seconds: uptime,
        sessions_running: running,
    })
}
