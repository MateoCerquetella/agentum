use serde::Serialize;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Serialize)]
pub struct AppIdentity {
    pub name: String,
    pub version: String,
    pub platform: String,
}

#[tauri::command]
pub fn app_get_identity() -> AppIdentity {
    AppIdentity {
        name: "Orca".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: app_get_platform(),
    }
}

#[tauri::command]
pub fn app_get_platform() -> String {
    if cfg!(target_os = "macos") {
        "darwin".to_string()
    } else if cfg!(target_os = "windows") {
        "win32".to_string()
    } else {
        "linux".to_string()
    }
}

#[tauri::command]
pub fn app_relaunch(app: tauri::AppHandle) {
    app.restart();
}

#[tauri::command]
pub fn app_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownDocument {
    pub file_path: String,
    pub relative_path: String,
    pub basename: String,
    pub name: String,
}

#[tauri::command]
pub fn app_restart(app: tauri::AppHandle) {
    app.restart();
}

#[tauri::command]
pub fn app_reload(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("window.location.reload()");
    }
}

#[tauri::command]
pub fn app_get_keyboard_input_source_id() -> Option<String> {
    // Why: native macOS keyboard-layout probe isn't ported yet; null matches the
    // web adapter and the documented non-Darwin behavior.
    None
}

#[tauri::command]
pub fn app_set_unread_dock_badge_count(_count: i64) {
    // Why: macOS Dock badge integration isn't ported yet — no-op on every
    // platform, matching the web adapter (Windows/Linux are no-ops by contract).
}

#[tauri::command]
pub fn app_get_floating_terminal_cwd() -> String {
    dirs::home_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

#[tauri::command]
pub fn app_get_floating_markdown_directory() -> Result<String, String> {
    let base = dirs::data_dir().ok_or_else(|| "no data directory".to_string())?;
    let dir = base.join("Agentum").join("FloatingMarkdown");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.display().to_string())
}

#[tauri::command]
pub async fn app_pick_floating_workspace_directory(app: tauri::AppHandle) -> Option<String> {
    // Non-blocking dialog + oneshot: blocking_pick_* would deadlock on the
    // command thread.
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten().map(|path| path.to_string())
}

#[tauri::command]
pub async fn app_pick_floating_markdown_document(
    app: tauri::AppHandle,
) -> Option<MarkdownDocument> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown"])
        .pick_file(move |path| {
            let _ = tx.send(path);
        });
    let selected = rx.await.ok().flatten()?;
    let path_buf = selected.into_path().ok()?;
    let basename = path_buf.file_name()?.to_string_lossy().to_string();
    let name = path_buf
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| basename.clone());
    Some(MarkdownDocument {
        file_path: path_buf.display().to_string(),
        relative_path: basename.clone(),
        basename,
        name,
    })
}
