//! WS `/api/events` — broadcasts watchdog + lifecycle events to the UI.
//!
//! On connect we replay one "current state" `agent.*` event per session
//! from the persisted log, marked `{"replay": true}`. That bootstraps a
//! fresh client (dashboard tab opened mid-flight, daemon restarted
//! under a long-lived TUI) with the activity overlay — without it a
//! `running` session whose agent finished pre-connect reads as a
//! misleading live green dot forever, because `agent.finished` only
//! fires on transition.
//!
//! After the snapshot we subscribe to the broadcast bus and stream
//! live events. Slow clients that fall behind by more than the bus
//! capacity see a single skipped-N marker and resume.

use std::sync::Arc;

use agentum_core::Event;
use agentum_store::Store;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/events", get(stream))
}

async fn stream(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    // Subscribe BEFORE the snapshot query so any event that fires
    // while we're loading historical state lands in the receiver
    // queue and gets delivered after the snapshot — no gap, no
    // duplicate.
    let rx = state.bus.subscribe();
    let store = state.store.clone();
    ws.on_upgrade(move |socket| run(socket, rx, store))
}

async fn run(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<Event>,
    store: Arc<Store>,
) {
    // Send the current-state snapshot first. Each event is tagged
    // `replay: true` so client toast handlers (which already
    // suppress during the RECONNECT_QUIET window) skip them
    // explicitly — the dot/awaiting state still updates because the
    // event itself is dispatched through the normal handler.
    match store.latest_agent_event_per_session().await {
        Ok(snapshot) => {
            for mut ev in snapshot {
                // Merge `replay: true` into payload without trashing
                // any existing fields (e.g. `initial: true` from an
                // Unknown→Idle event still survives).
                if let Some(obj) = ev.payload.as_object_mut() {
                    obj.insert("replay".to_string(), serde_json::Value::Bool(true));
                } else {
                    ev.payload = json!({"replay": true});
                }
                let payload = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    return;
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = ?e, "events snapshot query failed; client starts cold");
        }
    }

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let payload = serde_json::to_string(&ev)
                        .unwrap_or_else(|_| "{}".to_string());
                    if socket.send(Message::Text(payload.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    let payload = json!({
                        "kind": "bus.lagged",
                        "payload": { "skipped": skipped }
                    });
                    let s = payload.to_string();
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
