use serde_json::{json, Value};

// The GitHub PR/issue response cache isn't ported; return an empty cache and no-op
// writes (every fetch goes straight to the API once gh is wired up).

#[tauri::command]
pub fn cache_get_git_hub() -> Value {
    json!({ "pr": {}, "issue": {} })
}

#[tauri::command]
pub fn cache_set_git_hub() {}
