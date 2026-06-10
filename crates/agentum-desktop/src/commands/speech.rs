//! Tauri command surface for Voice dictation. Names + argument shapes match the
//! renderer's wire contract (`ui/src/tauri/speech.ts`); the real work lives in
//! `crate::speech`.

use serde_json::Value;
use tauri::{AppHandle, State};

use crate::speech_engine::catalog::{SpeechModelManifest, SPEECH_MODEL_CATALOG};
use crate::speech_engine::model_manager::SpeechModelState;
use crate::speech_engine::SpeechState;

#[tauri::command]
pub fn speech_get_catalog() -> Vec<SpeechModelManifest> {
    SPEECH_MODEL_CATALOG.to_vec()
}

#[tauri::command]
pub fn speech_get_model_states(state: State<'_, SpeechState>) -> Vec<SpeechModelState> {
    state.models.get_model_states()
}

#[tauri::command]
pub async fn speech_download_model(
    app: AppHandle,
    state: State<'_, SpeechState>,
    value: String,
) -> Result<(), String> {
    // Clone the Arc out so the download can outlive the borrowed State across awaits.
    let models = state.models.clone();
    models
        .download_model(&app, &value)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn speech_cancel_download(state: State<'_, SpeechState>, value: String) {
    state.models.cancel_download(&value);
}

#[tauri::command]
pub fn speech_delete_model(state: State<'_, SpeechState>, value: String) -> Result<(), String> {
    state.models.delete_model(&value).map_err(|e| e.to_string())
}

// startDictation(modelId, hotwordsFilePath?, sessionId) → { args: [...] }.
// Async + spawn_blocking: loading the recognizer (cold start) takes seconds and
// must not block the webview/UI thread, or the window freezes and clicking it
// while "Starting…" can crash the app.
#[tauri::command]
pub async fn speech_start_dictation(
    state: State<'_, SpeechState>,
    args: Vec<Value>,
) -> Result<(), String> {
    let model_id = args
        .first()
        .and_then(Value::as_str)
        .ok_or("missing modelId")?
        .to_string();
    // sessionId is the last positional arg (hotwordsFilePath in the middle is
    // currently always undefined from the renderer).
    let session_id = args
        .get(2)
        .or_else(|| args.last())
        .and_then(Value::as_str)
        .ok_or("missing sessionId")?
        .to_string();
    let service = state.service.clone();
    let models = state.models.clone();
    tauri::async_runtime::spawn_blocking(move || service.start(&models, &model_id, &session_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// feedAudio: the hottest path (~12 chunks/sec during dictation). The renderer
// sends the Float32 samples as the raw IPC body (the ArrayBuffer) with the sample
// rate + session id in headers, so there is zero JSON (de)serialization and no
// per-sample boxing — just a byte→f32 reinterpret. A JSON body (number[] +
// rate + sessionId) is still accepted as a fallback for any non-raw caller.
#[tauri::command]
pub fn speech_feed_audio(
    app: AppHandle,
    state: State<'_, SpeechState>,
    request: tauri::ipc::Request<'_>,
) {
    match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => {
            let headers = request.headers();
            let sample_rate = headers
                .get("x-sample-rate")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let Some(session_id) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) else {
                return;
            };
            let samples = bytes_to_f32_le(bytes);
            state.service.feed(&app, &samples, sample_rate, session_id);
        }
        tauri::ipc::InvokeBody::Json(value) => {
            let args = value
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let Some(samples) = args.first().and_then(parse_samples) else {
                return;
            };
            let sample_rate = args.get(1).and_then(Value::as_f64).map(|n| n as u32).unwrap_or(0);
            let Some(session_id) = args.get(2).or_else(|| args.last()).and_then(Value::as_str)
            else {
                return;
            };
            state.service.feed(&app, &samples, sample_rate, session_id);
        }
    }
}

/// Reinterpret little-endian f32 bytes (the platform-native layout of a JS
/// Float32Array on x86/ARM) as samples. A trailing partial sample is ignored.
fn bytes_to_f32_le(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

// stopDictation(sessionId) → single non-object arg → { value }.
// Async + spawn_blocking: an offline recognizer decodes the entire buffered
// utterance here, which is heavy and must not block the UI thread.
#[tauri::command]
pub async fn speech_stop_dictation(
    app: AppHandle,
    state: State<'_, SpeechState>,
    value: String,
) -> Result<(), String> {
    let service = state.service.clone();
    tauri::async_runtime::spawn_blocking(move || service.stop(&app, &value))
        .await
        .map_err(|e| e.to_string())
}

fn parse_samples(value: &Value) -> Option<Vec<f32>> {
    let arr = value.as_array()?;
    Some(
        arr.iter()
            .filter_map(|v| v.as_f64().map(|n| n as f32))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::bytes_to_f32_le;

    #[test]
    fn reinterprets_le_f32_bytes_and_ignores_trailing_partial() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1.0f32.to_le_bytes());
        bytes.extend_from_slice(&(-0.5f32).to_le_bytes());
        bytes.push(0x00); // trailing partial sample — must be dropped
        assert_eq!(bytes_to_f32_le(&bytes), vec![1.0, -0.5]);
    }
}
