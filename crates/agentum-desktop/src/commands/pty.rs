use std::io::{Read, Write};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, State};

use crate::{
    commands::shell::default_shell_path,
    state::{AppState, PtyHandle},
};

#[derive(Debug, Clone, Serialize)]
struct PtyOutputEvent {
    id: String,
    data: String,
    error: Option<String>,
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub async fn pty_create(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    shell: Option<String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    {
        let ptys = state.ptys.lock();
        if ptys.contains_key(&id) {
            return Err(format!("pty already exists: {id}"));
        }
    }

    let shell_path = shell.unwrap_or_else(default_shell_path);
    let id_for_reader = id.clone();
    let app_handle = app.clone();

    let handle = tokio::task::spawn_blocking(move || -> Result<PtyHandle, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(map_err)?;

        let mut command = CommandBuilder::new(shell_path);
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }

        let child = pair.slave.spawn_command(command).map_err(map_err)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(map_err)?;
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        let data = String::from_utf8_lossy(&buffer[..read]).to_string();
                        let _ = app_handle.emit(
                            "pty:output",
                            PtyOutputEvent {
                                id: id_for_reader.clone(),
                                data,
                                error: None,
                            },
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = app_handle.emit(
                            "pty:output",
                            PtyOutputEvent {
                                id: id_for_reader.clone(),
                                data: String::new(),
                                error: Some(error.to_string()),
                            },
                        );
                        break;
                    }
                }
            }
        });

        let writer = pair.master.take_writer().map_err(map_err)?;
        Ok(PtyHandle {
            master: pair.master,
            writer,
            child,
        })
    })
    .await
    .map_err(map_err)??;

    state.ptys.lock().insert(id, handle);
    Ok(())
}

// pty methods use positional args, so the bridge delivers them as { args: [...] }
// (or { value: x } for a single arg). Flatten back to a positional list.
fn positional_args(request: &tauri::ipc::Request<'_>) -> Vec<Value> {
    let tauri::ipc::InvokeBody::Json(value) = request.body() else {
        return Vec::new();
    };
    if let Some(array) = value.get("args").and_then(Value::as_array) {
        return array.clone();
    }
    value
        .get("value")
        .map(|single| vec![single.clone()])
        .unwrap_or_default()
}

#[tauri::command]
pub fn pty_write(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    let args = positional_args(&request);
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| "pty_write: missing id".to_string())?;
    let data = args.get(1).and_then(Value::as_str).unwrap_or_default();
    let mut ptys = state.ptys.lock();
    let handle = ptys.get_mut(id).ok_or_else(|| format!("unknown pty: {id}"))?;
    handle.writer.write_all(data.as_bytes()).map_err(map_err)?;
    handle.writer.flush().map_err(map_err)
}

#[tauri::command]
pub fn pty_resize(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    let args = positional_args(&request);
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| "pty_resize: missing id".to_string())?;
    let cols = args.get(1).and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = args.get(2).and_then(Value::as_u64).unwrap_or(24) as u16;
    let mut ptys = state.ptys.lock();
    let handle = ptys.get_mut(id).ok_or_else(|| format!("unknown pty: {id}"))?;
    handle
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(map_err)
}

#[tauri::command]
pub fn pty_kill(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    let args = positional_args(&request);
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| "pty_kill: missing id".to_string())?;
    // opts (keepHistory) is accepted but not yet honored.
    let mut handle = state
        .ptys
        .lock()
        .remove(id)
        .ok_or_else(|| format!("unknown pty: {id}"))?;
    handle.child.kill().map_err(map_err)
}

#[tauri::command]
pub fn pty_write_accepted(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<bool, String> {
    let args = positional_args(&request);
    let id = args
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| "pty_write_accepted: missing id".to_string())?;
    let data = args.get(1).and_then(Value::as_str).unwrap_or_default();
    let mut ptys = state.ptys.lock();
    let handle = ptys.get_mut(id).ok_or_else(|| format!("unknown pty: {id}"))?;
    handle.writer.write_all(data.as_bytes()).map_err(map_err)?;
    handle.writer.flush().map_err(map_err)?;
    Ok(true)
}

#[tauri::command]
pub fn pty_pause_output(request: tauri::ipc::Request<'_>) {
    // Output pausing needs read-loop cooperation, which isn't ported yet. No-op.
    let _ = request;
}

#[tauri::command]
pub fn pty_has_child_processes(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> bool {
    let args = positional_args(&request);
    let Some(id) = args.first().and_then(Value::as_str) else {
        return false;
    };
    let ptys = state.ptys.lock();
    let Some(handle) = ptys.get(id) else {
        return false;
    };
    // A foreground command is running when the fg process group leader isn't the shell.
    match (foreground_pid(handle), handle.child.process_id()) {
        (Some(fg), Some(shell)) => fg as u32 != shell,
        _ => false,
    }
}

#[tauri::command]
pub fn pty_list_sessions(state: State<'_, AppState>) -> Vec<Value> {
    // cwd/title aren't tracked on PtyHandle yet; return ids with empty fields.
    state
        .ptys
        .lock()
        .keys()
        .map(|id| serde_json::json!({ "id": id, "cwd": "", "title": "" }))
        .collect()
}

// Buffer snapshots, foreground-process detection, pane-serializer lifecycle, signals,
// cwd, and the management RPC aren't ported. Snapshots/process/cwd are null; the
// serializer + lifecycle methods no-op.
#[tauri::command]
pub fn pty_get_main_buffer_snapshot() -> Option<String> {
    None
}

// --- Live PTY process introspection (ports local-pty-provider getForegroundProcess/
// getCwd/hasChildProcesses). On Unix the foreground process group leader is the
// tcgetpgrp equivalent exposed by portable_pty. ---

// Foreground process group leader of the pty (None where the platform can't report it).
#[cfg(unix)]
fn foreground_pid(handle: &PtyHandle) -> Option<i32> {
    handle.master.process_group_leader()
}
#[cfg(not(unix))]
fn foreground_pid(_handle: &PtyHandle) -> Option<i32> {
    None
}

// Basename of a pid's executable, matching node-pty's `.process` (e.g. "zsh", "vim").
fn process_name(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!raw.is_empty()).then(|| raw.rsplit(['/', '\\']).next().unwrap_or(&raw).to_string())
}

// cwd of a process by pid: lsof on macOS, /proc on Linux, unsupported elsewhere.
#[cfg(target_os = "macos")]
fn resolve_process_cwd(pid: u32) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n').map(str::to_string))
}
#[cfg(target_os = "linux")]
fn resolve_process_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn resolve_process_cwd(_pid: u32) -> Option<String> {
    None
}

#[tauri::command]
pub fn pty_get_foreground_process(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Option<String> {
    let args = positional_args(&request);
    let id = args.first().and_then(Value::as_str)?;
    let pid = {
        let ptys = state.ptys.lock();
        foreground_pid(ptys.get(id)?)?
    };
    process_name(pid as u32)
}

#[tauri::command]
pub fn pty_clear_pending_pane_serializer() {}

#[tauri::command]
pub fn pty_management() -> Option<Value> {
    None
}

#[tauri::command]
pub fn pty_ack_cold_restore() {}

#[tauri::command]
pub fn pty_declare_pending_pane_serializer() {}

#[tauri::command]
pub fn pty_settle_pane_serializer() {}

#[tauri::command]
pub fn pty_signal() {}

#[tauri::command]
pub fn pty_send_serialized_buffer() {}

#[tauri::command]
pub fn pty_get_cwd(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Option<String> {
    let args = positional_args(&request);
    let id = args.first().and_then(Value::as_str)?;
    // The pty's direct child is the shell; its cwd reflects `cd` (a shell builtin).
    let pid = {
        let ptys = state.ptys.lock();
        ptys.get(id)?.child.process_id()?
    };
    resolve_process_cwd(pid)
}

#[tauri::command]
pub fn pty_spawn() {}

#[tauri::command]
pub fn pty_report_geometry() {}
