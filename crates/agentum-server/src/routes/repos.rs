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

use super::util::SlugReason;
use super::util::now_millis;
use std::path::{Path as StdPath, PathBuf};

use agentum_core::{Host, LOCAL_HOST_ID};
use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::process::Command;
use uuid::Uuid;

use crate::AppState;
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
        .route("/api/repos/{id}/slug", get(repo_slug))
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
    /// Server host (`/api/hosts`) the repo lives on. Resolved CLIENT-side
    /// from `connection_id` at add time (mirroring how sessions carry
    /// `host_id`), so server git/worktree/agent ops can route through
    /// `host_runtime` without knowing the desktop's native SSH-target ids.
    /// Absent / null = the daemon's local host.
    #[serde(skip_serializing_if = "Option::is_none")]
    host_id: Option<Uuid>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// Distinct, low-clash badge colors assigned round-robin by add order.
const BADGE_COLORS: [&str; 8] = [
    "#5b8def", "#27c498", "#e0556a", "#d99e3f", "#9b6ef3", "#3fb6d9", "#e07a3f", "#7a8aa0",
];

/// `~/.agentum/repos.json` — the legacy desktop registry location (see module doc).
fn registry_path() -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::Internal("no home directory".into()))?;
    Ok(home.join(".agentum").join("repos.json"))
}

/// `(id, path)` pairs for browser-scope resolution (spec 014 D2: `Repo.id` is
/// the project identity). Tolerant: an unreadable registry → empty table.
pub(crate) fn scope_repo_pairs() -> Vec<(String, String)> {
    read_repos()
        .map(scope_pairs_locals_first)
        .unwrap_or_default()
}

/// Pure core of [`scope_repo_pairs`]: local entries first (stable), so a
/// bare-path browser scope on a spec-015 dual entry (same path, local +
/// remote) resolves to the LOCAL id — a local Chromium can only ever serve
/// local checkouts, and a registry reorder must not silently migrate which
/// id keys its profile.
fn scope_pairs_locals_first(mut repos: Vec<Repo>) -> Vec<(String, String)> {
    // sort_by_key is stable: locals keep registry order, then remotes in order.
    repos.sort_by_key(|repo| repo.connection_id.is_some());
    repos.into_iter().map(|r| (r.id, r.path)).collect()
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

/// Legacy provider preference for deterministic tracker migration. This reads
/// only the requested registry row; it never consults UI/global settings.
pub(crate) fn legacy_tracker_provider(repo_id: &str) -> Result<Option<String>, ApiError> {
    let repo = read_repos()?
        .into_iter()
        .find(|repo| repo.id == repo_id)
        .ok_or_else(|| ApiError::NotFound(format!("repo not found: {repo_id}")))?;
    Ok(repo
        .extra
        .get("trackerProvider")
        .and_then(Value::as_str)
        .filter(|provider| matches!(*provider, "github" | "linear"))
        .map(str::to_string))
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

fn detect_kind(directory: &std::fs::File) -> String {
    let is_git = cap_primitives::fs::stat(
        directory,
        StdPath::new(".git"),
        cap_primitives::fs::FollowSymlinks::No,
    )
    .is_ok_and(|metadata| !metadata.is_symlink() && (metadata.is_dir() || metadata.is_file()));
    if is_git { "git" } else { "folder" }.into()
}

fn basename(path: &str) -> String {
    StdPath::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Pure core of registration (spec 015 D6): a repo's identity is WHERE it
/// lives (its desktop connection, `None` = local) plus its path there.
/// Returns the existing entry for (path, connection_id) or appends a new
/// one; `true` = appended (the caller persists).
fn register_repo(
    repos: &mut Vec<Repo>,
    path: String,
    kind: Option<String>,
    connection_id: Option<String>,
    host_id: Option<Uuid>,
) -> (Repo, bool) {
    if let Some(existing) = repos
        .iter()
        .find(|repo| repo.path == path && repo.connection_id == connection_id)
    {
        return (existing.clone(), false);
    }
    // detect_kind probes the LOCAL filesystem, which is meaningless for a remote
    // (connection_id) path. Use the caller's kind, else default remote repos to
    // 'git' and only local-detect when there's no connection.
    let resolved_kind = kind.unwrap_or_else(|| {
        if connection_id.is_some() {
            "git".to_string()
        } else {
            // HTTP registration resolves local kinds before this pure registry
            // seam. Tests and internal callers that deliberately omit it get
            // the conservative non-Git default without probing an ambient path.
            "folder".to_string()
        }
    });
    let repo = Repo {
        id: uuid::Uuid::new_v4().to_string(),
        display_name: basename(&path),
        badge_color: BADGE_COLORS[repos.len() % BADGE_COLORS.len()].to_string(),
        added_at: now_millis(),
        kind: Some(resolved_kind),
        connection_id,
        host_id,
        path,
        extra: Map::new(),
    };
    repos.push(repo.clone());
    (repo, true)
}

/// Adds `path` to the registry (idempotent by (path, connection)) and returns
/// the Repo. Shared by add/create/clone so registration stays in one place.
fn append_repo(
    path: String,
    kind: Option<String>,
    connection_id: Option<String>,
    host_id: Option<Uuid>,
) -> Result<Repo, ApiError> {
    let mut repos = read_repos()?;
    let (repo, added) = register_repo(&mut repos, path, kind, connection_id, host_id);
    if added {
        write_repos(&repos)?;
    }
    Ok(repo)
}

/// `GET /api/repos` — the registered repos, in order.
async fn list() -> Result<Json<Vec<Repo>>, ApiError> {
    Ok(Json(read_repos()?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddBody {
    path: String,
    #[serde(default)]
    kind: Option<String>,
    /// SSH target id (a desktop native connection) the repo lives on. When set,
    /// the path is on a remote host — skip the local existence check, and the
    /// repo's sessions/git route through that connection (resolved to a server
    /// host on the client).
    #[serde(default)]
    connection_id: Option<String>,
    /// Server host id the client resolved from `connection_id`. Persisted so
    /// server-side git/worktree/agent ops can route through `host_runtime`.
    #[serde(default)]
    host_id: Option<Uuid>,
}

/// `POST /api/repos` — register `path`. Returns `{repo}` or `{error}` (the
/// renderer's add-project dialogs branch on `'error' in result`).
async fn add(Json(body): Json<AddBody>) -> Result<Json<Value>, ApiError> {
    if body.connection_id.is_some() && body.host_id.is_none() {
        return Err(ApiError::BadRequest(
            "remote repo is missing hostId; reconnect or re-add the project before server-side operations"
                .into(),
        ));
    }
    // A local registration is an explicit user-selected directory capability.
    // Resolve it once, pin a no-follow handle, and store the canonical path so
    // later operations cannot be redirected by a linked ancestor. Remote paths
    // stay opaque and are never reinterpreted on the daemon's filesystem.
    let (path, kind) = if body.connection_id.is_none() {
        let path = match canonical_local_directory(&body.path) {
            Ok(path) => path,
            Err(_) => {
                return Ok(Json(serde_json::json!({
                    "error": format!("path does not exist or is not a directory: {}", body.path)
                })));
            }
        };
        let directory = crate::host_runtime::open_local_directory_chain_nofollow(&path)
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        let kind = body.kind.or_else(|| Some(detect_kind(&directory)));
        (path.to_string_lossy().into_owned(), kind)
    } else {
        (body.path, body.kind)
    };
    let repo = append_repo(path, kind, body.connection_id, body.host_id)?;
    Ok(Json(serde_json::json!({ "repo": repo })))
}

/// Pure PATCH merge with identity protection: `id`/`path`/`addedAt` were
/// always immutable, and spec 015 adds `connectionId` — it is half of the
/// registry's (path, connection) identity key, so an edit could collide two
/// entries onto one key. `hostId` stays editable (routing metadata,
/// repairable).
fn apply_repo_updates(repo: &Repo, updates: Map<String, Value>) -> Result<Repo, ApiError> {
    let mut object = serde_json::to_value(repo)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| ApiError::Internal("failed to serialize repo".into()))?;
    for (key, value) in updates {
        if key == "id" || key == "path" || key == "addedAt" || key == "connectionId" {
            continue;
        }
        object.insert(key, value);
    }
    serde_json::from_value(Value::Object(object)).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// `PATCH /api/repos/{id}` — apply `updates`; id/path/addedAt/connectionId are
/// not updatable.
async fn update(
    Path(repo_id): Path<String>,
    Json(updates): Json<Map<String, Value>>,
) -> Result<Json<Repo>, ApiError> {
    let mut repos = read_repos()?;
    let index = repos
        .iter()
        .position(|repo| repo.id == repo_id)
        .ok_or_else(|| ApiError::NotFound(format!("repo not found: {repo_id}")))?;
    let updated = apply_repo_updates(&repos[index], updates)?;
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
    if !matches!(body.kind.as_str(), "git" | "folder") {
        return Err(ApiError::BadRequest("kind must be git or folder".into()));
    }
    #[cfg(not(unix))]
    if body.kind == "git" {
        return Err(ApiError::BadRequest(
            "secure descriptor-bound git initialization is unsupported on this operating system"
                .into(),
        ));
    }
    validate_repository_child_name(&body.name)?;
    let (target, _directory_guard) =
        match create_local_repository_directory(&body.parent_path, &body.name) {
            Ok(created) => created,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let display = StdPath::new(&body.parent_path).join(&body.name);
                return Ok(Json(serde_json::json!({
                    "error": format!("path already exists: {}", display.display())
                })));
            }
            Err(error) => return Err(ApiError::BadRequest(error.to_string())),
        };
    #[cfg(unix)]
    if body.kind == "git" {
        let status = git_init_directory(&_directory_guard)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if !status.success() {
            return Ok(Json(serde_json::json!({ "error": "git init failed" })));
        }
    }
    let repo = append_repo(
        target.to_string_lossy().into_owned(),
        Some(body.kind),
        None,
        None,
    )?;
    Ok(Json(serde_json::json!({ "repo": repo })))
}

fn canonical_local_directory(raw: &str) -> Result<PathBuf, std::io::Error> {
    let path = StdPath::new(raw);
    if raw.trim().is_empty() || !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local repository path must be absolute",
        ));
    }
    let path = path.canonicalize()?;
    if !path.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local repository path must be a directory",
        ));
    }
    Ok(path)
}

fn create_local_repository_directory(
    parent_raw: &str,
    name: &str,
) -> Result<(PathBuf, std::fs::File), std::io::Error> {
    let parent = canonical_local_directory(parent_raw)?;
    let parent_directory = crate::host_runtime::open_local_directory_chain_nofollow(&parent)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    cap_primitives::fs::create_dir(
        &parent_directory,
        StdPath::new(name),
        &cap_primitives::fs::DirOptions::new(),
    )?;
    let directory = cap_primitives::fs::open_dir_nofollow(&parent_directory, StdPath::new(name))?;
    Ok((parent.join(name), directory))
}

/// Initialize Git with the already-open child directory as the process working
/// directory. An ambient path is not safe here: another process can rename the
/// newly-created directory and replace its old name with a symlink between
/// creation and `git init`. `fchdir` binds the child process to the held inode
/// before `exec`, so Git can only modify the directory Agentum created.
#[cfg(unix)]
async fn git_init_directory(
    directory: &std::fs::File,
) -> Result<std::process::ExitStatus, std::io::Error> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt as _;

    let descriptor = directory.as_raw_fd();
    let mut command = Command::new("git");
    command.args(["init", "--", "."]);
    // SAFETY: `fchdir` is async-signal-safe, the descriptor remains owned by
    // `directory` until the child has been spawned, and the callback performs
    // no allocation or other non-signal-safe work after `fork`.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            if libc::fchdir(descriptor) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    command.status().await
}

fn validate_repository_child_name(name: &str) -> Result<(), ApiError> {
    let mut components = StdPath::new(name).components();
    let valid = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
        && !name.starts_with('-')
        && !name.contains(['\0', '/', '\\']);
    if !valid {
        return Err(ApiError::BadRequest(
            "name must be one non-option path segment".into(),
        ));
    }
    Ok(())
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
    Ok(Json(append_repo(
        body.destination,
        Some("git".to_string()),
        None,
        None,
    )?))
}

/// `DELETE /api/repos/{id}` — drop from the registry.
async fn remove(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if let Some(config) = state.store.get_project_tracker_config(&repo_id).await? {
        let _ = state
            .store
            .delete_project_tracker_config(&repo_id, Some(config.revision))
            .await?;
    }
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

/// Every registered repo's id, in registry order. `pub(crate)` so the
/// worktrees route can scan all repos (e.g. a prune with no `repoId` filter)
/// without duplicating the registry read or exposing the private `Repo`.
pub(crate) fn all_repo_ids() -> Result<Vec<String>, ApiError> {
    Ok(read_repos()?.into_iter().map(|repo| repo.id).collect())
}

#[cfg(test)]
#[derive(Clone)]
struct TestRepository {
    path: String,
    host_id: Option<Uuid>,
}

#[cfg(test)]
fn test_repositories()
-> &'static std::sync::Mutex<std::collections::HashMap<String, TestRepository>> {
    static REPOSITORIES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, TestRepository>>,
    > = std::sync::OnceLock::new();
    REPOSITORIES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
pub(crate) struct TestRepositoryRegistration(String);

#[cfg(test)]
impl Drop for TestRepositoryRegistration {
    fn drop(&mut self) {
        test_repositories()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

/// Scoped, in-process repository injection for tests. This avoids changing
/// process-wide account-directory variables or touching a developer's real
/// repository registry.
#[cfg(test)]
pub(crate) fn register_test_repo(
    repo_id: impl Into<String>,
    path: impl Into<String>,
) -> TestRepositoryRegistration {
    let repo_id = repo_id.into();
    test_repositories()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            repo_id.clone(),
            TestRepository {
                path: path.into(),
                host_id: None,
            },
        );
    TestRepositoryRegistration(repo_id)
}

/// Scoped remote repository injection for route-boundary tests. Keeping the
/// host identity beside the path proves callers do not silently reinterpret a
/// remote checkout as local when a capability is unavailable.
#[cfg(test)]
pub(crate) fn register_test_remote_repo(
    repo_id: impl Into<String>,
    path: impl Into<String>,
    host_id: Uuid,
) -> TestRepositoryRegistration {
    let repo_id = repo_id.into();
    test_repositories()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            repo_id.clone(),
            TestRepository {
                path: path.into(),
                host_id: Some(host_id),
            },
        );
    TestRepositoryRegistration(repo_id)
}

/// Resolve a repoId to its checkout path via the registry. `pub(crate)` so the
/// worktrees route can resolve the same path without duplicating the read.
pub(crate) fn resolve_repo_path(repo_id: &str) -> Result<String, ApiError> {
    #[cfg(test)]
    if let Some(repository) = test_repositories()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(repo_id)
        .cloned()
    {
        return Ok(repository.path);
    }
    read_repos()?
        .iter()
        .find(|repo| repo.id == repo_id)
        .map(|repo| repo.path.clone())
        .ok_or_else(|| ApiError::NotFound(format!("repo not found: {repo_id}")))
}

/// The server host id a repo lives on, or `None` for a local repo. Legacy
/// remote rows that predate `host_id` are rejected: their path is not local
/// merely because the newer routing field is absent.
pub(crate) fn resolve_repo_host_id(repo_id: &str) -> Result<Option<Uuid>, ApiError> {
    #[cfg(test)]
    if let Some(repository) = test_repositories()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(repo_id)
        .cloned()
    {
        return Ok(repository.host_id);
    }
    host_id_of(&read_repos()?, repo_id)
}

/// Pure core of [`resolve_repo_host_id`], split out so the repoId→host
/// contract is testable without the registry file: `Ok(None)` = local,
/// `Ok(Some(_))` = a server host, `Err(NotFound)` = the id isn't registered —
/// never a silent local fallback (spec 020 D1).
fn host_id_of(repos: &[Repo], repo_id: &str) -> Result<Option<Uuid>, ApiError> {
    let repo = repos
        .iter()
        .find(|repo| repo.id == repo_id)
        .ok_or_else(|| ApiError::NotFound(format!("repo not found: {repo_id}")))?;
    match (repo.host_id, repo.connection_id.as_deref()) {
        (Some(host_id), _) => Ok(Some(host_id)),
        (None, Some(_)) => Err(ApiError::BadRequest(format!(
            "remote repo {repo_id} is missing hostId; reconnect or re-add the project"
        ))),
        (None, None) => Ok(None),
    }
}

/// Load the [`Host`] a repo's git/worktree ops run on. Mirrors
/// `sessions::load_host_for_session`: the repo's `host_id` (or the local host
/// for an explicitly local row) → `store.get_host`. `pub(crate)` so the worktrees
/// route resolves the same host. A repo whose recorded host has since been
/// deleted surfaces a clear error rather than silently running locally.
pub(crate) async fn load_host_for_repo(state: &AppState, repo_id: &str) -> Result<Host, ApiError> {
    let host_id = resolve_repo_host_id(repo_id)?.unwrap_or(LOCAL_HOST_ID);
    state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("repo host is missing: {host_id}")))
}

/// Run `git <args>` with `path` as the working dir on `host`; stdout on
/// success, `None` on a non-zero exit or transport failure. Host-aware so
/// a remote repo's refs resolve over SSH.
async fn git_out(host: &Host, path: &str, args: &[&str]) -> Option<String> {
    let out = crate::host_runtime::git_in_dir(host, path, args)
        .await
        .ok()?;
    out.success.then(|| out.stdout_string().trim().to_string())
}

/// `GET /api/repos/{id}/base-ref-default` — origin's default head, else local
/// main/master, else the current branch; plus the remote count.
async fn base_ref_default(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let path = resolve_repo_path(&repo_id)?;
    let host = load_host_for_repo(&state, &repo_id).await?;
    let remote_count = git_out(&host, &path, &["remote"])
        .await
        .map(|out| out.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
    let default = if let Some(head) = git_out(
        &host,
        &path,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    {
        Some(head.trim_start_matches("origin/").to_string())
    } else if git_out(
        &host,
        &path,
        &["rev-parse", "--verify", "-q", "refs/heads/main"],
    )
    .await
    .is_some()
    {
        Some("main".to_string())
    } else if git_out(
        &host,
        &path,
        &["rev-parse", "--verify", "-q", "refs/heads/master"],
    )
    .await
    .is_some()
    {
        Some("master".to_string())
    } else {
        git_out(&host, &path, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .filter(|branch| branch != "HEAD")
    };
    Ok(Json(
        serde_json::json!({ "defaultBaseRef": default, "remoteCount": remote_count }),
    ))
}

/// (refName, localBranchName) pairs across local + remote branches.
async fn collect_refs(host: &Host, path: &str) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    if let Some(locals) = git_out(
        host,
        path,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .await
    {
        for name in locals.lines().filter(|line| !line.is_empty()) {
            refs.push((name.to_string(), name.to_string()));
        }
    }
    if let Some(remotes) = git_out(
        host,
        path,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    )
    .await
    {
        for name in remotes
            .lines()
            .filter(|line| !line.is_empty() && !line.ends_with("/HEAD"))
        {
            let local = name
                .split_once('/')
                .map_or(name, |(_, rest)| rest)
                .to_string();
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
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Query(query): Query<RefSearchQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let path = resolve_repo_path(&repo_id)?;
    let host = load_host_for_repo(&state, &repo_id).await?;
    let needle = query.q.to_lowercase();
    Ok(Json(
        collect_refs(&host, &path)
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
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    Query(query): Query<RefSearchQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let path = resolve_repo_path(&repo_id)?;
    let host = load_host_for_repo(&state, &repo_id).await?;
    let needle = query.q.to_lowercase();
    Ok(Json(
        collect_refs(&host, &path)
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

/// `GET /api/repos/{id}/slug` response. An object, not a bare string, so
/// future fields stay add-only. No `source` field — the only source is the
/// `origin` read (the route takes no hint by design), so it would be a
/// constant.
#[derive(Debug, Serialize)]
struct RepoSlugResponse {
    slug: String,
}

/// Pure: `SlugReason` → (status, code, message). `NoGithubRemote` is semantic
/// (422); `HostUnreachable` is transport (502, a gateway problem) — the wire
/// must never let an SSH failure masquerade as "no origin" (spec 020
/// invariant), even though the renderer's fail-closed index treats both as
/// "excluded".
fn slug_reason_wire(reason: SlugReason) -> (StatusCode, &'static str, &'static str) {
    match reason {
        SlugReason::NoGithubRemote => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_github_remote",
            "the repo has no `origin` remote pointing at GitHub",
        ),
        SlugReason::HostUnreachable => (
            StatusCode::BAD_GATEWAY,
            "host_unreachable",
            "could not read the repo's git origin — its host is unreachable",
        ),
    }
}

/// Host-aware core of [`repo_slug`], split from the handler so the
/// with/without-origin contract is testable against a temp git repo without
/// touching the real `~/.agentum` registry.
async fn slug_on_host(host: &Host, path: &str) -> Result<RepoSlugResponse, ApiError> {
    let slug = super::util::resolve_github_slug(host, path, None)
        .await
        .map_err(|reason| {
            let (status, code, message) = slug_reason_wire(reason);
            ApiError::Custom(
                status,
                serde_json::json!({ "error": { "code": code, "message": message } }),
            )
        })?;
    Ok(RepoSlugResponse { slug })
}

/// `GET /api/repos/{id}/slug` — the repo's GitHub `owner/repo`, resolved by
/// reading `origin` ON THE REPO'S HOST (spec 020 F2). The renderer's slug
/// index uses this for SSH repos, which the local-only native read can never
/// see. Deliberately no hint/workdir params: this route IS how a client
/// learns the slug, and the server owns id→path consistency (the registry
/// path, like every `base_ref_*` sibling). Slug case is passed through as
/// resolved; the client lowercases. Fail-closed: any error excludes the repo
/// from the client's index.
async fn repo_slug(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
) -> Result<Json<RepoSlugResponse>, ApiError> {
    let path = resolve_repo_path(&repo_id)?; // 404 unknown id
    let host = load_host_for_repo(&state, &repo_id).await?;
    Ok(Json(slug_on_host(&host, &path).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal registry entry for the pure-registration tests (no fs, no env).
    fn repo_with(path: &str, connection_id: Option<&str>) -> Repo {
        Repo {
            id: uuid::Uuid::new_v4().to_string(),
            path: path.to_string(),
            display_name: basename(path),
            badge_color: BADGE_COLORS[0].to_string(),
            added_at: 1,
            kind: Some("git".into()),
            connection_id: connection_id.map(str::to_string),
            host_id: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn repository_child_names_are_exactly_one_non_option_component() {
        assert!(validate_repository_child_name("project").is_ok());
        for unsafe_name in ["", ".", "..", "../escape", "a/b", "a\\b", "-flag"] {
            assert!(
                validate_repository_child_name(unsafe_name).is_err(),
                "accepted {unsafe_name:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn repository_creation_never_reuses_a_racing_symlink_destination() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), parent.path().join("project")).unwrap();
        let result = create_local_repository_directory(&parent.path().to_string_lossy(), "project");
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_init_remains_bound_after_created_directory_is_renamed_and_replaced() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let (target, directory) =
            create_local_repository_directory(&parent.path().to_string_lossy(), "project").unwrap();
        let moved = parent.path().join("moved-project");
        std::fs::rename(&target, &moved).unwrap();
        symlink(outside.path(), &target).unwrap();

        let status = git_init_directory(&directory).await.unwrap();
        assert!(status.success());
        assert!(moved.join(".git").is_dir());
        assert!(!outside.path().join(".git").exists());
    }

    // ── spec 015 F1: identity is (path, connection_id) ──────────────────────

    #[test]
    fn same_path_local_then_remote_registers_two_entries() {
        let mut repos = Vec::new();
        let host = Uuid::new_v4();
        let (local, added_local) =
            register_repo(&mut repos, "/x/proj".into(), Some("git".into()), None, None);
        assert!(added_local);
        // AC 1: the remote add must NOT return the pre-existing local entry.
        let (remote, added_remote) = register_repo(
            &mut repos,
            "/x/proj".into(),
            None,
            Some("ssh-1".into()),
            Some(host),
        );
        assert!(added_remote);
        assert_eq!(repos.len(), 2);
        assert_ne!(local.id, remote.id);
        assert_eq!(remote.connection_id.as_deref(), Some("ssh-1"));
        assert_eq!(remote.host_id, Some(host));
        // The pre-existing local entry is untouched.
        assert_eq!(repos[0].id, local.id);
        assert!(repos[0].connection_id.is_none());
        assert!(repos[0].host_id.is_none());
    }

    #[test]
    fn same_path_same_connection_is_idempotent() {
        let mut repos = Vec::new();
        let (first, added_first) = register_repo(
            &mut repos,
            "/x/proj".into(),
            None,
            Some("ssh-1".into()),
            Some(Uuid::new_v4()),
        );
        assert!(added_first);
        let (second, added_second) = register_repo(
            &mut repos,
            "/x/proj".into(),
            None,
            Some("ssh-1".into()),
            Some(Uuid::new_v4()),
        );
        assert!(!added_second); // AC 2 remote: no duplicate row, no rewrite
        assert_eq!(repos.len(), 1);
        assert_eq!(first.id, second.id);
    }

    #[test]
    fn local_readd_stays_idempotent() {
        let mut repos = Vec::new();
        let (first, _) =
            register_repo(&mut repos, "/x/proj".into(), Some("git".into()), None, None);
        let (second, added) =
            register_repo(&mut repos, "/x/proj".into(), Some("git".into()), None, None);
        assert!(!added); // AC 2 local: None == None dedupes
        assert_eq!(repos.len(), 1);
        assert_eq!(first.id, second.id);
    }

    #[test]
    fn two_connections_same_path_are_two_entries() {
        // D6: key is connection_id, NOT host_id — two desktop connections to
        // one server host stay two entries (one per UI host bucket).
        let mut repos = Vec::new();
        let host = Uuid::new_v4();
        let (a, _) = register_repo(
            &mut repos,
            "/x/proj".into(),
            None,
            Some("ssh-1".into()),
            Some(host),
        );
        let (b, added) = register_repo(
            &mut repos,
            "/x/proj".into(),
            None,
            Some("ssh-2".into()),
            Some(host),
        );
        assert!(added);
        assert_eq!(repos.len(), 2);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn remote_register_defaults_kind_git() {
        // detect_kind probes the LOCAL fs, meaningless for a remote path — the
        // pre-015 default-to-git behavior must survive the refactor.
        let mut repos = Vec::new();
        let (repo, _) = register_repo(
            &mut repos,
            "/definitely/not/here".into(),
            None,
            Some("ssh-1".into()),
            None,
        );
        assert_eq!(repo.kind.as_deref(), Some("git"));
    }

    #[test]
    fn update_refuses_connection_id_edit() {
        // connectionId is half the (path, connection) identity key — a PATCH
        // must not be able to collide two entries onto one key. hostId stays
        // editable (routing metadata, repairable).
        let repo = repo_with("/x/proj", Some("ssh-1"));
        let host = Uuid::new_v4();
        let mut updates = Map::new();
        updates.insert("connectionId".into(), Value::String("ssh-2".into()));
        updates.insert("displayName".into(), Value::String("renamed".into()));
        updates.insert("hostId".into(), Value::String(host.to_string()));
        let updated = apply_repo_updates(&repo, updates).unwrap();
        assert_eq!(updated.connection_id.as_deref(), Some("ssh-1")); // refused
        assert_eq!(updated.display_name, "renamed"); // ordinary keys still apply
        assert_eq!(updated.host_id, Some(host)); // hostId remains editable

        // Nulling it out is refused too (a remote entry can't be made local).
        let mut null_update = Map::new();
        null_update.insert("connectionId".into(), Value::Null);
        let updated = apply_repo_updates(&repo, null_update).unwrap();
        assert_eq!(updated.connection_id.as_deref(), Some("ssh-1"));
    }

    #[test]
    fn scope_pairs_lists_locals_first_stably() {
        // Dual entries share a path; the browser-scope table must resolve the
        // LOCAL id first regardless of registry order (and keep relative order
        // within each partition).
        let r1 = repo_with("/a", Some("ssh-1"));
        let l1 = repo_with("/a", None);
        let r2 = repo_with("/b", Some("ssh-2"));
        let l2 = repo_with("/c", None);
        let pairs = scope_pairs_locals_first(vec![r1.clone(), l1.clone(), r2.clone(), l2.clone()]);
        let ids: Vec<&str> = pairs.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec![&l1.id, &l2.id, &r1.id, &r2.id]);
    }

    // ── spec 020 F1: repoId → host threading (the pure registry core) ──────

    #[test]
    fn host_id_of_known_local_is_none() {
        let local = repo_with("/x/proj", None);
        let repos = vec![local.clone()];
        assert_eq!(host_id_of(&repos, &local.id).unwrap(), None);
    }

    #[test]
    fn host_id_of_known_remote_is_its_host() {
        let mut remote = repo_with("/x/proj", Some("ssh-1"));
        let host = Uuid::new_v4();
        remote.host_id = Some(host);
        let repos = vec![remote.clone()];
        assert_eq!(host_id_of(&repos, &remote.id).unwrap(), Some(host));
    }

    #[test]
    fn host_id_of_legacy_remote_never_falls_back_local() {
        let remote = repo_with("/srv/project", Some("ssh-legacy"));
        let err = host_id_of(std::slice::from_ref(&remote), &remote.id).unwrap_err();
        match err {
            ApiError::BadRequest(message) => {
                assert!(message.contains("missing hostId"), "got: {message}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn host_id_of_unknown_id_is_not_found() {
        // D1: an unknown repoId is a loud NotFound, never a silent local pick.
        let repos = vec![repo_with("/x/proj", None)];
        let err = host_id_of(&repos, "no-such-id").unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    }

    // ── spec 020 F2: `GET /api/repos/{id}/slug` (host-aware slug route) ────

    /// Minimal local [`Host`] for the slug-route tests (the board_goals test
    /// pattern): the resolver only reads `host.kind` to pick local vs SSH.
    fn local_host() -> Host {
        Host {
            id: LOCAL_HOST_ID,
            name: "local".into(),
            kind: agentum_core::HostKind::Local,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    /// `git init` + optionally an `origin` remote — enough for
    /// `remote get-url origin` (the slug read never touches history, so no
    /// commit is needed).
    fn init_repo_with_origin(dir: &StdPath, origin: Option<&str>) {
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git available in test env")
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        if let Some(url) = origin {
            run(&["remote", "add", "origin", url]);
        }
    }

    /// Pure: the transport failure (502 `host_unreachable`) must never
    /// masquerade as the semantic miss (422 `no_github_remote`) — the spec
    /// 020 invariant this wire exists to carry.
    #[test]
    fn slug_reason_wire_distinguishes_transport_from_semantic() {
        use super::super::util::SlugReason;
        let (semantic_status, semantic_code, semantic_msg) =
            slug_reason_wire(SlugReason::NoGithubRemote);
        let (transport_status, transport_code, transport_msg) =
            slug_reason_wire(SlugReason::HostUnreachable);
        assert_eq!(semantic_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(semantic_code, "no_github_remote");
        assert_eq!(transport_status, StatusCode::BAD_GATEWAY);
        assert_eq!(transport_code, "host_unreachable");
        assert_ne!(
            semantic_msg, transport_msg,
            "reasons must not collapse into one message"
        );
    }

    /// The response is `{"slug": "owner/repo"}` — an object so future fields
    /// stay add-only; slug case is passed through (the client lowercases).
    #[test]
    fn repo_slug_response_serializes_slug_only() {
        let v = serde_json::to_value(RepoSlugResponse {
            slug: "Owner/Repo".into(),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({ "slug": "Owner/Repo" }));
    }

    /// A local repo with a GitHub origin resolves to its slug.
    #[tokio::test]
    async fn slug_on_host_reads_github_origin() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_origin(dir.path(), Some("git@github.com:owner/repo.git"));
        let res = slug_on_host(&local_host(), &dir.path().to_string_lossy())
            .await
            .unwrap();
        assert_eq!(res.slug, "owner/repo");
    }

    /// A repo with no GitHub origin is the SEMANTIC miss — 422 with code
    /// `no_github_remote`, never the transport 502.
    #[tokio::test]
    async fn slug_on_host_without_origin_is_no_github_remote_422() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_with_origin(dir.path(), None);
        let err = slug_on_host(&local_host(), &dir.path().to_string_lossy())
            .await
            .unwrap_err();
        match err {
            ApiError::Custom(status, body) => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(body["error"]["code"], "no_github_remote");
            }
            other => panic!("expected Custom 422, got {other:?}"),
        }
    }

    /// Unknown repo id → the handler's first gate (`resolve_repo_path`) 404s
    /// before any host/git work. Env-tolerant: a random uuid misses whatever
    /// `~/.agentum/repos.json` holds (the 015 house rule — no env mutation).
    #[test]
    fn repo_slug_unknown_id_is_not_found() {
        let id = format!("020-no-such-repo-{}", Uuid::new_v4());
        let err = resolve_repo_path(&id).unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    }

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
            host_id: None,
            extra,
        };
        let v = serde_json::to_value(&repo).unwrap();
        assert_eq!(v["displayName"], "p");
        assert_eq!(v["badgeColor"], "#5b8def");
        assert_eq!(v["addedAt"], 42);
        assert_eq!(v["pinned"], true); // flattened
        assert!(v.get("connectionId").is_none()); // skipped when None
        assert!(v.get("hostId").is_none()); // skipped when None
    }

    /// Spec 379 F2: the per-project tracker choice is persisted by the generic
    /// PATCH path — this layer has no `trackerProvider` field, so the value
    /// must land in `extra` and survive the registry's serde round-trip
    /// (what `write_repos`/`read_repos` do), or a reopened settings surface
    /// would silently fall back to Auto after relaunch.
    #[test]
    fn tracker_provider_update_persists_and_round_trips() {
        let repo = Repo {
            id: "r1".into(),
            path: "/p".into(),
            display_name: "p".into(),
            badge_color: "#5b8def".into(),
            added_at: 42,
            kind: Some("git".into()),
            connection_id: None,
            host_id: None,
            extra: Map::new(),
        };
        let mut updates = Map::new();
        updates.insert("trackerProvider".into(), Value::String("linear".into()));
        let updated = apply_repo_updates(&repo, updates).unwrap();
        assert_eq!(
            updated.extra.get("trackerProvider"),
            Some(&Value::String("linear".into()))
        );

        let raw = serde_json::to_string(&vec![updated]).unwrap();
        let read: Vec<Repo> = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            read[0].extra.get("trackerProvider"),
            Some(&Value::String("linear".into()))
        );

        // Re-picking replaces the saved choice rather than duplicating it.
        let mut updates = Map::new();
        updates.insert("trackerProvider".into(), Value::String("github".into()));
        let repicked = apply_repo_updates(&read[0], updates).unwrap();
        assert_eq!(
            repicked.extra.get("trackerProvider"),
            Some(&Value::String("github".into()))
        );
    }

    #[test]
    fn linear_project_binding_object_null_and_sibling_fields_round_trip() {
        let repo = Repo {
            id: "r1".into(),
            path: "/p".into(),
            display_name: "p".into(),
            badge_color: "#5b8def".into(),
            added_at: 42,
            kind: Some("git".into()),
            connection_id: None,
            host_id: None,
            extra: Map::from_iter([("unrelated".into(), json!({ "kept": true }))]),
        };
        let binding = json!({
            "workspaceId": "workspace-a", "workspaceName": "Workspace A",
            "projectId": "project-a", "projectName": "Project A",
            "projectUrl": "https://linear.app/workspace/project/project-a"
        });
        let updated = apply_repo_updates(
            &repo,
            Map::from_iter([("linearProjectBinding".into(), binding.clone())]),
        )
        .unwrap();
        assert_eq!(updated.extra["linearProjectBinding"], binding);
        assert_eq!(updated.extra["unrelated"], json!({ "kept": true }));
        let raw = serde_json::to_string(&updated).unwrap();
        let reloaded: Repo = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            reloaded.extra["linearProjectBinding"]["projectId"],
            "project-a"
        );
        assert_eq!(reloaded.extra["unrelated"], json!({ "kept": true }));
        let cleared = apply_repo_updates(
            &reloaded,
            Map::from_iter([("linearProjectBinding".into(), Value::Null)]),
        )
        .unwrap();
        assert_eq!(cleared.extra["linearProjectBinding"], Value::Null);
        assert_eq!(cleared.extra["unrelated"], json!({ "kept": true }));
    }
}
