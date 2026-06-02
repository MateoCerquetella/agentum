use serde_json::Value;

// Agent-status tracking (the stateful onSet store + the interrupt heuristic) isn't
// ported. Snapshots are empty, inferInterrupt is false, drop is a no-op.

#[tauri::command]
pub fn agent_status_get_snapshot() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn agent_status_infer_interrupt() -> bool {
    false
}

#[tauri::command]
pub fn agent_status_get_migration_unsupported_snapshot() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn agent_status_drop() {}
