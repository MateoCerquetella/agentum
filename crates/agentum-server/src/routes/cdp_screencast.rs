//! `WS /api/cdp-browser/screencast` — render the agent-driven CDP-Chromium
//! **inside agentum's pane** (009c-3).
//!
//! This is the bridge half of the in-agentum screencast: it connects the pane's
//! WebSocket to a [`crate::cdp_screencast`] CDP client. Frames flow pane-ward as
//! the `0x62` binary protocol the pane already decodes; the pane's
//! mouse/keyboard/scroll/navigation flows browser-ward as CDP `Input.*`/`Page.*`.
//!
//! **One bridge for local AND host.** The only thing that differs is the CDP port
//! on `127.0.0.1`: a local browser binds it directly; a host browser is reached
//! over the 009a `ssh -L` forward tunnel, which also surfaces as a `127.0.0.1`
//! port. A `?cdpPort=` query param selects it (default: the local shared browser).
//!
//! Authed like every `/api/*` route — the WS client passes the bearer as
//! `?token=` (browsers can't set headers on upgrade); the embedded loopback
//! server is no-auth so the desktop connects with no token.

use axum::Router;
use axum::extract::Query;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt as _, StreamExt as _};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, watch};

use crate::cdp_browser;
use crate::cdp_screencast::{
    FrameFormat, InputCommand, ScreencastOptions, parse_input_message, run_screencast_bridge,
};

pub fn router() -> Router<crate::AppState> {
    Router::new().route("/api/cdp-browser/screencast", get(screencast))
}

/// Screencast subscribe params, mirroring the pane's subscribe call. All optional
/// — anything absent falls back to [`ScreencastOptions::default`]. `cdpPort`
/// selects which `127.0.0.1` CDP port to attach to (local browser, or a host
/// browser surfaced through the 009a tunnel).
#[derive(Debug, Deserialize, Default)]
struct ScreencastQuery {
    #[serde(rename = "cdpPort")]
    cdp_port: Option<u16>,
    format: Option<String>,
    quality: Option<u8>,
    #[serde(rename = "maxWidth")]
    max_width: Option<u32>,
    #[serde(rename = "maxHeight")]
    max_height: Option<u32>,
    #[serde(rename = "everyNthFrame")]
    every_nth_frame: Option<u32>,
    /// Per-worktree isolation: with no explicit `cdpPort`, attach to (and launch
    /// on demand) THIS worktree's own Chromium instead of the shared one, so each
    /// worktree's browser is independent. Empty/absent → the shared default.
    #[serde(rename = "worktreeId")]
    worktree_id: Option<String>,
}

impl ScreencastQuery {
    fn options(&self) -> ScreencastOptions {
        let d = ScreencastOptions::default();
        ScreencastOptions {
            format: match self.format.as_deref() {
                Some("png") => FrameFormat::Png,
                _ => FrameFormat::Jpeg,
            },
            quality: self.quality.unwrap_or(d.quality),
            max_width: self.max_width.unwrap_or(d.max_width),
            max_height: self.max_height.unwrap_or(d.max_height),
            every_nth_frame: self.every_nth_frame.unwrap_or(d.every_nth_frame),
        }
    }

    /// The CDP HTTP base to attach to: the requested `127.0.0.1` port, or the
    /// shared local browser's port by default.
    fn cdp_http_base(&self) -> String {
        let port = self.cdp_port.unwrap_or_else(cdp_browser::port);
        cdp_browser::cdp_endpoint_for(port)
    }
}

async fn screencast(ws: WebSocketUpgrade, Query(q): Query<ScreencastQuery>) -> impl IntoResponse {
    let opts = q.options();
    // Resolve the CDP endpoint to attach to. With no explicit `cdpPort` this is
    // the shared LOCAL browser — launch it on demand so a plain user-opened tab
    // (not just an agent-driven one) has a Chromium to attach to; the launch is
    // idempotent, so concurrent tabs reuse the one browser. An explicit `cdpPort`
    // is an external/tunneled browser (009a/SSH) we must never launch ourselves.
    // We resolve the `(endpoint, port)` pair (not just the endpoint) so we can
    // record the port as the foreground browser below.
    let worktree = q
        .worktree_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let resolved: Result<(String, u16), String> = if let Some(port) = q.cdp_port {
        // Explicit/tunneled browser (009a/SSH) — attach as-is, never launch it.
        Ok((q.cdp_http_base(), port))
    } else if let Some(wt) = worktree {
        // Per-worktree isolation: launch/reuse THIS worktree's own Chromium so it
        // doesn't share tabs with other worktrees. The worktree's agent resolves
        // the same browser, so they still watch/drive one instance per worktree.
        cdp_browser::ensure_local_cdp_browser_for(wt)
            .await
            .map_err(|e| format!("{e:#}"))
    } else {
        // Shared local browser — launch on demand (idempotent) so a plain
        // user-opened tab (no worktree context) has a Chromium to attach to.
        cdp_browser::ensure_local_cdp_browser()
            .await
            .map(|endpoint| (endpoint, cdp_browser::port()))
            .map_err(|e| format!("{e:#}"))
    };
    // Remember which browser the user is now watching: a contextless MCP
    // `agentum_browser` op (no worktreeId/cdpPort) drives THIS port so the agent
    // acts on the same browser the user sees. Last attach wins (the foreground
    // pane); a failed resolve leaves the previous value untouched.
    if let Ok((_, port)) = &resolved {
        cdp_browser::set_foreground_cdp_port(*port);
    }
    let base = resolved.map(|(endpoint, _)| endpoint);
    ws.on_upgrade(move |socket| run(socket, base, opts))
}

/// Input channel depth. Input is tiny and bursty; 64 is ample. (Frames use a
/// `watch` channel instead of a depth-bounded queue — latest-wins, so a slow pane
/// drops stale frames rather than buffering a backlog and stalling Chrome; the
/// bridge acks each frame immediately. See `run` and `cdp_screencast::run_screencast_bridge`.)
const INPUT_CHANNEL_DEPTH: usize = 64;

async fn run(socket: WebSocket, cdp_http_base: Result<String, String>, opts: ScreencastOptions) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // A failed on-demand launch (no system Chrome AND no Playwright build)
    // surfaces to the pane as an actionable error frame instead of a silent blank.
    let cdp_http_base = match cdp_http_base {
        Ok(base) => base,
        Err(message) => {
            let _ = ws_tx
                .send(Message::Text(
                    json!({ "type": "error", "message": message })
                        .to_string()
                        .into(),
                ))
                .await;
            let _ = ws_tx
                .send(Message::Text(json!({ "type": "end" }).to_string().into()))
                .await;
            return;
        }
    };

    // Tell the pane we're live (its `onResponse('ready')` flips the pane to the
    // screencast surface). Best-effort: if this send fails the client is already
    // gone and the loop below exits immediately.
    let format = match opts.format {
        FrameFormat::Png => "png",
        FrameFormat::Jpeg => "jpeg",
    };
    let _ = ws_tx
        .send(Message::Text(
            json!({ "type": "ready", "format": format })
                .to_string()
                .into(),
        ))
        .await;

    // Latest-wins frame sink: the bridge overwrites any undrained frame, so the
    // pane always renders the freshest one and a slow pane never stalls Chrome.
    // `None` is the pre-first-frame sentinel (never delivered — the initial value
    // is "already seen", so `changed()` only fires on a real frame).
    let (frame_tx, mut frame_rx) = watch::channel::<Option<Vec<u8>>>(None);
    let (input_tx, input_rx) = mpsc::channel::<InputCommand>(INPUT_CHANNEL_DEPTH);

    // The CDP client runs in its own task; it ends (dropping `frame_tx`) when the
    // browser closes, when we drop `input_tx` (pane disconnected), or on error.
    let bridge = tokio::spawn(async move {
        run_screencast_bridge(&cdp_http_base, opts, input_rx, frame_tx).await
    });

    loop {
        tokio::select! {
            changed = frame_rx.changed() => match changed {
                Ok(()) => {
                    // Take the freshest frame; intermediate frames the pane couldn't
                    // keep up with were already overwritten (latest-wins). Clone out
                    // before the await so we don't hold the watch borrow across it.
                    let latest = frame_rx.borrow_and_update().clone();
                    if let Some(bytes) = latest {
                        if ws_tx.send(Message::Binary(bytes.into())).await.is_err() {
                            break; // pane gone
                        }
                    }
                }
                Err(_) => break, // bridge ended: sender dropped (clean close or error)
            },
            msg = ws_rx.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    if let Some(cmd) = parse_input_message(&t) {
                        // A real human action gives the human the co-browse wheel,
                        // so the agent's input ops yield for a short window (F12).
                        if cmd.is_human_action() {
                            crate::cdp_driver::note_human_input();
                        }
                        // Best-effort: a full input queue (pane spamming faster than
                        // CDP drains) drops the event rather than blocking frames.
                        let _ = input_tx.try_send(cmd);
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {} // ping/pong/binary from the pane — ignored
            },
        }
    }

    // Dropping `input_tx` signals the bridge to stop; await it so a connect/
    // protocol failure surfaces to the pane as an `error` control frame instead
    // of a silent blank pane.
    drop(input_tx);
    if let Ok(Err(e)) = bridge.await {
        let _ = ws_tx
            .send(Message::Text(
                json!({ "type": "error", "message": format!("{e:#}") })
                    .to_string()
                    .into(),
            ))
            .await;
    }
    let _ = ws_tx
        .send(Message::Text(json!({ "type": "end" }).to_string().into()))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_defaults_and_overrides_map_to_options() {
        // Empty query → the shared local browser + jpeg defaults.
        let q = ScreencastQuery::default();
        let o = q.options();
        assert_eq!(o.format, FrameFormat::Jpeg);
        assert_eq!(o.quality, ScreencastOptions::default().quality);
        assert_eq!(
            q.cdp_http_base(),
            cdp_browser::cdp_endpoint_for(cdp_browser::port())
        );

        // Overrides flow through, incl. a host port surfaced via the tunnel.
        let q = ScreencastQuery {
            cdp_port: Some(9201),
            format: Some("png".into()),
            quality: Some(50),
            max_width: Some(1280),
            max_height: Some(720),
            every_nth_frame: Some(1),
            worktree_id: None,
        };
        let o = q.options();
        assert_eq!(o.format, FrameFormat::Png);
        assert_eq!(o.quality, 50);
        assert_eq!(o.max_width, 1280);
        assert_eq!(o.every_nth_frame, 1);
        assert_eq!(q.cdp_http_base(), "http://127.0.0.1:9201");
    }
}
