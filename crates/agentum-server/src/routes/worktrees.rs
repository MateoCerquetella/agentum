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
use super::util::now_millis;

use agentum_core::{Host, HostKind};
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::AppState;
use crate::error::ApiError;
use crate::host_runtime::{self, git_in_dir};
use crate::routes::repos::{all_repo_ids, load_host_for_repo, resolve_repo_path};

pub fn router() -> Router<crate::AppState> {
    Router::new()
        .route("/api/worktrees", get(list))
        .route("/api/worktrees/detected", get(detected))
        .route("/api/worktrees/lineage", get(lineage))
        .route("/api/worktrees/update-meta", post(update_meta))
        .route("/api/worktrees/create", post(create))
        .route("/api/worktrees/remove", post(remove))
        .route("/api/worktrees/prune", post(prune))
        .route("/api/worktrees/sort-order", post(persist_sort_order))
        .route(
            "/api/worktrees/force-delete-branch",
            post(force_delete_branch),
        )
        .route("/api/worktrees/resolve-pr-base", get(resolve_pr_base))
}

/// Registry-backed worktree. Required+nullable fields stay `Option` (serialize as
/// null); `extra` round-trips fields not managed here. camelCase on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Worktree {
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

fn registry_path() -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::Internal("no home directory".into()))?;
    Ok(home.join(".agentum").join("worktrees.json"))
}

pub(crate) fn read_worktrees() -> Result<Vec<Worktree>, ApiError> {
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
        wt.extra
            .insert("path".into(), Value::String(wt_path.clone()));
    }
    if !wt.extra.contains_key("branch") {
        let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "HEAD".into());
        wt.extra.insert("branch".into(), Value::String(branch));
    }
    if !wt.extra.contains_key("head") {
        wt.extra.insert(
            "head".into(),
            Value::String(git(&["rev-parse", "HEAD"]).unwrap_or_default()),
        );
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
        return Err(ApiError::BadRequest(format!(
            "{label} must not start with '-'"
        )));
    }
    Ok(())
}

/// Where a worktree create ran, for the human-readable non-git error. `Local`
/// for the daemon's own machine; `Ssh(hostname)` for a remote host.
fn host_location(host: &Host) -> String {
    match &host.kind {
        HostKind::Local => "this machine".to_string(),
        HostKind::Ssh { hostname, .. } => format!("the remote host {hostname}"),
    }
}

/// Map a failed `git worktree add` stderr to a friendly create error. When the
/// target path isn't a git repo (spec 006's FinanzasArgy case), `git` emits a
/// `fatal: not a git repository` line that means nothing to a user — replace it
/// with one that names the path + host and points at the fix. Returns `None` for
/// every other failure so the caller keeps surfacing the raw git stderr.
fn non_git_create_error_message(
    stderr: &str,
    repo_path: &str,
    host_location: &str,
) -> Option<String> {
    if stderr.contains("not a git repository") {
        Some(format!(
            "{repo_path} on {host_location} is not a git repository — re-add the project with the correct path"
        ))
    } else {
        None
    }
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
async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, ApiError> {
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
    let host = load_host_for_repo(&state, &body.repo_id).await?;
    // Build the worktree path as a plain string (not PathBuf): for a remote
    // repo this is a POSIX path on the *remote* fs, not the daemon's. Both
    // local and remote hosts are unix, so `/`-joined strings are correct
    // either way — and PathBuf would canonicalize against the wrong machine.
    let worktrees_root = format!("{}/.claude/worktrees", repo_path.trim_end_matches('/'));
    // `git worktree add` creates the leaf, but not the `.claude/worktrees`
    // parent — make it on the repo's host (the local create_dir_all was the
    // `ENOTSUP (os error 45)` 500 on remote repos).
    host_runtime::mkdir_p(&host, &worktrees_root)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let worktree_path_string = format!("{worktrees_root}/{}", body.name);
    let branch = body
        .branch_name_override
        .clone()
        .unwrap_or_else(|| body.name.clone());

    // Try to create a NEW branch; if it already exists, attach to it instead.
    let mut new_branch_args = vec!["worktree", "add", "-b", &branch, &worktree_path_string];
    if let Some(base) = body.base_branch.as_deref() {
        new_branch_args.push(base);
    }
    let mut output = git_in_dir(&host, &repo_path, &new_branch_args)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if !output.success && output.stderr.contains("already exists") {
        output = git_in_dir(
            &host,
            &repo_path,
            &["worktree", "add", &worktree_path_string, &branch],
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    if !output.success {
        // A non-git target path 400s with `fatal: not a git repository`, which
        // is opaque to a user who registered the wrong remote path. Swap in a
        // message that names the path + host; keep the raw stderr otherwise.
        if let Some(friendly) =
            non_git_create_error_message(&output.stderr, &repo_path, &host_location(&host))
        {
            return Err(ApiError::BadRequest(friendly));
        }
        return Err(ApiError::BadRequest(output.stderr.trim().to_string()));
    }

    let head = git_in_dir(&host, &worktree_path_string, &["rev-parse", "HEAD"])
        .await
        .ok()
        .filter(|o| o.success)
        .map(|o| o.stdout_string().trim().to_string())
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
async fn remove(
    State(state): State<AppState>,
    Json(body): Json<RemoveBody>,
) -> Result<Json<Value>, ApiError> {
    let (repo_id, worktree_path) = body.worktree_id.split_once("::").ok_or_else(|| {
        ApiError::BadRequest(format!("invalid worktree id: {}", body.worktree_id))
    })?;
    reject_dashed("worktree path", worktree_path)?;
    let repo_path = resolve_repo_path(repo_id)?;
    let host = load_host_for_repo(&state, repo_id).await?;

    let mut args = vec!["worktree", "remove"];
    if body.force.unwrap_or(false) {
        args.push("--force");
    }
    args.push(worktree_path);
    let output = git_in_dir(&host, &repo_path, &args)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !output.success {
        let stderr = &output.stderr;
        let is_stale_entry = stderr.contains("is a main working tree")
            || stderr.contains("is not a working tree")
            || stderr.contains("not a working tree")
            || stderr.contains("No such file or directory");
        if !is_stale_entry {
            return Err(ApiError::BadRequest(stderr.trim().to_string()));
        }
        let _ = git_in_dir(&host, &repo_path, &["worktree", "prune"]).await;
    }

    let mut worktrees = read_worktrees()?;
    worktrees.retain(|wt| wt.id != body.worktree_id);
    write_worktrees(&worktrees)?;
    Ok(Json(serde_json::json!({})))
}

// ───────────────────────────────── prune ─────────────────────────────────
// Bulk-remove the stale worktrees sessions leave behind (issue #8, "clean up
// stale git worktrees"). Conservative by construction: classification is
// git-authoritative, a worktree with uncommitted work is NEVER removed, and
// dry-run is the default — nothing is destroyed without an explicit `apply`.

/// One worktree as `git worktree list --porcelain` reports it, reduced to the
/// fields classification needs. A pure parse target so the classifier can be
/// unit-tested without invoking git.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PorcelainWorktree {
    path: String,
    branch: Option<String>,
    /// The first entry git lists is the repo's primary (main) working tree.
    is_primary: bool,
    /// A `git worktree lock`ed tree — never auto-pruned.
    locked: bool,
    /// git itself flags the working tree as gone (a `prunable <reason>` line);
    /// `git worktree prune` would drop it. Always safe to remove.
    prunable: bool,
}

/// Parse `git worktree list --porcelain` into [`PorcelainWorktree`]s. Each
/// `worktree ` line starts an entry; `branch`/`locked`/`prunable` attach to the
/// entry in progress. Tolerant of trailing `\r` and lines we don't model.
fn parse_worktree_porcelain(text: &str) -> Vec<PorcelainWorktree> {
    let mut out: Vec<PorcelainWorktree> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(path) = line.strip_prefix("worktree ") {
            let is_primary = out.is_empty();
            out.push(PorcelainWorktree {
                path: path.to_string(),
                branch: None,
                is_primary,
                locked: false,
                prunable: false,
            });
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(last) = out.last_mut() {
                last.branch = Some(branch.to_string());
            }
        } else if line == "locked" || line.starts_with("locked ") {
            if let Some(last) = out.last_mut() {
                last.locked = true;
            }
        } else if line == "prunable" || line.starts_with("prunable ") {
            if let Some(last) = out.last_mut() {
                last.prunable = true;
            }
        }
    }
    out
}

/// How prune treats one worktree. Serialized into the response so the CLI/UI can
/// show *why* each tree was kept or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum PruneClass {
    /// Primary worktree, or `git worktree lock`ed — never touched.
    Keep,
    /// Working tree is gone (git-prunable); removing it just drops a stale admin
    /// entry. Always pruned.
    Gone,
    /// Exists, non-primary, unlocked, no uncommitted changes. Pruned only with
    /// `includeClean` (a clean tree may still be wanted).
    Clean,
    /// Has uncommitted changes, or its state couldn't be read. NEVER
    /// auto-pruned — losing uncommitted work is the one unrecoverable mistake.
    Dirty,
}

/// Pure classification. `dirty` is the outcome of a `git status --porcelain`
/// check on the worktree: `Some(false)` = clean, `Some(true)` = dirty, `None` =
/// couldn't check. Unknown collapses to `Dirty`, so an unreadable tree is
/// preserved rather than destroyed.
fn classify_worktree(wt: &PorcelainWorktree, dirty: Option<bool>) -> PruneClass {
    if wt.is_primary || wt.locked {
        return PruneClass::Keep;
    }
    if wt.prunable {
        return PruneClass::Gone;
    }
    match dirty {
        Some(false) => PruneClass::Clean,
        _ => PruneClass::Dirty,
    }
}

/// Whether a class is removed at the requested aggressiveness. `Gone` always;
/// `Clean` only when the caller opts in; `Keep`/`Dirty` never.
fn should_prune(class: PruneClass, include_clean: bool) -> bool {
    matches!(class, PruneClass::Gone) || (include_clean && matches!(class, PruneClass::Clean))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PruneBody {
    /// Limit to one repo; omit to sweep every registered repo.
    #[serde(default)]
    repo_id: Option<String>,
    /// Actually remove (default false = dry-run preview).
    #[serde(default)]
    apply: bool,
    /// Also prune clean (no-uncommitted-changes) non-primary worktrees, not just
    /// the git-prunable (gone) ones.
    #[serde(default)]
    include_clean: bool,
}

/// `POST /api/worktrees/prune` — bulk-remove stale worktrees across one repo or
/// all of them. Host-aware (each repo's git runs on the repo's host). Dry-run
/// unless `apply`. Returns `{dryRun, pruned:[{id,path,branch,class}], kept:[…]}`.
async fn prune(
    State(state): State<AppState>,
    Json(body): Json<PruneBody>,
) -> Result<Json<Value>, ApiError> {
    let repo_ids = match &body.repo_id {
        Some(id) => vec![id.clone()],
        None => all_repo_ids()?,
    };

    let mut pruned: Vec<Value> = Vec::new();
    let mut kept: Vec<Value> = Vec::new();

    for repo_id in repo_ids {
        // A repo whose host was deleted/unreachable, or whose path no longer
        // resolves, shouldn't abort the whole sweep — skip it, keep going.
        let (Ok(host), Ok(repo_path)) = (
            load_host_for_repo(&state, &repo_id).await,
            resolve_repo_path(&repo_id),
        ) else {
            continue;
        };
        let listing =
            match git_in_dir(&host, &repo_path, &["worktree", "list", "--porcelain"]).await {
                Ok(out) if out.success => out.stdout_string(),
                _ => continue,
            };

        for wt in parse_worktree_porcelain(&listing) {
            // Only an existing, non-primary, unlocked tree needs the dirty check;
            // primary/locked/gone trees skip the extra git call.
            let dirty = if wt.is_primary || wt.locked || wt.prunable {
                None
            } else {
                match git_in_dir(&host, &wt.path, &["status", "--porcelain"]).await {
                    Ok(out) if out.success => Some(!out.stdout_string().trim().is_empty()),
                    _ => None, // unreadable → treated as Dirty (kept)
                }
            };
            let class = classify_worktree(&wt, dirty);
            let entry = serde_json::json!({
                "id": format!("{repo_id}::{}", wt.path),
                "repoId": repo_id,
                "path": wt.path,
                "branch": wt.branch,
                "class": class,
            });

            if should_prune(class, body.include_clean) {
                if body.apply {
                    // --force: a Clean tree has nothing to lose (status --porcelain
                    // was empty) and a Gone tree's dir is already absent. The
                    // follow-up `prune` sweeps the leftover admin entry git's
                    // `remove` can't (the missing-dir case).
                    let _ = git_in_dir(
                        &host,
                        &repo_path,
                        &["worktree", "remove", "--force", &wt.path],
                    )
                    .await;
                    let _ = git_in_dir(&host, &repo_path, &["worktree", "prune"]).await;
                }
                pruned.push(entry);
            } else {
                kept.push(entry);
            }
        }
    }

    // Deregister every removed worktree from the registry in one read/write.
    if body.apply && !pruned.is_empty() {
        let removed: std::collections::HashSet<&str> = pruned
            .iter()
            .filter_map(|entry| entry.get("id").and_then(Value::as_str))
            .collect();
        let mut registry = read_worktrees()?;
        registry.retain(|wt| !removed.contains(wt.id.as_str()));
        write_worktrees(&registry)?;
    }

    Ok(Json(serde_json::json!({
        "dryRun": !body.apply,
        "pruned": pruned,
        "kept": kept,
    })))
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
    let serialized = serde_json::to_string_pretty(&body.ordered_ids)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
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
async fn force_delete_branch(
    State(state): State<AppState>,
    Json(body): Json<ForceDeleteBranchBody>,
) -> Result<Json<Value>, ApiError> {
    reject_dashed("branchName", &body.branch_name)?;
    let repo_id = body
        .worktree_id
        .split_once("::")
        .map(|(repo, _)| repo)
        .unwrap_or(&body.worktree_id);
    let repo_path = resolve_repo_path(repo_id)?;
    let host = load_host_for_repo(&state, repo_id).await?;
    let output = git_in_dir(
        &host,
        &repo_path,
        &["branch", "-D", "--", &body.branch_name],
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if output.success {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Ok(Json(serde_json::json!({
            "deleted": false,
            "error": output.stderr.trim()
        })))
    }
}

/// On-disk worktree detection via `git worktree list --porcelain`, overlaying
/// persisted metadata onto the git-authoritative path/branch (so a re-scan
/// doesn't reset the user's pin/rename/comment). First entry is the primary.
async fn scan_git_worktrees(host: &Host, repo_id: &str) -> Result<Vec<Value>, ApiError> {
    let repo_path = resolve_repo_path(repo_id)?;
    let output = git_in_dir(host, &repo_path, &["worktree", "list", "--porcelain"])
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !output.success {
        return Ok(Vec::new());
    }
    let text = output.stdout_string();
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
                // Pinning is EXPLICIT: a worktree with no registry row is NOT
                // pinned. Defaulting the primary to pinned made it impossible to
                // keep unpinned — deleting a worktree drops its row, so it
                // reverted to auto-pinned, and a repo's primary worktree (which
                // `git worktree remove` can't delete) reappeared pinned forever.
                "isPinned": meta.map(|m| m.is_pinned).unwrap_or(false),
                "sortOrder": meta.map(|m| m.sort_order).unwrap_or(idx as i64),
                "lastActivityAt": meta.map(|m| m.last_activity_at).unwrap_or(0),
                "path": path,
                "branch": branch,
                "ownership": "self",
                "selectedCheckout": is_primary,
                // The first `git worktree list` entry is the repo's primary
                // worktree. The sidebar's "Hide default branch" filter keys off
                // this; without it the flag defaulted to false for every row and
                // the filter silently did nothing.
                "isMainWorktree": is_primary,
                "visible": true
            })
        })
        .collect())
}

/// `GET /api/worktrees/detected?repoId=` — git-authoritative worktree list.
async fn detected(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let repo_id = q
        .repo_id
        .ok_or_else(|| ApiError::BadRequest("repoId is required".into()))?;
    let host = load_host_for_repo(&state, &repo_id).await?;
    let worktrees = scan_git_worktrees(&host, &repo_id)
        .await
        .unwrap_or_default();
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
    fn non_git_stderr_maps_to_friendly_message() {
        // The FinanzasArgy case: a registered path that isn't a repo on the host.
        let stderr = "fatal: not a git repository (or any of the parent directories): .git";
        let msg = non_git_create_error_message(
            stderr,
            "/home/malloc/Developer/projects/CerqueTech/FinanzasArgy",
            "the remote host forge.lan",
        )
        .expect("non-git stderr should map to a friendly message");
        assert!(msg.contains("/home/malloc/Developer/projects/CerqueTech/FinanzasArgy"));
        assert!(msg.contains("the remote host forge.lan"));
        assert!(msg.contains("not a git repository"));
        assert!(msg.contains("re-add the project"));
        // The raw git `fatal:` prefix must not leak into the user-facing copy.
        assert!(!msg.contains("fatal:"));
    }

    #[test]
    fn other_create_stderr_is_not_rewritten() {
        // Any other failure (branch conflict, dirty tree, …) keeps the raw stderr.
        assert!(
            non_git_create_error_message(
                "fatal: a branch named 'feature' already exists",
                "/repo",
                "this machine",
            )
            .is_none()
        );
        assert!(non_git_create_error_message("", "/repo", "this machine").is_none());
    }

    #[test]
    fn host_location_describes_local_and_ssh() {
        use agentum_core::{HostKind, LOCAL_HOST_ID};
        use time::OffsetDateTime;
        let now = OffsetDateTime::now_utc();
        let local = Host {
            id: LOCAL_HOST_ID,
            name: "local".into(),
            kind: HostKind::Local,
            created_at: now,
            updated_at: now,
            last_seen_at: None,
        };
        assert_eq!(host_location(&local), "this machine");
        let ssh = Host {
            id: LOCAL_HOST_ID,
            name: "forge".into(),
            kind: HostKind::Ssh {
                user: "malloc".into(),
                hostname: "forge.lan".into(),
                port: 22,
                auth: agentum_core::SshAuth::Agent,
            },
            created_at: now,
            updated_at: now,
            last_seen_at: None,
        };
        assert_eq!(host_location(&ssh), "the remote host forge.lan");
    }

    #[test]
    fn porcelain_parses_primary_branch_locked_and_prunable() {
        // First entry is primary; later entries carry branch/locked/prunable.
        let text = "\
worktree /repo
HEAD aaaa
branch refs/heads/main

worktree /repo/.claude/worktrees/feat
HEAD bbbb
branch refs/heads/feat

worktree /repo/.claude/worktrees/held
HEAD cccc
branch refs/heads/held
locked manual hold

worktree /repo/.claude/worktrees/gone
HEAD dddd
detached
prunable gitdir file points to non-existent location
";
        let wts = parse_worktree_porcelain(text);
        assert_eq!(wts.len(), 4);
        assert!(wts[0].is_primary && wts[0].branch.as_deref() == Some("main"));
        assert!(!wts[1].is_primary && !wts[1].locked && !wts[1].prunable);
        assert!(wts[2].locked, "`locked <reason>` must set locked");
        assert!(wts[3].prunable, "`prunable <reason>` must set prunable");
        assert_eq!(wts[3].branch, None); // detached → no branch
    }

    fn wt(is_primary: bool, locked: bool, prunable: bool) -> PorcelainWorktree {
        PorcelainWorktree {
            path: "/p".into(),
            branch: None,
            is_primary,
            locked,
            prunable,
        }
    }

    #[test]
    fn classify_keeps_primary_and_locked_always() {
        // Primary and locked are kept no matter how clean — even gone/clean.
        for dirty in [Some(false), Some(true), None] {
            assert_eq!(
                classify_worktree(&wt(true, false, false), dirty),
                PruneClass::Keep
            );
            assert_eq!(
                classify_worktree(&wt(false, true, false), dirty),
                PruneClass::Keep
            );
        }
    }

    #[test]
    fn classify_gone_clean_and_dirty() {
        // Gone (git-prunable) regardless of the dirty probe.
        assert_eq!(
            classify_worktree(&wt(false, false, true), None),
            PruneClass::Gone
        );
        // Existing tree: clean vs dirty vs unknown.
        assert_eq!(
            classify_worktree(&wt(false, false, false), Some(false)),
            PruneClass::Clean
        );
        assert_eq!(
            classify_worktree(&wt(false, false, false), Some(true)),
            PruneClass::Dirty
        );
        // Unknown dirty state is preserved (never destroyed).
        assert_eq!(
            classify_worktree(&wt(false, false, false), None),
            PruneClass::Dirty
        );
    }

    #[test]
    fn should_prune_gates_clean_behind_opt_in() {
        // Gone is always pruned; Dirty/Keep never.
        assert!(should_prune(PruneClass::Gone, false));
        assert!(!should_prune(PruneClass::Dirty, true));
        assert!(!should_prune(PruneClass::Keep, true));
        // Clean only when include_clean is set.
        assert!(!should_prune(PruneClass::Clean, false));
        assert!(should_prune(PruneClass::Clean, true));
    }

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
