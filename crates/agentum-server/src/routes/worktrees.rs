//! `/api/worktrees/*` — the worktree registry + git-worktree ops the desktop
//! used to own natively (`crates/agentum-desktop/src/commands/worktrees.rs`).
//!
//! Registry: `~/.agentum/worktrees.json` (same legacy location as the repos
//! registry — see `routes::repos`). repoId→path resolution reuses
//! `repos::resolve_repo_path` (DRY). Faithful port of the native logic.
//!
//! Worktree ids are `repoId::/abs/path` (they contain `/`), so id-bearing ops
//! are POST-with-body rather than `{id}` path params, which can't capture slashes.

use super::util::now_millis;
use std::path::PathBuf;

use agentum_core::{HarnessScope, Host, HostKind};
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::AppState;
use crate::error::ApiError;
use crate::host_runtime::{self, HostCommandOutput, HostRuntimeError, git_in_dir};
use crate::routes::repos::{
    all_repo_ids, load_host_for_repo, resolve_repo_host_id, resolve_repo_path,
};

pub fn router() -> Router<crate::AppState> {
    Router::new()
        .route("/api/worktrees", get(list))
        .route("/api/worktrees/detected", get(detected))
        .route("/api/worktrees/lineage", get(lineage))
        .route("/api/worktrees/update-meta", post(update_meta))
        .route(
            "/api/worktrees/reconcile-github-status",
            post(reconcile_github_status),
        )
        .route(
            "/api/worktrees/reconcile-linear-status",
            post(reconcile_linear_status),
        )
        .route(
            "/api/worktrees/transition-tracker",
            post(transition_tracker),
        )
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
    // Spec 012 tracker-sync coords: which tracker owns this workspace's item and
    // the canonical issue URL a later transition targets, plus the last-written
    // pipeline phase (the monotonic no-thrash guard + poller terminal-stop).
    // `#[serde(default)] Option<String>` and — like every registry field — NO
    // `#[serde(alias)]` (spec 004 lesson: an alias makes serde see a legacy
    // shadowed key twice → parse error → `read_worktrees` wipes to `[]`). An
    // old-shape registry deserializes each to `None`, never `[]`.
    #[serde(default)]
    tracker_provider: Option<String>,
    #[serde(default)]
    tracker_url: Option<String>,
    #[serde(default)]
    tracker_phase: Option<String>,
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

/// `(repo_id, full registry id)` pairs for browser-scope resolution
/// (`crate::cdp_browser::resolve_browser_scope`, spec 014). Tolerant: an
/// unreadable registry yields an empty table (resolution then falls through to
/// the git probe / adhoc isolation).
pub(crate) fn scope_worktree_pairs() -> Vec<(String, String)> {
    read_worktrees()
        .map(|rows| rows.into_iter().map(|w| (w.repo_id, w.id)).collect())
        .unwrap_or_default()
}

pub(crate) fn read_worktrees() -> Result<Vec<Worktree>, ApiError> {
    Ok(read_worktrees_raw()?
        .into_iter()
        .map(enrich_worktree)
        .collect())
}

/// Registry rows without local git enrichment. Identity resolution must use
/// this path: probing a remote worktree with local `git -C` is both wrong and
/// unnecessary when the id already encodes the authoritative path.
fn read_worktrees_raw() -> Result<Vec<Worktree>, ApiError> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| ApiError::Internal(e.to_string()))?;
    // Tolerate a corrupt registry rather than wedging the app on every call.
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn normalized_claimed_path(path: &str) -> &str {
    let trimmed = path.trim();
    if trimmed == "/" {
        trimmed
    } else {
        trimmed.trim_end_matches('/')
    }
}

fn scope_from_registry(
    worktrees: &[Worktree],
    worktree_id: &str,
    claimed_workdir: &str,
    host_id: uuid::Uuid,
) -> Result<HarnessScope, ApiError> {
    let row = worktrees
        .iter()
        .find(|row| row.id == worktree_id)
        .ok_or_else(|| ApiError::NotFound(format!("worktree not found: {worktree_id}")))?;
    let (encoded_repo, encoded_path) = row.id.split_once("::").ok_or_else(|| {
        ApiError::BadRequest(format!("invalid registered worktree id: {}", row.id))
    })?;
    if encoded_repo != row.repo_id {
        return Err(ApiError::BadRequest(format!(
            "worktree/repo identity mismatch for {}",
            row.id
        )));
    }
    if !std::path::Path::new(encoded_path).is_absolute()
        || std::path::Path::new(encoded_path)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(ApiError::BadRequest(format!(
            "worktree path is not an absolute, traversal-free path: {encoded_path}"
        )));
    }
    if normalized_claimed_path(claimed_workdir) != normalized_claimed_path(encoded_path) {
        return Err(ApiError::BadRequest(format!(
            "workdir does not match worktreeId: claimed {:?}, registered {:?}",
            claimed_workdir, encoded_path
        )));
    }
    Ok(HarnessScope {
        worktree_id: Some(row.id.clone()),
        repo_id: Some(row.repo_id.clone()),
        host_id: Some(host_id),
        path: normalized_claimed_path(encoded_path).to_string(),
    })
}

/// Resolve an exact registered worktree to its repo, server host, and path.
/// Supplying `worktreeId` opts into strict ownership: an unknown id, malformed
/// registry row, deleted host, or claimed-path mismatch is an error and never
/// falls back to local execution.
pub(crate) async fn resolve_harness_scope(
    state: &AppState,
    worktree_id: &str,
    claimed_workdir: &str,
) -> Result<(HarnessScope, Host), ApiError> {
    let rows = read_worktrees_raw()?;
    let repo_id = rows
        .iter()
        .find(|row| row.id == worktree_id)
        .map(|row| row.repo_id.clone())
        .ok_or_else(|| ApiError::NotFound(format!("worktree not found: {worktree_id}")))?;
    let host = load_host_for_repo(state, &repo_id).await?;
    let scope = scope_from_registry(&rows, worktree_id, claimed_workdir, host.id)?;
    Ok((scope, host))
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

/// A worktree's tracker-sync coordinates (spec 012) — exactly the fields the
/// session-start reactor and the PR/merge poller (`crate::tracker_sync`) read
/// off the registry. A plain owned view so those sibling modules never touch
/// `Worktree`'s private fields.
#[derive(Debug, Clone)]
pub(crate) struct TrackerWorktree {
    pub id: String,
    /// Local/remote worktree path. The lifecycle poller uses it for a
    /// best-effort local `develop` ancestry probe; remote-only paths simply
    /// fail closed and retain the GitHub PR polling path.
    pub path: Option<String>,
    /// The worktree's branch, when known and not detached — the poller's
    /// `gh pr list --head <branch>` key.
    pub branch: Option<String>,
    /// The branch tip recorded when Agentum created the worktree. This lets the
    /// local integration detector distinguish a real feature commit from a
    /// fresh, unchanged branch that naturally equals `develop`.
    pub initial_head: Option<String>,
    pub tracker_provider: Option<String>,
    pub tracker_url: Option<String>,
    pub tracker_phase: Option<String>,
    pub linked_pr: Option<i64>,
    pub linked_linear_issue: Option<String>,
}

/// Project a registry `Worktree` (already enriched with git path/branch) into
/// the tracker-sync view. The path/branch come from the persisted `extra` the
/// create handler stamped, so no git subprocess runs here (spec 009's
/// no-N×remote-sweep discipline).
fn tracker_view(wt: &Worktree) -> TrackerWorktree {
    let path = wt
        .extra
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|p| !p.is_empty())
        .or_else(|| wt.id.split_once("::").map(|(_, p)| p.to_string()));
    let branch = wt
        .extra
        .get("branch")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|b| !b.is_empty() && b != "HEAD");
    let initial_head = wt
        .extra
        .get("head")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|head| !head.is_empty());
    TrackerWorktree {
        id: wt.id.clone(),
        path,
        branch,
        initial_head,
        tracker_provider: wt.tracker_provider.clone(),
        tracker_url: wt.tracker_url.clone(),
        tracker_phase: wt.tracker_phase.clone(),
        linked_pr: wt.linked_pr,
        linked_linear_issue: wt.linked_linear_issue.clone(),
    }
}

/// Every worktree as a tracker-sync view (spec 012). Best-effort: a corrupt or
/// absent registry reads as empty, so the poller tick never wedges.
pub(crate) fn list_tracker_worktrees() -> Vec<TrackerWorktree> {
    read_worktrees()
        .map(|wts| wts.iter().map(tracker_view).collect())
        .unwrap_or_default()
}

/// The tracker-sync view for the worktree whose on-disk path == `path` (the
/// session workdir the reactor resolves from a `session.started` event). `None`
/// when no registry row matches — a plain, non-registered workdir is a silent
/// no-op.
pub(crate) fn find_tracker_worktree_by_path(path: &str) -> Option<TrackerWorktree> {
    let want = path.trim_end_matches('/');
    read_worktrees()
        .ok()?
        .iter()
        .find(|wt| {
            let by_extra = wt
                .extra
                .get("path")
                .and_then(Value::as_str)
                .map(|p| p.trim_end_matches('/'));
            let by_id = wt.id.split_once("::").map(|(_, p)| p.trim_end_matches('/'));
            by_extra == Some(want) || by_id == Some(want)
        })
        .map(tracker_view)
}

/// Persist tracker lifecycle progress (spec 012) onto a worktree by id: the
/// last-written pipeline phase and/or the detected PR number. Best-effort — a
/// missing worktree is a silent `Ok(())` no-op (the reactor/poller never halt
/// on a registry miss). Reuses the same read/write path `update_meta` does, so
/// the registry stays single-shape.
pub(crate) fn persist_tracker_progress(
    worktree_id: &str,
    phase: Option<&str>,
    linked_pr: Option<i64>,
) -> Result<(), ApiError> {
    let mut worktrees = read_worktrees()?;
    let Some(wt) = worktrees.iter_mut().find(|w| w.id == worktree_id) else {
        return Ok(());
    };
    if let Some(phase) = phase {
        wt.tracker_phase = Some(phase.to_string());
    }
    if let Some(pr) = linked_pr {
        wt.linked_pr = Some(pr);
    }
    write_worktrees(&worktrees)
}

fn apply_confirmed_tracker_phase(
    worktrees: &mut [Worktree],
    worktree_id: &str,
    phase: &str,
) -> bool {
    let Some(wt) = worktrees.iter_mut().find(|w| w.id == worktree_id) else {
        return false;
    };
    if wt.tracker_phase.as_deref() == Some(phase) {
        return false;
    }
    // Reconciliation is intentionally allowed to move backward: GitHub has
    // just confirmed the external value, so a stale local In Progress must be
    // replaced when the Project says TODO.
    wt.tracker_phase = Some(phase.to_string());
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReconcileGithubStatusBody {
    worktree_id: String,
    status_option_id: String,
}

/// Persist an implementation/cache phase only after the desktop's live GitHub
/// read returned the bound Status option id. The option id is checked against
/// the server-owned binding before any registry write; unknown/ambiguous ids
/// leave the cache untouched while the UI continues to show GitHub's name.
async fn reconcile_github_status(
    Json(body): Json<ReconcileGithubStatusBody>,
) -> Result<Json<Value>, ApiError> {
    let rows = read_worktrees()?;
    let wt = rows
        .iter()
        .find(|wt| wt.id == body.worktree_id)
        .ok_or_else(|| ApiError::BadRequest("worktree is not registered".into()))?;
    if wt.tracker_provider.as_deref() != Some("github") {
        return Err(ApiError::BadRequest(
            "worktree is not linked to GitHub".into(),
        ));
    }
    let url = wt
        .tracker_url
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("worktree has no GitHub issue URL".into()))?;
    let (slug, _) = crate::task_sink::github_slug_and_number_from_issue_url(url)
        .ok_or_else(|| ApiError::BadRequest("worktree has an invalid GitHub issue URL".into()))?;
    let binding = crate::github_projects::binding_for_slug(&slug)
        .ok_or_else(|| ApiError::BadRequest("repository has no GitHub Project binding".into()))?;
    let Some(phase) = binding
        .status_mapping
        .tracker_phase_for_option_id(&body.status_option_id)
    else {
        return Ok(Json(serde_json::json!({
            "reconciled": false,
            "phase": null,
        })));
    };
    let wire = phase.wire_str();
    let mut rows = rows;
    let changed = apply_confirmed_tracker_phase(&mut rows, &body.worktree_id, wire);
    if changed {
        write_worktrees(&rows)?;
    }
    Ok(Json(serde_json::json!({
        "reconciled": changed,
        "phase": wire,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReconcileLinearStatusBody {
    worktree_id: String,
    state_name: String,
}

/// Persist Linear's externally reported workflow state as the canonical local
/// cache phase. Unknown and ambiguous names are truthful no-ops: the board can
/// keep its prior confirmed lane without guessing from Linear's custom names.
async fn reconcile_linear_status(
    Json(body): Json<ReconcileLinearStatusBody>,
) -> Result<Json<Value>, ApiError> {
    let rows = read_worktrees()?;
    let wt = rows
        .iter()
        .find(|wt| wt.id == body.worktree_id)
        .ok_or_else(|| ApiError::BadRequest("worktree is not registered".into()))?;
    if crate::tracker_sync::resolve_binding(
        wt.tracker_provider.as_deref(),
        wt.tracker_url.as_deref(),
    )
    .filter(|(provider, _)| provider == "linear")
    .is_none()
    {
        return Err(ApiError::BadRequest(
            "worktree is not linked to Linear".into(),
        ));
    }
    let map = crate::linear::LinearStateMap::from_env();
    let Some(phase) = map.tracker_phase_for_state_name(&body.state_name) else {
        return Ok(Json(serde_json::json!({
            "reconciled": false,
            "phase": null,
        })));
    };
    let wire = phase.wire_str();
    let mut rows = rows;
    let changed = apply_confirmed_tracker_phase(&mut rows, &body.worktree_id, wire);
    if changed {
        write_worktrees(&rows)?;
    }
    Ok(Json(serde_json::json!({
        "reconciled": changed,
        "phase": wire,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransitionTrackerBody {
    worktree_id: String,
    target_phase: String,
}

fn validate_workspace_board_tracker_mapping(
    wt: &Worktree,
    target: crate::task_sink::TrackerPhase,
) -> Result<(String, String, String), ApiError> {
    let (provider, url) = crate::tracker_sync::resolve_binding(
        wt.tracker_provider.as_deref(),
        wt.tracker_url.as_deref(),
    )
    .ok_or_else(|| ApiError::BadRequest("worktree is not linked to a supported tracker".into()))?;

    match provider.as_str() {
        "github" => {
            let (slug, _) = crate::task_sink::github_slug_and_number_from_issue_url(&url)
                .ok_or_else(|| {
                    ApiError::BadRequest("worktree has an invalid GitHub issue URL".into())
                })?;
            let binding = crate::github_projects::binding_for_slug(&slug).ok_or_else(|| {
                ApiError::BadRequest("repository has no GitHub Project binding".into())
            })?;
            let option_id = binding.status_mapping.option_id(target.into());
            if binding
                .status_mapping
                .tracker_phase_for_option_id(option_id)
                != Some(target)
            {
                return Err(ApiError::BadRequest(
                    "target phase is not uniquely mapped in the GitHub Project".into(),
                ));
            }
        }
        "linear" => {
            let map = crate::linear::LinearStateMap::from_env();
            if map.tracker_phase_for_state_name(map.name_for(target)) != Some(target) {
                return Err(ApiError::BadRequest(
                    "target phase is not uniquely mapped in Linear".into(),
                ));
            }
        }
        _ => unreachable!("resolve_binding only returns supported providers"),
    }

    let tracker_id =
        crate::tracker_sync::tracker_id_for(&provider, &url, wt.linked_linear_issue.as_deref());
    Ok((provider, url, tracker_id))
}

fn acknowledged_workspace_board_tracker_phase(
    result: anyhow::Result<crate::task_sink::TransitionResult>,
    target: crate::task_sink::TrackerPhase,
) -> Result<&'static str, ApiError> {
    match result {
        Ok(crate::task_sink::TransitionResult::Applied) => Ok(target.wire_str()),
        Ok(crate::task_sink::TransitionResult::Skipped(reason)) => Err(ApiError::Conflict(reason)),
        Err(error) => Err(ApiError::Internal(format!(
            "tracker transition failed: {error}"
        ))),
    }
}

fn persist_workspace_board_tracker_phase(worktree_id: &str, phase: &str) -> Result<(), ApiError> {
    let mut rows = read_worktrees()?;
    if !rows.iter().any(|row| row.id == worktree_id) {
        return Err(ApiError::NotFound(format!(
            "worktree was removed before tracker acknowledgement: {worktree_id}"
        )));
    }
    if apply_confirmed_tracker_phase(&mut rows, worktree_id, phase) {
        write_worktrees(&rows)?;
    }
    Ok(())
}

/// Move one registered workspace through its linked tracker. The external
/// provider remains authoritative: only an acknowledged `Applied` result is
/// persisted, while skipped/failed transitions return a non-success response.
async fn transition_tracker(
    State(state): State<AppState>,
    Json(body): Json<TransitionTrackerBody>,
) -> Result<Json<Value>, ApiError> {
    let target = crate::task_sink::parse_tracker_phase(&body.target_phase)
        .ok_or_else(|| ApiError::BadRequest("invalid targetPhase".into()))?;
    let rows = read_worktrees()?;
    let wt = rows
        .iter()
        .find(|wt| wt.id == body.worktree_id)
        .ok_or_else(|| ApiError::BadRequest("worktree is not registered".into()))?;
    let (provider, url, tracker_id) = validate_workspace_board_tracker_mapping(wt, target)?;
    let emit = crate::task_sink::TrackerEmit {
        bus: &state.bus,
        worktree_id: Some(&body.worktree_id),
    };
    let result = crate::task_sink::apply_tracker_transition(
        &provider,
        &tracker_id,
        Some(&url),
        target,
        emit,
    )
    .await;
    let phase = acknowledged_workspace_board_tracker_phase(result, target)?;
    persist_workspace_board_tracker_phase(&body.worktree_id, phase)?;
    Ok(Json(serde_json::json!({
        "applied": true,
        "phase": phase,
    })))
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

/// SSH reserves exit status 255 for client/transport failures. A remote `git`
/// failure (dirty worktree, lock, bad path, …) exits with its own non-zero code
/// and must keep its original error so the UI can offer the appropriate action.
fn is_ssh_transport_output(host: &Host, output: &HostCommandOutput) -> bool {
    matches!(host.kind, HostKind::Ssh { .. })
        && !output.success
        && (output.code == Some(255) || output.code.is_none())
}

/// Errors returned before an SSH child produces an exit status are connection
/// failures when they are a timeout or an I/O failure (including a missing SSH
/// executable). Quote/programming errors are not mislabeled as host downtime.
fn is_ssh_transport_error(host: &Host, error: &HostRuntimeError) -> bool {
    matches!(host.kind, HostKind::Ssh { .. })
        && matches!(error, HostRuntimeError::Timeout | HostRuntimeError::Io(_))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoveOutputKind {
    Removed,
    Stale,
    SshUnavailable,
    Rejected,
}

/// Decide whether a completed child process removed the tree, proved a stale
/// registration, failed at the SSH transport, or was rejected by Git. Keeping
/// this decision pure makes the local/reachable/offline boundary regression-
/// testable without depending on a live SSH daemon.
fn classify_remove_output(host: &Host, output: &HostCommandOutput) -> RemoveOutputKind {
    if output.success {
        return RemoveOutputKind::Removed;
    }
    if is_ssh_transport_output(host, output) {
        return RemoveOutputKind::SshUnavailable;
    }
    if output.stderr.contains("is a main working tree")
        || output.stderr.contains("is not a working tree")
        || output.stderr.contains("not a working tree")
        || output.stderr.contains("No such file or directory")
    {
        return RemoveOutputKind::Stale;
    }
    RemoveOutputKind::Rejected
}

/// Consistent user-facing failure for every unavailable-SSH delete path. The
/// last sentence documents the important recovery invariant: retrying later is
/// safe because the local registry still points at the remote checkout.
fn unavailable_ssh_remove_error(host: Option<&Host>, detail: &str) -> ApiError {
    let location = host
        .map(host_location)
        .unwrap_or_else(|| "the configured SSH host".to_string());
    let detail = detail.trim();
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(" Details: {detail}")
    };
    ApiError::BadRequest(format!(
        "Cannot delete this worktree because {location} is unavailable. Reconnect the SSH host and retry. Agentum kept the worktree registered locally so the remote checkout is not orphaned.{detail}"
    ))
}

/// Rebuildable process/profile state is independent of the remote checkout and
/// is safe to remove even when SSH is unavailable. Registry metadata is *not*
/// handled here: it is retained on a connection failure and removed only after
/// `git worktree remove` succeeds (or proves the entry stale).
async fn cleanup_local_worktree_state(worktree_id: &str) {
    if let Err(error) = crate::cdp_browser::stop_local_cdp_browser_for(worktree_id).await {
        tracing::warn!(
            worktree_id,
            %error,
            "failed to clean up local browser state during worktree delete"
        );
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

/// Map wire-casing variants onto the registry struct's serde names before the
/// metadata upsert. The UI writes `linkedPR` (shared/types.ts pins that
/// casing) but the registry field serializes as `linkedPr`; without this the
/// update lands in `extra` and shadows the typed field forever (spec 004 C3).
fn canonical_meta_key(key: &str) -> &str {
    match key {
        "linkedPR" => "linkedPr",
        other => other,
    }
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
        let key = canonical_meta_key(&key).to_string();
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
    /// Linked work-item metadata (spec 004 AC 2). The UI sends `linkedPR`
    /// (shared/types.ts pins that casing) while camelCase yields `linkedPr` —
    /// alias HERE only, never on the registry `Worktree` struct: legacy rows
    /// can carry a shadowed `linkedPR` in `extra`, and a struct alias would
    /// make serde see the field twice → parse error → `read_worktrees`
    /// collapses to `[]` and the next write wipes the registry.
    #[serde(default)]
    linked_issue: Option<i64>,
    #[serde(default, alias = "linkedPR")]
    linked_pr: Option<i64>,
    #[serde(default)]
    linked_linear_issue: Option<String>,
    /// Spec 012 bind coords: the tracker provider (`github`/`linear`) and the
    /// picked item's canonical URL, persisted so the session-start reactor and
    /// PR/merge poller can drive the item's status without a per-event
    /// `git remote` lookup. Optional + no alias: an old client that omits them
    /// still creates a workspace, bound to nothing (fail-closed, AC 3).
    #[serde(default)]
    tracker_provider: Option<String>,
    #[serde(default)]
    tracker_url: Option<String>,
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
        linked_issue: body.linked_issue,
        linked_pr: body.linked_pr,
        linked_linear_issue: body.linked_linear_issue,
        tracker_provider: body.tracker_provider,
        tracker_url: body.tracker_url,
        // The reactor stamps `in_progress` on first session start; a create is
        // Todo (no phase written) so the monotonic guard advances cleanly.
        tracker_phase: None,
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
/// anyway after a `worktree prune`; real failures (dirty/locked) surface. An
/// unavailable SSH host keeps the registry row, cleans rebuildable local state,
/// and returns an actionable reconnect-and-retry error.
async fn remove(
    State(state): State<AppState>,
    Json(body): Json<RemoveBody>,
) -> Result<Json<Value>, ApiError> {
    let (repo_id, worktree_path) = body.worktree_id.split_once("::").ok_or_else(|| {
        ApiError::BadRequest(format!("invalid worktree id: {}", body.worktree_id))
    })?;
    reject_dashed("worktree path", worktree_path)?;
    let repo_path = resolve_repo_path(repo_id)?;
    let remote_repo = resolve_repo_host_id(repo_id)?.is_some();
    let host = match load_host_for_repo(&state, repo_id).await {
        Ok(host) => host,
        Err(error) if remote_repo => {
            // The saved host may have been removed while its repo/worktree
            // registrations remain. There is no safe remote deletion without
            // those connection details, but ephemeral local state can go.
            cleanup_local_worktree_state(&body.worktree_id).await;
            return Err(unavailable_ssh_remove_error(None, &error.to_string()));
        }
        Err(error) => return Err(error),
    };

    let mut args = vec!["worktree", "remove"];
    if body.force.unwrap_or(false) {
        args.push("--force");
    }
    args.push(worktree_path);
    let output = match git_in_dir(&host, &repo_path, &args).await {
        Ok(output) => output,
        Err(error) if is_ssh_transport_error(&host, &error) => {
            cleanup_local_worktree_state(&body.worktree_id).await;
            return Err(unavailable_ssh_remove_error(
                Some(&host),
                &error.to_string(),
            ));
        }
        Err(error) => return Err(ApiError::Internal(error.to_string())),
    };
    match classify_remove_output(&host, &output) {
        RemoveOutputKind::Removed => {}
        RemoveOutputKind::SshUnavailable => {
            cleanup_local_worktree_state(&body.worktree_id).await;
            return Err(unavailable_ssh_remove_error(Some(&host), &output.stderr));
        }
        RemoveOutputKind::Rejected => {
            return Err(ApiError::BadRequest(output.stderr.trim().to_string()));
        }
        RemoveOutputKind::Stale => {
            let _ = git_in_dir(&host, &repo_path, &["worktree", "prune"]).await;
        }
    }

    // Remote cleanup is now known to be complete or unnecessary. Tear down the
    // local browser before fallible registry I/O so it cannot leak when a local
    // bookkeeping write fails. This is also run on the unavailable-SSH paths
    // above, where the independently important registry row is retained.
    // Use the full `<repoId>::<path>` identity. It remains project-scoped even
    // if a later registry write fails, and preserves spec 014's shared-browser
    // claim semantics while this row is still available for resolution.
    cleanup_local_worktree_state(&body.worktree_id).await;
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
                    // Release the worktree's claim on its project browser too
                    // (best-effort, idempotent; no-op for remote/never-launched
                    // worktrees). Full `<repoId>::<path>` id so it resolves to
                    // the project even after deregistration (spec 014); the
                    // project profile persists.
                    let _ = crate::cdp_browser::stop_local_cdp_browser_for(&format!(
                        "{repo_id}::{}",
                        wt.path
                    ))
                    .await;
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
        .map(|(idx, (path, branch))| detected_row(repo_id, idx, path, branch, &registry))
        .collect())
}

/// One `/api/worktrees/detected` row: the git-authoritative path/branch
/// overlaid with persisted registry metadata. Pure (no git, no IO) so the wire
/// shape — including the spec 014 tracker keys — is unit-testable.
fn detected_row(
    repo_id: &str,
    idx: usize,
    path: String,
    branch: Option<String>,
    registry: &[Worktree],
) -> Value {
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
        // The UI reads `linkedPR` (shared/types.ts pins that casing);
        // the old `linkedPr` key here was dead — no reader anywhere.
        "linkedPR": meta.and_then(|m| m.linked_pr),
        "linkedLinearIssue": meta.and_then(|m| m.linked_linear_issue.clone()),
        // Spec 014 F2 (AC 4): the persisted tracker coords + phase ride
        // on the detected rows so the sidebar chip has a cold-truth
        // source. Unbound (no registry row / no bind) ⇒ all three null
        // ⇒ no chip (fail-closed). Registry `Worktree` shape untouched.
        "trackerProvider": meta.and_then(|m| m.tracker_provider.clone()),
        "trackerUrl": meta.and_then(|m| m.tracker_url.clone()),
        "trackerPhase": meta.and_then(|m| m.tracker_phase.clone()),
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

    fn test_host(name: &str, kind: HostKind) -> Host {
        use agentum_core::LOCAL_HOST_ID;
        use time::OffsetDateTime;

        let now = OffsetDateTime::now_utc();
        Host {
            id: LOCAL_HOST_ID,
            name: name.into(),
            kind,
            created_at: now,
            updated_at: now,
            last_seen_at: None,
        }
    }

    fn local_host() -> Host {
        test_host("local", HostKind::Local)
    }

    fn ssh_host() -> Host {
        test_host(
            "forge",
            HostKind::Ssh {
                user: "malloc".into(),
                hostname: "forge.lan".into(),
                port: 22,
                auth: agentum_core::SshAuth::Agent,
            },
        )
    }

    fn remove_output(success: bool, code: Option<i32>, stderr: &str) -> HostCommandOutput {
        HostCommandOutput {
            success,
            code,
            stdout: Vec::new(),
            stderr: stderr.into(),
        }
    }

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
        let local = local_host();
        assert_eq!(host_location(&local), "this machine");
        let ssh = ssh_host();
        assert_eq!(host_location(&ssh), "the remote host forge.lan");
    }

    #[test]
    fn worktree_remove_classifies_local_and_reachable_ssh_results() {
        let removed = remove_output(true, Some(0), "");
        assert_eq!(
            classify_remove_output(&local_host(), &removed),
            RemoveOutputKind::Removed
        );
        assert_eq!(
            classify_remove_output(&ssh_host(), &removed),
            RemoveOutputKind::Removed
        );

        // A reachable host passed Git's own dirty-worktree error back through
        // ssh. This must remain a normal rejection so force-delete stays
        // available; it is not a connectivity failure.
        let dirty = remove_output(
            false,
            Some(128),
            "fatal: '/repo/wt' contains modified or untracked files, use --force to delete it",
        );
        assert_eq!(
            classify_remove_output(&ssh_host(), &dirty),
            RemoveOutputKind::Rejected
        );

        let stale = remove_output(false, Some(128), "fatal: '/repo/wt' is not a working tree");
        assert_eq!(
            classify_remove_output(&local_host(), &stale),
            RemoveOutputKind::Stale
        );
        assert_eq!(
            classify_remove_output(&ssh_host(), &stale),
            RemoveOutputKind::Stale
        );
    }

    #[test]
    fn worktree_remove_classifies_unavailable_ssh_transport_only() {
        let refused = remove_output(
            false,
            Some(255),
            "ssh: connect to host forge.lan port 22: Connection refused",
        );
        assert_eq!(
            classify_remove_output(&ssh_host(), &refused),
            RemoveOutputKind::SshUnavailable
        );

        // Exit 255 is SSH-specific. A local child with the same status must not
        // take the remote recovery path or change local deletion semantics.
        assert_eq!(
            classify_remove_output(&local_host(), &refused),
            RemoveOutputKind::Rejected
        );

        assert!(is_ssh_transport_error(
            &ssh_host(),
            &HostRuntimeError::Timeout
        ));
        assert!(!is_ssh_transport_error(
            &local_host(),
            &HostRuntimeError::Timeout
        ));
        assert!(!is_ssh_transport_error(
            &ssh_host(),
            &HostRuntimeError::Quote
        ));
    }

    #[test]
    fn unavailable_ssh_worktree_remove_error_is_actionable_and_safe() {
        let error = unavailable_ssh_remove_error(
            Some(&ssh_host()),
            "ssh: connect to host forge.lan port 22: Connection refused",
        );
        let ApiError::BadRequest(message) = error else {
            panic!("unavailable SSH should be a user-correctable bad request");
        };
        assert!(message.contains("remote host forge.lan is unavailable"));
        assert!(message.contains("Reconnect the SSH host and retry"));
        assert!(message.contains("kept the worktree registered locally"));
        assert!(message.contains("Connection refused"));

        let missing_host = unavailable_ssh_remove_error(None, "repo host is missing");
        let ApiError::BadRequest(message) = missing_host else {
            panic!("missing configured SSH host should be actionable");
        };
        assert!(message.contains("configured SSH host is unavailable"));
        assert!(message.contains("kept the worktree registered locally"));
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

    fn registered_worktree(id: &str, repo_id: &str) -> Worktree {
        Worktree {
            id: id.into(),
            repo_id: repo_id.into(),
            display_name: "test".into(),
            comment: String::new(),
            linked_issue: None,
            linked_pr: None,
            linked_linear_issue: None,
            tracker_provider: None,
            tracker_url: None,
            tracker_phase: None,
            is_archived: false,
            is_unread: false,
            is_pinned: false,
            sort_order: 0,
            last_activity_at: 0,
            extra: Map::new(),
        }
    }

    fn absolute_test_worktree_path(name: &str) -> String {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn harness_scope_requires_exact_registry_identity_and_path() {
        let host_id = uuid::Uuid::new_v4();
        let worktree_path = absolute_test_worktree_path("agentum-harness-scope-wt");
        let worktree_id = format!("repo::{worktree_path}");
        let rows = vec![registered_worktree(&worktree_id, "repo")];
        let claimed_workdir = format!("{worktree_path}/");
        let scope = scope_from_registry(&rows, &worktree_id, &claimed_workdir, host_id).unwrap();
        assert_eq!(scope.worktree_id.as_deref(), Some(worktree_id.as_str()));
        assert_eq!(scope.repo_id.as_deref(), Some("repo"));
        assert_eq!(scope.host_id, Some(host_id));
        assert_eq!(scope.path, worktree_path);

        let missing_id = format!(
            "repo::{}",
            absolute_test_worktree_path("agentum-harness-scope-missing")
        );
        assert!(matches!(
            scope_from_registry(&rows, &missing_id, &scope.path, host_id),
            Err(ApiError::NotFound(_))
        ));
        let lookalike = absolute_test_worktree_path("agentum-harness-scope-lookalike");
        assert!(matches!(
            scope_from_registry(&rows, &worktree_id, &lookalike, host_id),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn harness_scope_rejects_tampered_repo_prefix_and_traversal() {
        let host_id = uuid::Uuid::new_v4();
        let worktree_path = absolute_test_worktree_path("agentum-harness-scope-tampered");
        let tampered_id = format!("other::{worktree_path}");
        let tampered = vec![registered_worktree(&tampered_id, "repo")];
        assert!(matches!(
            scope_from_registry(&tampered, &tampered_id, &worktree_path, host_id),
            Err(ApiError::BadRequest(_))
        ));
        let traversal_path = std::env::temp_dir()
            .join("agentum-harness-scope")
            .join("..")
            .join("secret")
            .to_string_lossy()
            .into_owned();
        let traversal_id = format!("repo::{traversal_path}");
        let traversal = vec![registered_worktree(&traversal_id, "repo")];
        assert!(matches!(
            scope_from_registry(&traversal, &traversal_id, &traversal_path, host_id),
            Err(ApiError::BadRequest(_))
        ));
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
            tracker_provider: None,
            tracker_url: None,
            tracker_phase: None,
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

        // Spec 004 regression guard (the no-alias rule): the registry struct's
        // on-disk keys are exactly these — `linkedPr`, never `linkedPR`.
        // Aliasing the struct would make serde see legacy rows' shadowed
        // `linkedPR` extra key as a duplicate field → `read_worktrees` wipes
        // the registry to `[]` on the next write.
        let obj = v.as_object().unwrap();
        assert!(obj.contains_key("linkedPr"));
        assert!(obj.contains_key("linkedIssue"));
        assert!(obj.contains_key("linkedLinearIssue"));
        assert!(!obj.contains_key("linkedPR"));
    }

    /// Spec 004 AC 2: the create body accepts the UI's exact wire casing
    /// (`linkedPR` — shared/types.ts) and the camelCase spelling (`linkedPr`).
    #[test]
    fn create_body_accepts_ui_linked_keys() {
        let body: CreateBody = serde_json::from_value(serde_json::json!({
            "repoId": "r1",
            "name": "feat",
            "linkedIssue": 42,
            "linkedPR": 7,
            "linkedLinearIssue": "ENG-9"
        }))
        .unwrap();
        assert_eq!(body.linked_issue, Some(42));
        assert_eq!(body.linked_pr, Some(7));
        assert_eq!(body.linked_linear_issue.as_deref(), Some("ENG-9"));

        // The camelCase variant is accepted too (alias, not rename).
        let body: CreateBody = serde_json::from_value(serde_json::json!({
            "repoId": "r1",
            "name": "feat",
            "linkedPr": 7
        }))
        .unwrap();
        assert_eq!(body.linked_pr, Some(7));
    }

    /// An old client's payload (the original five keys only) still parses;
    /// the linked fields default to None — purely additive widening.
    #[test]
    fn create_body_defaults_absent_linked_fields() {
        let body: CreateBody = serde_json::from_value(serde_json::json!({
            "repoId": "r1",
            "name": "feat",
            "baseBranch": "develop",
            "branchNameOverride": "feat/x",
            "displayName": "Feat"
        }))
        .unwrap();
        assert_eq!(body.linked_issue, None);
        assert_eq!(body.linked_pr, None);
        assert_eq!(body.linked_linear_issue, None);
    }

    /// Spec 012 AC 2/AC 3: the create body accepts the tracker bind coords
    /// (`trackerProvider` + `trackerUrl`) and, when a client omits them (the
    /// old shape), defaults both to `None` — an unbound-but-created workspace.
    #[test]
    fn create_body_accepts_and_defaults_tracker_coords() {
        let body: CreateBody = serde_json::from_value(serde_json::json!({
            "repoId": "r1",
            "name": "feat",
            "linkedIssue": 42,
            "trackerProvider": "github",
            "trackerUrl": "https://github.com/o/r/issues/42"
        }))
        .unwrap();
        assert_eq!(body.linked_issue, Some(42));
        assert_eq!(body.tracker_provider.as_deref(), Some("github"));
        assert_eq!(
            body.tracker_url.as_deref(),
            Some("https://github.com/o/r/issues/42")
        );

        // An old client that never sends the coords still parses; both default None.
        let body: CreateBody = serde_json::from_value(serde_json::json!({
            "repoId": "r1",
            "name": "feat"
        }))
        .unwrap();
        assert_eq!(body.tracker_provider, None);
        assert_eq!(body.tracker_url, None);
    }

    /// Spec 012 AC 2: the persisted `Worktree` serializes the tracker coords as
    /// camelCase (`trackerProvider`/`trackerUrl`/`trackerPhase`) — the exact
    /// keys `find_tracker_worktree_by_path` reads back — and never as a
    /// `tracker_*` snake_case key.
    #[test]
    fn worktree_serializes_tracker_coords_camel_case() {
        let wt = Worktree {
            id: "r1::/p".into(),
            repo_id: "r1".into(),
            display_name: "p".into(),
            comment: String::new(),
            linked_issue: Some(42),
            linked_pr: None,
            linked_linear_issue: None,
            tracker_provider: Some("github".into()),
            tracker_url: Some("https://github.com/o/r/issues/42".into()),
            tracker_phase: Some("in_progress".into()),
            is_archived: false,
            is_unread: false,
            is_pinned: false,
            sort_order: 0,
            last_activity_at: 0,
            extra: Map::new(),
        };
        let v = serde_json::to_value(&wt).unwrap();
        assert_eq!(v["trackerProvider"], "github");
        assert_eq!(v["trackerUrl"], "https://github.com/o/r/issues/42");
        assert_eq!(v["trackerPhase"], "in_progress");
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("tracker_provider"));
        assert!(!obj.contains_key("tracker_url"));
    }

    #[test]
    fn confirmed_external_todo_reconciles_stale_local_in_progress() {
        let mut rows = vec![Worktree {
            id: "r1::/p".into(),
            repo_id: "r1".into(),
            display_name: "p".into(),
            comment: String::new(),
            linked_issue: Some(42),
            linked_pr: None,
            linked_linear_issue: None,
            tracker_provider: Some("github".into()),
            tracker_url: Some("https://github.com/o/r/issues/42".into()),
            tracker_phase: Some("in_progress".into()),
            is_archived: false,
            is_unread: false,
            is_pinned: false,
            sort_order: 0,
            last_activity_at: 0,
            extra: Map::new(),
        }];
        assert!(apply_confirmed_tracker_phase(&mut rows, "r1::/p", "todo"));
        assert_eq!(rows[0].tracker_phase.as_deref(), Some("todo"));
    }

    #[test]
    fn workspace_board_tracker_applied_phase_preserves_legacy_workspace_status() {
        let mut wt = registered_worktree("r1::/p", "r1");
        wt.tracker_phase = Some("todo".into());
        wt.extra.insert(
            "workspaceStatus".into(),
            Value::String("private-review".into()),
        );
        let mut rows = vec![wt];

        assert!(apply_confirmed_tracker_phase(&mut rows, "r1::/p", "done"));
        assert_eq!(rows[0].tracker_phase.as_deref(), Some("done"));
        assert_eq!(
            rows[0].extra.get("workspaceStatus").and_then(Value::as_str),
            Some("private-review")
        );
    }

    #[test]
    fn workspace_board_tracker_persists_only_applied_outcomes() {
        use crate::task_sink::{TrackerPhase, TransitionResult};

        assert_eq!(
            acknowledged_workspace_board_tracker_phase(
                Ok(TransitionResult::Applied),
                TrackerPhase::ReadyToTest
            )
            .unwrap(),
            "ready_to_test"
        );
        assert!(matches!(
            acknowledged_workspace_board_tracker_phase(
                Ok(TransitionResult::Skipped("unmapped".into())),
                TrackerPhase::Done
            ),
            Err(ApiError::Conflict(_))
        ));
        assert!(matches!(
            acknowledged_workspace_board_tracker_phase(
                Err(anyhow::anyhow!("offline")),
                TrackerPhase::Done
            ),
            Err(ApiError::Internal(_))
        ));
    }

    /// Spec 014 F2 (AC 4): a `detected` row for a BOUND worktree carries the
    /// three camelCase tracker keys from the registry; a worktree with NO
    /// registry row exposes all three as null (fail-closed → no chip, AC 6).
    #[test]
    fn detected_row_exposes_tracker_keys_bound_and_null_unbound() {
        let wt = Worktree {
            id: "r1::/p".into(),
            repo_id: "r1".into(),
            display_name: "p".into(),
            comment: String::new(),
            linked_issue: Some(42),
            linked_pr: None,
            linked_linear_issue: None,
            tracker_provider: Some("github".into()),
            tracker_url: Some("https://github.com/o/r/issues/42".into()),
            tracker_phase: Some("in_progress".into()),
            is_archived: false,
            is_unread: false,
            is_pinned: false,
            sort_order: 0,
            last_activity_at: 0,
            extra: Map::new(),
        };

        // Bound: registry row matches by id → the persisted coords ride out.
        let row = detected_row("r1", 0, "/p".into(), Some("feat/x".into()), &[wt]);
        assert_eq!(row["trackerProvider"], "github");
        assert_eq!(row["trackerUrl"], "https://github.com/o/r/issues/42");
        assert_eq!(row["trackerPhase"], "in_progress");

        // Unbound: no registry row → all three null, keys still present.
        let row = detected_row("r1", 0, "/p".into(), Some("feat/x".into()), &[]);
        let obj = row.as_object().unwrap();
        for key in ["trackerProvider", "trackerUrl", "trackerPhase"] {
            assert!(obj.contains_key(key), "{key} must be present");
            assert!(row[key].is_null(), "{key} must be null when unbound");
        }
    }

    /// Spec 012 invariant #7 (the spec-004 lesson, restated for the new fields):
    /// an OLD-shape registry — a full worktree list written before the tracker
    /// fields existed — deserializes with `tracker_* == None` and the list
    /// **preserved**, NOT collapsed to `[]`. This is exactly what
    /// `read_worktrees`' `unwrap_or_default()` would turn into a registry wipe
    /// if a new field broke deserialization.
    #[test]
    fn old_shape_registry_round_trips_to_none_not_wiped() {
        // Two rows in the pre-012 shape (no tracker keys at all), plus a stray
        // legacy `extra` key to prove flatten still absorbs the unknown.
        let raw = r#"[
            {
                "id": "r1::/a", "repoId": "r1", "displayName": "a", "comment": "",
                "linkedIssue": 7, "linkedPr": null, "linkedLinearIssue": null,
                "isArchived": false, "isUnread": false, "isPinned": false,
                "sortOrder": 0, "lastActivityAt": 1, "branch": "feat/a"
            },
            {
                "id": "r2::/b", "repoId": "r2", "displayName": "b", "comment": "",
                "linkedIssue": null, "linkedPr": null, "linkedLinearIssue": null,
                "isArchived": false, "isUnread": false, "isPinned": false,
                "sortOrder": 1, "lastActivityAt": 2
            }
        ]"#;
        // The exact call `read_worktrees` makes; a broken widening would panic
        // into `unwrap_or_default()` → `[]` in production.
        let worktrees: Vec<Worktree> = serde_json::from_str(raw).unwrap_or_default();
        assert_eq!(worktrees.len(), 2, "old-shape list must NOT be wiped to []");
        for wt in &worktrees {
            assert_eq!(wt.tracker_provider, None);
            assert_eq!(wt.tracker_url, None);
            assert_eq!(wt.tracker_phase, None);
        }
        // The unknown legacy `branch` key rode into `extra` (flatten), untouched.
        assert_eq!(
            worktrees[0].extra.get("branch").and_then(Value::as_str),
            Some("feat/a")
        );
        // And the tracker view reads that branch back without a git call.
        assert_eq!(
            tracker_view(&worktrees[0]).branch.as_deref(),
            Some("feat/a")
        );
        assert_eq!(tracker_view(&worktrees[0]).path.as_deref(), Some("/a"));
        assert_eq!(tracker_view(&worktrees[0]).initial_head, None);
    }

    #[test]
    #[allow(non_snake_case)]
    fn canonical_meta_key_maps_linkedPR() {
        assert_eq!(canonical_meta_key("linkedPR"), "linkedPr");
        // Everything else passes through untouched.
        assert_eq!(canonical_meta_key("linkedPr"), "linkedPr");
        assert_eq!(canonical_meta_key("linkedIssue"), "linkedIssue");
        assert_eq!(canonical_meta_key("comment"), "comment");
    }
}
