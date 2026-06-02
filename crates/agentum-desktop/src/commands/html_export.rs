use serde_json::{json, Value};

// HTML→PDF export needs a headless print renderer that isn't wired up; report failure.
#[tauri::command]
pub fn export_html_to_pdf() -> Value {
    json!({ "success": false, "error": "PDF export isn't available in this build." })
}
