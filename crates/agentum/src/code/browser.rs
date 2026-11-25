//! The browser's one WebSocket, `/api/ws` (signed in by its cookie, like every /api route).
//! It subscribes to channels and gets every message published on them; terminal keystrokes go
//! the other way, straight to the machine.
use crate::state::App;
use agentum_proto::api::{WsIn, WsOut};
use agentum_proto::machine::{Req, TermReq};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub async fn upgrade(State(app): State<App>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |s| run(app, s))
}

async fn run(app: App, sock: WebSocket) {
    let (mut sink, mut stream) = sock.split();
    let subs: Arc<Mutex<HashSet<String>>> = Arc::default();
    let mut rx = app.code.out.subscribe();
    let mut app_rx = app.events.subscribe();
    let (tx, mut own) = tokio::sync::mpsc::unbounded_channel::<String>();
    let s2 = subs.clone();
    let writer = tokio::spawn(async move {
        loop {
            let text = tokio::select! {
                m = rx.recv() => match m {
                    Ok(o) => if s2.lock().unwrap().contains(&o.0) { o.1.clone() } else { continue },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                m = app_rx.recv() => match m {
                    Ok(v) => if s2.lock().unwrap().contains("app") { serde_json::to_string(&WsOut::App { payload: v }).unwrap_or_default().into() } else { continue },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                m = own.recv() => match m { Some(t) => t.into(), None => break },
            };
            if sink.send(Message::Text(text.to_string().into())).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(t) = msg else {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(m) = serde_json::from_str::<WsIn>(&t) else { continue };
        match m {
            WsIn::Sub { ch } => subs.lock().unwrap().extend(ch),
            WsIn::Unsub { ch } => {
                let mut s = subs.lock().unwrap();
                for c in ch {
                    s.remove(&c);
                }
            }
            WsIn::TermInput { machine, term, data } => {
                if let Some(c) = app.code.conn(&machine) {
                    c.fire(Req::Term(TermReq::Input { term, data }));
                }
            }
            WsIn::TermResize { machine, term, cols, rows } => {
                if let Some(c) = app.code.conn(&machine) {
                    c.fire(Req::Term(TermReq::Resize { term, cols, rows }));
                }
            }
            WsIn::Ping => {
                let _ = tx.send(serde_json::to_string(&WsOut::Pong).unwrap_or_default());
            }
        }
    }
    writer.abort();
}
