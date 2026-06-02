use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};
use tauri_plugin_dialog::DialogExt;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub fn default_shell_path() -> String {
    if cfg!(target_os = "windows") {
        std::env::var("COMSPEC")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "cmd.exe".to_string())
    } else {
        std::env::var("SHELL")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                which::which("zsh")
                    .ok()
                    .map(|path| path.display().to_string())
            })
            .or_else(|| {
                which::which("bash")
                    .ok()
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_else(|| "/bin/sh".to_string())
    }
}

async fn open_target(target: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = tokio::process::Command::new("open");
        command.arg(target);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/C", "start", "", target]);
        command
    } else {
        let mut command = tokio::process::Command::new("xdg-open");
        command.arg(target);
        command
    };

    let status = command.status().await.map_err(map_err)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to open target: {target}"))
    }
}

#[tauri::command]
pub async fn shell_get_default() -> Result<String, String> {
    Ok(default_shell_path())
}

#[tauri::command]
pub async fn shell_open_external(url: String) -> Result<(), String> {
    open_target(&url).await
}

#[tauri::command]
pub async fn shell_open_path(path: String) -> Result<(), String> {
    let target = Path::new(&path);
    if !target.exists() {
        return Err(format!("path does not exist: {path}"));
    }

    open_target(&path).await
}

#[tauri::command]
pub async fn shell_open_url(url: String) -> Result<(), String> {
    open_target(&url).await
}

#[tauri::command]
pub async fn shell_open_file_uri(uri: String) -> Result<(), String> {
    // The OS opener wants a path, not a file:// URI.
    let target = uri.strip_prefix("file://").unwrap_or(&uri);
    open_target(target).await
}

#[tauri::command]
pub async fn shell_open_file_path(path: String) -> Result<bool, String> {
    Ok(open_target(&path).await.is_ok())
}

#[tauri::command]
pub async fn shell_path_exists(path: String) -> Result<bool, String> {
    Ok(tokio::fs::metadata(&path).await.is_ok())
}

#[tauri::command]
pub async fn shell_copy_file(src_path: String, dest_path: String) -> Result<(), String> {
    if let Some(parent) = Path::new(&dest_path).parent() {
        tokio::fs::create_dir_all(parent).await.map_err(map_err)?;
    }
    tokio::fs::copy(src_path, dest_path).await.map_err(map_err)?;
    Ok(())
}

// Non-blocking file picker with optional extension filter (blocking_* would deadlock).
async fn pick_file(app: tauri::AppHandle, filter: Option<(&str, &[&str])>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut builder = app.dialog().file();
    if let Some((name, extensions)) = filter {
        builder = builder.add_filter(name, extensions);
    }
    builder.pick_file(move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten().map(|path| path.to_string())
}

#[tauri::command]
pub async fn shell_pick_directory(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten().map(|path| path.to_string())
}

#[tauri::command]
pub async fn shell_pick_image(app: tauri::AppHandle) -> Option<String> {
    pick_file(
        app,
        Some(("Images", &["png", "jpg", "jpeg", "gif", "webp", "svg"])),
    )
    .await
}

#[tauri::command]
pub async fn shell_pick_audio(app: tauri::AppHandle) -> Option<String> {
    pick_file(app, Some(("Audio", &["mp3", "wav", "m4a", "ogg", "flac"]))).await
}

#[tauri::command]
pub async fn shell_pick_attachment(app: tauri::AppHandle) -> Option<String> {
    pick_file(app, None).await
}

#[tauri::command]
pub async fn shell_open_in_file_manager(path: String) -> Result<Value, String> {
    let status = if cfg!(target_os = "macos") {
        tokio::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .status()
            .await
    } else if cfg!(target_os = "windows") {
        tokio::process::Command::new("explorer")
            .arg(format!("/select,{path}"))
            .status()
            .await
    } else {
        // No portable "reveal" on Linux; open the parent directory.
        let parent = Path::new(&path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| Path::new(&path).to_path_buf());
        tokio::process::Command::new("xdg-open")
            .arg(parent)
            .status()
            .await
    };
    Ok(match status {
        Ok(_) => json!({ "ok": true }),
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    })
}

#[tauri::command]
pub async fn shell_open_in_external_editor(
    path: String,
    command: Option<String>,
) -> Result<Value, String> {
    let editor = command
        .or_else(|| std::env::var("VISUAL").ok())
        .or_else(|| std::env::var("EDITOR").ok());
    match editor {
        Some(editor) if !editor.trim().is_empty() => {
            // $EDITOR may include flags ("code --wait"); split off the program.
            let mut parts = editor.split_whitespace();
            let program = parts.next().unwrap_or_default();
            let mut process = tokio::process::Command::new(program);
            for arg in parts {
                process.arg(arg);
            }
            process.arg(&path);
            Ok(match process.status().await {
                Ok(status) if status.success() => json!({ "ok": true }),
                Ok(status) => json!({ "ok": false, "error": format!("editor exited: {status}") }),
                Err(error) => json!({ "ok": false, "error": error.to_string() }),
            })
        }
        // No editor configured; fall back to the OS default handler.
        _ => Ok(match open_target(&path).await {
            Ok(()) => json!({ "ok": true }),
            Err(error) => json!({ "ok": false, "error": error }),
        }),
    }
}

#[tauri::command]
pub async fn shell_pick_repo_icon_image(app: tauri::AppHandle) -> Option<Value> {
    let path = pick_file(
        app,
        Some(("Images", &["png", "jpg", "jpeg", "gif", "webp", "svg"])),
    )
    .await?;
    let bytes = tokio::fs::read(&path).await.ok()?;
    let lower = path.to_lowercase();
    let mime = if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let file_name = Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    Some(json!({ "dataUrl": format!("data:{mime};base64,{encoded}"), "fileName": file_name }))
}
