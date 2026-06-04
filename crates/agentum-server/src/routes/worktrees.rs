//! `/api/worktrees/*` — the worktree registry + git-worktree ops the desktop
//! used to own natively (`crates/agentum-desktop/src/commands/worktrees.rs`).
//!
//! Registry: `~/.agentum/worktrees.json` (same legacy location as the repos
//! registry — see `routes::repos`). repoId→path resolution reuses
//! `repos::resolve_repo_path` (DRY). Faithful port of the native logic.
//!
//! Worktree ids are `repoId::/abs/path` (they contain `/`), so id-bearing ops
//! are POST-with-body rather than `{id}` path params, which can't capture slashes.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::Query;
use axum::routing::{get, post};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::process::Command;

use crate::error::ApiError;
use crate::routes::repos::resolve_repo_path;

pub fn router() -> Router<crate::AppState> {
    Router::new()
        .route("/api/worktrees", get(list))
        .route("/api/worktrees/detected", get(detected))
        .route("/api/worktrees/lineage", get(lineage))
        .route("/api/worktrees/update-meta", post(update_meta))
        .route("/api/worktrees/create", post(create))
        .route("/api/worktrees/remove", post(remove))
        .route("/api/worktrees/sort-order", post(persist_sort_order))
        .route("/api/worktrees/force-delete-branch", post(force_delete_branch))
        .route("/api/worktrees/resolve-pr-base", get(resolve_pr_base))
}

/// Registry-backed worktree. Required+nullable fields stay `Option` (serialize as
/// null); `extra` round-trips fields not managed here. camelCase on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Worktree {
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

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn registry_path() -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::Internal("no home directory".into()))?;
    Ok(home.join(".agentum").join("worktrees.json"))
}

fn read_worktrees() -> Result<Vec<Worktree>, ApiError> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| ApiError::Internal(e.to_string()))?;
    // Tolerate a corrupt registry rather than wedging the app on every call.
    let worktrees: Vec<Worktree> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(worktrees.into_iter().map(enrich_worktree).collect())
}

fn write_worktrees(worktrees: &[Worktree]) -> Result<(), ApiError> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    let serialized =
        serde_json::to_string_pretty(worktrees).map_err(|e| ApiError::Internal(e.to_string()))?;
    std::fs::write(path, format!("{serialized}\n")).map_err(|e| ApiError::Internal(e.to_string()))
}

/// Backfill the GitWorktreeInfo fields the UI's `Worktree` type requires
/// (`path`/`branch`/`head`/`isBare`/`isMainWorktree`). Persisted rows carry only
/// user metadata; the path is encoded in the id (`repoId::path`), branch/head
/// come from git. Missing/non-git paths degrade to safe defaults.
fn enrich_worktree(mut wt: Worktree) -> Worktree {
    let Some(wt_path) = wt.id.split_once("::").map(|(_, p)| p.to_string()) else {
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
        wt.extra.insert("path".into(), Value::String(wt_path.clone()));
    }
    if !wt.extra.contains_key("branch") {
        let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "HEAD".into());
        wt.extra.insert("branch".into(), Value::String(branch));
    }
    if !wt.extra.contains_key("head") {
        wt.extra
            .insert("head".into(), Value::String(git(&["rev-parse", "HEAD"]).unwrap_or_default()));
    }
    if !wt.extra.contains_key("isBare") {
        wt.extra.insert("isBare".into(), Value::Bool(false));
    }
    if !wt.extra.contains_key("isMainWorktree") {
        wt.extra.insert("isMainWorktree".into(), Value::Bool(false));
    }
    wt
}

/// Reject a value that would be parsed as a git option (`-x`), so user-supplied
/// refs/names/paths can't smuggle flags into a `git` argv. The server may run as
/// a shared daemon, so this matters more than it did in the desktop-local command.
fn reject_dashed(label: &str, value: &str) -> Result<(), ApiError> {
    if value.starts_with('-') {
        return Err(ApiError::BadRequest(format!("{label} must not start with '-'")));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    /// Filter to one repo; omit for all worktrees.
    #[serde(default)]
    repo_id: Option<String>,
}

/// `GET /api/worktrees[?repoId=]` — registry worktrees (optionally one repo's).
async fn list(Query(q): Query<ListQuery>) -> Result<Json<Vec<Worktree>>, ApiError> {
    let worktrees = read_worktrees()?;
    Ok(Json(match q.repo_id {
        Some(repo_id) => worktrees
            .into_iter()
            .filter(|wt| wt.repo_id == repo_id)
            .collect(),
        None => worktrees,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMetaBody {
    worktree_id: String,
    updates: Map<String, Value>,
}

/// `POST /api/worktrees/update-meta` — upsert metadata for a worktree (git-detected
/// trees often have no registry row, so this seeds a minimal one rather than 404).
async fn update_meta(Json(body): Json<UpdateMetaBody>) -> Result<Json<Worktree>, ApiError> {
    let mut worktrees = read_worktrees()?;
    let index = worktrees.iter().position(|wt| wt.id == body.worktree_id);

    let mut object = match index {
        Some(i) => serde_json::to_value(&worktrees[i])
            .ok()
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| ApiError::Internal("failed to serialize worktree".into()))?,
        None => {
            let repo_id = body
                .worktree_id
                .split_once("::")
                .map(|(repo, _)| repo.to_string())
                .unwrap_or_default();
            let mut seed = Map::new();
            seed.insert("id".into(), Value::String(body.worktree_id.clone()));
            seed.insert("repoId".into(), Value::String(repo_id));
            seed.insert("displayName".into(), Value::String(String::new()));
            seed.insert("comment".into(), Value::String(String::new()));
            seed.insert("isArchived".into(), Value::Bool(false));
            seed.insert("isUnread".into(), Value::Bool(false));
            seed.insert("isPinned".into(), Value::Bool(false));
            seed.insert("sortOrder".into(), Value::Number(0.into()));
            seed.insert("lastActivityAt".into(), Value::Number(now_millis().into()));
            seed
        }
    };
    for (key, value) in body.updates {
        if key == "id" || key == "repoId" {
            continue;
        }
        object.insert(key, value);
    }
    let updated: Worktree = serde_json::from_value(Value::Object(object))
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    match index {
        Some(i) => worktrees[i] = updated.clone(),
        None => worktrees.push(updated.clone()),
    }
    write_worktrees(&worktrees)?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    repo_id: String,
    name: String,
    #[serde(default)]
    base_branch: Option<String>,
    #[serde(default)]
    branch_name_override: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

/// `POST /api/worktrees/create` — `git worktree add` under
/// `<repo>/.claude/worktrees/<name>` (same place the TUI/daemon use), creating a
/// new branch or attaching to an existing one. Returns `{worktree}`.
async fn create(Json(body): Json<CreateBody>) -> Result<Json<Value>, ApiError> {
    // `name` becomes a directory under `.claude/worktrees/` and (by default) the
    // branch — keep it a plain segment so it can't escape the dir or smuggle a flag.
    reject_dashed("name", &body.name)?;
    if body.name.contains('/') || body.name.contains('\\') || body.name == ".." {
        return Err(ApiError::BadRequest(
            "name must be a single path segment (no '/' or '..')".into(),
        ));
    }
    if let Some(base) = &body.base_branch {
        reject_dashed("baseBranch", base)?;
    }
    if let Some(branch) = &body.branch_name_override {
        reject_dashed("branchNameOverride", branch)?;
    }
    let repo_path = resolve_repo_path(&body.repo_id)?;
    let worktrees_root = PathBuf::from(&repo_path).join(".claude").join("worktrees");
    std::fs::create_dir_all(&worktrees_root).map_err(|e| ApiError::Internal(e.to_string()))?;
    let worktree_path = worktrees_root.join(&body.name);
    let worktree_path_string = worktree_path.to_string_lossy().into_owned();
    let branch = body.branch_name_override.clone().unwrap_or_else(|| body.name.clone());

    // Try to create a NEW branch; if it already exists, attach to it instead.
    let mut new_branch_args = vec![
        "-C".to_string(),
        repo_path.clone(),
        "worktree".to_string(),
        "add".to_string(),
        "-b".to_string(),
        branch.clone(),
        worktree_path_string.clone(),
    ];
    if let Some(base) = body.base_branch.clone() {
        new_branch_args.push(base);
    }
    let mut output = Command::new("git")
        .args(&new_branch_args)
        .output()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if !output.status.success()
        && String::from_utf8_lossy(&output.stderr).contains("already exists")
    {
        output = Command::new("git")
            .args([
                "-C",
                &repo_path,
                "worktree",
                "add",
                &worktree_path_string,
                &branch,
            ])
            .output()
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if !output.status.success() {
        return Err(ApiError::BadRequest(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let head = Command::new("git")
        .args(["-C", &worktree_path_string, "rev-parse", "HEAD"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let mut extra = Map::new();
    extra.insert("path".into(), Value::String(worktree_path_string.clone()));
    extra.insert("branch".into(), Value::String(branch));
    extra.insert("head".into(), Value::String(head));
    extra.insert("isBare".into(), Value::Bool(false));
    extra.insert("isMainWorktree".into(), Value::Bool(false));

    let worktree = Worktree {
        id: format!("{}::{worktree_path_string}", body.repo_id),
        repo_id: body.repo_id,
        display_name: body.display_name.unwrap_or(body.name),
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
    Ok(Json(serde_json::json!({ "worktree": worktree })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveBody {
    worktree_id: String,
    #[serde(default)]
    force: Option<bool>,
    // archival isn't ported; accepted for signature parity.
    #[serde(default)]
    #[allow(dead_code)]
    skip_archive: Option<bool>,
}

/// `POST /api/worktrees/remove` — `git worktree remove` + deregister. Stale
/// registry entries (point at a main tree, already gone, …) are deregistered
/// anyway after a `worktree prune`; real failures (dirty/locked) surface.
async fn remove(Json(body): Json<RemoveBody>) -> Result<Json<Value>, ApiError> {
    let (repo_id, worktree_path) = body
        .worktree_id
        .split_once("::")
        .ok_or_else(|| ApiError::BadRequest(format!("invalid worktree id: {}", body.worktree_id)))?;
    reject_dashed("worktree path", worktree_path)?;
    let repo_path = resolve_repo_path(repo_id)?;

    let mut args = vec![
        "-C".to_string(),
        repo_path.clone(),
        "worktree".to_string(),
        "remove".to_string(),
    ];
    if body.force.unwrap_or(false) {
        args.push("--force".to_string());
    }
    args.push(worktree_path.to_string());
    let output = Command::new("git")
        .args(&args)
        .output()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let is_stale_entry = stderr.contains("is a main working tree")
            || stderr.contains("is not a working tree")
            || stderr.contains("not a working tree")
            || stderr.contains("No such file or directory");
        if !is_stale_entry {
            return Err(ApiError::BadRequest(stderr.trim().to_string()));
        }
        let _ = Command::new("git")
            .args(["-C", &repo_path, "worktree", "prune"])
            .output()
            .await;
    }

    let mut worktrees = read_worktrees()?;
    worktrees.retain(|wt| wt.id != body.worktree_id);
    write_worktrees(&worktrees)?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SortOrderBody {
    ordered_ids: Vec<String>,
}

/// `POST /api/worktrees/sort-order` — persist the renderer's worktree ordering
/// (an id array under `~/.agentum/worktree-sort-order.json`).
async fn persist_sort_order(Json(body): Json<SortOrderBody>) -> Result<Json<Value>, ApiError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::Internal("no home directory".into()))?;
    let dir = home.join(".agentum");
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::Internal(e.to_string()))?;
    let serialized =
        serde_json::to_string_pretty(&body.ordered_ids).map_err(|e| ApiError::Internal(e.to_string()))?;
    std::fs::write(dir.join("worktree-sort-order.json"), serialized)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

/// `GET /api/worktrees/lineage` — parent/child tracking isn't ported yet.
async fn lineage() -> Json<Value> {
    Json(Value::Object(Map::new()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForceDeleteBranchBody {
    worktree_id: String,
    branch_name: String,
    // HEAD-match safety guard isn't enforced yet; accepted for parity.
    #[serde(default)]
    #[allow(dead_code)]
    expected_head: Option<String>,
}

/// `POST /api/worktrees/force-delete-branch` — `git branch -D <branch>`.
async fn force_delete_branch(Json(body): Json<ForceDeleteBranchBody>) -> Result<Json<Value>, ApiError> {
    reject_dashed("branchName", &body.branch_name)?;
    let repo_id = body
        .worktree_id
        .split_once("::")
        .map(|(repo, _)| repo)
        .unwrap_or(&body.worktree_id);
    let repo_path = resolve_repo_path(repo_id)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo_path)
        .args(["branch", "-D", "--", &body.branch_name])
        .output()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if output.status.success() {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Ok(Json(serde_json::json!({
            "deleted": false,
            "error": String::from_utf8_lossy(&output.stderr).trim()
        })))
    }
}

/// On-disk worktree detection via `git worktree list --porcelain`, overlaying
/// persisted metadata onto the git-authoritative path/branch (so a re-scan
/// doesn't reset the user's pin/rename/comment). First entry is the primary.
fn scan_git_worktrees(repo_id: &str) -> Result<Vec<Value>, ApiError> {
    let repo_path = resolve_repo_path(repo_id)?;
    let output = std::process::Command::new("git")
        .args(["-C", &repo_path, "worktree", "list", "--porcelain"])
        .output()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
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
    let registry = read_worktrees().unwrap_or_default();
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
            let id = format!("{repo_id}::{path}");
            let meta = registry.iter().find(|wt| wt.id == id);
            serde_json::json!({
                "id": id,
                "repoId": repo_id,
                "displayName": meta
                    .map(|m| m.display_name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or(name),
                "comment": meta.map(|m| m.comment.clone()).unwrap_or_default(),
                "linkedIssue": meta.and_then(|m| m.linked_issue),
                "linkedPr": meta.and_then(|m| m.linked_pr),
                "linkedLinearIssue": meta.and_then(|m| m.linked_linear_issue.clone()),
                "isArchived": meta.map(|m| m.is_archived).unwrap_or(false),
                "isUnread": meta.map(|m| m.is_unread).unwrap_or(false),
                "isPinned": meta.map(|m| m.is_pinned).unwrap_or(is_primary),
                "sortOrder": meta.map(|m| m.sort_order).unwrap_or(idx as i64),
                "lastActivityAt": meta.map(|m| m.last_activity_at).unwrap_or(0),
                "path": path,
                "branch": branch,
                "ownership": "self",
                "selectedCheckout": is_primary,
                "visible": true
            })
        })
        .collect())
}

/// `GET /api/worktrees/detected?repoId=` — git-authoritative worktree list.
async fn detected(Query(q): Query<ListQuery>) -> Result<Json<Value>, ApiError> {
    let repo_id = q
        .repo_id
        .ok_or_else(|| ApiError::BadRequest("repoId is required".into()))?;
    let worktrees = scan_git_worktrees(&repo_id).unwrap_or_default();
    let authoritative = !worktrees.is_empty();
    Ok(Json(serde_json::json!({
        "repoId": repo_id,
        "authoritative": authoritative,
        "source": if authoritative { "git" } else { "metadata-fallback" },
        "worktrees": worktrees
    })))
}

/// `GET /api/worktrees/resolve-pr-base` — needs the GitHub API; not ported.
async fn resolve_pr_base() -> Json<Value> {
    Json(serde_json::json!({
        "error": "Resolving a PR base requires the GitHub API, which isn't available yet."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_id_splits_repo_and_path() {
        // ids are `repoId::/abs/path`; split_once keeps `::` in the path intact.
        let (repo, path) = "r1::/a/b/c".split_once("::").unwrap();
        assert_eq!(repo, "r1");
        assert_eq!(path, "/a/b/c");
    }

    #[test]
    fn worktree_serializes_camel_case_and_flattens_extra() {
        let mut extra = Map::new();
        extra.insert("branch".into(), Value::String("main".into()));
        let wt = Worktree {
            id: "r1::/p".into(),
            repo_id: "r1".into(),
            display_name: "p".into(),
            comment: String::new(),
            linked_issue: None,
            linked_pr: Some(7),
            linked_linear_issue: None,
            is_archived: false,
            is_unread: false,
            is_pinned: true,
            sort_order: 3,
            last_activity_at: 9,
            extra,
        };
        let v = serde_json::to_value(&wt).unwrap();
        assert_eq!(v["repoId"], "r1");
        assert_eq!(v["isPinned"], true);
        assert_eq!(v["sortOrder"], 3);
        assert_eq!(v["linkedPr"], 7);
        assert!(v["linkedIssue"].is_null()); // required+nullable serialize as null
        assert_eq!(v["branch"], "main"); // flattened from extra
    }
}
