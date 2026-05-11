use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use agentum_core::{Event, NewSession, Session, Status};
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
use crate::StreamCheckpoint;
use crate::error::ApiError;

const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list).post(create))
        .route(
            "/api/sessions/{id}",
            get(get_one).patch(patch_session).delete(delete),
        )
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
    // Lazy-start a transcript watcher per known session so plan/todo
    // updates stream live. `ensure_started` is idempotent — calling it
    // for sessions that already have a watcher is cheap.
    for s in &rows {
        state
            .transcripts
            .ensure_started(s.id, PathBuf::from(&s.workdir), &s.tool);
    }
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
    name: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    flags: Option<Vec<String>>,
    #[serde(default)]
    model: Option<Option<String>>,
    #[serde(default)]
    pinned: Option<bool>,
}

async fn patch_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let running = matches!(session.status, Status::Running);

    // Rename is allowed even on a running session — it's pure metadata
    // (tmux target stays put because we keyed it by id at start time, not
    // by name).
    let mut current = session;
    if let Some(raw_name) = body.name.as_ref() {
        let name = raw_name.trim();
        if name.is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".into()));
        }
        if name.len() > 64 {
            return Err(ApiError::BadRequest("name too long (max 64 chars)".into()));
        }
        if name != current.name {
            let updated = state.store.patch_session_name(id, name).await?;
            let _ = state
                .bus
                .send(Event::new("session.renamed").with_session(updated.id, &updated.name));
            current = updated;
        }
    }

    // Tool patch is allowed at any time — the watchdog uses this path
    // to keep the chip in sync with the foreground process. UI clients
    // are also free to PATCH it manually if they want to relabel.
    if let Some(raw_tool) = body.tool.as_ref() {
        let tool = raw_tool.trim();
        if tool.is_empty() {
            return Err(ApiError::BadRequest("tool cannot be empty".into()));
        }
        if tool != current.tool {
            let updated = state.store.patch_session_tool(id, tool).await?;
            let _ = state.bus.send(
                Event::new("session.tool_changed")
                    .with_session(updated.id, &updated.name)
                    .with_payload(serde_json::json!({"tool": updated.tool})),
            );
            current = updated;
        }
    }

    // Pin toggle is metadata-only — works on running and stopped
    // sessions alike. Always emits `session.pinned` so multi-tab
    // dashboards stay in sync without polling.
    if let Some(pinned) = body.pinned
        && current.pinned != pinned
    {
        let updated = state.store.patch_session_pinned(id, pinned).await?;
        let _ = state.bus.send(
            Event::new("session.pinned")
                .with_session(updated.id, &updated.name)
                .with_payload(serde_json::json!({"pinned": updated.pinned})),
        );
        current = updated;
    }

    // Flags + model still require the session to be stopped — they
    // affect how the agent is launched and rewriting them under a live
    // process would be a footgun.
    if body.flags.is_some() || body.model.is_some() {
        if running {
            return Err(ApiError::BadRequest(
                "cannot patch flags / model on a running session; stop it first".into(),
            ));
        }
        if let Some(flags) = body.flags {
            current = state.store.patch_session_flags(id, &flags).await?;
        }
        if let Some(model) = body.model {
            // Future: patch model — not yet implemented in store
            let _ = model;
        }
    }
    Ok(Json(current))
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
    emit_stopped(&state, &session, "stop").await;
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
    emit_stopped(&state, &session, "kill").await;
    Ok(Json(load(&state, id).await?))
}

/// Persist + broadcast a `session.stopped` event so the UI gets a benign
/// "stopped" toast instead of letting the watchdog notice the dead pane on
/// its next tick and fire `session.crashed`. The watchdog itself sees the
/// `Stopped` status and bows out silently.
async fn emit_stopped(state: &AppState, session: &Session, reason: &str) {
    let ev = Event::new("session.stopped")
        .with_session(session.id, &session.name)
        .with_payload(serde_json::json!({"reason": reason}));
    if let Err(e) = state.store.insert_event(&ev).await {
        tracing::warn!(error = ?e, "could not persist session.stopped event");
    }
    // send() returns Err only when there are zero subscribers, which is
    // fine — the event is already in the persisted log.
    let _ = state.bus.send(ev);
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

#[derive(Deserialize, Default)]
struct StreamQuery {
    /// `?resume=true` tells the WS handler the client has cached vt100
    /// parser state for this session and just wants the missed log
    /// delta — not a fresh `capture-pane` snapshot. Travels in the URL
    /// because old daemons silently drop unknown query params; if it
    /// were a wire frame they'd forward it to `tmux send-keys` as a
    /// keystroke (the v0.6.20 regression).
    #[serde(default)]
    resume: bool,
}

async fn stream(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let target = tmux_target(&session);
    let positions = state.stream_positions.clone();
    let resume = q.resume;
    Ok(ws.on_upgrade(move |socket| stream_session(socket, id, target, positions, resume)))
}

const BACKFILL_BYTES: u64 = 4096;
const READ_CHUNK: usize = 8192;

/// How long the WS handler waits for the client's first `{"resize":...}` text
/// frame before falling back to capturing at tmux's current pane size. Modern
/// clients send this within milliseconds of `onopen` / first frame, so the
/// wait virtually never reaches the timeout. The cap exists so legacy
/// clients (or a stalled connection) don't block the connect indefinitely.
const INITIAL_RESIZE_WAIT: Duration = Duration::from_millis(250);

/// Hard cap on how long we wait for the embedded process's post-SIGWINCH
/// repaint burst to settle before snapshotting. Reached by genuinely
/// active streams (claude code mid-response) — at that point the
/// snapshot inevitably reflects mid-stream content, and the live tail
/// will continue painting fresh bytes after.
const POST_RESIZE_SETTLE_MAX: Duration = Duration::from_millis(400);

/// Polling cadence for the post-resize quiet check. Two consecutive
/// no-growth intervals (≈80 ms total) classifies the embedded TUI as
/// "done repainting".
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(40);

/// If we resized but see no log activity at all within this window, the
/// embedded process probably isn't going to react (size already matched,
/// or it's blocked on something). Bail rather than burning the full
/// max-budget on a no-op wait. Long enough that a slow ratatui app has
/// time to start emitting bytes.
const POST_RESIZE_NO_ACTIVITY_BAIL: Duration = Duration::from_millis(180);

async fn stream_session(
    mut socket: WebSocket,
    id: Uuid,
    target: String,
    positions: Arc<std::sync::Mutex<std::collections::HashMap<Uuid, StreamCheckpoint>>>,
    resume_requested: bool,
) {
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

    // Resize tmux to match the client's viewport BEFORE we snapshot. Without
    // this we capture-pane at tmux's stale pane size (80×24 default for fresh
    // detached sessions, or whatever the previous client sat at), the embedded
    // TUI keeps emitting cursor-position escapes against that size, and the
    // client's vt100 parser — sized to the actual viewport — places the
    // characters in the wrong cells. Symptom: status-line text like "esc to
    // interrupt" overpaints scrollback content and you end up with artefacts
    // like `okterrupt` permanently baked into the scrollback buffer.
    //
    // Modern clients (TUI ≥ 0.6.7, dashboard ≥ 0.6.7) push a `{"resize":...}`
    // text frame within milliseconds of WS open, so this wait almost never
    // reaches the timeout in practice. Old clients fall through to the
    // existing capture-at-current-size path.
    let mut early_input: Vec<Bytes> = Vec::new();
    let mut got_resize = false;
    // Captured on the first resize frame so the resume-replay path can
    // bail out when the client's viewport changed during a disconnect
    // — replaying bytes emitted at a different grid produces visible
    // layout corruption (cursor moves target stale cells).
    let mut current_size: Option<(u16, u16)> = None;
    let resize_deadline = tokio::time::Instant::now() + INITIAL_RESIZE_WAIT;
    loop {
        let remaining = resize_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                if let Some((cols, rows)) = parse_resize(&t) {
                    let _ = agentum_tmux::resize_window(&target, cols, rows).await;
                    got_resize = true;
                    current_size = Some((cols, rows));
                    break;
                }
                // Non-resize text frame — preserve the legacy "treat as raw
                // input" behaviour by buffering for replay after the snapshot.
                early_input.push(Bytes::copy_from_slice(t.as_bytes()));
            }
            Ok(Some(Ok(Message::Binary(b)))) if !b.is_empty() => {
                early_input.push(b);
            }
            Ok(Some(Ok(_))) => {}                  // ping/pong/etc.
            Ok(Some(Err(_))) | Ok(None) => return, // socket already gone
            Err(_) => break,                       // timeout — fall through with no resize
        }
    }
    if got_resize {
        // Wait for the embedded process's post-SIGWINCH repaint burst to
        // settle before snapshotting. Fixed sleeps don't work: idle panes
        // are quiet immediately, but a ratatui-based agent (claude code,
        // codex, opencode) reacting to a real size change can take well
        // over 100 ms to start emitting its full repaint, then several
        // dozen ms more to finish it. Capturing during that window
        // returned a half-painted frame — tool indicator drawn but
        // input box / footer missing, or status-line characters
        // overpainting scrollback content because cursor moves still
        // referenced the old grid.
        //
        // The pane log file (pipe-pane sink) gives us a cheap activity
        // probe: bytes the embedded process emits are appended in real
        // time, so file-size growth is direct evidence of repaint
        // activity. Wait for activity to start, then for it to quiet.
        // Fall back to a "no activity" bail-out if the resize was a
        // no-op (size already matched), so connect doesn't pay the full
        // budget for a settle that will never come.
        let mut last_size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
        let mut activity_seen = false;
        let mut quiet_streak: u32 = 0;
        let start = tokio::time::Instant::now();
        let max_deadline = start + POST_RESIZE_SETTLE_MAX;
        loop {
            sleep(SETTLE_POLL_INTERVAL).await;
            let now_size = file.metadata().await.map(|m| m.len()).unwrap_or(last_size);
            if now_size != last_size {
                activity_seen = true;
                quiet_streak = 0;
                last_size = now_size;
            } else {
                quiet_streak = quiet_streak.saturating_add(1);
            }
            let now = tokio::time::Instant::now();
            // Activity → quiet: repaint burst is over, capture is safe.
            if activity_seen && quiet_streak >= 2 {
                break;
            }
            // No activity within the bail window: probably a no-op resize
            // (size already matched, no SIGWINCH propagated). Don't burn
            // the full max budget waiting for a wake-up that won't come.
            if !activity_seen && now >= start + POST_RESIZE_NO_ACTIVITY_BAIL {
                break;
            }
            // Hard cap so an actively-streaming agent can't hold connect
            // open indefinitely.
            if now >= max_deadline {
                break;
            }
        }
    }

    // Replay path. Two modes:
    //
    // 1. RESUME: client has cached parser state and just wants the bytes
    //    it missed during the WS gap. Look up the saved log position and
    //    forward `log[saved..end]` as binary. The client's parser was
    //    not reset, so playing back those bytes brings it from the
    //    pre-disconnect state to the live tail without clobbering
    //    anything. Without this, switching agents and switching back
    //    used to wipe all visible chat history because we'd send a
    //    `capture-pane` snapshot reflecting whatever the embedded TUI's
    //    UI looks like *now* — which after a task completes can be
    //    almost empty.
    //
    // 2. FRESH SNAPSHOT (default): client has no cached state (or its
    //    cached state is invalid because the pane size changed during
    //    the disconnect), so we `capture-pane -e` the current visible
    //    grid and ship it after an `ESC c` (RIS) full parser reset.
    let mut snapshot_sent = false;
    // Resume only if we have a saved checkpoint AND its pane size matches
    // the current viewport. Two guards:
    //
    //  1. `unwrap_or(0)` after a daemon restart wiped `stream_positions`
    //     (in-memory only) made the daemon ship the ENTIRE log as
    //     "delta" on top of the client's existing parser state —
    //     duplicate footer/content baked into scrollback every time
    //     the daemon was bounced. v0.6.26 fixed this by falling through
    //     to the fresh snapshot path when no checkpoint exists.
    //
    //  2. Replaying bytes emitted at a stale grid size (e.g., user
    //     dragged their tmux window during the disconnect) places
    //     cursor moves and line wraps against the wrong cells, so the
    //     visible layout ends up corrupted in ways that survive in the
    //     vt100 parser's history. Mismatch → fall through to a fresh
    //     snapshot at the new size and let the client's parser reset.
    let saved_checkpoint: Option<StreamCheckpoint> =
        positions.lock().ok().and_then(|map| map.get(&id).copied());
    let resume_size_matches = match (saved_checkpoint, current_size) {
        (Some(cp), Some((cols, rows))) => cp.cols == cols && cp.rows == rows,
        // No first-resize frame from the client, or no checkpoint — let
        // the existing "resume only with checkpoint" gate handle it.
        (Some(_), None) => true,
        _ => false,
    };
    if let (true, Some(cp), true) = (resume_requested, saved_checkpoint, resume_size_matches) {
        if let Ok(end) = file.seek(std::io::SeekFrom::End(0)).await
            && end >= cp.pos
        {
            let delta = end - cp.pos;
            if delta > 0 && file.seek(std::io::SeekFrom::Start(cp.pos)).await.is_ok() {
                let mut buf = vec![0u8; delta as usize];
                if file.read_exact(&mut buf).await.is_ok()
                    && socket
                        .send(Message::Binary(Bytes::from(buf)))
                        .await
                        .is_err()
                {
                    return;
                }
            }
            // Position file at end so tail picks up only post-delta bytes.
            let _ = file.seek(std::io::SeekFrom::End(0)).await;
            snapshot_sent = true;
        }
    }
    if !snapshot_sent
        && let Ok(snap) = agentum_tmux::capture_pane_ansi(&target).await
        && !snap.is_empty()
    {
        // Reset the client parser before painting the snapshot so EVERY
        // bit of stale state from the previous session is discarded —
        // not just the visible grid.
        //
        //   ESC c (RIS, "Reset to Initial State")
        //
        // This is more thorough than the previous `\x1b[2J\x1b[H`
        // (clear-screen + cursor-home), which left SGR colors, saved
        // cursor positions, scroll regions, alternate-screen state,
        // application keypad/cursor mode, and mouse-tracking modes
        // intact. Carrying any of those across a session-switch (or
        // a crash-and-resume on a different agent type) showed up as
        // hard-to-pin-down corruption: text in the wrong color long
        // after the agent stopped emitting that SGR, scroll regions
        // clipping vt100-parser updates to a strip of the screen,
        // mouse events firing on a session that never asked for them.
        let mut payload = Vec::with_capacity(snap.len() + 4);
        payload.extend_from_slice(b"\x1bc");
        payload.extend_from_slice(&snap);
        if socket
            .send(Message::Binary(Bytes::from(payload)))
            .await
            .is_err()
        {
            return;
        }
        snapshot_sent = true;
        // Skip past the existing log so tailing only emits NEW bytes —
        // otherwise the snapshot and the log replay would render the same
        // content twice (cosmetic, but visible as flicker).
        let _ = file.seek(std::io::SeekFrom::End(0)).await;
    }

    // Fallback: if capture-pane didn't yield anything (early in session
    // life, before tmux has rendered, or for non-tmux sessions), keep the
    // old 4 KB tail behaviour so users still see *something* on connect.
    if !snapshot_sent && let Ok(end) = file.seek(std::io::SeekFrom::End(0)).await {
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

    // Replay any non-resize input that arrived during the resize-wait window.
    // Rare in practice (a fast typer connecting and hammering keys before the
    // first frame fires), but preserves the previous "every byte forwarded"
    // contract so we never silently drop a keystroke at connect time.
    for chunk in early_input {
        let _ = agentum_tmux::send_bytes(&target, &chunk).await;
    }

    // Tail the pane log on a dedicated task and pipe chunks through an mpsc.
    // The main loop multiplexes `tail_rx` (output) and `socket.recv()` (input)
    // so a chatty pane never starves keystrokes — and vice versa.
    //
    // We also remember the log position the tail starts from so the outer
    // loop can save where the client left off on disconnect — that's what
    // makes `{"resume":true}` reconnects deliver only the missed delta.
    let tail_start_pos = file.stream_position().await.unwrap_or(0);
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

    let mut bytes_forwarded: u64 = 0;
    loop {
        tokio::select! {
            chunk = tail_rx.recv() => match chunk {
                Some(bytes) => {
                    let len = bytes.len() as u64;
                    if socket.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                    bytes_forwarded += len;
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
                    // Text frames double as a side-channel for control
                    // messages — currently only `{"resize":{"cols":N,"rows":N}}`.
                    // Anything that isn't a recognised JSON envelope is
                    // forwarded as raw input bytes (preserves the old
                    // behaviour for clients that send keystrokes as text).
                    if let Some((cols, rows)) = parse_resize(&t) {
                        // Track every successful resize so the disconnect
                        // checkpoint records the size at the time the
                        // client actually left, not the size we captured
                        // in the early-input window.
                        current_size = Some((cols, rows));
                        if let Err(e) = agentum_tmux::resize_window(&target, cols, rows).await
                            && socket
                                .send(Message::Text(format!("[resize dropped: {e}]").into()))
                                .await
                                .is_err()
                        {
                            break;
                        }
                    } else if let Err(e) = agentum_tmux::send_bytes(&target, t.as_bytes()).await
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
    // Save the log position + pane size the next reconnect should
    // resume from. We started tailing at `tail_start_pos` and the outer
    // loop forwarded `bytes_forwarded` bytes to the client, so the
    // file position to pick up from is the sum. The size is whatever
    // the client last asked for (defaulting to 0×0 if they never sent
    // a resize, in which case the size-mismatch gate will force a
    // fresh snapshot on next connect — the safe choice).
    if let Ok(mut map) = positions.lock() {
        let (cols, rows) = current_size.unwrap_or((0, 0));
        map.insert(
            id,
            StreamCheckpoint {
                pos: tail_start_pos.saturating_add(bytes_forwarded),
                cols,
                rows,
            },
        );
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// Recognise `{"resize":{"cols":N,"rows":N}}` text frames. Returns the
/// `(cols, rows)` pair on a hit; `None` for any other shape (the caller
/// then treats the frame as raw input bytes for backward compatibility).
fn parse_resize(t: &str) -> Option<(u16, u16)> {
    let trimmed = t.trim();
    if !trimmed.starts_with('{') || !trimmed.contains("resize") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let resize = v.get("resize")?;
    let cols = resize.get("cols")?.as_u64()?;
    let rows = resize.get("rows")?.as_u64()?;
    Some((
        cols.min(u16::MAX as u64) as u16,
        rows.min(u16::MAX as u64) as u16,
    ))
}

#[cfg(test)]
mod tests {
    use super::parse_resize;

    #[test]
    fn parse_resize_recognises_envelope() {
        assert_eq!(
            parse_resize(r#"{"resize":{"cols":120,"rows":40}}"#),
            Some((120, 40))
        );
    }

    #[test]
    fn parse_resize_ignores_other_text() {
        assert_eq!(parse_resize("hello"), None);
        assert_eq!(parse_resize(r#"{"send":"x"}"#), None);
        assert_eq!(parse_resize(""), None);
    }
}
