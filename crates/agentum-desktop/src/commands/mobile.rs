use serde_json::{json, Value};

// Mobile pairing isn't wired into the desktop shell yet. These handlers return
// safe "unavailable / empty" shapes (matching the web client's fallbacks) so the
// Settings → Runtime Pairing UI degrades gracefully instead of throwing
// "command not found".

#[tauri::command]
pub fn mobile_get_runtime_pairing_url() -> Value {
    json!({ "available": false })
}

#[tauri::command]
pub fn mobile_list_network_interfaces() -> Value {
    json!({ "interfaces": [] })
}

#[tauri::command]
pub fn mobile_list_runtime_access_grants() -> Value {
    json!({ "grants": [] })
}

#[tauri::command]
pub fn mobile_revoke_runtime_access() -> Value {
    json!({ "revoked": false })
}
