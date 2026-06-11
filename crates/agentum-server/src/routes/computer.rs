//! `/api/computer/*` — macOS computer-use, forwarded to the desktop's
//! Accessibility engine via the `DesktopBridge`. Without a bridge these return
//! 501: the engine runs in the .app process that holds the Accessibility TCC
//! grant, so a standalone daemon can't (and shouldn't) drive it.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/computer/capabilities", post(capabilities))
        .route("/api/computer/permissions", post(permissions))
        .route("/api/computer/list-apps", post(list_apps))
        .route("/api/computer/get-app-state", post(get_app_state))
        .route("/api/computer/click", post(click))
        .route("/api/computer/set-value", post(set_value))
        .route("/api/computer/type-text", post(type_text))
        .route("/api/computer/press-key", post(press_key))
        .route("/api/computer/scroll", post(scroll))
}

async fn forward(state: &AppState, op: &str, mut body: Value) -> Result<Json<Value>, ApiError> {
    let Some(bridge) = &state.desktop_bridge else {
        return Err(ApiError::Custom(
            StatusCode::NOT_IMPLEMENTED,
            json!({ "error": "computer-use requires the agentum desktop app (macOS)" }),
        ));
    };
    if let Some(obj) = body.as_object_mut() {
        obj.insert("op".into(), Value::String(op.to_string()));
    }
    bridge
        .computer(body)
        .await
        .map(Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

async fn capabilities(State(s): State<AppState>, body: Option<Json<Value>>) -> Result<Json<Value>, ApiError> {
    forward(&s, "capabilities", body.map(|b| b.0).unwrap_or(json!({}))).await
}
async fn permissions(State(s): State<AppState>, body: Option<Json<Value>>) -> Result<Json<Value>, ApiError> {
    forward(&s, "permissions", body.map(|b| b.0).unwrap_or(json!({}))).await
}
async fn list_apps(State(s): State<AppState>, body: Option<Json<Value>>) -> Result<Json<Value>, ApiError> {
    forward(&s, "list-apps", body.map(|b| b.0).unwrap_or(json!({}))).await
}
async fn get_app_state(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "get-app-state", b).await
}
async fn click(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "click", b).await
}
async fn set_value(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "set-value", b).await
}
async fn type_text(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "type-text", b).await
}
async fn press_key(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "press-key", b).await
}
async fn scroll(State(s): State<AppState>, Json(b): Json<Value>) -> Result<Json<Value>, ApiError> {
    forward(&s, "scroll", b).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{BridgeFuture, DesktopBridge};
    use std::sync::Arc;

    struct FakeBridge;
    impl DesktopBridge for FakeBridge {
        fn browser(&self, _op: Value) -> BridgeFuture<'_> {
            Box::pin(async { Ok(json!({ "ok": true })) })
        }
        fn computer(&self, op: Value) -> BridgeFuture<'_> {
            Box::pin(async move { Ok(json!({ "echo": op })) })
        }
    }

    async fn state_with(bridge: Option<Arc<dyn DesktopBridge>>) -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&dir.path().join("t.db"))
            .await
            .unwrap();
        let (bus, _) = tokio::sync::broadcast::channel(16);
        let mut st = AppState::new(store, bus);
        st.desktop_bridge = bridge;
        std::mem::forget(dir); // keep the tempdir alive for the pool's lifetime
        st
    }

    #[tokio::test]
    async fn no_bridge_returns_501() {
        let st = state_with(None).await;
        let err = forward(&st, "capabilities", json!({})).await.unwrap_err();
        let resp = axum::response::IntoResponse::into_response(err);
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn with_bridge_forwards_and_tags_op() {
        let st = state_with(Some(Arc::new(FakeBridge))).await;
        let Json(v) = forward(&st, "list-apps", json!({ "x": 1 })).await.unwrap();
        // The fake echoes the op it received — the op tag must be injected.
        assert_eq!(v["echo"]["op"], "list-apps");
        assert_eq!(v["echo"]["x"], 1);
    }
}
