use std::path::PathBuf;

use serde_json::{Map, Value};
use tauri::Manager;
use tauri_plugin_clipboard_manager::ClipboardExt;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// PersistedUIState is a renderer-owned preferences blob; this layer stores it
// opaquely (no per-field modeling) and shallow-merges partial updates.
fn ui_state_path() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(".agentum")
        .join("ui-state.json"))
}

fn read_ui_state() -> Map<String, Value> {
    let Ok(path) = ui_state_path() else {
        return Map::new();
    };
    if !path.exists() {
        return Map::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn write_ui_state(state: &Map<String, Value>) -> Result<(), String> {
    let path = ui_state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized =
        serde_json::to_string_pretty(&Value::Object(state.clone())).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

#[tauri::command]
pub fn ui_get() -> Result<Value, String> {
    Ok(Value::Object(read_ui_state()))
}

#[tauri::command]
pub fn ui_record_feature_interaction(value: String) -> Result<Value, String> {
    let mut state = read_ui_state();
    let interactions = state
        .entry("featureInteractions")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(object) = interactions.as_object_mut() {
        object.insert(value, Value::Bool(true));
    }
    write_ui_state(&state)?;
    Ok(Value::Object(state))
}

#[tauri::command]
pub fn ui_write_clipboard_text(app: tauri::AppHandle, value: String) -> Result<(), String> {
    app.clipboard().write_text(value).map_err(map_err)
}

#[tauri::command]
pub fn ui_read_clipboard_text(app: tauri::AppHandle) -> Result<String, String> {
    app.clipboard().read_text().map_err(map_err)
}

fn main_window(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window("main")
}

#[tauri::command]
pub fn ui_minimize(app: tauri::AppHandle) {
    if let Some(window) = main_window(&app) {
        let _ = window.minimize();
    }
}

#[tauri::command]
pub fn ui_maximize(app: tauri::AppHandle) {
    if let Some(window) = main_window(&app) {
        if window.is_maximized().unwrap_or(false) {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
    }
}

#[tauri::command]
pub fn ui_is_maximized(app: tauri::AppHandle) -> bool {
    main_window(&app)
        .and_then(|window| window.is_maximized().ok())
        .unwrap_or(false)
}

#[tauri::command]
pub fn ui_request_close(app: tauri::AppHandle) {
    if let Some(window) = main_window(&app) {
        let _ = window.close();
    }
}

// ui.set passes a Partial<PersistedUIState> as the WHOLE invoke payload (no wrapper
// key), so it reads the raw request body rather than a named arg, then shallow-merges.
#[tauri::command]
pub fn ui_set(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let updates = match request.body() {
        tauri::ipc::InvokeBody::Json(value) => value.as_object().cloned().unwrap_or_default(),
        tauri::ipc::InvokeBody::Raw(bytes) => serde_json::from_slice::<Value>(bytes)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default(),
    };
    let mut state = read_ui_state();
    for (key, value) in updates {
        state.insert(key, value);
    }
    write_ui_state(&state)
}

#[tauri::command]
pub fn ui_get_zoom_level() -> f64 {
    read_ui_state()
        .get("zoomLevel")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

#[tauri::command]
pub fn ui_set_zoom_level(app: tauri::AppHandle, value: f64) -> Result<(), String> {
    let mut state = read_ui_state();
    state.insert("zoomLevel".into(), Value::from(value));
    write_ui_state(&state)?;
    // Electron zoom levels are logarithmic (1.2^level); convert to a scale factor.
    if let Some(window) = main_window(&app) {
        let _ = window.set_zoom(1.2_f64.powf(value));
    }
    Ok(())
}

// Focus-state setters inform main for global-shortcut routing, which isn't ported
// yet — accept and no-op so callers don't error.
#[tauri::command]
pub fn ui_set_markdown_editor_focused(value: bool) {
    let _ = value;
}

#[tauri::command]
pub fn ui_set_floating_terminal_input_focused(value: bool) {
    let _ = value;
}

#[tauri::command]
pub fn ui_set_terminal_input_focused(value: bool) {
    let _ = value;
}

#[tauri::command]
pub fn ui_set_shortcut_recorder_focused(value: bool) {
    let _ = value;
}

// Request/reply responses (main-initiated tab/terminal flows) and native chrome
// ops (traffic-light sync, context menu, close confirmation) aren't ported yet.
// These are fire-and-forget void methods, so accept and no-op.
#[tauri::command]
pub fn ui_reply_tab_create() {}

#[tauri::command]
pub fn ui_reply_tab_set_profile() {}

#[tauri::command]
pub fn ui_reply_tab_close() {}

#[tauri::command]
pub fn ui_reply_terminal_create() {}

#[tauri::command]
pub fn ui_sync_traffic_lights() {}

#[tauri::command]
pub fn ui_popup_menu() {}

#[tauri::command]
pub fn ui_confirm_window_close() {}

// Clipboard image read/write isn't ported; save-as-temp returns no path, write no-ops.
#[tauri::command]
pub fn ui_save_clipboard_image_as_temp_file() -> Option<String> {
    None
}

#[tauri::command]
pub fn ui_write_clipboard_image() {}
