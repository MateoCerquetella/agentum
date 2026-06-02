use serde_json::{json, Value};

// The mobile pairing bridge (QR pairing + WebSocket relay) isn't ported. Revokes
// report success (nothing to revoke), grants are empty, the bridge is never ready.

#[tauri::command]
pub fn mobile_revoke_device() -> Value {
    json!({ "revoked": true })
}

#[tauri::command]
pub fn mobile_revoke_runtime_access() -> Value {
    json!({ "revoked": true })
}

#[tauri::command]
pub fn mobile_list_runtime_access_grants() -> Value {
    json!({ "grants": [] })
}

#[tauri::command]
pub fn mobile_is_web_socket_ready() -> Value {
    json!({ "ready": false, "endpoint": null })
}

#[tauri::command]
pub fn mobile_get_pairing_qr() -> Value {
    json!({ "available": false })
}

#[tauri::command]
pub fn mobile_get_runtime_pairing_url() -> Value {
    json!({ "available": false })
}

#[tauri::command]
pub fn mobile_list_devices() -> Value {
    json!({ "devices": [] })
}

#[tauri::command]
pub fn mobile_list_network_interfaces() -> Value {
    json!({ "interfaces": [] })
}
