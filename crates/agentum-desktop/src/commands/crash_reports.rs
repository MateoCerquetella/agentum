use serde_json::Value;
use std::io::Write as _;
use tauri::Manager as _;

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
pub fn crash_reports_record_renderer_error(
    app: tauri::AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Value {
    // Persist the React error-boundary report (boundaryId + error name/message/
    // stack + componentStack). The boundary UI only shows a generic "This page
    // hit an error" message, so without this the actual stack is lost and render
    // crashes (e.g. config → Terminal) can't be diagnosed. We append to
    // `renderer-errors.log` in the app log dir (next to agentum.log) and also
    // emit via `log` so it reaches the plugin's stdout/webview targets.
    if let tauri::ipc::InvokeBody::Json(value) = request.body() {
        log::error!("[renderer-error] {value}");
        if let Ok(dir) = app.path().app_log_dir() {
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("renderer-errors.log"))
            {
                let _ = writeln!(file, "{value}");
            }
        }
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
