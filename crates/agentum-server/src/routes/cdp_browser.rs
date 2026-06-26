//! `/api/cdp-browser` — status / launch / stop for the shared local
//! CDP-Chromium browser agents drive via the bound Playwright MCP (009c-1).
//!
//! This is the in-agentum surface for the headed browser: the desktop can show
//! that it's running (and its CDP endpoint), explicitly pre-launch it so the
//! user can watch *before* an agent session starts, and stop it. The browser
//! itself is a long-lived per-machine singleton owned by [`crate::cdp_browser`];
//! these routes are a thin view + control over that lifecycle, never a second
//! launcher.
//!
//! Authed like every other `/api/*` route (not listed in `auth::is_public`).
//! A launch that fails because Chromium/Playwright isn't installed surfaces the
//! fail-loud message (with the `npx playwright install chromium` hint) in the
//! response body, so the UI can show an actionable error instead of hanging.

use axum::Json;
use axum::Router;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::cdp_browser;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/cdp-browser", get(status).post(launch).delete(stop))
        // A bare DELETE alias is handy for clients that prefer it explicit.
        .route("/api/cdp-browser/stop", post(stop))
        // The in-pane annotate picker hit-tests the shared CDP page here.
        .route("/api/cdp-browser/node-at-point", post(node_at_point))
        // "Open Browser (persistent)": launch/stop a worktree's HEADED Chrome (a
        // real window) the agent drives over CDP. See the headed-agent-browser spec.
        .route(
            "/api/cdp-browser/headed",
            post(launch_headed).delete(stop_headed),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadedBody {
    #[serde(default)]
    worktree_id: String,
}

/// `POST /api/cdp-browser/headed` — launch (or attach to) a worktree's **headed**
/// Chrome (a real OS window) and return its CDP port. Body `{worktreeId}`. The
/// agent drives THIS window via the MCP (which prefers the headed browser when one
/// is registered). Native Chrome UX — no screencast stream.
async fn launch_headed(Json(body): Json<HeadedBody>) -> Result<Json<Value>, ApiError> {
    let (endpoint, port) = cdp_browser::ensure_headed_cdp_browser_for(&body.worktree_id)
        .await
        .map_err(|e| ApiError::Internal(format!("{e:#}")))?;
    Ok(Json(json!({
        "running": true,
        "port": port,
        "cdpEndpoint": endpoint,
    })))
}

/// `DELETE /api/cdp-browser/headed` — stop a worktree's headed Chrome. Idempotent.
async fn stop_headed(Json(body): Json<HeadedBody>) -> Result<Json<Value>, ApiError> {
    cdp_browser::stop_headed_cdp_browser_for(&body.worktree_id)
        .await
        .map_err(|e| ApiError::Internal(format!("{e:#}")))?;
    Ok(Json(json!({ "running": false })))
}

#[derive(Serialize)]
struct CdpBrowserStatus {
    /// Whether the shared local CDP browser is currently serving.
    running: bool,
    /// The loopback CDP port it binds (configured; valid whether up or not).
    port: u16,
    /// The CDP base URL a Playwright MCP attaches to via `--cdp-endpoint`.
    cdp_endpoint: String,
}

fn snapshot(running: bool) -> CdpBrowserStatus {
    let port = cdp_browser::port();
    CdpBrowserStatus {
        running,
        port,
        cdp_endpoint: cdp_browser::cdp_endpoint_for(port),
    }
}

/// `GET /api/cdp-browser` — report status without launching anything.
async fn status() -> Json<CdpBrowserStatus> {
    Json(snapshot(cdp_browser::is_running().await))
}

/// `POST /api/cdp-browser` — ensure the shared browser is up (launch if needed),
/// returning its endpoint. Fails loud (install hint in the body) if the engine
/// is missing.
async fn launch() -> Result<Json<CdpBrowserStatus>, ApiError> {
    cdp_browser::ensure_local_cdp_browser()
        .await
        .map_err(|e| ApiError::Internal(format!("{e:#}")))?;
    Ok(Json(snapshot(true)))
}

/// `DELETE /api/cdp-browser` (or `POST /api/cdp-browser/stop`) — tear the shared
/// browser down. Idempotent.
async fn stop() -> Result<Json<CdpBrowserStatus>, ApiError> {
    cdp_browser::stop_local_cdp_browser()
        .await
        .map_err(|e| ApiError::Internal(format!("{e:#}")))?;
    Ok(Json(snapshot(false)))
}

/// `POST /api/cdp-browser/node-at-point` — hit-test the CDP page at a viewport
/// pixel for the in-pane annotate picker. Body `{x, y, capture?, cdpPort?, worktreeId?}`.
/// With a `worktreeId` (and no explicit `cdpPort`), resolves THIS worktree's own
/// browser and injects its `cdpPort` — so the picker hit-tests the SAME instance the
/// worktree's screencast renders (per-worktree isolation). Returns the driver JSON
/// (`{ok, clip, label, [path, image_b64, …]}`): hover calls it clip-only, click with
/// `capture:true`. Launch-on-demand is idempotent.
async fn node_at_point(Json(mut body): Json<Value>) -> Result<Json<Value>, ApiError> {
    if body.get("cdpPort").is_none() {
        let worktree = body
            .get("worktreeId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if let Some(wt) = worktree {
            let (_, port) = cdp_browser::ensure_local_cdp_browser_for(&wt)
                .await
                .map_err(|e| ApiError::Internal(format!("{e:#}")))?;
            if let Some(obj) = body.as_object_mut() {
                obj.insert("cdpPort".to_string(), Value::from(port));
            }
        }
    }
    let result = crate::cdp_driver::run_browser_op("node_at_point", &body)
        .await
        .map_err(|e| ApiError::Internal(format!("{e:#}")))?;
    Ok(Json(result))
}
