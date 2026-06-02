use serde_json::{json, Value};

// Codex usage scanning isn't ported. Mirrors claude_usage with codex-specific
// hasAny field names; returns empty state / zeroed summary / empty lists.

fn scan_state(enabled: bool) -> Value {
    json!({
        "enabled": enabled,
        "isScanning": false,
        "lastScanStartedAt": null,
        "lastScanCompletedAt": null,
        "lastScanError": null,
        "hasAnyCodexData": false
    })
}

#[tauri::command]
pub fn codex_usage_get_scan_state() -> Value {
    scan_state(false)
}

#[tauri::command]
pub fn codex_usage_set_enabled(enabled: bool) -> Value {
    scan_state(enabled)
}

#[tauri::command]
pub fn codex_usage_refresh() -> Value {
    scan_state(false)
}

#[tauri::command]
pub fn codex_usage_get_summary(request: tauri::ipc::Request<'_>) -> Value {
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
        "inputTokens": 0,
        "outputTokens": 0,
        "cacheReadTokens": 0,
        "cacheWriteTokens": 0,
        "estimatedCostUsd": null,
        "topModel": null,
        "topProject": null,
        "hasAnyCodexData": false
    })
}

#[tauri::command]
pub fn codex_usage_get_daily() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn codex_usage_get_breakdown() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn codex_usage_get_recent_sessions() -> Vec<Value> {
    Vec::new()
}
