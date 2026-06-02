use serde_json::Value;

// The automation engine (scheduling, dispatch, run history, external managers) isn't
// ported. Lists are empty, precheck is null, and the void mutators no-op. The methods
// that construct Automation/AutomationRun objects (create/update/runNow/
// markDispatchResult) are intentionally omitted until the engine lands.

#[tauri::command]
pub fn automations_list() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn automations_list_runs() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn automations_list_external_managers() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn automations_run_precheck() -> Option<Value> {
    None
}

#[tauri::command]
pub fn automations_delete() {}

#[tauri::command]
pub fn automations_renderer_ready() {}

#[tauri::command]
pub fn automations_create_external() {}

#[tauri::command]
pub fn automations_update_external() {}

#[tauri::command]
pub fn automations_run_external_action() {}

// Engine mutators: markDispatchResult/runNow no-op; create/update return null until
// the automation engine lands.
#[tauri::command]
pub fn automations_mark_dispatch_result() {}

#[tauri::command]
pub fn automations_run_now() {}

#[tauri::command]
pub fn automations_create() -> Option<Value> {
    None
}

#[tauri::command]
pub fn automations_update() -> Option<Value> {
    None
}
