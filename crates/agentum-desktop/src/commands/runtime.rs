use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStatus {
    pub workspace_root: Option<String>,
    pub active_project: Option<String>,
    pub active_session_id: Option<String>,
    pub healthy: bool,
    pub running_agents: usize,
}

#[tauri::command]
pub async fn runtime_get_status(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    let runtime = state.runtime.lock();
    Ok(RuntimeStatus {
        workspace_root: runtime.workspace.workspace_root.clone(),
        active_project: runtime.workspace.active_project.clone(),
        active_session_id: runtime.workspace.active_session_id.clone(),
        healthy: runtime.workspace.healthy,
        // The desktop's in-memory agent registry was removed (dead: never
        // populated). Real running agents live in the embedded server's
        // sessions; surface 0 here until runtime status routes through it.
        running_agents: 0,
    })
}

// The runtime RPC layer (terminal/browser drivers, fit overrides, environments)
// isn't ported; report none.
#[tauri::command]
pub fn runtime_get_terminal_fit_overrides() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn runtime_get_terminal_drivers() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn runtime_get_browser_drivers() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn runtime_reclaim_browser_for_desktop() -> Value {
    json!({ "reclaimed": false })
}

#[tauri::command]
pub fn runtime_environments_list() -> Vec<Value> {
    Vec::new()
}

// Generic runtime RPC + remote-environment management aren't ported; calls return
// null and subscribe/remove/sync no-op.
#[tauri::command]
pub fn runtime_call() -> Option<Value> {
    None
}

#[tauri::command]
pub fn runtime_sync_window_graph() {}

#[tauri::command]
pub fn runtime_environments_call() -> Option<Value> {
    None
}

#[tauri::command]
pub fn runtime_environments_remove() {}

#[tauri::command]
pub fn runtime_environments_add_from_pairing_code() -> Option<Value> {
    None
}

#[tauri::command]
pub fn runtime_environments_subscribe() {}
