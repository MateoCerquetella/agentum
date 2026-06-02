use serde_json::Value;

// Crash-report persistence isn't ported. The renderer checks for pending reports on
// startup, so these return null (no pending crash) / no-op rather than erroring.

#[tauri::command]
pub fn crash_reports_get_latest_pending() -> Option<Value> {
    None
}

#[tauri::command]
pub fn crash_reports_get_latest_report() -> Option<Value> {
    None
}

#[tauri::command]
pub fn crash_reports_dismiss(report_id: String) -> Option<Value> {
    let _ = report_id;
    None
}

#[tauri::command]
pub fn crash_reports_record_breadcrumb() {
    // Breadcrumb persistence isn't ported; accept and drop.
}

#[tauri::command]
pub fn crash_reports_record_renderer_error() -> Value {
    // React error-boundary reports aren't persisted yet; acknowledge so the renderer's
    // reporting promise resolves (it checks `result.ok`) instead of rejecting.
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub fn crash_reports_submit() -> Value {
    serde_json::json!({ "ok": false, "error": "Crash-report submission isn't available in this build." })
}

#[tauri::command]
pub fn crash_reports_copy_latest_diagnostics() -> Value {
    serde_json::json!({ "ok": false, "error": "Diagnostics bundles aren't available in this build." })
}
