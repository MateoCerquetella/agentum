//! Embedded agentum-server endpoint exposed to the webview.
//!
//! The desktop boots `agentum-server` in-process on a loopback port (see the
//! `setup` hook in `lib.rs`) and stores the resulting base URL here so the React
//! app drives the same HTTP/WS core as the TUI rather than a parallel local
//! backend.

use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEndpoint {
    /// Base URL for `/api/*` and WS routes, e.g. `http://127.0.0.1:54321`.
    pub url: String,
    /// Bearer token, or `null` when the loopback server runs with auth disabled.
    pub token: Option<String>,
}

#[tauri::command]
pub fn app_get_server_endpoint(endpoint: tauri::State<'_, ServerEndpoint>) -> ServerEndpoint {
    endpoint.inner().clone()
}
