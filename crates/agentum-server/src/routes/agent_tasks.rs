//! `/api/sessions/{id}/agent-tasks` — read-only snapshot of an agent's
//! current plan / todos / background tasks. Backed by the in-memory
//! [`crate::TranscriptStore`], which tails the JSONL transcripts Claude
//! Code writes under `~/.claude/projects/<encoded-cwd>/`.

use std::path::PathBuf;

use agentum_core::Status;
use agentum_core::transcript::AgentTaskState;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::transcript_store::ObservationMode;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/{id}/agent-tasks", get(get_agent_tasks))
        .route(
            "/api/sessions/{id}/agent-tasks/reset",
            post(reset_agent_tasks),
        )
}

pub(crate) async fn get_agent_tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<AgentTaskState>, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let _lifecycle = state.transcripts.lock_session_lifecycle(id).await;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    #[cfg(test)]
    state.transcripts.pause_agent_task_after_load().await;

    let mode = if session.tool == "claude" && session.status == Status::Running {
        ObservationMode::Live
    } else {
        ObservationMode::SnapshotOnly
    };
    let snap = state
        .transcripts
        .read(id, PathBuf::from(&session.workdir), &session.tool, mode);
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
    let _lifecycle = state.transcripts.lock_session_lifecycle(id).await;
    // Verify the session exists so a stray POST against a deleted id
    // returns 404 instead of silently succeeding.
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    state
        .transcripts
        .reset(id, PathBuf::from(&session.workdir), &session.tool);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_core::{NewSession, Status};
    use agentum_store::Store;
    use axum::extract::{Path, State};
    use tokio::sync::broadcast;

    async fn fixture(
        tool: &str,
    ) -> (
        tempfile::TempDir,
        AppState,
        agentum_core::Session,
        crate::transcript_store::ObserverCounts,
    ) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("test.sqlite")).await.unwrap();
        let (bus, _) = broadcast::channel(32);
        let mut state = AppState::new(store, bus.clone());
        let (transcripts, counts) = crate::TranscriptStore::with_counting_factory(bus);
        state.transcripts = transcripts;
        let workdir = root.path().join("workspace");
        std::fs::create_dir_all(&workdir).unwrap();
        let session = state
            .store
            .create_session(NewSession {
                name: format!("agent-tasks-{tool}"),
                workdir: workdir.to_string_lossy().into_owned(),
                tool: tool.into(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();
        (root, state, session, counts)
    }

    #[tokio::test]
    async fn running_claude_read_is_live_but_historical_read_is_snapshot_only() {
        let (_root, state, session, counts) = fixture("claude").await;
        state
            .store
            .update_status_and_target(session.id, Status::Running, Some("agentum-test"))
            .await
            .unwrap();
        let _ = get_agent_tasks(State(state.clone()), Path(session.id.to_string()))
            .await
            .unwrap();
        assert_eq!(state.transcripts.observing_count(), 1);
        assert_eq!(counts.created(), 1);

        state
            .store
            .update_status_and_target(session.id, Status::Stopped, None)
            .await
            .unwrap();
        let _ = get_agent_tasks(State(state.clone()), Path(session.id.to_string()))
            .await
            .unwrap();
        assert_eq!(state.transcripts.observing_count(), 0);
        assert_eq!(counts.dropped(), 1);

        if let Some(path) =
            agentum_core::transcript::project_dir_for(PathBuf::from(&session.workdir).as_path())
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    #[tokio::test]
    async fn non_claude_route_read_and_reset_never_create_cached_state() {
        let (_root, state, session, _counts) = fixture("codex").await;
        let response = get_agent_tasks(State(state.clone()), Path(session.id.to_string()))
            .await
            .unwrap();
        assert!(response.0.is_empty());
        reset_agent_tasks(State(state.clone()), Path(session.id.to_string()))
            .await
            .unwrap();
        assert_eq!(state.transcripts.cache_count(), 0);
    }
}
