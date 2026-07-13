use serde_json::Value;

// Browser automation (embedded webview guests, grab mode, cookie profiles) isn't
// ported. These are the methods whose contracts allow a simple bool/empty answer;
// the value-constructing ones (grab results, screenshots, profile objects) are
// intentionally omitted until the browser subsystem lands.

#[tauri::command]
pub fn browser_unregister_guest() {}

#[tauri::command]
pub fn browser_open_dev_tools() -> bool {
    false
}

#[tauri::command]
pub fn browser_cancel_download() -> bool {
    false
}

#[tauri::command]
pub fn browser_cancel_grab() -> bool {
    false
}

#[tauri::command]
pub fn browser_session_list_profiles() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn browser_session_detect_browsers() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn browser_session_delete_profile() -> bool {
    // Honest failure (spec 014 AC 5): the legacy cookie-profile subsystem isn't
    // ported, so deleting one CANNOT succeed — the old hardcoded `true` faked
    // success and hid that. Project-scoped clearing lives in
    // `browser_clear_project_data` + `POST /api/cdp-browser/clear-project-data`.
    false
}

#[tauri::command]
pub fn browser_session_clear_default_cookies() -> bool {
    true
}

#[tauri::command]
pub fn browser_notify_active_tab_changed() -> bool {
    true
}

// Grab mode, screenshots, viewport override, hover payloads, downloads, and cookie
// profile import aren't ported. Bool acks are false; value-returning reads are null.
#[tauri::command]
pub fn browser_set_grab_mode() -> bool {
    false
}

#[tauri::command]
pub fn browser_capture_selection_screenshot() -> Option<Value> {
    None
}

#[tauri::command]
pub fn browser_set_viewport_override() -> bool {
    false
}

#[tauri::command]
pub fn browser_await_grab_selection() -> Option<Value> {
    None
}

#[tauri::command]
pub fn browser_extract_hover_payload() -> Option<Value> {
    None
}

#[tauri::command]
pub fn browser_accept_download() -> bool {
    false
}

#[tauri::command]
pub fn browser_session_create_profile() -> Option<Value> {
    None
}

#[tauri::command]
pub fn browser_session_import_cookies() -> bool {
    false
}

#[tauri::command]
pub fn browser_session_import_from_browser() -> bool {
    false
}
