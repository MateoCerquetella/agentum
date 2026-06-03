use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// Registry-backed Worktree (see SUBSYSTEMS.md). Mirrors Worktree in
// agentum/src/shared/types.ts; required+nullable fields stay Option (serialize as
// null), and `extra` round-trips fields not yet managed here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    id: String,
    repo_id: String,
    display_name: String,
    comment: String,
    linked_issue: Option<i64>,
    linked_pr: Option<i64>,
    linked_linear_issue: Option<String>,
    is_archived: bool,
    is_unread: bool,
    is_pinned: bool,
    sort_order: i64,
    last_activity_at: u64,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn worktrees_registry_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    Ok(home.join(".agentum").join("worktrees.json"))
}

fn read_worktrees() -> Result<Vec<Worktree>, String> {
    let path = worktrees_registry_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(map_err)?;
    // Tolerate a corrupt registry rather than wedging the app on every call.
    let worktrees: Vec<Worktree> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(worktrees.into_iter().map(enrich_worktree).collect())
}

/// Backfill the GitWorktreeInfo fields the UI's `Worktree` type requires
/// (`path`/`branch`/`head`/`isBare`/`isMainWorktree`). Persisted registry rows
/// only carry user metadata, so without this the renderer crashes on
/// `worktree.branch.replace(...)`. The path is encoded in the id (`repoId::path`);
/// branch/head come from git. Missing/non-git paths degrade to safe defaults.
fn enrich_worktree(mut wt: Worktree) -> Worktree {
    let wt_path = wt.id.split_once("::").map(|(_, p)| p.to_string());
    let Some(wt_path) = wt_path else {
        return wt;
    };
    let git = |args: &[&str]| -> Option<String> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&wt_path)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    if !wt.extra.contains_key("path") {
        wt.extra
            .insert("path".into(), Value::String(wt_path.clone()));
    }
    if !wt.extra.contains_key("branch") {
        let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "HEAD".into());
        wt.extra.insert("branch".into(), Value::String(branch));
    }
    if !wt.extra.contains_key("head") {
        let head = git(&["rev-parse", "HEAD"]).unwrap_or_default();
        wt.extra.insert("head".into(), Value::String(head));
    }
    if !wt.extra.contains_key("isBare") {
        wt.extra.insert("isBare".into(), Value::Bool(false));
    }
    if !wt.extra.contains_key("isMainWorktree") {
        wt.extra.insert("isMainWorktree".into(), Value::Bool(false));
    }
    wt
}

fn write_worktrees(worktrees: &[Worktree]) -> Result<(), String> {
    let path = worktrees_registry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized = serde_json::to_string_pretty(worktrees).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

#[tauri::command]
pub fn worktrees_list(repo_id: String) -> Result<Vec<Worktree>, String> {
    let worktrees = read_worktrees()?;
    Ok(worktrees
        .into_iter()
        .filter(|worktree| worktree.repo_id == repo_id)
        .collect())
}

#[tauri::command]
pub fn worktrees_list_all() -> Result<Vec<Worktree>, String> {
    read_worktrees()
}

#[tauri::command]
pub fn worktrees_update_meta(
    worktree_id: String,
    updates: Map<String, Value>,
) -> Result<Worktree, String> {
    let mut worktrees = read_worktrees()?;
    let index = worktrees
        .iter()
        .position(|worktree| worktree.id == worktree_id)
        .ok_or_else(|| format!("worktree not found: {worktree_id}"))?;

    let mut object = serde_json::to_value(&worktrees[index])
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "failed to serialize worktree".to_string())?;
    for (key, value) in updates {
        // id encodes repoId::path; neither is user-updatable.
        if key == "id" || key == "repoId" {
            continue;
        }
        object.insert(key, value);
    }
    let updated: Worktree = serde_json::from_value(Value::Object(object)).map_err(map_err)?;
    worktrees[index] = updated.clone();
    write_worktrees(&worktrees)?;
    Ok(updated)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

// Resolves a repoId to its checkout path via the repos registry written by repos.rs.
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorktreeResult {
    worktree: Worktree,
}

#[tauri::command]
pub async fn worktrees_create(
    repo_id: String,
    name: String,
    base_branch: Option<String>,
    branch_name_override: Option<String>,
    display_name: Option<String>,
) -> Result<CreateWorktreeResult, String> {
    let repo_path = repo_path_for(&repo_id)?;
    // Agentum keeps worktrees under <repo>/.claude/worktrees/<name> (the same place
    // the TUI/daemon use), not as siblings of the repo. The old sibling scheme
    // polluted the projects dir and collided with unrelated folders
    // ("'.../Test' already exists").
    let worktrees_root = PathBuf::from(&repo_path).join(".claude").join("worktrees");
    std::fs::create_dir_all(&worktrees_root).map_err(map_err)?;
    let worktree_path = worktrees_root.join(&name);
    let worktree_path_string = worktree_path.to_string_lossy().into_owned();
    let branch = branch_name_override.unwrap_or_else(|| name.clone());

    // First try to create a NEW branch (the "Name" flow). If the branch already
    // exists (a leftover from a failed attempt, or the "Branch" flow that targets
    // an existing branch), attach the worktree to that branch instead of failing.
    let mut new_branch_args = vec![
        "-C".to_string(),
        repo_path.clone(),
        "worktree".to_string(),
        "add".to_string(),
        "-b".to_string(),
        branch.clone(),
        worktree_path_string.clone(),
    ];
    if let Some(base) = base_branch.clone() {
        new_branch_args.push(base);
    }
    let mut output = tokio::process::Command::new("git")
        .args(&new_branch_args)
        .output()
        .await
        .map_err(map_err)?;

    if !output.status.success()
        && String::from_utf8_lossy(&output.stderr).contains("already exists")
    {
        let existing_branch_args = vec![
            "-C".to_string(),
            repo_path.clone(),
            "worktree".to_string(),
            "add".to_string(),
            worktree_path_string.clone(),
            branch.clone(),
        ];
        output = tokio::process::Command::new("git")
            .args(&existing_branch_args)
            .output()
            .await
            .map_err(map_err)?;
    }

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    // The HEAD the new branch points at (its base commit).
    let head = tokio::process::Command::new("git")
        .args(["-C", &worktree_path_string, "rev-parse", "HEAD"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // Why: the UI's Worktree type is `metadata & GitWorktreeInfo`; without the
    // git fields (branch/path/head/...) it crashes on `worktree.branch.replace`.
    // The `extra` map is flattened, so these serialize at the top level.
    let mut extra = Map::new();
    extra.insert("path".into(), Value::String(worktree_path_string.clone()));
    extra.insert("branch".into(), Value::String(branch));
    extra.insert("head".into(), Value::String(head));
    extra.insert("isBare".into(), Value::Bool(false));
    extra.insert("isMainWorktree".into(), Value::Bool(false));

    let worktree = Worktree {
        id: format!("{repo_id}::{worktree_path_string}"),
        repo_id,
        display_name: display_name.unwrap_or(name),
        comment: String::new(),
        linked_issue: None,
        linked_pr: None,
        linked_linear_issue: None,
        is_archived: false,
        is_unread: false,
        is_pinned: false,
        sort_order: 0,
        last_activity_at: now_millis(),
        extra,
    };
    let mut worktrees = read_worktrees()?;
    worktrees.push(worktree.clone());
    write_worktrees(&worktrees)?;
    Ok(CreateWorktreeResult { worktree })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveWorktreeResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    preserved_branch: Option<Value>,
}

#[tauri::command]
pub async fn worktrees_remove(
    worktree_id: String,
    force: Option<bool>,
    skip_archive: Option<bool>,
) -> Result<RemoveWorktreeResult, String> {
    let _ = skip_archive; // archival isn't ported yet; accepted for signature parity.
    // id is `${repoId}::${path}` — split once so paths containing "::" survive.
    let (repo_id, worktree_path) = worktree_id
        .split_once("::")
        .ok_or_else(|| format!("invalid worktree id: {worktree_id}"))?;
    let repo_path = repo_path_for(repo_id)?;

    let mut args = vec![
        "-C".to_string(),
        repo_path,
        "worktree".to_string(),
        "remove".to_string(),
    ];
    if force.unwrap_or(false) {
        args.push("--force".to_string());
    }
    args.push(worktree_path.to_string());
    let output = tokio::process::Command::new("git")
        .args(&args)
        .output()
        .await
        .map_err(map_err)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Structural problems mean the registry entry is stale/invalid (e.g. it
        // points at a main working tree from the old sibling-path bug, was already
        // removed, or never was a real linked worktree). Deregister it anyway so the
        // user can clear bad entries from the UI. Real failures (uncommitted changes,
        // locked worktree) are still surfaced so nothing is silently discarded.
        let is_stale_entry = stderr.contains("is a main working tree")
            || stderr.contains("is not a working tree")
            || stderr.contains("not a working tree")
            || stderr.contains("No such file or directory");
        if !is_stale_entry {
            return Err(stderr.trim().to_string());
        }
        // Best-effort cleanup of git's stale worktree metadata.
        let _ = tokio::process::Command::new("git")
            .args(["-C", &repo_path_for(repo_id)?, "worktree", "prune"])
            .output()
            .await;
    }

    let mut worktrees = read_worktrees()?;
    worktrees.retain(|worktree| worktree.id != worktree_id);
    write_worktrees(&worktrees)?;
    Ok(RemoveWorktreeResult {
        preserved_branch: None,
    })
}

// Leaf command: persists the renderer's worktree ordering by id. Independent of
// the (not-yet-ported) repo/worktree registry — it only stores the id array.
#[tauri::command]
pub fn worktrees_persist_sort_order(ordered_ids: Vec<String>) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    let dir = home.join(".agentum");
    std::fs::create_dir_all(&dir).map_err(map_err)?;
    let serialized = serde_json::to_string_pretty(&ordered_ids).map_err(map_err)?;
    std::fs::write(dir.join("worktree-sort-order.json"), serialized).map_err(map_err)
}

#[tauri::command]
pub fn worktrees_list_lineage() -> Value {
    // Lineage tracking (parent/child worktree relationships) isn't ported yet.
    Value::Object(Map::new())
}

#[tauri::command]
pub async fn worktrees_force_delete_preserved_branch(
    worktree_id: String,
    branch_name: String,
    expected_head: String,
) -> Result<Value, String> {
    let _ = expected_head; // HEAD-match safety guard isn't enforced yet.
    let repo_id = worktree_id
        .split_once("::")
        .map(|(repo, _)| repo)
        .unwrap_or(&worktree_id);
    let repo_path = repo_path_for(repo_id)?;
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&repo_path)
        .args(["branch", "-D", &branch_name])
        .output()
        .await
        .map_err(map_err)?;
    if output.status.success() {
        Ok(serde_json::json!({ "deleted": true }))
    } else {
        Ok(serde_json::json!({
            "deleted": false,
            "error": String::from_utf8_lossy(&output.stderr).trim()
        }))
    }
}

// On-disk worktree detection via `git worktree list --porcelain`. Returns
// DetectedWorktree objects (full Worktree shape + ownership/selectedCheckout/visible)
// so a freshly-added repo surfaces its primary worktree instead of "No workspaces
// found". The first entry is the primary checkout.
fn scan_git_worktrees(repo_id: &str) -> Result<Vec<Value>, String> {
    let repo_path = repo_path_for(repo_id)?;
    let output = std::process::Command::new("git")
        .args(["-C", &repo_path, "worktree", "list", "--porcelain"])
        .output()
        .map_err(map_err)?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Collect (path, branch) from each porcelain block.
    let mut entries: Vec<(String, Option<String>)> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.push((path.to_string(), None));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(last) = entries.last_mut() {
                last.1 = Some(branch.to_string());
            }
        }
    }
    Ok(entries
        .into_iter()
        .enumerate()
        .map(|(idx, (path, branch))| {
            let name = branch.clone().unwrap_or_else(|| {
                std::path::Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            });
            let is_primary = idx == 0;
            serde_json::json!({
                "id": format!("{repo_id}::{path}"),
                "repoId": repo_id,
                "displayName": name,
                "comment": "",
                "linkedIssue": null,
                "linkedPr": null,
                "linkedLinearIssue": null,
                "isArchived": false,
                "isUnread": false,
                "isPinned": is_primary,
                "sortOrder": idx as i64,
                "lastActivityAt": 0,
                "path": path,
                "branch": branch,
                "ownership": "self",
                "selectedCheckout": is_primary,
                "visible": true
            })
        })
        .collect())
}

#[tauri::command]
pub fn worktrees_list_detected(repo_id: String) -> Value {
    let worktrees = scan_git_worktrees(&repo_id).unwrap_or_default();
    let authoritative = !worktrees.is_empty();
    serde_json::json!({
        "repoId": repo_id,
        "authoritative": authoritative,
        "source": if authoritative { "git" } else { "metadata-fallback" },
        "worktrees": worktrees
    })
}

#[tauri::command]
pub fn worktrees_resolve_pr_base() -> Value {
    serde_json::json!({ "error": "Resolving a PR base requires the GitHub API, which isn't available yet." })
}

#[tauri::command]
pub fn worktrees_update_lineage() -> Option<Value> {
    None
}
