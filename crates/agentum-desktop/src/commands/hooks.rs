use std::path::{Path, PathBuf};

use serde_json::{json, Value};

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn repo_path_for(repo_id: &str) -> Result<String, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    let path = home.join(".agentum").join("repos.json");
    let raw = std::fs::read_to_string(&path).map_err(|_| format!("repo not found: {repo_id}"))?;
    let repos: Value = serde_json::from_str(&raw).map_err(map_err)?;
    repos
        .as_array()
        .and_then(|repos| {
            repos
                .iter()
                .find(|repo| repo.get("id").and_then(Value::as_str) == Some(repo_id))
        })
        .and_then(|repo| repo.get("path").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| format!("repo not found: {repo_id}"))
}

fn issue_command_path(repo_id: &str) -> Result<PathBuf, String> {
    Ok(Path::new(&repo_path_for(repo_id)?)
        .join(".agentum")
        .join("issue-command"))
}

// Hook installation/inspection isn't ported; report no hooks and no setup imports.
#[tauri::command]
pub fn hooks_check() -> Value {
    json!({ "hooks": null, "mayNeedUpdate": false })
}

#[tauri::command]
pub fn hooks_inspect_setup_script_imports() -> Vec<Value> {
    Vec::new()
}

// Real: the per-repo issue-command template at <repo>/.agentum/issue-command. The
// shared variant isn't ported, so effective == local.
#[tauri::command]
pub fn hooks_read_issue_command(repo_id: String) -> Result<Value, String> {
    let path = issue_command_path(&repo_id)?;
    let local_content = std::fs::read_to_string(&path).ok();
    let source = if local_content.is_some() {
        "local"
    } else {
        "none"
    };
    Ok(json!({
        "status": "ok",
        "localContent": local_content.clone(),
        "sharedContent": null,
        "effectiveContent": local_content,
        "localFilePath": path.display().to_string(),
        "source": source
    }))
}

#[tauri::command]
pub fn hooks_write_issue_command(repo_id: String, content: String) -> Result<(), String> {
    let path = issue_command_path(&repo_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    std::fs::write(path, content).map_err(map_err)
}
