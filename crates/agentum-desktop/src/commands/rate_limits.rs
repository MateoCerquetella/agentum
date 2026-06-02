use serde_json::{json, Value};

// Rate-limit tracking (reading agent-CLI quota data) isn't ported. Return an empty
// RateLimitState so the UI shows "no data" rather than erroring; mutators no-op.
fn default_state() -> Value {
    json!({
        "claude": null,
        "codex": null,
        "gemini": null,
        "opencodeGo": null,
        "claudeTarget": { "runtime": "host", "wslDistro": null },
        "codexTarget": { "runtime": "host", "wslDistro": null },
        "inactiveClaudeAccounts": [],
        "inactiveCodexAccounts": []
    })
}

#[tauri::command]
pub fn rate_limits_get() -> Value {
    default_state()
}

#[tauri::command]
pub fn rate_limits_refresh() -> Value {
    default_state()
}

#[tauri::command]
pub fn rate_limits_refresh_codex_for_target() -> Value {
    default_state()
}

#[tauri::command]
pub fn rate_limits_refresh_claude_for_target() -> Value {
    default_state()
}

#[tauri::command]
pub fn rate_limits_set_polling_interval() {}

#[tauri::command]
pub fn rate_limits_fetch_inactive_claude_accounts() {}

#[tauri::command]
pub fn rate_limits_fetch_inactive_codex_accounts() {}
