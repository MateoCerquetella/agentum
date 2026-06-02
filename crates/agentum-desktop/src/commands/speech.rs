use serde_json::Value;

// The speech-to-text engine (model download + on-device dictation) isn't ported.
// Query methods return empty so the renderer shows "no models" rather than crashing;
// mutators are honest no-ops. modelId arrives as { value } (single positional arg).

#[tauri::command]
pub fn speech_get_catalog() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn speech_get_model_states() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn speech_download_model(value: String) {
    let _ = value;
}

#[tauri::command]
pub fn speech_cancel_download(value: String) {
    let _ = value;
}

#[tauri::command]
pub fn speech_delete_model(value: String) {
    let _ = value;
}

// On-device dictation (audio capture → streaming STT) isn't ported; accept and no-op.
#[tauri::command]
pub fn speech_start_dictation() {}

#[tauri::command]
pub fn speech_feed_audio() {}

#[tauri::command]
pub fn speech_stop_dictation() {}
