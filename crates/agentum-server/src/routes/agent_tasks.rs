//! `/api/sessions/{id}/agent-tasks` — read-only snapshot of an agent's
//! current plan / todos / background tasks. Backed by the in-memory
//! [`crate::TranscriptStore`], which tails the JSONL transcripts Claude
//! Code writes under `~/.claude/projects/<encoded-cwd>/`.

use std::path::PathBuf;

use agentum_core::transcript::AgentTaskState;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/{id}/agent-tasks", get(get_agent_tasks))
        .route(
            "/api/sessions/{id}/agent-tasks/reset",
            post(reset_agent_tasks),
        )
}

async fn get_agent_tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<AgentTaskState>, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;

    // Lazy-start the watcher on the first read for this session so the
    // server doesn't waste a watcher on agents the TUI never opens.
    state
        .transcripts
        .ensure_started(id, PathBuf::from(&session.workdir), &session.tool);

    let snap = state.transcripts.snapshot(id).unwrap_or_default();
    Ok(Json(snap))
}

/// `POST /api/sessions/{id}/agent-tasks/reset` — clear the cached
/// plan/todos/tasks for this session and fast-forward the transcript
/// cursor to the current end-of-file. The TUI hits this when the user
/// runs `/clear` (or `\clear`) inside the agent pane, so the panel
/// follows the agent's own context wipe without leaving stale
/// entries behind.
async fn reset_agent_tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    // Verify the session exists so a stray POST against a deleted id
    // returns 404 instead of silently succeeding.
    state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    state.transcripts.reset(id);
    Ok(StatusCode::NO_CONTENT)
}
