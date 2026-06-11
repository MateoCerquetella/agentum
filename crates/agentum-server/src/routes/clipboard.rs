//! `/api/clipboard` — broker that hops a clipboard request from a remote
//! TUI to a long-running clipboard agent running on the host that owns
//! the user's local clipboard.
//!
//! Two surfaces:
//!
//! - `GET /api/clipboard/agent` (WS) — every `agentum clip-agent` opens
//!   one of these per daemon profile. The handler subscribes to
//!   `state.clipboard_request_bus` and forwards each frame as JSON
//!   `{"type":"clipboard_request", "request_id":"…", "session_id":"…"}`.
//!   The agent reads the local OS clipboard, encodes a PNG, and POSTs
//!   it to the existing `/api/sessions/{id}/uploads` route with an
//!   `X-Clipboard-Request-Id: <uuid>` header so the broker can match
//!   the upload to a pending request. On "no image" it sends back a
//!   plain WS frame `{"type":"no_image","request_id":"…"}` so the
//!   broker can short-circuit the timer instead of letting the TUI
//!   wait the full timeout.
//!
//! - `POST /api/clipboard/request` — every TUI that gets a Ctrl-V hits
//!   this. It fast-fails with 503 `agent_not_connected` when no agent
//!   is subscribed; otherwise it inserts a oneshot into
//!   `state.clipboard_pending`, broadcasts a request frame, and waits
//!   up to `timeout_ms` for either the upload or a no-image / timeout
//!   resolution.
//!
//! Auth: NOT public. Both routes ride the bearer-token middleware that
//! wraps the whole router merge in `lib.rs::router()`. The WS upgrade
//! pulls the bearer from `?token=` because browsers can't set headers
//! on upgrade (same pattern `events.rs` already uses — that's NOT a
//! public-route exception).

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, extract::Json as JsonExtract};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

/// Hard cap on `timeout_ms` so a buggy or malicious client can't park
/// a oneshot for arbitrary time. 10 s is comfortably above the
/// expected p99 round trip (local clipboard read + PNG encode + upload
/// on a residential connection lands well under 2 s for typical
/// screenshots).
const MAX_TIMEOUT_MS: u64 = 10_000;

/// Outcome the broker hands back to the waiting TUI when a clipboard
/// agent resolves a pending request — either the agent uploaded
/// the image (and the uploads route woke us up via
/// `state.clipboard_pending`) or it reported that the user's
/// clipboard had no image to grab.
#[derive(Debug, Clone)]
pub enum ClipboardOutcome {
    Uploaded {
        path: String,
        relative_path: String,
        /// `u64` to match `routes::uploads::UploadResponse.size_bytes`.
        /// Same type all the way to the TUI's `UploadResponse` shim so
        /// no downstream cast is needed.
        size_bytes: u64,
    },
    NoImage,
}

/// Wire frame broadcast on `state.clipboard_request_bus`. One per
/// TUI `POST /request`; consumed by every connected agent (the first
/// to upload wins, see `tests_helpers_complete_clipboard_request`).
#[derive(Debug, Clone, Serialize)]
pub struct ClipboardRequestFrame {
    pub request_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct RequestBody {
    session_id: Uuid,
    /// Client-supplied timeout. Capped at `MAX_TIMEOUT_MS`. Default
    /// 3 s matches the TUI's typical p95 expectation (X11 + Wayland
    /// selection negotiation + JPEG/PNG encode + LAN round trip).
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    3000
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/clipboard/agent", get(agent_ws))
        .route("/api/clipboard/request", post(request))
}

/// `GET /api/clipboard/agent` — clipboard agent attaches over WS.
async fn agent_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let rx = state.clipboard_request_bus.subscribe();
    ws.on_upgrade(move |socket| run_agent(socket, rx, state))
}

async fn run_agent(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<ClipboardRequestFrame>,
    state: AppState,
) {
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(frame) => {
                    let payload = json!({
                        "type": "clipboard_request",
                        "request_id": frame.request_id.to_string(),
                        "session_id": frame.session_id.to_string(),
                    });
                    let s = payload.to_string();
                    if socket.send(Message::Text(s.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    // Mirror events.rs: log + keep the WS open so a
                    // freshly subscribed agent receives the next
                    // request cleanly even after a burst that
                    // overflowed the broadcast capacity.
                    tracing::warn!(skipped, "clipboard request bus lagged");
                    continue;
                }
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => {
                    // Parse loosely — unknown frame types are ignored so
                    // the wire contract stays forward-compatible.
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
                        continue;
                    };
                    let kind = v.get("type").and_then(|k| k.as_str()).unwrap_or("");
                    match kind {
                        "ack" => {} // ignored
                        "no_image" => {
                            if let Some(rid) = v.get("request_id").and_then(|r| r.as_str())
                                && let Ok(uuid) = Uuid::parse_str(rid) {
                                    tests_helpers_complete_clipboard_request(
                                        &state,
                                        uuid,
                                        ClipboardOutcome::NoImage,
                                    );
                                }
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            }
        }
    }
}

/// `POST /api/clipboard/request` — TUI asks the broker for an image.
async fn request(
    State(state): State<AppState>,
    JsonExtract(body): JsonExtract<RequestBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Fast-fail BEFORE allocating a oneshot. The receiver_count
    // check is racy with the broadcast::send below — covered by the
    // post-send fallback path. Either way the user gets a clean 503
    // in milliseconds instead of staring at a 3 s wait for nothing.
    if state.clipboard_request_bus.receiver_count() == 0 {
        return Err(ApiError::Custom(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "no clipboard agent connected",
                "kind": "agent_not_connected",
            }),
        ));
    }

    let timeout_ms = body.timeout_ms.min(MAX_TIMEOUT_MS);
    let request_id = Uuid::new_v4();
    let (tx, rx) = oneshot::channel::<ClipboardOutcome>();
    {
        let mut pending = state
            .clipboard_pending
            .lock()
            .expect("clipboard pending mutex poisoned");
        pending.insert(request_id, tx);
    }

    let frame = ClipboardRequestFrame {
        request_id,
        session_id: body.session_id,
    };
    if state.clipboard_request_bus.send(frame).is_err() {
        // Race with receiver_count: the last agent disconnected
        // between our check and the send. Reclaim the slot and
        // surface the same 503 the early-exit returned.
        let mut pending = state
            .clipboard_pending
            .lock()
            .expect("clipboard pending mutex poisoned");
        pending.remove(&request_id);
        return Err(ApiError::Custom(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "no clipboard agent connected",
                "kind": "agent_not_connected",
            }),
        ));
    }

    match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
        Ok(Ok(ClipboardOutcome::Uploaded {
            path,
            relative_path,
            size_bytes,
        })) => Ok(Json(json!({
            "path": path,
            "relative_path": relative_path,
            "size_bytes": size_bytes,
        }))),
        Ok(Ok(ClipboardOutcome::NoImage)) => Err(ApiError::Custom(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "no image in clipboard",
                "kind": "no_image",
            }),
        )),
        Ok(Err(_canceled)) => {
            // Sender dropped before sending — agent crashed mid-request.
            // The pending entry has already been removed by whoever
            // dropped the tx (or it never will be, in which case we
            // remove it here defensively).
            let mut pending = state
                .clipboard_pending
                .lock()
                .expect("clipboard pending mutex poisoned");
            pending.remove(&request_id);
            Err(ApiError::Internal(
                "clipboard agent dropped the request without responding".into(),
            ))
        }
        Err(_elapsed) => {
            // Timer fired. CRITICAL: remove the pending entry BEFORE
            // returning so a late upload doesn't leak a oneshot into
            // the map (the uploads route's send-to-closed-channel
            // would be a no-op anyway, but the map would still hold
            // a dead Sender forever otherwise).
            let mut pending = state
                .clipboard_pending
                .lock()
                .expect("clipboard pending mutex poisoned");
            pending.remove(&request_id);
            Err(ApiError::Custom(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({
                    "error": "clipboard agent did not respond in time",
                    "kind": "timeout",
                }),
            ))
        }
    }
}

/// Resolve a pending clipboard request. Used by:
/// - the uploads route, when an upload arrives with a matching
///   `X-Clipboard-Request-Id` header (sends `Uploaded`),
/// - this module's WS handler, when an agent sends a `no_image` frame,
/// - and the test suite, to simulate either outcome without spinning
///   a real WS.
///
/// Missing-entry is a deliberate no-op: when two agents race to fulfil
/// the same request, the loser's call finds an empty slot — that's
/// the intended "first wins" semantics, not an error.
pub(crate) fn tests_helpers_complete_clipboard_request(
    state: &AppState,
    request_id: Uuid,
    outcome: ClipboardOutcome,
) {
    let tx = {
        let mut pending = state
            .clipboard_pending
            .lock()
            .expect("clipboard pending mutex poisoned");
        pending.remove(&request_id)
    };
    if let Some(tx) = tx {
        // Receiver may already be gone (timeout fired between our
        // remove and this send) — that's fine, send errors are
        // discarded.
        let _ = tx.send(outcome);
    }
}

#[cfg(test)]
mod tests {
    //! Handler-level tests for the clipboard broker.
    //!
    //! Auth middleware is verified at the lib.rs::router() merge site
    //! (top-level `require_token` layer); this harness calls handlers
    //! through `tower::ServiceExt::oneshot` so the middleware is
    //! exercised end-to-end only at integration-test scale.

    use super::*;
    use crate::AppState;
    use crate::TranscriptStore;
    use agentum_store::Store;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    async fn fresh_state() -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        // Leak the tempdir for the lifetime of the test — Store
        // holds an SqlitePool that needs the file to exist past the
        // helper's return.
        std::mem::forget(dir);
        let store = Store::open(&p).await.unwrap();
        let (bus, _rx) = broadcast::channel(16);
        let (clip_bus, _crx) = broadcast::channel::<ClipboardRequestFrame>(64);
        AppState {
            store: Arc::new(store),
            bus,
            started_at: Instant::now(),
            version: "test",
            auth_limiter: Arc::new(crate::ratelimit::RateLimiter::new(
                8,
                Duration::from_secs(60),
            )),
            cert_fingerprint: Arc::new(String::new()),
            transcripts: TranscriptStore::new(broadcast::channel(16).0),
            stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: "test".to_string(),
            no_auth: true,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus: clip_bus,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            api_base_url: None,
        }
    }

    fn app(state: AppState) -> axum::Router {
        super::router().with_state(state)
    }

    fn req_body(session_id: Uuid, timeout_ms: u64) -> Request<Body> {
        let body = json!({
            "session_id": session_id.to_string(),
            "timeout_ms": timeout_ms,
        })
        .to_string();
        Request::builder()
            .method("POST")
            .uri("/api/clipboard/request")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn request_503_when_no_agent_connected() {
        let state = fresh_state().await;
        // Sanity: no subscribers right after a fresh broadcast channel.
        assert_eq!(state.clipboard_request_bus.receiver_count(), 0);
        let start = Instant::now();
        let resp = app(state.clone())
            .oneshot(req_body(Uuid::new_v4(), 5000))
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            elapsed.as_millis() < 50,
            "fast-fail expected, got {elapsed:?}"
        );
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["kind"], "agent_not_connected");
        assert!(state.clipboard_pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn request_succeeds_when_agent_uploads() {
        let state = fresh_state().await;
        // Subscribe a fake agent and wire it to complete the request
        // with an `Uploaded` outcome the moment the frame lands.
        let mut rx = state.clipboard_request_bus.subscribe();
        let agent_state = state.clone();
        tokio::spawn(async move {
            if let Ok(frame) = rx.recv().await {
                tests_helpers_complete_clipboard_request(
                    &agent_state,
                    frame.request_id,
                    ClipboardOutcome::Uploaded {
                        path: "/tmp/x.png".into(),
                        relative_path: ".agentum-uploads/a.png".into(),
                        size_bytes: 42u64,
                    },
                );
            }
        });

        let resp = app(state)
            .oneshot(req_body(Uuid::new_v4(), 3000))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["path"], "/tmp/x.png");
        assert_eq!(v["relative_path"], ".agentum-uploads/a.png");
        assert_eq!(v["size_bytes"], 42);
    }

    #[tokio::test]
    async fn request_fast_fails_on_no_image_frame() {
        let state = fresh_state().await;
        let mut rx = state.clipboard_request_bus.subscribe();
        let agent_state = state.clone();
        tokio::spawn(async move {
            if let Ok(frame) = rx.recv().await {
                tests_helpers_complete_clipboard_request(
                    &agent_state,
                    frame.request_id,
                    ClipboardOutcome::NoImage,
                );
            }
        });
        let start = Instant::now();
        let resp = app(state)
            .oneshot(req_body(Uuid::new_v4(), 3000))
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            elapsed.as_millis() < 500,
            "fast-fail expected, got {elapsed:?}"
        );
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["kind"], "no_image");
    }

    #[tokio::test]
    async fn request_times_out_when_agent_silent() {
        let state = fresh_state().await;
        // Subscribe a fake agent but never respond — the broker's
        // timeout path is the one under test.
        let _rx = state.clipboard_request_bus.subscribe();
        let resp = app(state.clone())
            .oneshot(req_body(Uuid::new_v4(), 100))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["kind"], "timeout");
        // Critical: no leak in clipboard_pending after the timer fires.
        assert!(state.clipboard_pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn request_two_agents_first_wins() {
        let state = fresh_state().await;
        let mut rx1 = state.clipboard_request_bus.subscribe();
        let mut rx2 = state.clipboard_request_bus.subscribe();

        let agent_state_a = state.clone();
        tokio::spawn(async move {
            if let Ok(frame) = rx1.recv().await {
                // First agent wins — uploads immediately.
                tests_helpers_complete_clipboard_request(
                    &agent_state_a,
                    frame.request_id,
                    ClipboardOutcome::Uploaded {
                        path: "/tmp/first.png".into(),
                        relative_path: ".agentum-uploads/first.png".into(),
                        size_bytes: 7,
                    },
                );
            }
        });

        let agent_state_b = state.clone();
        tokio::spawn(async move {
            if let Ok(frame) = rx2.recv().await {
                // Second agent arrives after the slot is gone — the
                // helper treats missing entries as no-ops, exactly
                // the semantics we want for the "loser" path.
                tokio::time::sleep(Duration::from_millis(50)).await;
                tests_helpers_complete_clipboard_request(
                    &agent_state_b,
                    frame.request_id,
                    ClipboardOutcome::Uploaded {
                        path: "/tmp/second.png".into(),
                        relative_path: ".agentum-uploads/second.png".into(),
                        size_bytes: 999,
                    },
                );
            }
        });

        let resp = app(state)
            .oneshot(req_body(Uuid::new_v4(), 3000))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["path"], "/tmp/first.png");
        assert_eq!(v["size_bytes"], 7);
    }

    #[tokio::test]
    async fn clipboard_request_bus_recovers_from_lag() {
        let state = fresh_state().await;
        // Subscribe early so the broadcaster keeps the slow consumer
        // around; then overflow capacity-64 by sending 65 frames
        // without recv-ing on it.
        let mut slow = state.clipboard_request_bus.subscribe();
        for _ in 0..65 {
            // ignore send errors (e.g. when only the slow rx
            // is around) — what matters is the channel survives.
            let _ = state.clipboard_request_bus.send(ClipboardRequestFrame {
                request_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
            });
        }
        // Subscribe a fresh agent AFTER the lag.
        let mut fresh = state.clipboard_request_bus.subscribe();
        let agent_state = state.clone();
        tokio::spawn(async move {
            if let Ok(frame) = fresh.recv().await {
                tests_helpers_complete_clipboard_request(
                    &agent_state,
                    frame.request_id,
                    ClipboardOutcome::Uploaded {
                        path: "/tmp/post-lag.png".into(),
                        relative_path: ".agentum-uploads/post-lag.png".into(),
                        size_bytes: 1,
                    },
                );
            }
        });
        // The slow subscriber should see RecvError::Lagged on its
        // next recv but the channel keeps working for the fresh
        // subscriber.
        let lag_err = slow.try_recv();
        assert!(matches!(
            lag_err,
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));

        let resp = app(state)
            .oneshot(req_body(Uuid::new_v4(), 3000))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
