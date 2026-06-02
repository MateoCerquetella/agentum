use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use base64::Engine as _;
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{Emitter, State};

use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    name: String,
    is_directory: bool,
    is_symlink: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    size: u64,
    is_directory: bool,
    mtime: u64,
}

// Mirrors FsChangeEvent/FsChangedPayload in agentum/src/shared/types.ts. Emitted on
// the `fs-fs-changed` event that the renderer's fs.onFsChanged subscribes to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsChangeEvent {
    kind: String,
    absolute_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_absolute_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_directory: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsChangedPayload {
    worktree_path: String,
    events: Vec<FsChangeEvent>,
}

fn classify(kind: &notify::EventKind) -> &'static str {
    match kind {
        notify::EventKind::Create(_) => "create",
        notify::EventKind::Remove(_) => "delete",
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => "rename",
        notify::EventKind::Modify(_) => "update",
        _ => "update",
    }
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn guess_mime(file_path: &str) -> Option<&'static str> {
    let lower = file_path.to_lowercase();
    if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if lower.ends_with(".gif") {
        Some("image/gif")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else if lower.ends_with(".svg") {
        Some("image/svg+xml")
    } else if lower.ends_with(".pdf") {
        Some("application/pdf")
    } else {
        None
    }
}

// Returns { content, isBinary, isImage?, mimeType? }. Binary content is base64.
#[tauri::command]
pub async fn fs_read_file(
    file_path: String,
    connection_id: Option<String>,
) -> Result<Value, String> {
    let _ = connection_id; // SSH transport not ported yet; local only.
    let bytes = tokio::fs::read(&file_path).await.map_err(map_err)?;
    let mut object = Map::new();
    if is_binary(&bytes) {
        object.insert(
            "content".into(),
            base64::engine::general_purpose::STANDARD
                .encode(&bytes)
                .into(),
        );
        object.insert("isBinary".into(), true.into());
        if let Some(mime) = guess_mime(&file_path) {
            object.insert("isImage".into(), mime.starts_with("image/").into());
            object.insert("mimeType".into(), mime.into());
        }
    } else {
        object.insert(
            "content".into(),
            String::from_utf8_lossy(&bytes).into_owned().into(),
        );
        object.insert("isBinary".into(), false.into());
    }
    Ok(Value::Object(object))
}

#[tauri::command]
pub async fn fs_write_file(
    file_path: String,
    content: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    if let Some(parent) = Path::new(&file_path).parent() {
        tokio::fs::create_dir_all(parent).await.map_err(map_err)?;
    }
    tokio::fs::write(file_path, content).await.map_err(map_err)
}

#[tauri::command]
pub async fn fs_read_dir(
    dir_path: String,
    connection_id: Option<String>,
) -> Result<Vec<DirEntry>, String> {
    let _ = connection_id;
    let mut entries = tokio::fs::read_dir(&dir_path).await.map_err(map_err)?;
    let mut results = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(map_err)? {
        let file_type = entry.file_type().await.map_err(map_err)?;
        results.push(DirEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            is_directory: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
        });
    }
    results.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(results)
}

#[tauri::command]
pub async fn fs_create_file(
    file_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    if let Some(parent) = Path::new(&file_path).parent() {
        tokio::fs::create_dir_all(parent).await.map_err(map_err)?;
    }
    // create_new: never truncate an existing file.
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file_path)
        .await
        .map_err(map_err)?;
    Ok(())
}

#[tauri::command]
pub async fn fs_create_dir(
    dir_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    tokio::fs::create_dir_all(dir_path).await.map_err(map_err)
}

#[tauri::command]
pub async fn fs_rename(
    old_path: String,
    new_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    tokio::fs::rename(old_path, new_path).await.map_err(map_err)
}

#[tauri::command]
pub async fn fs_copy(
    source_path: String,
    destination_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    if let Some(parent) = Path::new(&destination_path).parent() {
        tokio::fs::create_dir_all(parent).await.map_err(map_err)?;
    }
    tokio::fs::copy(source_path, destination_path)
        .await
        .map_err(map_err)?;
    Ok(())
}

#[tauri::command]
pub async fn fs_delete_path(
    target_path: String,
    connection_id: Option<String>,
    recursive: Option<bool>,
) -> Result<(), String> {
    let _ = connection_id;
    let metadata = tokio::fs::metadata(&target_path).await.map_err(map_err)?;
    if metadata.is_dir() {
        if recursive.unwrap_or(true) {
            tokio::fs::remove_dir_all(target_path).await.map_err(map_err)
        } else {
            tokio::fs::remove_dir(target_path).await.map_err(map_err)
        }
    } else {
        tokio::fs::remove_file(target_path).await.map_err(map_err)
    }
}

#[tauri::command]
pub async fn fs_stat(file_path: String, connection_id: Option<String>) -> Result<FileStat, String> {
    let _ = connection_id;
    let metadata = tokio::fs::metadata(&file_path).await.map_err(map_err)?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0);
    Ok(FileStat {
        size: metadata.len(),
        is_directory: metadata.is_dir(),
        mtime,
    })
}

#[tauri::command]
pub fn fs_authorize_external_path(target_path: String) -> Result<(), String> {
    let _ = target_path;
    // Why: Electron sandboxes renderer fs access behind an allowlist; our commands
    // read any path the OS permits, so authorization is a no-op here.
    Ok(())
}

// Content search (ripgrep) and the agent-upload staging pipeline aren't ported;
// these report empty results in the exact contract shapes.
#[tauri::command]
pub fn fs_search() -> Value {
    serde_json::json!({ "files": [], "totalMatches": 0, "truncated": false })
}

#[tauri::command]
pub fn fs_import_external_paths() -> Value {
    serde_json::json!({ "results": [] })
}

#[tauri::command]
pub fn fs_stage_external_paths_for_runtime_upload() -> Value {
    serde_json::json!({ "sources": [] })
}

#[tauri::command]
pub fn fs_resolve_dropped_paths_for_agent() -> Value {
    serde_json::json!({ "resolvedPaths": [], "skipped": [], "failed": [] })
}

#[tauri::command]
pub async fn fs_list_files(
    root_path: String,
    connection_id: Option<String>,
    exclude_paths: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    let _ = connection_id;
    let excludes = exclude_paths.unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        let mut stack = vec![PathBuf::from(&root_path)];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().is_some_and(|name| name == ".git") {
                    continue;
                }
                let path_str = path.to_string_lossy();
                if excludes.iter().any(|exclude| path_str.contains(exclude.as_str())) {
                    continue;
                }
                match entry.file_type() {
                    Ok(file_type) if file_type.is_dir() => stack.push(path),
                    Ok(file_type) if file_type.is_file() => files.push(path_str.into_owned()),
                    _ => {}
                }
            }
        }
        files.sort();
        Ok(files)
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn fs_list_markdown_documents(
    root_path: String,
    connection_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let _ = connection_id;
    tokio::task::spawn_blocking(move || {
        let root = PathBuf::from(&root_path);
        let mut documents = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .file_name()
                    .is_some_and(|name| name == ".git" || name == "node_modules")
                {
                    continue;
                }
                match entry.file_type() {
                    Ok(file_type) if file_type.is_dir() => stack.push(path),
                    Ok(file_type) if file_type.is_file() => {
                        let lower = path.to_string_lossy().to_lowercase();
                        if !(lower.ends_with(".md") || lower.ends_with(".markdown")) {
                            continue;
                        }
                        let file_path = path.to_string_lossy().into_owned();
                        let relative_path = path
                            .strip_prefix(&root)
                            .map(|relative| relative.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| file_path.clone());
                        let basename = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let name = path
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_else(|| basename.clone());
                        documents.push(serde_json::json!({
                            "filePath": file_path,
                            "relativePath": relative_path,
                            "basename": basename,
                            "name": name,
                        }));
                    }
                    _ => {}
                }
            }
        }
        Ok(documents)
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn fs_exists(path: String) -> Result<bool, String> {
    Ok(tokio::fs::metadata(path).await.is_ok())
}

#[tauri::command]
pub async fn fs_mkdir(path: String, recursive: bool) -> Result<(), String> {
    if recursive {
        tokio::fs::create_dir_all(path).await.map_err(map_err)
    } else {
        tokio::fs::create_dir(path).await.map_err(map_err)
    }
}

#[tauri::command]
pub async fn fs_remove(path: String) -> Result<(), String> {
    let metadata = tokio::fs::metadata(&path).await.map_err(map_err)?;
    if metadata.is_dir() {
        tokio::fs::remove_dir_all(path).await.map_err(map_err)
    } else {
        tokio::fs::remove_file(path).await.map_err(map_err)
    }
}

#[tauri::command]
pub async fn fs_watch_worktree(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    worktree_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    let watch_path = PathBuf::from(&worktree_path);
    if !watch_path.exists() {
        return Err(format!("path does not exist: {worktree_path}"));
    }

    let emit_path = worktree_path.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        let kind = classify(&event.kind);
        // A two-path rename reports old→new in a single event.
        let events: Vec<FsChangeEvent> = if kind == "rename" && event.paths.len() == 2 {
            vec![FsChangeEvent {
                kind: kind.to_string(),
                absolute_path: event.paths[1].display().to_string(),
                old_absolute_path: Some(event.paths[0].display().to_string()),
                is_directory: None,
            }]
        } else {
            event
                .paths
                .iter()
                .map(|path| FsChangeEvent {
                    kind: kind.to_string(),
                    absolute_path: path.display().to_string(),
                    old_absolute_path: None,
                    is_directory: path.is_dir().then_some(true),
                })
                .collect()
        };
        if !events.is_empty() {
            let _ = app.emit(
                "fs-fs-changed",
                FsChangedPayload {
                    worktree_path: emit_path.clone(),
                    events,
                },
            );
        }
    })
    .map_err(map_err)?;

    watcher
        .watch(&watch_path, RecursiveMode::Recursive)
        .map_err(map_err)?;
    state.watchers.lock().insert(worktree_path, watcher);
    Ok(())
}

#[tauri::command]
pub fn fs_unwatch_worktree(
    state: State<'_, AppState>,
    worktree_path: String,
    connection_id: Option<String>,
) -> Result<(), String> {
    let _ = connection_id;
    state.watchers.lock().remove(&worktree_path);
    Ok(())
}
