use std::path::PathBuf;

use serde_json::{json, Value};

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn session_path() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(".agentum")
        .join("session.json"))
}

fn default_state() -> Value {
    json!({
        "activeRepoId": null,
        "activeWorktreeId": null,
        "activeTabId": null,
        "tabsByWorktree": {},
        "terminalLayoutsByTabId": {}
    })
}

fn read_state() -> Value {
    let Ok(path) = session_path() else {
        return default_state();
    };
    if !path.exists() {
        return default_state();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or_else(default_state)
}

fn write_state(state: &Value) -> Result<(), String> {
    let path = session_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized = serde_json::to_string_pretty(state).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

#[tauri::command]
pub fn session_get() -> Value {
    read_state()
}

// set/setSync replace the whole state; both pass it as the entire payload.
fn replace_from_request(request: &tauri::ipc::Request<'_>) -> Result<(), String> {
    if let tauri::ipc::InvokeBody::Json(value) = request.body() {
        write_state(value)?;
    }
    Ok(())
}

#[tauri::command]
pub fn session_set(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    replace_from_request(&request)
}

#[tauri::command]
pub fn session_set_sync(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    replace_from_request(&request)
}

#[tauri::command]
pub fn session_patch(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Json(patch) = request.body() else {
        return Ok(());
    };
    let mut state = read_state();
    if let (Some(target), Some(updates)) = (state.as_object_mut(), patch.as_object()) {
        for (key, value) in updates {
            target.insert(key.clone(), value.clone());
        }
    }
    write_state(&state)
}
