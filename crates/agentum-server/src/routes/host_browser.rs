//! `/api/host-browser` — spec 009a Phase 2: drive a host-resident headless
//! Chromium and stream its CDP screencast.
//!
//! `POST` starts (or re-attaches to) the browser for `{host_id, workdir}` and
//! forward-tunnels its CDP port; the `screencast` WS streams JPEG frames out (the
//! `0x62` binary protocol) and takes input messages in; `navigate` points it at a
//! URL; `GET`/`DELETE` report status / tear it down. All behind the top-level
//! auth layer (merged in `lib.rs::router`).
//!
//! The screencast WS here is the standalone Phase-2 transport verified by a
//! scratch client; Phase 3 reconciles it with the desktop's runtime-environments
//! `browser.*` RPC broker.

use agentum_core::Host;
use axum::Json;
use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::host_browser::{self, HostBrowserStatus, StartedHostBrowser};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/host-browser", post(start))
        .route("/api/host-browser/install", post(install))
        .route("/api/host-browser/{id}", get(get_status).delete(delete))
        .route("/api/host-browser/{id}/navigate", post(navigate))
        .route("/api/host-browser/{id}/screencast", get(screencast))
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    host_id: String,
    workdir: String,
}

#[derive(Debug, Deserialize)]
struct NavigateRequest {
    url: String,
}

#[derive(Debug, Deserialize)]
struct InstallRequest {
    host_id: String,
}

#[derive(Debug, Serialize)]
struct InstallResult {
    ok: bool,
    output: String,
}

/// Resolve a host UUID against the store (404 when unknown, 400 when malformed).
async fn resolve_host(state: &AppState, host_id: &str) -> Result<Host, ApiError> {
    let id = Uuid::parse_str(host_id)
        .map_err(|_| ApiError::BadRequest(format!("invalid host_id: {host_id}")))?;
    state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(host_id.to_string()))
}

async fn start(
    State(state): State<AppState>,
    Json(req): Json<StartRequest>,
) -> Result<Json<StartedHostBrowser>, ApiError> {
    let host_id = Uuid::parse_str(&req.host_id)
        .map_err(|_| ApiError::BadRequest(format!("invalid host_id: {}", req.host_id)))?;
    // Serialize the authoritative Store reload and every launch/tunnel action
    // with PUT/delete/session lifecycle work for this host. The guard ends when
    // start returns; it is never retained for the browser's lifetime.
    let _host_guard = crate::routes::sessions::acquire_host_lifecycle(host_id).await;
    let host = resolve_host(&state, &req.host_id).await?;
    let started = host_browser::start_host_browser_from_store(
        &host,
        state.store.clone(),
        std::path::Path::new(&req.workdir),
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(started))
}

/// `POST /api/host-browser/install` — offer-install: run `npx playwright install
/// chromium` on the host when preflight found no browser (spec 009a AC #5).
async fn install(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> Result<Json<InstallResult>, ApiError> {
    let host_id = Uuid::parse_str(&req.host_id)
        .map_err(|_| ApiError::BadRequest(format!("invalid host_id: {}", req.host_id)))?;
    // Installation runs through SSH too, so resolve it under the same host
    // lifecycle lease as launch and host PUT/delete.
    let _host_guard = crate::routes::sessions::acquire_host_lifecycle(host_id).await;
    let host = resolve_host(&state, &req.host_id).await?;
    let output = crate::host_runtime::install_host_chromium(&host)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(InstallResult { ok: true, output }))
}

async fn get_status(Path(id): Path<String>) -> Result<Json<HostBrowserStatus>, ApiError> {
    host_browser::status(&id)
        .await
        .map(Json)
        .ok_or(ApiError::NotFound(id))
}

async fn navigate(
    Path(id): Path<String>,
    Json(req): Json<NavigateRequest>,
) -> Result<StatusCode, ApiError> {
    host_browser::navigate(&id, &req.url)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete(Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    host_browser::stop(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn screencast(ws: WebSocketUpgrade, Path(id): Path<String>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        host_browser::run_screencast(&id, socket).await;
    })
}
