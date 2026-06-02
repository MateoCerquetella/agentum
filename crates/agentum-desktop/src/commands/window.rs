fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub fn window_set_title(window: tauri::Window, title: String) -> Result<(), String> {
    window.set_title(&title).map_err(map_err)
}

#[tauri::command]
pub fn window_minimize(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(map_err)
}

#[tauri::command]
pub fn window_maximize(window: tauri::Window) -> Result<(), String> {
    window.maximize().map_err(map_err)
}

#[tauri::command]
pub fn window_close(window: tauri::Window) -> Result<(), String> {
    window.close().map_err(map_err)
}

#[tauri::command]
pub fn window_set_size(window: tauri::Window, width: f64, height: f64) -> Result<(), String> {
    window
        .set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)))
        .map_err(map_err)
}
