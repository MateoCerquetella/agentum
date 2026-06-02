use serde_json::Value;

// Per-repo sparse-checkout presets aren't ported; list is empty, remove is a no-op.

#[tauri::command]
pub fn sparse_presets_list() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn sparse_presets_remove() {}

// Saving a sparse-checkout preset isn't ported; no-op.
#[tauri::command]
pub fn sparse_presets_save() {}
