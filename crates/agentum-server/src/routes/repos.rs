//! `/api/repos/*` — the repo registry the desktop used to own natively
//! (`crates/agentum-desktop/src/commands/repos.rs`). Moved here so the registry
//! + git-ref logic lives in one place the TUI/dashboard/desktop all share.
//!
//! The registry is the JSON file `~/.agentum/repos.json` — the SAME legacy
//! location the desktop wrote, so existing project lists round-trip unchanged.
//! (That `~/.agentum` path predates the XDG/`directories` layout in
//! `agentum_store::paths`; unifying them is a separate, data-migrating cleanup
//! and deliberately NOT done here.)
//!
//! The native folder-picker dialog stays in the desktop shell (it needs a Tauri
//! window); everything else — list/add/update/create/clone/remove/reorder and
//! the base-ref helpers — is here.

use std::path::{Path as StdPath, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Path, Query};
use axum::routing::{get, patch, post};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::process::Command;

use crate::error::ApiError;

pub fn router() -> Router<crate::AppState> {
    Router::new()
        .route("/api/repos", get(list).post(add))
        .route("/api/repos/create", post(create))
        .route("/api/repos/clone", post(clone))
        .route("/api/repos/reorder", post(reorder))
        .route("/api/repos/{id}", patch(update).delete(remove))
        .route("/api/repos/{id}/base-ref-default", get(base_ref_default))
        .route("/api/repos/{id}/base-refs", get(base_refs))
        .route("/api/repos/{id}/base-ref-details", get(base_ref_details))
}

/// Keystone of the repo registry. `extra` round-trips fields this layer doesn't
/// manage yet so nothing is lost on rewrite. Mirrors `Repo` in the desktop's
/// `shared/types.ts` (camelCase on the wire).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repo {
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

/// Distinct, low-clash badge colors assigned round-robin by add order.
const BADGE_COLORS: [&str; 8] = [
    "#5b8def", "#27c498", "#e0556a", "#d99e3f", "#9b6ef3", "#3fb6d9", "#e07a3f", "#7a8aa0",
];

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

/// `~/.agentum/repos.json` — the legacy desktop registry location (see module doc).
fn registry_path() -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::Internal("no home directory".into()))?;
    Ok(home.join(".agentum").join("repos.json"))
}

fn read_repos() -> Result<Vec<Repo>, ApiError> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| ApiError::Internal(e.to_string()))?;
    // Tolerate a corrupt registry rather than wedging the app on every call.
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn write_repos(repos: &[Repo]) -> Result<(), ApiError> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    let serialized =
        serde_json::to_string_pretty(repos).map_err(|e| ApiError::Internal(e.to_string()))?;
    std::fs::write(path, format!("{serialized}\n")).map_err(|e| ApiError::Internal(e.to_string()))
}

fn detect_kind(path: &str) -> String {
    if StdPath::new(path).join(".git").exists() {
        "git".to_string()
    } else {
        "folder".to_string()
    }
}

fn basename(path: &str) -> String {
    StdPath::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Adds `path` to the registry (idempotent by path) and returns the Repo. Shared
/// by add/create/clone so registration stays in one place.
fn append_repo(path: String, kind: Option<String>) -> Result<Repo, ApiError> {
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

/// `GET /api/repos` — the registered repos, in order.
async fn list() -> Result<Json<Vec<Repo>>, ApiError> {
    Ok(Json(read_repos()?))
}

#[derive(Debug, Deserialize)]
struct AddBody {
    path: String,
    #[serde(default)]
    kind: Option<String>,
}

/// `POST /api/repos` — register `path`. Returns `{repo}` or `{error}` (the
/// renderer's add-project dialogs branch on `'error' in result`).
async fn add(Json(body): Json<AddBody>) -> Result<Json<Value>, ApiError> {
    if !StdPath::new(&body.path).exists() {
        return Ok(Json(
            serde_json::json!({ "error": format!("path does not exist: {}", body.path) }),
        ));
    }
    let repo = append_repo(body.path, body.kind)?;
    Ok(Json(serde_json::json!({ "repo": repo })))
}

/// `PATCH /api/repos/{id}` — apply `updates`; id/path/addedAt are not updatable.
async fn update(
    Path(repo_id): Path<String>,
    Json(updates): Json<Map<String, Value>>,
) -> Result<Json<Repo>, ApiError> {
    let mut repos = read_repos()?;
    let index = repos
        .iter()
        .position(|repo| repo.id == repo_id)
        .ok_or_else(|| ApiError::NotFound(format!("repo not found: {repo_id}")))?;

    let mut object = serde_json::to_value(&repos[index])
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| ApiError::Internal("failed to serialize repo".into()))?;
    for (key, value) in updates {
        if key == "id" || key == "path" || key == "addedAt" {
            continue;
        }
        object.insert(key, value);
    }
    let updated: Repo = serde_json::from_value(Value::Object(object))
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    repos[index] = updated.clone();
    write_repos(&repos)?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    parent_path: String,
    name: String,
    kind: String,
}

/// `POST /api/repos/create` — make a new folder (optionally `git init`) + register.
async fn create(Json(body): Json<CreateBody>) -> Result<Json<Value>, ApiError> {
    // `name` is a single new folder under `parent_path` — reject separators/`..`
    // so it can't escape, and a leading dash so `git init` can't read it as a flag.
    if body.name.contains('/') || body.name.contains('\\') || body.name == ".." {
        return Err(ApiError::BadRequest(
            "name must be a single path segment (no '/' or '..')".into(),
        ));
    }
    let target = StdPath::new(&body.parent_path).join(&body.name);
    if target.exists() {
        return Ok(Json(serde_json::json!({
            "error": format!("path already exists: {}", target.display())
        })));
    }
    std::fs::create_dir_all(&target).map_err(|e| ApiError::Internal(e.to_string()))?;
    if body.kind == "git" {
        let status = Command::new("git")
            .arg("init")
            .arg("--")
            .arg(&target)
            .status()
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if !status.success() {
            return Ok(Json(serde_json::json!({ "error": "git init failed" })));
        }
    }
    let repo = append_repo(target.to_string_lossy().into_owned(), Some(body.kind))?;
    Ok(Json(serde_json::json!({ "repo": repo })))
}

#[derive(Debug, Deserialize)]
struct CloneBody {
    url: String,
    destination: String,
}

/// `POST /api/repos/clone` — `git clone <url> <destination>` + register.
async fn clone(Json(body): Json<CloneBody>) -> Result<Json<Repo>, ApiError> {
    // `--` + leading-dash rejection so a `-`-prefixed url/destination can't smuggle
    // a `git clone` flag (the server may run as a shared daemon).
    if body.url.starts_with('-') || body.destination.starts_with('-') {
        return Err(ApiError::BadRequest(
            "url/destination must not start with '-'".into(),
        ));
    }
    let output = Command::new("git")
        .args(["clone", "--", &body.url, &body.destination])
        .output()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !output.status.success() {
        return Err(ApiError::BadRequest(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(Json(append_repo(body.destination, Some("git".to_string()))?))
}

/// `DELETE /api/repos/{id}` — drop from the registry.
async fn remove(Path(repo_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let mut repos = read_repos()?;
    repos.retain(|repo| repo.id != repo_id);
    write_repos(&repos)?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReorderBody {
    ordered_ids: Vec<String>,
}

/// `POST /api/repos/reorder` — apply an explicit order; rejected unless it
/// references exactly the known repos.
async fn reorder(Json(body): Json<ReorderBody>) -> Result<Json<Value>, ApiError> {
    let mut repos = read_repos()?;
    let known: std::collections::HashSet<&String> = repos.iter().map(|repo| &repo.id).collect();
    let requested: std::collections::HashSet<&String> = body.ordered_ids.iter().collect();
    if known != requested {
        return Ok(Json(serde_json::json!({ "status": "rejected" })));
    }
    repos.sort_by_key(|repo| {
        body.ordered_ids
            .iter()
            .position(|id| id == &repo.id)
            .unwrap_or(usize::MAX)
    });
    write_repos(&repos)?;
    Ok(Json(serde_json::json!({ "status": "applied" })))
}

/// Resolve a repoId to its checkout path via the registry. `pub(crate)` so the
/// worktrees route can resolve the same path without duplicating the read.
pub(crate) fn resolve_repo_path(repo_id: &str) -> Result<String, ApiError> {
    read_repos()?
        .iter()
        .find(|repo| repo.id == repo_id)
        .map(|repo| repo.path.clone())
        .ok_or_else(|| ApiError::NotFound(format!("repo not found: {repo_id}")))
}

/// Run `git -C <path> <args>`; stdout on success, None on failure.
async fn git_out(path: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
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

/// `GET /api/repos/{id}/base-ref-default` — origin's default head, else local
/// main/master, else the current branch; plus the remote count.
async fn base_ref_default(Path(repo_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let path = resolve_repo_path(&repo_id)?;
    let remote_count = git_out(&path, &["remote"])
        .await
        .map(|out| out.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
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
    Ok(Json(
        serde_json::json!({ "defaultBaseRef": default, "remoteCount": remote_count }),
    ))
}

/// (refName, localBranchName) pairs across local + remote branches.
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
            let local = name.splitn(2, '/').nth(1).unwrap_or(name).to_string();
            refs.push((name.to_string(), local));
        }
    }
    refs
}

#[derive(Debug, Deserialize)]
struct RefSearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /api/repos/{id}/base-refs?q=&limit=` — matching ref names.
async fn base_refs(
    Path(repo_id): Path<String>,
    Query(query): Query<RefSearchQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let path = resolve_repo_path(&repo_id)?;
    let needle = query.q.to_lowercase();
    Ok(Json(
        collect_refs(&path)
            .await
            .into_iter()
            .map(|(ref_name, _)| ref_name)
            .filter(|ref_name| needle.is_empty() || ref_name.to_lowercase().contains(&needle))
            .take(query.limit.unwrap_or(20))
            .collect(),
    ))
}

/// `GET /api/repos/{id}/base-ref-details?q=&limit=` — `{refName, localBranchName}`.
async fn base_ref_details(
    Path(repo_id): Path<String>,
    Query(query): Query<RefSearchQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let path = resolve_repo_path(&repo_id)?;
    let needle = query.q.to_lowercase();
    Ok(Json(
        collect_refs(&path)
            .await
            .into_iter()
            .filter(|(ref_name, _)| needle.is_empty() || ref_name.to_lowercase().contains(&needle))
            .take(query.limit.unwrap_or(20))
            .map(|(ref_name, local)| {
                serde_json::json!({ "refName": ref_name, "localBranchName": local })
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_takes_last_segment() {
        assert_eq!(basename("/a/b/c"), "c");
        assert_eq!(basename("/a/b/c/"), "c");
        assert_eq!(basename("solo"), "solo");
    }

    #[test]
    fn badge_colors_cycle_by_index() {
        assert_eq!(BADGE_COLORS[0], "#5b8def");
        assert_eq!(BADGE_COLORS[8 % BADGE_COLORS.len()], "#5b8def"); // wraps
        assert_eq!(BADGE_COLORS.len(), 8);
    }

    #[test]
    fn repo_serializes_camel_case_and_flattens_extra() {
        let mut extra = Map::new();
        extra.insert("pinned".into(), Value::Bool(true));
        let repo = Repo {
            id: "r1".into(),
            path: "/p".into(),
            display_name: "p".into(),
            badge_color: "#5b8def".into(),
            added_at: 42,
            kind: Some("git".into()),
            connection_id: None,
            extra,
        };
        let v = serde_json::to_value(&repo).unwrap();
        assert_eq!(v["displayName"], "p");
        assert_eq!(v["badgeColor"], "#5b8def");
        assert_eq!(v["addedAt"], 42);
        assert_eq!(v["pinned"], true); // flattened
        assert!(v.get("connectionId").is_none()); // skipped when None
    }
}
