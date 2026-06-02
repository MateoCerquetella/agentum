use serde_json::{json, Value};

// Claude usage scanning (reading ~/.claude session logs) isn't ported. Return empty
// scan state / zeroed summary / empty lists so the usage UI shows "no data".

fn scan_state(enabled: bool) -> Value {
    json!({
        "enabled": enabled,
        "isScanning": false,
        "lastScanStartedAt": null,
        "lastScanCompletedAt": null,
        "lastScanError": null,
        "hasAnyClaudeData": false
    })
}

#[tauri::command]
pub fn claude_usage_get_scan_state() -> Value {
    scan_state(false)
}

#[tauri::command]
pub fn claude_usage_set_enabled(enabled: bool) -> Value {
    scan_state(enabled)
}

#[tauri::command]
pub fn claude_usage_refresh() -> Value {
    scan_state(false)
}

#[tauri::command]
pub fn claude_usage_get_summary(request: tauri::ipc::Request<'_>) -> Value {
    // Echo the requested scope/range so the renderer keys the result correctly.
    let (scope, range) = match request.body() {
        tauri::ipc::InvokeBody::Json(value) => (
            value.get("scope").cloned().unwrap_or(Value::Null),
            value.get("range").cloned().unwrap_or(Value::Null),
        ),
        _ => (Value::Null, Value::Null),
    };
    json!({
        "scope": scope,
        "range": range,
        "sessions": 0,
        "turns": 0,
        "zeroCacheReadTurns": 0,
        "inputTokens": 0,
        "outputTokens": 0,
        "cacheReadTokens": 0,
        "cacheWriteTokens": 0,
        "cacheReuseRate": null,
        "estimatedCostUsd": null,
        "topModel": null,
        "topProject": null,
        "hasAnyClaudeData": false
    })
}

#[tauri::command]
pub fn claude_usage_get_daily() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn claude_usage_get_breakdown() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn claude_usage_get_recent_sessions() -> Vec<Value> {
    Vec::new()
}
