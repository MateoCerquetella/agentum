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
use serde::Serialize;

use crate::AppState;
use crate::cdp_browser;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/cdp-browser",
            get(status).post(launch).delete(stop),
        )
        // A bare DELETE alias is handy for clients that prefer it explicit.
        .route("/api/cdp-browser/stop", post(stop))
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
