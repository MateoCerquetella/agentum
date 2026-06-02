use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// Workspace port scanning (listing/killing processes bound to dev ports) isn't
// ported. Scan reports no ports; kill is a vacuous success.

#[tauri::command]
pub fn workspace_ports_scan() -> Value {
    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0);
    let platform = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    };
    json!({
        "platform": platform,
        "scannedAt": scanned_at,
        "ports": [],
        "unavailableReason": "Port scanning isn't available in this build."
    })
}

#[tauri::command]
pub fn workspace_ports_kill() -> Value {
    json!({ "ok": true })
}
