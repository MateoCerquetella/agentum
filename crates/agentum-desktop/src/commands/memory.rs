use serde_json::{json, Value};

// Process memory snapshots aren't ported; report an empty snapshot.
#[tauri::command]
pub fn memory_get_snapshot() -> Value {
    json!({})
}
