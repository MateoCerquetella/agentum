use serde_json::{json, Value};

// Aggregate usage stats aren't ported; report an empty summary.
#[tauri::command]
pub fn stats_get_summary() -> Value {
    json!({})
}
