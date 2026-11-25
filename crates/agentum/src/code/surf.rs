//! The projects' browsers (Surf): their live viewer, shown inside the app.
//!
//! Steel listens on its machine's 127.0.0.1 only, so the hub serves the viewer itself, under
//! `/api/p/browser/view/<machine>/<id>/…` (an /api route: the person's session cookie is checked
//! like everywhere else). Its page comes through agentd ([`SurfReq::Get`]) with Steel's own
//! address (`ws://127.0.0.1:<port>`) rewritten to this path, and its screencast WebSocket is
//! relayed message by message over agentd's connection ([`SurfReq::Open`], `Sock` frames).
use super::machine_call;
use crate::state::App;
use agentum_proto::machine::{FromAgentd, Req, SockMsg, SurfReq, ToAgentd};
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;

/// Messages waiting for a slow page, per viewer socket (screencast frames, mostly).
const BUFFER: usize = 256;

pub fn routes() -> Router<App> {
    Router::new().route("/api/p/browser/view/{machine}/{id}/{*rest}", get(view))
}

fn header<'a>(h: &'a HeaderMap, k: &str) -> Option<&'a str> {
    h.get(k).and_then(|v| v.to_str().ok()).map(|v| v.split(',').next().unwrap_or("").trim()).filter(|v| !v.is_empty())
}

async fn view(
    State(app): State<App>,
    Path((machine, id, rest)): Path<(String, String, String)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let path = format!("/{rest}{}", q.map(|q| format!("?{q}")).unwrap_or_default());
    match ws {
        // Only from this app's own pages (the session cookie is SameSite=Lax too).
        Ok(_) if !same_origin(&headers) => (StatusCode::FORBIDDEN, "cross-origin").into_response(),
        Ok(ws) if rest == "v1/sessions/cast" => open(app, machine, id, path, ws).await,
        Err(_) if rest == "v1/sessions/debug" => page(app, machine, id, path, &headers).await,
        _ => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn same_origin(h: &HeaderMap) -> bool {
    let Some(origin) = header(h, "origin") else { return true };
    let host = header(h, "x-forwarded-host").or(header(h, "host")).unwrap_or("");
    origin.split_once("://").map(|(_, o)| o) == Some(host)
}

/// The viewer's page, pointed at this hub.
async fn page(app: App, machine: String, id: String, path: String, headers: &HeaderMap) -> Response {
    let r = match machine_call(&app, &machine, Req::Surf(SurfReq::Get { id: id.clone(), path })).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("The browser isn't reachable: {e}")).into_response(),
    };
    let status = StatusCode::from_u16(r["status"].as_u64().unwrap_or(502) as u16).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = r["body"].as_str().unwrap_or("");
    let origin = r["origin"].as_str().unwrap_or("");
    let host = header(headers, "x-forwarded-host").or(header(headers, "host")).unwrap_or("localhost");
    let https = header(headers, "x-forwarded-proto") == Some("https");
    let base = format!("{host}/api/p/browser/view/{machine}/{id}");
    let body = if origin.is_empty() {
        body.to_string()
    } else {
        body.replace(&format!("ws://{origin}"), &format!("{}://{base}", if https { "wss" } else { "ws" }))
            .replace(&format!("http://{origin}"), &format!("{}://{base}", if https { "https" } else { "http" }))
    };
    (
        status,
        [
            ("content-type", "text/html; charset=utf-8"),
            ("cache-control", "no-store"),
            ("content-security-policy", "frame-ancestors 'self'"),
            ("referrer-policy", "no-referrer"),
        ],
        body,
    )
        .into_response()
}

fn new_sock() -> String {
    use rand::Rng;
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut r = rand::rng();
    format!("s_{}", (0..16).map(|_| A[r.random_range(0..A.len())] as char).collect::<String>())
}

/// A screencast socket: agentd opens Steel's, then the two are relayed.
async fn open(app: App, machine: String, id: String, path: String, ws: WebSocketUpgrade) -> Response {
    let Some(conn) = app.code.conn(&machine) else { return (StatusCode::CONFLICT, format!("{machine} is offline")).into_response() };
    let sock = new_sock();
    let (tx, rx) = mpsc::channel::<SockMsg>(BUFFER);
    app.code.socks.lock().unwrap().insert(sock.clone(), (machine.clone(), tx));
    if let Err(e) = machine_call(&app, &machine, Req::Surf(SurfReq::Open { id, sock: sock.clone(), path })).await {
        app.code.socks.lock().unwrap().remove(&sock);
        return (StatusCode::BAD_GATEWAY, format!("The browser isn't reachable: {e}")).into_response();
    }
    ws.max_message_size(32 << 20).on_upgrade(move |s| relay(app, conn, sock, s, rx))
}

async fn relay(app: App, conn: std::sync::Arc<super::Conn>, sock: String, ws: WebSocket, mut rx: mpsc::Receiver<SockMsg>) {
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            m = rx.recv() => {
                let Some(m) = m else { break };
                let msg = match (m.text, m.data) {
                    (Some(t), _) => Message::Text(t.into()),
                    (None, Some(d)) => {
                        use base64::Engine;
                        match base64::engine::general_purpose::STANDARD.decode(d) {
                            Ok(b) => Message::Binary(b.into()),
                            Err(_) => continue,
                        }
                    }
                    _ => continue,
                };
                if tokio::time::timeout(Duration::from_secs(30), sink.send(msg)).await.map_or(true, |r| r.is_err()) {
                    break;
                }
            }
            m = stream.next() => {
                let m = match m {
                    Some(Ok(m)) => m,
                    _ => break,
                };
                let out = match m {
                    Message::Text(t) => SockMsg { sock: sock.clone(), text: Some(t.to_string()), data: None },
                    Message::Binary(b) => {
                        use base64::Engine;
                        SockMsg { sock: sock.clone(), text: None, data: Some(base64::engine::general_purpose::STANDARD.encode(&b)) }
                    }
                    Message::Close(_) => break,
                    _ => continue,
                };
                conn.send(&ToAgentd::Sock(out));
            }
        }
    }
    if app.code.socks.lock().unwrap().remove(&sock).is_some() {
        conn.send(&ToAgentd::SockClose { sock });
    }
    let _ = sink.close().await;
}

/// A viewer socket's message or end, from agentd on `machine` (only its own sockets).
pub fn from_agentd(app: &App, machine: &str, m: FromAgentd) {
    match m {
        FromAgentd::Sock(m) => {
            let mut socks = app.code.socks.lock().unwrap();
            let Some((owner, tx)) = socks.get(&m.sock) else { return };
            if owner != machine {
                return;
            }
            match tx.try_send(m) {
                Ok(()) => {}
                // The page is too slow for the screencast: skip a frame rather than wait.
                Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(m)) => {
                    // The page never came (or left): agentd may close Steel's side.
                    socks.remove(&m.sock);
                    if let Some(c) = app.code.conn(machine) {
                        c.send(&ToAgentd::SockClose { sock: m.sock });
                    }
                }
            }
        }
        FromAgentd::SockClose { sock } => {
            let mut socks = app.code.socks.lock().unwrap();
            if socks.get(&sock).is_some_and(|(owner, _)| owner == machine) {
                socks.remove(&sock);
            }
        }
        _ => {}
    }
}

/// agentd on `machine` went away: so did the sockets it relayed.
pub fn machine_gone(app: &App, machine: &str) {
    app.code.socks.lock().unwrap().retain(|_, (owner, _)| owner != machine);
}
