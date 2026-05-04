//! WS `/api/events` — broadcasts watchdog + lifecycle events to the UI.
//!
//! Each connected client gets its own `broadcast::Receiver`. Slow clients
//! that fall behind by more than the bus capacity see a single skipped-N
//! marker and resume; we don't try to backfill from the persisted log here
//! (REST `/api/events` history endpoint can fill that role later).

use agentum_core::Event;
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
    let rx = state.bus.subscribe();
    ws.on_upgrade(move |socket| run(socket, rx))
}

async fn run(mut socket: WebSocket, mut rx: tokio::sync::broadcast::Receiver<Event>) {
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
