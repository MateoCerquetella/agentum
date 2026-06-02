use serde_json::Value;

// Custom-pet import/storage isn't ported: imports and reads return null, delete no-ops.

#[tauri::command]
pub fn pet_import() -> Option<Value> {
    None
}

#[tauri::command]
pub fn pet_import_pet_bundle() -> Option<Value> {
    None
}

#[tauri::command]
pub fn pet_read() -> Option<Value> {
    None
}

#[tauri::command]
pub fn pet_delete() {}
