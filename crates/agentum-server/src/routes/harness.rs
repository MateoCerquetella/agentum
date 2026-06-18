//! Harness Engine API — `/api/harness/*`.
//!
//! Thin HTTP+WS surface over [`crate::harness::HarnessEngine`] (held in
//! [`AppState::harness`]). The heavy lifting — spawning real agents, the
//! verification gate, advance/block — lives in [`crate::harness::drive`], which
//! this layer kicks off as a background task on `POST /{id}/run`.
//!
//! Mounted into the main `AppState` router so it inherits the bearer-token
//! middleware (free on the embedded loopback server, which is `no_auth`).

use std::path::PathBuf;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::harness::{HarnessConfig, HarnessFiles, HarnessStatus};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/harness", get(list).post(start))
        .route("/api/harness/events", get(events))
        .route("/api/harness/{id}", get(status).delete(stop))
        .route("/api/harness/{id}/run", post(run))
        .route("/api/harness/{id}/init", post(init))
        .route("/api/harness/{id}/verify", post(verify))
        .route("/api/harness/{id}/confirm", post(confirm))
        .route("/api/harness/{id}/files", get(files))
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    /// Project directory containing `.harness/`.
    workdir: String,
}

#[derive(Debug, Serialize)]
struct StartResponse {
    harness_id: Uuid,
}

/// `POST /api/harness` — register a run from a project dir (validates `.harness/`).
async fn start(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<StartRequest>,
) -> Result<axum::Json<StartResponse>, ApiError> {
    // Reuse the same `~`/relative expansion every workdir-taking route uses.
    let workdir = super::util::expand_workdir(&req.workdir)?;
    let harness_id = state
        .harness
        .start(workdir)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(axum::Json(StartResponse { harness_id }))
}

/// `GET /api/harness` — full status for every registered run (handy for a list
/// view without an extra round-trip per id).
async fn list(State(state): State<AppState>) -> axum::Json<Vec<HarnessStatus>> {
    let mut out = Vec::new();
    for id in state.harness.list().await {
        if let Ok(s) = state.harness.status(id).await {
            out.push(s);
        }
    }
    axum::Json(out)
}

/// `GET /api/harness/{id}` — one run's status snapshot.
async fn status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<axum::Json<HarnessStatus>, ApiError> {
    state
        .harness
        .status(id)
        .await
        .map(axum::Json)
        .map_err(|e| ApiError::NotFound(e.to_string()))
}

/// `POST /api/harness/{id}/run` — kick off the end-to-end drive loop in the
/// background. Idempotent-ish: a second call while a loop is live is rejected.
async fn run(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    let claimed = state
        .harness
        .claim_driver(id)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    if !claimed {
        return Err(ApiError::BadRequest("harness is already running".into()));
    }
    // The drive loop owns its own error handling (emits Error + Failed state).
    let st = state.clone();
    tokio::spawn(async move { crate::harness::drive(st, id).await });
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/harness/{id}/init` — run `init.sh` only (manual environment check).
async fn init(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<axum::Json<bool>, ApiError> {
    state
        .harness
        .run_init(id)
        .await
        .map(axum::Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

#[derive(Debug, Deserialize)]
struct VerifyRequest {
    feature_id: String,
}

/// `POST /api/harness/{id}/verify` — run the gate for one feature and finalize
/// it (manual single-shot; the drive loop has its own retry-aware path).
async fn verify(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    axum::Json(req): axum::Json<VerifyRequest>,
) -> Result<axum::Json<bool>, ApiError> {
    state
        .harness
        .run_verify(id, &req.feature_id)
        .await
        .map(axum::Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

#[derive(Debug, Deserialize)]
struct ConfirmRequest {
    feature_id: String,
}

/// `POST /api/harness/{id}/confirm` — human confirms a feature parked at the
/// HITL-at-QA gate: finalize it `Done` and resume the drive loop from the next
/// pending feature (the paused run already freed its driver slot).
async fn confirm(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    axum::Json(req): axum::Json<ConfirmRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .harness
        .confirm_feature(id, &req.feature_id)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    // Resume autonomously: re-claim the driver and continue with the next
    // pending feature. If something else already drives it, leave it be.
    if state.harness.claim_driver(id).await.unwrap_or(false) {
        let st = state.clone();
        tokio::spawn(async move { crate::harness::drive(st, id).await });
    }
    Ok(StatusCode::ACCEPTED)
}

/// `GET /api/harness/{id}/files` — current `.harness/` file contents for the viewer.
async fn files(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<axum::Json<HarnessFiles>, ApiError> {
    let workdir: PathBuf = state
        .harness
        .workdir(id)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    Ok(axum::Json(HarnessConfig::read_files(&workdir).await))
}

/// `DELETE /api/harness/{id}` — drop the run from the engine.
async fn stop(State(state): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, ApiError> {
    state
        .harness
        .stop(id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `WS /api/harness/events` — live `HarnessEvent` stream (all runs). Mirrors the
/// `events.rs` WS pattern: subscribe to the engine's broadcast bus and forward
/// each event as JSON text. A slow client that lags past the bus capacity gets
/// a single `harness.lagged` marker and resumes.
async fn events(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let rx = state.harness.subscribe();
    ws.on_upgrade(move |socket| run_events(socket, rx))
}

async fn run_events(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<crate::harness::HarnessEvent>,
) {
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let payload = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
                    if socket.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    let s = serde_json::json!({ "type": "lagged", "skipped": skipped }).to_string();
                    if socket.send(Message::Text(s.into())).await.is_err() { break; }
                }
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {} // ignore client pings/text
            }
        }
    }
}
