use serde_json::Value;

// The E2E test harness config (fixtures, mock toggles) isn't ported; report none so
// the app behaves normally outside automated test runs.
#[tauri::command]
pub fn e2e_get_config() -> Option<Value> {
    None
}
