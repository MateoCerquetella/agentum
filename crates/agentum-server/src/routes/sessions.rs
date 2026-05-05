use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use agentum_core::{NewSession, Session, Status};
use agentum_store::paths;
use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use bytes::Bytes;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::time::sleep;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list).post(create))
        .route("/api/sessions/{id}", get(get_one).patch(patch_session).delete(delete))
        .route("/api/sessions/{id}/start", post(start))
        .route("/api/sessions/{id}/stop", post(stop))
        .route("/api/sessions/{id}/kill", post(kill))
        .route("/api/sessions/{id}/send", post(send))
        .route("/api/sessions/{id}/stream", get(stream))
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Session>>, ApiError> {
    let status = match q.status.as_deref() {
        Some(s) => Some(Status::from_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))?),
        None => None,
    };
    let rows = state.store.list_sessions(status).await?;
    Ok(Json(rows))
}

async fn create(
    State(state): State<AppState>,
    Json(payload): Json<NewSession>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    let workdir = PathBuf::from(&payload.workdir);
    if !workdir.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        )));
    }
    let s = state.store.create_session(payload).await?;
    Ok((StatusCode::CREATED, Json(s)))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let s = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(Json(s))
}

#[derive(Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
struct PatchBody {
    #[serde(default)]
    flags: Option<Vec<String>>,
    #[serde(default)]
    model: Option<Option<String>>,
}

async fn patch_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    if matches!(session.status, Status::Running) {
        return Err(ApiError::BadRequest(
            "cannot patch a running session; stop it first".into(),
        ));
    }
    if let Some(flags) = body.flags {
        let updated = state.store.patch_session_flags(id, &flags).await?;
        return Ok(Json(updated));
    }
    if let Some(model) = body.model {
        // Future: patch model — not yet implemented in store
        let _ = model;
    }
    Ok(Json(session))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    if matches!(session.status, Status::Running) {
        if !q.force {
            return Err(ApiError::BadRequest(
                "session is running; pass ?force=true to kill and remove".into(),
            ));
        }
        let target = tmux_target(&session);
        agentum_tmux::kill_session(&target)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    state.store.delete_session(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let target = agentum_tmux::target_for(&session.name);

    let already = agentum_tmux::has_session(&target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if matches!(session.status, Status::Running) && already {
        return Ok(Json(session));
    }
    if already {
        return Err(ApiError::BadRequest(format!(
            "tmux session {target} already exists outside agentum; refuse to clobber"
        )));
    }

    let workdir = PathBuf::from(&session.workdir);
    if !workdir.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        )));
    }

    let adapter = agentum_executor::adapter_for(&session.tool);
    let launch = adapter.launch(&session);

    agentum_tmux::new_session(&target, &workdir, &launch.argv, &launch.env)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Err(e) = agentum_tmux::pipe_pane(&target, &log).await {
        let _ = agentum_tmux::kill_session(&target).await;
        return Err(ApiError::Internal(e.to_string()));
    }

    state
        .store
        .update_status_and_target(id, Status::Running, Some(&target))
        .await?;
    Ok(Json(load(&state, id).await?))
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let target = tmux_target(&session);
    agentum_tmux::graceful_stop(&target, GRACEFUL_STOP_TIMEOUT)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    state
        .store
        .update_status_and_target(id, Status::Stopped, None)
        .await?;
    Ok(Json(load(&state, id).await?))
}

async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let target = tmux_target(&session);
    agentum_tmux::kill_session(&target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    state
        .store
        .update_status_and_target(id, Status::Stopped, None)
        .await?;
    Ok(Json(load(&state, id).await?))
}

async fn load(state: &AppState, id: Uuid) -> Result<Session, ApiError> {
    state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))
}

fn tmux_target(session: &Session) -> String {
    session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name))
}

// ---------- /send ----------

#[derive(Deserialize)]
struct SendBody {
    /// Free-form text typed into the pane. Conceptually equivalent to a user typing.
    #[serde(default)]
    text: Option<String>,
    /// Raw tmux key spec (e.g. `C-c`, `Enter`, `M-x`). Sent literally.
    #[serde(default)]
    keys: Option<String>,
    /// Append a tmux `Enter` after the payload — useful for chat-style inputs.
    #[serde(default)]
    append_enter: bool,
}

async fn send(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let target = session
        .tmux_target
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("session is not running".into()))?;

    let payload = body
        .text
        .as_deref()
        .or(body.keys.as_deref())
        .ok_or_else(|| ApiError::BadRequest("must provide `text` or `keys`".into()))?;

    if !agentum_tmux::has_session(target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::BadRequest(
            "tmux session not active for this session".into(),
        ));
    }

    agentum_tmux::send_keys(target, payload, body.append_enter)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------- WS /stream ----------

async fn stream(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let target = tmux_target(&session);
    Ok(ws.on_upgrade(move |socket| stream_session(socket, id, target)))
}

const BACKFILL_BYTES: u64 = 4096;
const READ_CHUNK: usize = 8192;

async fn stream_session(mut socket: WebSocket, id: Uuid, target: String) {
    let log_path = match paths::pane_log(&id.to_string()) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[path error: {e}]").into()))
                .await;
            return;
        }
    };

    // Wait briefly for pipe-pane to create the file (it appears milliseconds
    // after `agentum up` returns).
    let mut waited = 0;
    while !log_path.exists() && waited < 50 {
        sleep(Duration::from_millis(100)).await;
        waited += 1;
    }
    if !log_path.exists() {
        let _ = socket
            .send(Message::Text("[no pane log — session not running]".into()))
            .await;
        return;
    }

    let mut file = match tokio::fs::File::open(&log_path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[open error: {e}]").into()))
                .await;
            return;
        }
    };

    // Backfill last 4KB so the user lands on recent context, not a blank pane.
    if let Ok(end) = file.seek(std::io::SeekFrom::End(0)).await {
        let backfill = end.min(BACKFILL_BYTES);
        if backfill > 0
            && file
                .seek(std::io::SeekFrom::End(-(backfill as i64)))
                .await
                .is_ok()
        {
            let mut backfill_buf = vec![0u8; backfill as usize];
            if file.read_exact(&mut backfill_buf).await.is_ok()
                && socket
                    .send(Message::Binary(Bytes::from(backfill_buf)))
                    .await
                    .is_err()
            {
                return;
            }
        }
    }

    // Tail the pane log on a dedicated task and pipe chunks through an mpsc.
    // The main loop multiplexes `tail_rx` (output) and `socket.recv()` (input)
    // so a chatty pane never starves keystrokes — and vice versa.
    let (tail_tx, mut tail_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    let tail_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match file.read(&mut buf).await {
                Ok(0) => sleep(Duration::from_millis(80)).await,
                Ok(n) => {
                    if tail_tx
                        .send(Bytes::copy_from_slice(&buf[..n]))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    loop {
        tokio::select! {
            chunk = tail_rx.recv() => match chunk {
                Some(bytes) => {
                    if socket.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
                None => break, // tail task ended (file error / eof on dead pane)
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) if !b.is_empty() => {
                    if let Err(e) = agentum_tmux::send_bytes(&target, &b).await
                        && socket
                            .send(Message::Text(format!("[input dropped: {e}]").into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Err(e) = agentum_tmux::send_bytes(&target, t.as_bytes()).await
                        && socket
                            .send(Message::Text(format!("[input dropped: {e}]").into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    tail_handle.abort();
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
}
