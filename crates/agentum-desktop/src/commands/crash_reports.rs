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
pub fn crash_reports_record_breadcrumb(request: tauri::ipc::Request<'_>) {
    // TEMP DIAGNOSTIC: surface renderer breadcrumbs (errors/rejections) to stderr.
    if let tauri::ipc::InvokeBody::Json(value) = request.body() {
        eprintln!("[breadcrumb] {value}");
    }
}

#[tauri::command]
pub fn crash_reports_record_renderer_error(request: tauri::ipc::Request<'_>) -> Value {
    // TEMP DIAGNOSTIC: log the React error-boundary report (boundaryId + error).
    if let tauri::ipc::InvokeBody::Json(value) = request.body() {
        eprintln!("[renderer-error] {value}");
    }
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
