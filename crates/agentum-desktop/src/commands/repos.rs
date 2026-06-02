use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri_plugin_dialog::DialogExt;

// Keystone of the repo/worktree registry (see SUBSYSTEMS.md). Mirrors Repo in
// agentum/src/shared/types.ts; `extra` round-trips fields this layer doesn't manage
// yet so nothing is lost on rewrite.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    id: String,
    path: String,
    display_name: String,
    badge_color: String,
    added_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_id: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

// Distinct, low-clash badge colors assigned round-robin by add order.
const BADGE_COLORS: [&str; 8] = [
    "#5b8def", "#27c498", "#e0556a", "#d99e3f", "#9b6ef3", "#3fb6d9", "#e07a3f", "#7a8aa0",
];

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn registry_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    Ok(home.join(".agentum").join("repos.json"))
}

fn read_repos() -> Result<Vec<Repo>, String> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    // Tolerate a corrupt registry rather than wedging the app on every call.
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn write_repos(repos: &[Repo]) -> Result<(), String> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let serialized = serde_json::to_string_pretty(repos).map_err(|error| error.to_string())?;
    std::fs::write(path, format!("{serialized}\n")).map_err(|error| error.to_string())
}

fn detect_kind(path: &str) -> String {
    if Path::new(path).join(".git").exists() {
        "git".to_string()
    } else {
        "folder".to_string()
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

#[tauri::command]
pub fn repos_list() -> Result<Vec<Repo>, String> {
    read_repos()
}

// Adds `path` to the registry (idempotent by path) and returns the Repo. Shared
// by add/create/clone so registration stays in one place.
fn append_repo(path: String, kind: Option<String>) -> Result<Repo, String> {
    let mut repos = read_repos()?;
    if let Some(existing) = repos.iter().find(|repo| repo.path == path) {
        return Ok(existing.clone());
    }
    let repo = Repo {
        id: uuid::Uuid::new_v4().to_string(),
        display_name: basename(&path),
        badge_color: BADGE_COLORS[repos.len() % BADGE_COLORS.len()].to_string(),
        added_at: now_millis(),
        kind: Some(kind.unwrap_or_else(|| detect_kind(&path))),
        connection_id: None,
        path,
        extra: Map::new(),
    };
    repos.push(repo.clone());
    write_repos(&repos)?;
    Ok(repo)
}

#[tauri::command]
pub fn repos_add(path: String, kind: Option<String>) -> Result<Value, String> {
    if !Path::new(&path).exists() {
        return Ok(serde_json::json!({ "error": format!("path does not exist: {path}") }));
    }
    let repo = append_repo(path, kind)?;
    Ok(serde_json::json!({ "repo": repo }))
}

#[tauri::command]
pub fn repos_update(repo_id: String, updates: Map<String, Value>) -> Result<Repo, String> {
    let mut repos = read_repos()?;
    let index = repos
        .iter()
        .position(|repo| repo.id == repo_id)
        .ok_or_else(|| format!("repo not found: {repo_id}"))?;

    let mut object = serde_json::to_value(&repos[index])
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "failed to serialize repo".to_string())?;
    for (key, value) in updates {
        // Identity fields are not user-updatable.
        if key == "id" || key == "path" || key == "addedAt" {
            continue;
        }
        object.insert(key, value);
    }
    let updated: Repo =
        serde_json::from_value(Value::Object(object)).map_err(|error| error.to_string())?;
    repos[index] = updated.clone();
    write_repos(&repos)?;
    Ok(updated)
}

#[tauri::command]
pub fn repos_create(parent_path: String, name: String, kind: String) -> Result<Value, String> {
    let target = Path::new(&parent_path).join(&name);
    if target.exists() {
        return Ok(serde_json::json!({
            "error": format!("path already exists: {}", target.display())
        }));
    }
    std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    if kind == "git" {
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(&target)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Ok(serde_json::json!({ "error": "git init failed" }));
        }
    }
    let repo = append_repo(target.to_string_lossy().into_owned(), Some(kind))?;
    Ok(serde_json::json!({ "repo": repo }))
}

#[tauri::command]
pub async fn repos_clone(url: String, destination: String) -> Result<Repo, String> {
    let output = tokio::process::Command::new("git")
        .args(["clone", &url, &destination])
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    append_repo(destination, Some("git".to_string()))
}

#[tauri::command]
pub fn repos_clone_abort() {
    // Why: repos_clone runs to completion before returning, so there is no tracked
    // in-flight process to cancel yet. No-op until clone streams onCloneProgress.
}

fn resolve_repo_path(repo_id: &str) -> Result<String, String> {
    read_repos()?
        .iter()
        .find(|repo| repo.id == repo_id)
        .map(|repo| repo.path.clone())
        .ok_or_else(|| format!("repo not found: {repo_id}"))
}

async fn git_out(path: &str, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
pub async fn repos_get_base_ref_default(repo_id: String) -> Result<Value, String> {
    let path = resolve_repo_path(&repo_id)?;
    let remote_count = git_out(&path, &["remote"])
        .await
        .map(|out| out.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
    // origin's default head, else local main/master, else the current branch.
    let default = if let Some(head) =
        git_out(&path, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).await
    {
        Some(head.trim_start_matches("origin/").to_string())
    } else if git_out(&path, &["rev-parse", "--verify", "-q", "refs/heads/main"])
        .await
        .is_some()
    {
        Some("main".to_string())
    } else if git_out(&path, &["rev-parse", "--verify", "-q", "refs/heads/master"])
        .await
        .is_some()
    {
        Some("master".to_string())
    } else {
        git_out(&path, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .filter(|branch| branch != "HEAD")
    };
    Ok(serde_json::json!({ "defaultBaseRef": default, "remoteCount": remote_count }))
}

// (refName, localBranchName) pairs across local + remote branches.
async fn collect_refs(path: &str) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    if let Some(locals) =
        git_out(path, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]).await
    {
        for name in locals.lines().filter(|line| !line.is_empty()) {
            refs.push((name.to_string(), name.to_string()));
        }
    }
    if let Some(remotes) =
        git_out(path, &["for-each-ref", "--format=%(refname:short)", "refs/remotes"]).await
    {
        for name in remotes
            .lines()
            .filter(|line| !line.is_empty() && !line.ends_with("/HEAD"))
        {
            // Strip the remote name; keep the rest (handles feature/x branches).
            let local = name.splitn(2, '/').nth(1).unwrap_or(name).to_string();
            refs.push((name.to_string(), local));
        }
    }
    refs
}

#[tauri::command]
pub async fn repos_search_base_refs(
    repo_id: String,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<String>, String> {
    let path = resolve_repo_path(&repo_id)?;
    let query = query.to_lowercase();
    Ok(collect_refs(&path)
        .await
        .into_iter()
        .map(|(ref_name, _)| ref_name)
        .filter(|ref_name| query.is_empty() || ref_name.to_lowercase().contains(&query))
        .take(limit.unwrap_or(20))
        .collect())
}

#[tauri::command]
pub async fn repos_search_base_ref_details(
    repo_id: String,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<Value>, String> {
    let path = resolve_repo_path(&repo_id)?;
    let query = query.to_lowercase();
    Ok(collect_refs(&path)
        .await
        .into_iter()
        .filter(|(ref_name, _)| query.is_empty() || ref_name.to_lowercase().contains(&query))
        .take(limit.unwrap_or(20))
        .map(|(ref_name, local)| {
            serde_json::json!({ "refName": ref_name, "localBranchName": local })
        })
        .collect())
}

#[tauri::command]
pub fn repos_remove(repo_id: String) -> Result<(), String> {
    let mut repos = read_repos()?;
    repos.retain(|repo| repo.id != repo_id);
    write_repos(&repos)
}

#[tauri::command]
pub fn repos_reorder(ordered_ids: Vec<String>) -> Result<Value, String> {
    let mut repos = read_repos()?;
    // Reject orderings that don't reference exactly the known repos.
    let known: std::collections::HashSet<&String> = repos.iter().map(|repo| &repo.id).collect();
    let requested: std::collections::HashSet<&String> = ordered_ids.iter().collect();
    if known != requested {
        return Ok(serde_json::json!({ "status": "rejected" }));
    }
    repos.sort_by_key(|repo| {
        ordered_ids
            .iter()
            .position(|id| id == &repo.id)
            .unwrap_or(usize::MAX)
    });
    write_repos(&repos)?;
    Ok(serde_json::json!({ "status": "applied" }))
}

#[tauri::command]
pub async fn repos_pick_folder(app: tauri::AppHandle) -> Option<String> {
    pick_folder(app).await
}

#[tauri::command]
pub async fn repos_pick_directory(app: tauri::AppHandle) -> Option<String> {
    pick_folder(app).await
}

// Non-blocking folder dialog (blocking_* would deadlock the command thread).
async fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten().map(|path| path.to_string())
}

// Remote (SSH) project add needs a live SSH connection, which isn't ported. The
// renderer's add-project dialogs expect a `{ repo } | { error }` union and do
// `if ('error' in result)`, so return the error variant — returning unit/null made
// `'error' in null` throw and crashed the surface ("This part of Agentum hit an error").
#[tauri::command]
pub fn repos_add_remote() -> serde_json::Value {
    serde_json::json!({
        "error": "Remote projects require an SSH connection, which isn't available in this build yet."
    })
}
