use std::path::PathBuf;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// Dedicated traces dir so clearTraces never touches settings.sqlite3 or other state.
fn traces_dir() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or_else(|| "no data directory".to_string())?;
    let dir = base.join("Agentum").join("traces");
    std::fs::create_dir_all(&dir).map_err(map_err)?;
    Ok(dir)
}

#[tauri::command]
pub async fn diagnostics_open_trace_folder() -> Result<(), String> {
    let target = traces_dir()?.display().to_string();
    let mut command = if cfg!(target_os = "macos") {
        let mut command = tokio::process::Command::new("open");
        command.arg(&target);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = tokio::process::Command::new("explorer");
        command.arg(&target);
        command
    } else {
        let mut command = tokio::process::Command::new("xdg-open");
        command.arg(&target);
        command
    };
    // Ignore the exit status: explorer returns non-zero even on success.
    command.status().await.map_err(map_err)?;
    Ok(())
}

#[tauri::command]
pub fn diagnostics_clear_traces() -> Result<(), String> {
    let dir = traces_dir()?;
    for entry in std::fs::read_dir(&dir).map_err(map_err)? {
        let path = entry.map_err(map_err)?.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

// Diagnostics bundles (collecting/uploading/previewing a log archive) aren't ported;
// these preview/delete mutators no-op.
#[tauri::command]
pub fn diagnostics_open_bundle_preview() {}

#[tauri::command]
pub fn diagnostics_discard_bundle_preview() {}

#[tauri::command]
pub fn diagnostics_delete_bundle() {}

// Bundle collect/upload + overall status aren't ported; status reports idle, collect
// returns null, upload reports not-available.
#[tauri::command]
pub fn diagnostics_get_status() -> serde_json::Value {
    serde_json::json!({ "running": false })
}

#[tauri::command]
pub fn diagnostics_collect_bundle() -> Option<serde_json::Value> {
    None
}

#[tauri::command]
pub fn diagnostics_upload_bundle() -> serde_json::Value {
    serde_json::json!({ "ok": false, "error": "Diagnostics upload isn't available in this build." })
}
