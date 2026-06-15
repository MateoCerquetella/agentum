//! `/api/browser/*` — browser-pane automation, forwarded to the desktop's
//! webviews via the `DesktopBridge`. Without a bridge (standalone daemon) these
//! return 501: only the process that owns the webviews can drive them.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/browser/tabs", post(tabs))
        .route("/api/browser/navigate", post(navigate))
        .route("/api/browser/snapshot", post(snapshot))
        .route("/api/browser/click", post(click))
        .route("/api/browser/fill", post(fill))
        .route("/api/browser/screenshot", post(screenshot))
}

/// Forward a browser op to the desktop bridge, or 501 if there is no desktop.
async fn forward(state: &AppState, op: &str, mut body: Value) -> Result<Json<Value>, ApiError> {
    let Some(bridge) = &state.desktop_bridge else {
        return Err(ApiError::Custom(
            StatusCode::NOT_IMPLEMENTED,
            json!({ "error": "browser automation requires the agentum desktop app" }),
        ));
    };
    if let Some(obj) = body.as_object_mut() {
        obj.insert("op".into(), Value::String(op.to_string()));
    }
    bridge
        .browser(body)
        .await
        .map(Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

async fn tabs(
    State(s): State<AppState>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    forward(&s, "tabs", body.map(|b| b.0).unwrap_or(json!({}))).await
}
async fn navigate(
    State(s): State<AppState>,
    Json(b): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    forward(&s, "navigate", b).await
}
async fn snapshot(
    State(s): State<AppState>,
    Json(b): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    forward(&s, "snapshot", b).await
}
async fn click(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "click", b).await
}
async fn fill(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "fill", b).await
}
async fn screenshot(
    State(s): State<AppState>,
    Json(b): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    forward(&s, "screenshot", b).await
}
