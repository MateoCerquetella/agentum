use tauri_plugin_clipboard_manager::ClipboardExt;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub async fn clipboard_read(app: tauri::AppHandle) -> Result<String, String> {
    app.clipboard().read_text().map_err(map_err)
}

#[tauri::command]
pub async fn clipboard_write(app: tauri::AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(map_err)
}
