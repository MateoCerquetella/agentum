use agentum_server::usage;
use serde::Serialize;
use serde_json::{json, Value};

use super::usage_prefs;

const PROVIDER: &str = "codex";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanState {
    enabled: bool,
    is_scanning: bool,
    last_scan_started_at: Option<i64>,
    last_scan_completed_at: Option<i64>,
    last_scan_error: Option<String>,
    has_any_codex_data: bool,
}

fn scan_state(enabled: bool) -> ScanState {
    ScanState {
        enabled,
        is_scanning: false,
        last_scan_started_at: None,
        last_scan_completed_at: None,
        last_scan_error: None,
        has_any_codex_data: enabled && usage::codex_has_any_data(),
    }
}

fn scope_range(request: &tauri::ipc::Request<'_>) -> (String, String) {
    if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        (
            v.get("scope")
                .and_then(|s| s.as_str())
                .unwrap_or("all")
                .to_string(),
            v.get("range")
                .and_then(|s| s.as_str())
                .unwrap_or("30d")
                .to_string(),
        )
    } else {
        ("all".to_string(), "30d".to_string())
    }
}

#[tauri::command]
pub fn codex_usage_get_scan_state() -> Value {
    json!(scan_state(usage_prefs::provider_enabled(PROVIDER, true)))
}

#[tauri::command]
pub fn codex_usage_set_enabled(enabled: bool) -> Value {
    usage::invalidate_usage_cache();
    usage_prefs::set_provider_enabled(PROVIDER, enabled);
    json!(scan_state(enabled))
}

#[tauri::command]
pub fn codex_usage_refresh() -> Value {
    usage::invalidate_usage_cache();
    json!(scan_state(usage_prefs::provider_enabled(PROVIDER, true)))
}

#[tauri::command]
pub fn codex_usage_get_summary(request: tauri::ipc::Request<'_>) -> Value {
    let (scope, range) = scope_range(&request);
    if !usage_prefs::provider_enabled(PROVIDER, true) {
        return json!({
            "scope": scope, "range": range, "sessions": 0, "events": 0,
            "inputTokens": 0, "cachedInputTokens": 0, "outputTokens": 0,
            "reasoningOutputTokens": 0, "totalTokens": 0, "estimatedCostUsd": null,
            "topModel": null, "topProject": null, "hasAnyCodexData": false
        });
    }
    json!(usage::codex_usage_summary(&scope, &range))
}

#[tauri::command]
pub fn codex_usage_get_daily(request: tauri::ipc::Request<'_>) -> Value {
    if !usage_prefs::provider_enabled(PROVIDER, true) {
        return json!([]);
    }
    let (scope, range) = scope_range(&request);
    json!(usage::codex_usage_daily(&scope, &range))
}

#[tauri::command]
pub fn codex_usage_get_breakdown(request: tauri::ipc::Request<'_>) -> Value {
    if !usage_prefs::provider_enabled(PROVIDER, true) {
        return json!([]);
    }
    let (scope, range) = scope_range(&request);
    let kind = if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        v.get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("model")
            .to_string()
    } else {
        "model".to_string()
    };
    json!(usage::codex_usage_breakdown(&scope, &range, &kind))
}

#[tauri::command]
pub fn codex_usage_get_recent_sessions(request: tauri::ipc::Request<'_>) -> Value {
    if !usage_prefs::provider_enabled(PROVIDER, true) {
        return json!([]);
    }
    let (scope, range) = scope_range(&request);
    let limit = if let tauri::ipc::InvokeBody::Json(v) = request.body() {
        v.get("limit").and_then(|l| l.as_u64()).unwrap_or(10) as usize
    } else {
        10
    };
    json!(usage::codex_usage_recent_sessions(&scope, &range, limit))
}
