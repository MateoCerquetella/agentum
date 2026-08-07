//! `/api/sessions/{id}/agent-tasks` — read-only snapshot of an agent's
//! current plan / todos / background tasks. Backed by the in-memory
//! [`crate::TranscriptStore`], which tails the JSONL transcripts Claude
//! Code writes under `~/.claude/projects/<encoded-cwd>/`.

use std::collections::HashMap;
use std::path::PathBuf;

use agentum_core::HostKind;
use agentum_core::LOCAL_HOST_ID;
use agentum_core::transcript::{self, AgentTaskSnapshot, AgentTaskSnapshotStatus, AgentTaskState};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::host_runtime::HostRuntimeError;

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
) -> Result<Json<AgentTaskSnapshot>, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;

    if session.tool != "claude" {
        return Ok(Json(
            state.transcripts.snapshot_with_status(id, &session.tool),
        ));
    }

    let host_id = session.host_id.unwrap_or(LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("host {host_id}")))?;

    match &host.kind {
        HostKind::Local => {
            // Lazy-start the watcher on the first local read. It performs a
            // full parse immediately, before this response is built.
            state
                .transcripts
                .ensure_started(id, PathBuf::from(&session.workdir), &session.tool);
            // `notify` is the low-latency path; an explicit reconciliation on
            // the bounded GET poll recovers from coalesced or missed events.
            state.transcripts.reconcile(id);
            let mut snap = state.transcripts.snapshot_with_status(id, &session.tool);
            snap.source_host_id = Some(host_id);
            Ok(Json(snap))
        }
        HostKind::Ssh { .. } => {
            // A local notify watcher cannot observe an SSH filesystem. Read the
            // UUID-pinned transcript on demand; the TUI's bounded catch-up poll
            // supplies reconciliation when no filesystem event can be emitted.
            let result =
                crate::host_runtime::read_claude_transcript(&host, &session.workdir, id).await;
            let mut snap = match result {
                Ok((path, Some(content))) => {
                    let start = state.transcripts.remote_parse_start(id, content.as_bytes());
                    let parsed = parse_transcript(&content[start..]);
                    let status = if parsed.is_empty() {
                        AgentTaskSnapshotStatus::Empty
                    } else {
                        AgentTaskSnapshotStatus::Current
                    };
                    let mut snap = AgentTaskSnapshot::new(parsed, status, &session.tool);
                    snap.transcript_path = Some(path);
                    snap.updated_at_ms = Some(now_ms());
                    snap
                }
                Ok((path, None)) => {
                    let mut snap = AgentTaskSnapshot::new(
                        AgentTaskState::default(),
                        AgentTaskSnapshotStatus::WaitingForTranscript,
                        &session.tool,
                    );
                    snap.transcript_path = Some(path);
                    snap.detail = Some("Claude has not created this session transcript yet".into());
                    snap
                }
                Err(e) => {
                    let mut snap = AgentTaskSnapshot::new(
                        AgentTaskState::default(),
                        classify_remote_read_error(&e),
                        &session.tool,
                    );
                    snap.detail = Some(e.to_string());
                    snap
                }
            };
            snap.source_host_id = Some(host_id);
            Ok(Json(snap))
        }
    }
}

fn classify_remote_read_error(error: &HostRuntimeError) -> AgentTaskSnapshotStatus {
    match error {
        HostRuntimeError::Timeout
        | HostRuntimeError::Io(_)
        | HostRuntimeError::NonZero {
            status: Some(255), ..
        } => AgentTaskSnapshotStatus::HostUnavailable,
        _ => AgentTaskSnapshotStatus::ReadError,
    }
}

fn parse_transcript(content: &str) -> AgentTaskState {
    let mut state = AgentTaskState::default();
    let mut pending = HashMap::new();
    for line in content.lines() {
        transcript::apply_line(&mut state, &mut pending, line);
    }
    state
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_transport_and_source_read_failures_stay_distinct() {
        assert_eq!(
            classify_remote_read_error(&HostRuntimeError::Timeout),
            AgentTaskSnapshotStatus::HostUnavailable
        );
        assert_eq!(
            classify_remote_read_error(&HostRuntimeError::NonZero {
                status: Some(255),
                stderr: "ssh: connect to host failed".into(),
            }),
            AgentTaskSnapshotStatus::HostUnavailable
        );
        assert_eq!(
            classify_remote_read_error(&HostRuntimeError::NonZero {
                status: Some(1),
                stderr: "cat: permission denied".into(),
            }),
            AgentTaskSnapshotStatus::ReadError
        );
        assert_eq!(
            classify_remote_read_error(&HostRuntimeError::NotUtf8(
                String::from_utf8(vec![0xff]).unwrap_err(),
            )),
            AgentTaskSnapshotStatus::ReadError
        );
    }
}
