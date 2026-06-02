use serde_json::{json, Value};

// Auto-update isn't wired. getVersion is real; getStatus reports idle (no update in
// progress, a valid UpdateStatus variant); the rest are no-ops.

#[tauri::command]
pub fn updater_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub fn updater_get_status() -> Value {
    json!({ "state": "idle" })
}

#[tauri::command]
pub fn updater_check() {}

#[tauri::command]
pub fn updater_download() {}

#[tauri::command]
pub fn updater_quit_and_install() {}

#[tauri::command]
pub fn updater_dismiss_nudge() {}
