use serde_json::Value;

// Remote-workspace state mirroring (over the SSH transport) isn't ported, so there's
// no snapshot and no connected targets to patch.

#[tauri::command]
pub fn remote_workspace_get() -> Option<Value> {
    None
}

#[tauri::command]
pub fn remote_workspace_set_for_connected_targets() -> Vec<Value> {
    Vec::new()
}
