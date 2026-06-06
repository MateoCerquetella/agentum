use std::{
    io::{Read, Write},
    sync::Arc,
};

use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, State};

use crate::{
    commands::shell::default_shell_path,
    state::{AppState, PtyHandle, PtyOutputBuffer},
};

// Emitted on the channels the renderer's pty-dispatcher listens to (onData ->
// "pty-data", onExit -> "pty-exit"). The old code emitted "pty:output", which no
// listener matched — that is why the local terminal produced no output.
//
// `seq`/`rawLength` let the renderer dedupe live chunks against a buffer
// snapshot during hidden-output restore: `seq` is the running total of raw
// bytes through this chunk, `rawLength` this chunk's raw byte count.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyDataEvent {
    id: String,
    data: String,
    seq: u64,
    raw_length: u64,
}

#[derive(Debug, Clone, Serialize)]
struct PtyExitEvent {
    id: String,
    code: i32,
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// What program to run in a fresh PTY and how to size/place it.
struct SpawnConfig {
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: Vec<(String, String)>,
    cols: u16,
    rows: u16,
}

// Open a PTY, spawn the program, wire the reader thread (-> "pty-data") and the
// exit watcher (-> "pty-exit"), and return the handle. Blocking; call via
// spawn_blocking. Shared by pty_create and pty_spawn so the read/exit wiring
// lives in exactly one place.
fn open_pty(app: tauri::AppHandle, id: String, cfg: SpawnConfig) -> Result<PtyHandle, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: cfg.rows,
            cols: cfg.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(map_err)?;

    let mut command = CommandBuilder::new(&cfg.program);
    for arg in &cfg.args {
        command.arg(arg);
    }
    if let Some(cwd) = &cfg.cwd {
        command.cwd(cwd);
    }
    for (key, value) in &cfg.env {
        command.env(key, value);
    }

    let child = pair.slave.spawn_command(command).map_err(map_err)?;
    drop(pair.slave);
    let child = Arc::new(Mutex::new(child));

    let output = Arc::new(Mutex::new(PtyOutputBuffer::new(cfg.cols, cfg.rows)));

    let mut reader = pair.master.try_clone_reader().map_err(map_err)?;
    let app_reader = app.clone();
    let id_reader = id.clone();
    let child_reader = child.clone();
    let output_reader = output.clone();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let chunk = &buffer[..read];
                    // Retain raw bytes (not the lossy string) so the snapshot can
                    // be rebuilt faithfully, and capture `seq` under the same lock
                    // so it matches exactly what the snapshot would include.
                    let seq = output_reader.lock().push(chunk);
                    let data = String::from_utf8_lossy(chunk).to_string();
                    let _ = app_reader.emit(
                        "pty-data",
                        PtyDataEvent {
                            id: id_reader.clone(),
                            data,
                            seq,
                            raw_length: read as u64,
                        },
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        // EOF means the child closed the slave, so wait() returns its code at once.
        let code = child_reader
            .lock()
            .wait()
            .ok()
            .map(|status| status.exit_code() as i32)
            .unwrap_or(0);
        let _ = app_reader.emit("pty-exit", PtyExitEvent { id: id_reader, code });
    });

    let writer = pair.master.take_writer().map_err(map_err)?;
    Ok(PtyHandle {
        master: pair.master,
        writer,
        child,
        output,
    })
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

    let program = shell.unwrap_or_else(default_shell_path);
    let ptys = state.ptys.clone();
    let id_for_insert = id.clone();
    let handle = tokio::task::spawn_blocking(move || {
        open_pty(
            app,
            id,
            SpawnConfig {
                program,
                args: Vec::new(),
                cwd,
                env: Vec::new(),
                cols,
                rows,
            },
        )
    })
    .await
    .map_err(map_err)??;

    ptys.lock().insert(id_for_insert, handle);
    Ok(())
}

// The renderer's IPC terminal transport calls `pty.spawn(options)` and expects a
// `{ id }` back (it allocates no id itself). Options: { cols, rows, cwd, env,
// command, shellOverride, … }. `command` is an optional startup command line; when
// absent we launch an interactive shell. Without this the transport saw `null` and
// threw "terminal failed to spawn (no pty handle returned)".
#[tauri::command]
pub async fn pty_spawn(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    // Take an owned copy of the options before any await — Request borrows the
    // invoke message and must not be held across the await point.
    let opts: Value = match request.body() {
        tauri::ipc::InvokeBody::Json(value) => value.clone(),
        _ => Value::Null,
    };
    drop(request);

    let cols = opts.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = opts.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
    let cwd = opts.get("cwd").and_then(Value::as_str).map(str::to_string);
    let command = opts
        .get("command")
        .and_then(Value::as_str)
        .filter(|cmd| !cmd.is_empty())
        .map(str::to_string);
    let shell_override = opts
        .get("shellOverride")
        .and_then(Value::as_str)
        .map(str::to_string);
    let env: Vec<(String, String)> = opts
        .get("env")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| value.as_str().map(|v| (key.clone(), v.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let program = shell_override.unwrap_or_else(default_shell_path);
    // No command -> plain interactive shell (empty args, like pty_create). A
    // command runs via `<shell> -c "<command>"`.
    let args = match command {
        Some(cmd) => vec!["-c".to_string(), cmd],
        None => Vec::new(),
    };

    let id = uuid::Uuid::new_v4().to_string();
    let ptys = state.ptys.clone();
    let id_for_open = id.clone();
    let handle = tokio::task::spawn_blocking(move || {
        open_pty(
            app,
            id_for_open,
            SpawnConfig {
                program,
                args,
                cwd,
                env,
                cols,
                rows,
            },
        )
    })
    .await
    .map_err(map_err)??;

    ptys.lock().insert(id.clone(), handle);
    Ok(serde_json::json!({ "id": id }))
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
        .map_err(map_err)?;
    // Keep the snapshot's reported dimensions current so a restored pane
    // replays at the size the bytes were produced for.
    {
        let mut out = handle.output.lock();
        out.cols = cols;
        out.rows = rows;
    }
    Ok(())
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
    let handle = state
        .ptys
        .lock()
        .remove(id)
        .ok_or_else(|| format!("unknown pty: {id}"))?;
    // Bind first so the child MutexGuard drops before `handle`.
    let result = handle.child.lock().kill().map_err(map_err);
    result
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
    // Bind the child pid first so the MutexGuard drops before `handle`/`ptys`.
    let shell_pid = handle.child.lock().process_id();
    match (foreground_pid(handle), shell_pid) {
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

// Foreground-process detection, pane-serializer lifecycle, signals, cwd, and the
// management RPC aren't ported (process/cwd are best-effort below; the
// serializer + lifecycle methods no-op).
//
// Return the retained output so the renderer can rebuild a hidden pane's screen.
// Replaying these raw bytes into a cleared xterm reconstructs the current screen
// (and as much scrollback as we kept). `seq` lets the renderer drop live chunks
// already covered here; `cols`/`rows` let it replay at the right dimensions.
// Args are positional: [ptyId, { scrollbackRows }] — the row hint is advisory,
// the byte cap governs how much we retained.
#[tauri::command]
pub fn pty_get_main_buffer_snapshot(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Option<Value> {
    let args = positional_args(&request);
    let id = args.first().and_then(Value::as_str)?;
    let ptys = state.ptys.lock();
    let handle = ptys.get(id)?;
    let out = handle.output.lock();
    // Nothing produced yet means there is nothing to restore; a null snapshot
    // is the renderer's "unavailable" signal, but that path is unreachable
    // while output is pending because restore is only requested after bytes
    // have actually arrived.
    if out.bytes.is_empty() {
        return None;
    }
    let data = String::from_utf8_lossy(&out.bytes).into_owned();
    let cols = out.cols;
    let rows = out.rows;
    let seq = out.total;
    drop(out);
    Some(serde_json::json!({
        "data": data,
        "cols": cols,
        "rows": rows,
        "seq": seq,
    }))
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

// Manage-Sessions panel (Settings). The renderer reaches these via the nested
// `pty.management.*` namespace, which maps to `pty_management_*` commands.
#[tauri::command]
pub fn pty_management_list_sessions(state: State<'_, AppState>) -> Vec<Value> {
    state
        .ptys
        .lock()
        .keys()
        .map(|id| serde_json::json!({ "id": id, "sessionId": id, "cwd": "", "title": "" }))
        .collect()
}

#[tauri::command]
pub fn pty_management_kill_one(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Value {
    let opts = match request.body() {
        tauri::ipc::InvokeBody::Json(value) => value.clone(),
        _ => Value::Null,
    };
    let id = opts
        .get("sessionId")
        .or_else(|| opts.get("id"))
        .and_then(Value::as_str);
    let success = match id {
        Some(id) => match state.ptys.lock().remove(id) {
            Some(handle) => handle.child.lock().kill().is_ok(),
            None => false,
        },
        None => false,
    };
    serde_json::json!({ "success": success })
}

#[tauri::command]
pub fn pty_management_kill_all(state: State<'_, AppState>) -> Value {
    let mut ptys = state.ptys.lock();
    let ids: Vec<String> = ptys.keys().cloned().collect();
    let mut killed = 0_u32;
    for id in &ids {
        if let Some(handle) = ptys.remove(id) {
            if handle.child.lock().kill().is_ok() {
                killed += 1;
            }
        }
    }
    serde_json::json!({ "killedCount": killed, "remainingCount": ptys.len() })
}

#[tauri::command]
pub fn pty_management_restart() -> Value {
    // Restart needs the original spawn args, which PtyHandle doesn't retain.
    serde_json::json!({ "success": false })
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
        let handle = ptys.get(id)?;
        // Bind first so the child MutexGuard drops before `handle`/`ptys`.
        let pid = handle.child.lock().process_id();
        pid
    }?;
    resolve_process_cwd(pid)
}

#[tauri::command]
pub fn pty_report_geometry() {}
