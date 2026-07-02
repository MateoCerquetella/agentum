use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

// Pipeline → GitHub status-label overrides (spec 005 F5). Persisted in
// `github.json` beside `linear.json`; the embedded server's
// `task_sink::GithubStateMap` re-reads this file on every tracker transition,
// so a Settings save applies on the next transition with no restart — the same
// freshness contract as the Linear state map.

/// The four pipeline phases → GitHub label names. Each field optional so a
/// partial override keeps the server's canonical `status/*` default for the
/// rest. Field names match the server's `task_sink::StoredGithubStateMap`
/// exactly — the server reads this same file.
#[derive(Default, Clone, Serialize, Deserialize)]
struct StoredStateMap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    todo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_progress: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ready_to_test: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    done: Option<String>,
}

/// `github.json` — GitHub pipeline config. Only `state_map` today; shaped as a
/// wrapper (mirroring `LinearCreds`) so future GitHub knobs slot in beside it.
#[derive(Default, Serialize, Deserialize)]
struct GithubConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state_map: Option<StoredStateMap>,
}

// Serialize read-modify-write so concurrent saves can't clobber the file
// (mirrors linear.rs::STORE_LOCK). Held only across the synchronous file IO.
static STORE_LOCK: Mutex<()> = Mutex::new(());

fn config_path() -> Result<PathBuf, String> {
    // Mirror linear.rs::creds_path so github.json sits beside linear.json
    // (and the server's task_sink::github_config_path finds it).
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or("failed to resolve data directory")?;
    let dir = base.join("Agentum");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("github.json"))
}

fn read_config() -> GithubConfig {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    config_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn update_config<F: FnOnce(&mut GithubConfig)>(apply: F) -> Result<GithubConfig, String> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = config_path()?;
    let mut config: GithubConfig = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    apply(&mut config);
    let serialized = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&path, serialized).map_err(|e| e.to_string())?;
    Ok(config)
}

/// The effective pipeline → GitHub status-label map (spec 005 F5): stored
/// overrides filled with the canonical defaults the server uses, so the
/// Settings inputs always show concrete names the user can edit.
#[tauri::command]
pub fn github_get_state_map() -> Value {
    let sm = read_config().state_map.unwrap_or_default();
    json!({
        "todo": sm.todo.unwrap_or_else(|| "status/todo".into()),
        "inProgress": sm.in_progress.unwrap_or_else(|| "status/in-progress".into()),
        "readyToTest": sm.ready_to_test.unwrap_or_else(|| "status/ready-to-test".into()),
        "done": sm.done.unwrap_or_else(|| "status/done".into()),
    })
}

/// Persist the pipeline → GitHub label-name overrides into `github.json`. An
/// empty/blank field clears that override (the server falls back to its
/// canonical `status/*` name). Returns the effective map so the UI can
/// re-render. FLAT named params (repo rule) — a `request: Struct` param
/// silently rejects the invoke.
#[tauri::command]
pub fn github_set_state_map(
    todo: Option<String>,
    in_progress: Option<String>,
    ready_to_test: Option<String>,
    done: Option<String>,
) -> Value {
    let norm = |s: Option<String>| s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let _ = update_config(move |config| {
        config.state_map = Some(StoredStateMap {
            todo: norm(todo),
            in_progress: norm(in_progress),
            ready_to_test: norm(ready_to_test),
            done: norm(done),
        });
    });
    github_get_state_map()
}
