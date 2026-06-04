//! Repo registry + git-ref logic moved to the embedded agentum-server
//! (`agentum-server/src/routes/repos.rs`); the desktop UI calls it over loopback
//! HTTP. Only the native folder-picker dialog stays here — it needs a Tauri
//! window, so it can't live in the server.

use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub async fn repos_pick_folder(app: tauri::AppHandle) -> Option<String> {
    pick_folder(app).await
}

#[tauri::command]
pub async fn repos_pick_directory(app: tauri::AppHandle) -> Option<String> {
    pick_folder(app).await
}

/// Non-blocking folder dialog (blocking_* would deadlock the command thread).
async fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten().map(|path| path.to_string())
}
