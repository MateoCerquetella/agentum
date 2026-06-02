use serde_json::{json, Value};

// Feedback submission posts to a backend service that isn't wired up; return the
// contract's failure variant.
#[tauri::command]
pub fn feedback_submit() -> Value {
    json!({
        "ok": false,
        "status": null,
        "error": "Feedback submission isn't available in this build."
    })
}
