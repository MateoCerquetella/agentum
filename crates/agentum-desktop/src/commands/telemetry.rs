use serde_json::{json, Value};

// The desktop has no telemetry backend. These handlers exist so the renderer's
// telemetry calls resolve instead of failing with "command not found". Tracking
// and opt-in are no-ops; consent always reports telemetry as disabled.

#[tauri::command]
pub fn telemetry_track() {}

#[tauri::command]
pub fn telemetry_set_opt_in() {}

#[tauri::command]
pub fn telemetry_acknowledge_banner() {}

#[tauri::command]
pub fn telemetry_get_consent_state() -> Value {
    // Matches TelemetryConsentState (shared/telemetry-consent-types.ts).
    json!({ "effective": "disabled", "reason": "agentum_disabled" })
}
