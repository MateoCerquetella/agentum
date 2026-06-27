//! `/api/sessions/{id}/git/*` — minimal git status / diff / commit surface
//! for a session's worktree.
//!
//! The cwd is the session's `worktree_path` when present, otherwise the
//! raw `workdir` — same precedence as `Session::effective_cwd()` used by
//! the launch path. Shell-out to `git` (no libgit2) so this stays a
//! zero-extra-native-dep addition, matching `crate::git` (the worktree
//! module) and the principle in CLAUDE.md.
//!
//! Auth: rides the standard bearer-token middleware applied at the
//! `lib.rs::router()` merge site — no public exception.
//!
//! Endpoints:
//!   * `GET  /api/sessions/{id}/git/status`              → JSON arrays
//!   * `GET  /api/sessions/{id}/git/diff?path=…&staged=` → text/plain unified diff
//!   * `GET  /api/sessions/{id}/git/file?path=…&rev=`    → one revision's text
//!     (`head|index|worktree`), for the dashboard's CodeMirror side-by-side diff
//!   * `POST /api/sessions/{id}/git/stage`               → `{paths,unstage}` → refreshed status
//!   * `POST /api/sessions/{id}/git/commit`              → `{"sha": "…"}`
//!
//! Commits are authored as `agentum-bot <agentum@localhost>` via
//! `git -c user.name=… -c user.email=…` so a worktree that inherits a
//! missing/incomplete identity from `~/.gitconfig` still commits cleanly.

use std::path::{Component, Path as StdPath, PathBuf};

use base64::Engine as _;

use agentum_core::{Host, LOCAL_HOST_ID};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::host_runtime::{self, git_in_dir};

mod history_routes;
use history_routes::*;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/{id}/git/status", get(status))
        .route("/api/sessions/{id}/git/diff", get(diff))
        .route("/api/sessions/{id}/git/file", get(file))
        .route("/api/sessions/{id}/git/stage", post(stage))
        .route("/api/sessions/{id}/git/commit", post(commit))
        .route("/api/sessions/{id}/git/branches", get(branches))
        .route("/api/sessions/{id}/git/log", get(log))
        .route("/api/sessions/{id}/git/fetch", post(fetch))
        .route("/api/sessions/{id}/git/pull", post(pull))
        .route("/api/sessions/{id}/git/push", post(push))
        .route("/api/sessions/{id}/git/discard", post(discard))
        .route("/api/sessions/{id}/git/upstream", get(upstream))
        .route("/api/sessions/{id}/git/conflict", get(conflict))
        .route("/api/sessions/{id}/git/rebase", post(rebase))
        .route("/api/sessions/{id}/git/abort-merge", post(abort_merge))
        .route("/api/sessions/{id}/git/abort-rebase", post(abort_rebase))
        .route("/api/sessions/{id}/git/branch-compare", get(branch_compare))
        .route("/api/sessions/{id}/git/commit-compare", get(commit_compare))
        .route("/api/sessions/{id}/git/status-entries", get(status_entries))
        .route("/api/sessions/{id}/git/commit-staged", post(commit_staged))
        .route("/api/sessions/{id}/git/check-ignore", post(check_ignore))
        .route("/api/sessions/{id}/git/fast-forward", post(fast_forward))
        .route(
            "/api/sessions/{id}/git/remote-file-url",
            get(remote_file_url),
        )
        .route("/api/sessions/{id}/git/blob", get(blob))
        .route("/api/sessions/{id}/git/history", get(history))
}

/// One working-tree change with its staging area, for the desktop's
/// source-control panel (richer than `/status`'s three path arrays, which the
/// TUI consumes — kept separate so that contract is untouched).
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StatusEntry {
    path: String,
    /// `modified|added|deleted|renamed|untracked|copied`.
    status: String,
    /// `staged|unstaged|untracked`.
    area: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
}

/// Map a porcelain XY code char to the desktop `GitFileStatus` vocabulary.
fn map_porcelain_status(code: u8) -> &'static str {
    match code {
        b'A' => "added",
        b'D' => "deleted",
        b'R' => "renamed",
        b'C' => "copied",
        b'?' => "untracked",
        _ => "modified",
    }
}

/// Parse `git status --porcelain=v1 -z` into per-file, per-area entries.
/// A file staged AND further modified yields two entries (staged + unstaged).
/// Rename/copy records carry a NUL-separated source path (the staged side's
/// `oldPath`); the follow-up record is consumed here.
fn parse_status_entries(bytes: &[u8]) -> Vec<StatusEntry> {
    let mut out = Vec::new();
    let mut it = bytes
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .peekable();
    while let Some(rec) = it.next() {
        if rec.len() < 3 {
            continue;
        }
        let x = rec[0];
        let y = rec[1];
        let path = String::from_utf8_lossy(&rec[3..]).into_owned();
        let is_rename = x == b'R' || x == b'C' || y == b'R' || y == b'C';
        let old_path = if is_rename {
            it.next().map(|b| String::from_utf8_lossy(b).into_owned())
        } else {
            None
        };
        if x == b'?' && y == b'?' {
            out.push(StatusEntry {
                path,
                status: "untracked".to_string(),
                area: "untracked".to_string(),
                old_path: None,
            });
            continue;
        }
        if x != b' ' && x != b'?' {
            out.push(StatusEntry {
                path: path.clone(),
                status: map_porcelain_status(x).to_string(),
                area: "staged".to_string(),
                old_path: old_path.clone(),
            });
        }
        if y != b' ' && y != b'?' {
            out.push(StatusEntry {
                path,
                status: map_porcelain_status(y).to_string(),
                area: "unstaged".to_string(),
                old_path,
            });
        }
    }
    out
}

/// `GET /api/sessions/{id}/git/status-entries` — per-file working-tree changes.
async fn status_entries(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<StatusEntry>>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let raw = run_git_bytes(&host, &cwd, &["status", "--porcelain=v1", "-z"]).await?;
    Ok(Json(parse_status_entries(&raw)))
}

/// Like `run_git` but returns raw bytes (porcelain `-z` output is NUL-delimited
/// and not guaranteed valid UTF-8 at record boundaries). Host-aware.
async fn run_git_bytes(host: &Host, cwd: &str, args: &[&str]) -> Result<Vec<u8>, ApiError> {
    let out = git_in_dir(host, cwd, args)
        .await
        .map_err(|e| ApiError::Internal(format!("git {}: {e}", args.join(" "))))?;
    if !out.success {
        // Surface "not a repo" as a 400, not a 500. Letting git itself report
        // this (instead of a separate `is_git_repo` pre-check) saves one full
        // SSH round trip on every status poll against a remote host.
        if out.stderr.contains("not a git repository") {
            return Err(ApiError::BadRequest(format!("not a git repository: {cwd}")));
        }
        return Err(ApiError::Internal(format!(
            "git {} failed: {}",
            args.join(" "),
            out.stderr.trim()
        )));
    }
    Ok(out.stdout)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitCompareSummary {
    commit_oid: String,
    parent_oid: Option<String>,
    compare_ref: String,
    base_ref: String,
    changed_files: u32,
    /// `ready|invalid-commit|error`.
    status: String,
}

#[derive(Debug, Serialize)]
struct CommitCompareResp {
    summary: CommitCompareSummary,
    entries: Vec<BranchChangeEntry>,
}

#[derive(Debug, Deserialize)]
struct CommitCompareQuery {
    commit: String,
}

/// `GET /api/sessions/{id}/git/commit-compare?commit=<oid>` — the diff a single
/// commit introduced (commit vs its first parent), reusing the name-status +
/// numstat parsing. A root commit (no parent) diffs against the empty tree.
async fn commit_compare(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<CommitCompareQuery>,
) -> Result<Json<CommitCompareResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let commit = q.commit.trim().to_string();
    let commit_oid = match run_git(
        &host,
        &cwd,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .await
    {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            return Ok(Json(CommitCompareResp {
                summary: CommitCompareSummary {
                    commit_oid: commit.clone(),
                    parent_oid: None,
                    compare_ref: commit.clone(),
                    base_ref: String::new(),
                    changed_files: 0,
                    status: "invalid-commit".into(),
                },
                entries: Vec::new(),
            }));
        }
    };
    // First parent, if any. Root commits diff against the empty-tree object.
    let parent_oid = run_git(
        &host,
        &cwd,
        &["rev-parse", "--verify", &format!("{commit_oid}^")],
    )
    .await
    .ok()
    .map(|s| s.trim().to_string());
    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let base = parent_oid.clone().unwrap_or_else(|| EMPTY_TREE.to_string());

    let range = format!("{base}..{commit_oid}");
    let name_status = run_git(&host, &cwd, &["diff", "--name-status", "-M", "-z", &range]).await?;
    let mut entries = parse_name_status_z(name_status.as_bytes());
    if let Ok(numstat) = run_git(&host, &cwd, &["diff", "--numstat", "-z", &range]).await {
        for rec in numstat.split('\0').filter(|r| !r.is_empty()) {
            let mut cols = rec.splitn(3, '\t');
            let added = cols.next().and_then(|s| s.parse::<u32>().ok());
            let removed = cols.next().and_then(|s| s.parse::<u32>().ok());
            if let Some(path) = cols.next() {
                if let Some(e) = entries.iter_mut().find(|e| e.path == path) {
                    e.added = added;
                    e.removed = removed;
                }
            }
        }
    }

    Ok(Json(CommitCompareResp {
        summary: CommitCompareSummary {
            changed_files: entries.len() as u32,
            commit_oid: commit_oid.clone(),
            parent_oid,
            compare_ref: commit_oid,
            base_ref: base,
            status: "ready".into(),
        },
        entries,
    }))
}

/// One changed path between two refs. Mirrors the desktop's
/// `GitBranchChangeEntry` (camelCase on the wire).
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct BranchChangeEntry {
    path: String,
    /// `modified|added|deleted|renamed|copied`.
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    added: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removed: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchCompareSummary {
    base_ref: String,
    base_oid: Option<String>,
    compare_ref: String,
    head_oid: Option<String>,
    merge_base: Option<String>,
    changed_files: u32,
    commits_ahead: Option<u32>,
    /// `ready|invalid-base|unborn-head|no-merge-base|error`.
    status: String,
}

#[derive(Debug, Serialize)]
struct BranchCompareResp {
    summary: BranchCompareSummary,
    entries: Vec<BranchChangeEntry>,
}

#[derive(Debug, Deserialize)]
struct BranchCompareQuery {
    base: String,
}

/// Map a `git diff --name-status` letter to the desktop status vocabulary.
fn map_change_status(letter: u8) -> &'static str {
    match letter {
        b'A' => "added",
        b'D' => "deleted",
        b'R' => "renamed",
        b'C' => "copied",
        _ => "modified", // M, T, and anything else read as a content change.
    }
}

/// Parse `git diff --name-status -M -z <range>` (NUL-delimited) into entries.
/// Rename/copy records are `R<score>\0<old>\0<new>`; others are `X\0<path>`.
/// Numstat (added/removed) is merged in separately by the caller.
fn parse_name_status_z(bytes: &[u8]) -> Vec<BranchChangeEntry> {
    let mut out = Vec::new();
    let mut it = bytes.split(|&b| b == 0).filter(|r| !r.is_empty());
    while let Some(code) = it.next() {
        let letter = code[0];
        let status = map_change_status(letter);
        if letter == b'R' || letter == b'C' {
            let old = it.next().map(|b| String::from_utf8_lossy(b).into_owned());
            let Some(new) = it.next() else { break };
            out.push(BranchChangeEntry {
                path: String::from_utf8_lossy(new).into_owned(),
                status: status.to_string(),
                old_path: old,
                added: None,
                removed: None,
            });
        } else {
            let Some(path) = it.next() else { break };
            out.push(BranchChangeEntry {
                path: String::from_utf8_lossy(path).into_owned(),
                status: status.to_string(),
                old_path: None,
                added: None,
                removed: None,
            });
        }
    }
    out
}

/// `GET /api/sessions/{id}/git/branch-compare?base=<ref>` — diff the worktree's
/// HEAD against `base` (3-dot, from the merge-base), with per-file add/remove counts.
async fn branch_compare(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BranchCompareQuery>,
) -> Result<Json<BranchCompareResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let base = q.base.trim().to_string();
    let compare_ref = run_git(&host, &cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "HEAD".into());
    let head_oid = run_git(&host, &cwd, &["rev-parse", "HEAD"])
        .await
        .ok()
        .map(|s| s.trim().to_string());
    let base_oid = run_git(&host, &cwd, &["rev-parse", &base])
        .await
        .ok()
        .map(|s| s.trim().to_string());

    let err = |status: &str| BranchCompareResp {
        summary: BranchCompareSummary {
            base_ref: base.clone(),
            base_oid: base_oid.clone(),
            compare_ref: compare_ref.clone(),
            head_oid: head_oid.clone(),
            merge_base: None,
            changed_files: 0,
            commits_ahead: None,
            status: status.to_string(),
        },
        entries: Vec::new(),
    };
    if base_oid.is_none() {
        return Ok(Json(err("invalid-base")));
    }
    if head_oid.is_none() {
        return Ok(Json(err("unborn-head")));
    }
    let merge_base = match run_git(&host, &cwd, &["merge-base", &base, "HEAD"]).await {
        Ok(s) => s.trim().to_string(),
        Err(_) => return Ok(Json(err("no-merge-base"))),
    };

    let range = format!("{merge_base}..HEAD");
    let name_status = run_git(&host, &cwd, &["diff", "--name-status", "-M", "-z", &range]).await?;
    let mut entries = parse_name_status_z(name_status.as_bytes());

    // Merge numstat (added/removed). `--numstat -z` gives `<add>\t<del>\t<path>\0`
    // (binary files show `-`); index by path so rename targets line up.
    if let Ok(numstat) = run_git(&host, &cwd, &["diff", "--numstat", "-z", &range]).await {
        for rec in numstat.split('\0').filter(|r| !r.is_empty()) {
            let mut cols = rec.splitn(3, '\t');
            let added = cols.next().and_then(|s| s.parse::<u32>().ok());
            let removed = cols.next().and_then(|s| s.parse::<u32>().ok());
            if let Some(path) = cols.next() {
                if let Some(e) = entries.iter_mut().find(|e| e.path == path) {
                    e.added = added;
                    e.removed = removed;
                }
            }
        }
    }

    let commits_ahead = run_git(&host, &cwd, &["rev-list", "--count", &range])
        .await
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());

    Ok(Json(BranchCompareResp {
        summary: BranchCompareSummary {
            base_ref: base,
            base_oid,
            compare_ref,
            head_oid,
            merge_base: Some(merge_base),
            changed_files: entries.len() as u32,
            commits_ahead,
            status: "ready".into(),
        },
        entries,
    }))
}

/// In-progress conflict operation in a worktree.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ConflictOp {
    Merge,
    Rebase,
    CherryPick,
    None,
}

#[derive(Debug, Serialize)]
struct ConflictResp {
    operation: ConflictOp,
}

/// True when `git rev-parse -q --verify <rev>` resolves (the ref/state exists).
async fn git_ref_exists(host: &Host, cwd: &str, rev: &str) -> bool {
    run_git(host, cwd, &["rev-parse", "-q", "--verify", rev])
        .await
        .is_ok()
}

/// True when the git state dir `<sub>` (e.g. `rebase-merge`/`rebase-apply`)
/// exists on `host`. `git rev-parse --git-path` yields a path relative to
/// cwd (or absolute); we resolve it on the host's fs, not the daemon's.
async fn git_state_dir_exists(host: &Host, cwd: &str, sub: &str) -> bool {
    let Ok(p) = run_git(host, cwd, &["rev-parse", "--git-path", sub]).await else {
        return false;
    };
    let p = p.trim();
    let abs = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("{cwd}/{p}")
    };
    host_runtime::path_exists(host, &abs).await.unwrap_or(false)
}

/// `GET /api/sessions/{id}/git/conflict` — which conflict op (if any) is mid-flight.
async fn conflict(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConflictResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    // Rebase leaves a state dir (rebase-merge / rebase-apply) rather than a ref.
    let rebase_dir = git_state_dir_exists(&host, &cwd, "rebase-merge").await
        || git_state_dir_exists(&host, &cwd, "rebase-apply").await;
    let operation = if rebase_dir {
        ConflictOp::Rebase
    } else if git_ref_exists(&host, &cwd, "MERGE_HEAD").await {
        ConflictOp::Merge
    } else if git_ref_exists(&host, &cwd, "CHERRY_PICK_HEAD").await {
        ConflictOp::CherryPick
    } else {
        ConflictOp::None
    };
    Ok(Json(ConflictResp { operation }))
}

#[derive(Debug, Deserialize)]
struct RebaseBody {
    base_ref: String,
}

/// `POST /api/sessions/{id}/git/rebase` — `git rebase <base_ref>`. On conflict
/// git exits non-zero; the error carries git's stderr so the UI can prompt to
/// resolve or abort.
async fn rebase(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RebaseBody>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    if body.base_ref.trim().is_empty() {
        return Err(ApiError::BadRequest("base_ref is empty".into()));
    }
    run_git(&host, &cwd, &["rebase", body.base_ref.trim()]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/abort-merge` — `git merge --abort`.
async fn abort_merge(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["merge", "--abort"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/abort-rebase` — `git rebase --abort`.
async fn abort_rebase(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["rebase", "--abort"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct DiscardBody {
    paths: Vec<String>,
}

/// `POST /api/sessions/{id}/git/discard` — restore the given tracked paths to
/// HEAD (drops staged + worktree changes). Untracked files are left untouched.
async fn discard(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DiscardBody>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    for p in &body.paths {
        ensure_safe_relative(p)?;
    }
    if !body.paths.is_empty() {
        let mut args = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
        args.extend(body.paths.iter().map(String::as_str));
        run_git(&host, &cwd, &args).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct UpstreamStatus {
    /// Upstream ref (e.g. `origin/main`), or null when none is set.
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
}

/// `GET /api/sessions/{id}/git/upstream` — tracking-branch + ahead/behind counts.
async fn upstream(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<UpstreamStatus>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    // Both calls fail harmlessly when there is no upstream, so run them
    // concurrently instead of gating rev-list on rev-parse — over SSH that
    // halves the wall-clock cost of this endpoint.
    let (upstream_out, counts_out) = tokio::join!(
        run_git(
            &host,
            &cwd,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        ),
        // `--left-right --count @{u}...HEAD` prints "<behind>\t<ahead>".
        run_git(
            &host,
            &cwd,
            &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
        )
    );
    let upstream = upstream_out
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let (ahead, behind) = match (&upstream, counts_out) {
        (Some(_), Ok(out)) => {
            let mut it = out.split_whitespace();
            let behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            (ahead, behind)
        }
        _ => (0, 0),
    };
    Ok(Json(UpstreamStatus {
        upstream,
        ahead,
        behind,
    }))
}

/// Run `git <args...>` with `cwd` as the working dir on `host`; return
/// stdout (lossy UTF-8) on success. Host-aware: `git -C` locally,
/// `cd && git` over SSH (see `host_runtime::git_in_dir`).
async fn run_git(host: &Host, cwd: &str, args: &[&str]) -> Result<String, ApiError> {
    let out = git_in_dir(host, cwd, args)
        .await
        .map_err(|e| ApiError::Internal(format!("git {}: {e}", args.join(" "))))?;
    if !out.success {
        return Err(ApiError::Internal(format!(
            "git {} failed: {}",
            args.join(" "),
            out.stderr.trim()
        )));
    }
    Ok(out.stdout_string())
}

#[derive(Debug, Serialize)]
struct BranchesResp {
    /// Current branch, or `None` in detached-HEAD.
    current: Option<String>,
    /// Local branch names (refs/heads), short form.
    branches: Vec<String>,
}

/// `GET /api/sessions/{id}/git/branches` — local branches + the current one.
async fn branches(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<BranchesResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    // Independent reads — run concurrently so a remote host pays one RTT.
    let (current_out, raw) = tokio::join!(
        run_git(&host, &cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
        run_git(
            &host,
            &cwd,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        )
    );
    let current = current_out
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "HEAD");
    let raw = raw?;
    let branches = raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Ok(Json(BranchesResp { current, branches }))
}

#[derive(Debug, Deserialize)]
struct LogQuery {
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
struct LogEntry {
    sha: String,
    subject: String,
    author: String,
    /// Author date, ISO-8601 (`%aI`).
    timestamp: String,
}

/// `GET /api/sessions/{id}/git/log?limit=N` — recent commits (default 50, max 500).
async fn log(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let limit_arg = format!("-{limit}");
    // \x1f (unit separator) won't appear in commit metadata — a safe field delimiter.
    let fmt_arg = "--format=%H%x1f%s%x1f%an%x1f%aI";
    let raw = run_git(&host, &cwd, &["log", &limit_arg, fmt_arg]).await?;
    Ok(Json(parse_log_lines(&raw)))
}

/// Parse `git log --format=%H\x1f%s\x1f%an\x1f%aI` output into entries. Each
/// non-empty line is one commit, fields separated by the unit separator (`\x1f`),
/// which cannot appear in commit metadata. Missing trailing fields default to
/// empty so a malformed line never drops a commit's SHA.
fn parse_log_lines(raw: &str) -> Vec<LogEntry> {
    raw.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            Some(LogEntry {
                sha: parts.next()?.to_string(),
                subject: parts.next().unwrap_or_default().to_string(),
                author: parts.next().unwrap_or_default().to_string(),
                timestamp: parts.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// `POST /api/sessions/{id}/git/fetch` — `git fetch --all --prune`.
async fn fetch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["fetch", "--all", "--prune"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/pull` — fast-forward-only pull.
async fn pull(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["pull", "--ff-only"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/push` — push the current branch, setting upstream
/// on first push so a fresh worktree branch publishes without extra ceremony.
async fn push(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["push", "--set-upstream", "origin", "HEAD"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// One side of a side-by-side diff. The CodeMirror merge view in the
/// dashboard fetches two of these (e.g. index + worktree) and computes the
/// diff client-side, which gives real syntax highlighting per file type —
/// something the old unified-diff text render couldn't do.
const MAX_FILE_BYTES: usize = 512 * 1024;

#[derive(Debug, Deserialize)]
struct FileQuery {
    /// Repo-relative path. Rejected if absolute or contains `..`.
    path: String,
    /// Which revision to read: `head` (`git show HEAD:path`), `index`
    /// (`git show :path`, the staged blob), or `worktree` (the file on
    /// disk). Defaults to `worktree`. A revision where the path doesn't
    /// exist (new/untracked file at HEAD, etc.) returns empty content
    /// rather than an error, so the diff view shows an add/delete cleanly.
    #[serde(default)]
    rev: Option<String>,
}

#[derive(Debug, Serialize)]
struct FileResp {
    content: String,
    /// True when the file exceeded `MAX_FILE_BYTES` and was cut — the UI
    /// shows a notice rather than pretending it has the whole file.
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct StageBody {
    paths: Vec<String>,
    /// `false` → `git add` (stage); `true` → `git restore --staged` (unstage).
    #[serde(default)]
    unstage: bool,
}

/// Result of `GET /api/sessions/{id}/git/status`. Each vec holds repo-
/// relative paths in the order `git status --porcelain` emitted them.
/// Files staged with further unstaged edits appear in BOTH `staged` and
/// `unstaged` — the dashboard treats them as two independent rows so the
/// user can stage/unstage either side.
#[derive(Debug, Default, Serialize)]
pub struct GitStatus {
    pub staged: Vec<String>,
    pub unstaged: Vec<String>,
    pub untracked: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DiffQuery {
    /// Repo-relative path. Rejected if absolute or contains `..`.
    path: String,
    /// `true` → `git diff --cached` (index vs HEAD). Default `false`
    /// returns the unstaged diff (worktree vs index).
    #[serde(default)]
    staged: bool,
}

#[derive(Debug, Deserialize)]
struct CommitBody {
    message: String,
    paths: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CommitResp {
    sha: String,
}

/// Resolve a session's working directory: its `worktree_path` when present,
/// otherwise the raw `workdir` (same precedence as `Session::effective_cwd()`).
/// `pub(crate)` so the forge routes can resolve the same cwd without
/// duplicating the lookup.
pub(crate) async fn cwd_for(state: &AppState, id: Uuid) -> Result<PathBuf, ApiError> {
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(PathBuf::from(session.effective_cwd()))
}

/// Resolve a session to `(host, cwd)`: the host its git ops run on (its
/// `host_id`, or the local host) and its effective working directory. The
/// cwd is a path on `host` — a remote path for a remote session — so all
/// git in this module routes through `host_runtime::git_in_dir`, never a
/// local `git -C` against a path that only exists on the remote.
async fn host_and_cwd_for(state: &AppState, id: Uuid) -> Result<(Host, String), ApiError> {
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let host_id = session.host_id.unwrap_or(LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("session host is missing: {host_id}")))?;
    Ok((host, session.effective_cwd().to_string()))
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// Reject anything that could escape the worktree before it reaches
/// `git`. Absolute paths, empty input, and `..` components are all
/// rejected. Git itself would refuse most of these, but failing here
/// gives a clearer error and a tighter trust boundary.
fn ensure_safe_relative(p: &str) -> Result<(), ApiError> {
    if p.is_empty() {
        return Err(ApiError::BadRequest("path is empty".into()));
    }
    if p.starts_with('/') {
        return Err(ApiError::BadRequest("path must be relative".into()));
    }
    if StdPath::new(p)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(ApiError::BadRequest("path must not contain ..".into()));
    }
    Ok(())
}

async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<GitStatus>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let raw = run_git_bytes(&host, &cwd, &["status", "--porcelain=v1", "-z"]).await?;
    Ok(Json(parse_porcelain_z(&raw)))
}

/// Parse `git status --porcelain=v1 -z` output.
///
/// Records are NUL-terminated `XY SP path` entries. For renames (`R`)
/// and copies (`C`) git follows the main record with a separate NUL-
/// terminated source-path record — we consume and discard it, surfacing
/// only the destination path on the staged side. The classification:
///   * `?? path`      → untracked
///   * `X` ≠ space/?  → staged (path lands in `staged`)
///   * `Y` ≠ space/?  → unstaged (path lands in `unstaged`)
///
/// A path with both X and Y set lands in BOTH lists, which is the
/// "added with further unsaved edits" case.
fn parse_porcelain_z(bytes: &[u8]) -> GitStatus {
    let mut out = GitStatus::default();
    let mut it = bytes
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .peekable();
    while let Some(rec) = it.next() {
        if rec.len() < 3 {
            continue;
        }
        let x = rec[0];
        let y = rec[1];
        let path = String::from_utf8_lossy(&rec[3..]).into_owned();
        if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
            // Source-path follow-up record — discard.
            let _ = it.next();
        }
        if x == b'?' && y == b'?' {
            out.untracked.push(path);
        } else {
            if x != b' ' && x != b'?' {
                out.staged.push(path.clone());
            }
            if y != b' ' && y != b'?' {
                out.unstaged.push(path);
            }
        }
    }
    out
}

/// `GET /api/sessions/{id}/git/diff?path=…&staged=bool`
///
/// Returns the unified diff for a single path as `text/plain`. For
/// untracked files (which `git diff` ignores) we fall back to
/// `git diff --no-index /dev/null <path>` so the dashboard can still
/// render the new content.
async fn diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DiffQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    ensure_safe_relative(&q.path)?;

    let mut diff_args = vec!["diff", "--no-color"];
    if q.staged {
        diff_args.push("--cached");
    }
    diff_args.push("--");
    diff_args.push(&q.path);
    // git_in_dir (not run_git): `git diff` exits non-zero in some states
    // we still want stdout from, and never errors on "no diff".
    let out = git_in_dir(&host, &cwd, &diff_args)
        .await
        .map_err(|e| ApiError::Internal(format!("git diff: {e}")))?;
    let mut body = out.stdout_string();

    // Empty diff + worktree side requested + the file exists on disk →
    // very likely an untracked file. `git diff --no-index /dev/null <path>`
    // synthesises a diff against an empty baseline so the UI shows the
    // new content. `--no-index` exits 1 when a diff exists; we ignore
    // status and just read stdout.
    let worktree_file = format!("{}/{}", cwd.trim_end_matches('/'), q.path);
    if body.is_empty()
        && !q.staged
        && host_runtime::path_exists(&host, &worktree_file)
            .await
            .unwrap_or(false)
    {
        let synth = git_in_dir(
            &host,
            &cwd,
            &[
                "diff",
                "--no-color",
                "--no-index",
                "--",
                "/dev/null",
                &q.path,
            ],
        )
        .await
        .map_err(|e| ApiError::Internal(format!("git diff --no-index: {e}")))?;
        body = synth.stdout_string();
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok((StatusCode::OK, headers, body))
}

/// `GET /api/sessions/{id}/git/file?path=…&rev=head|index|worktree`
///
/// Returns one revision of a file as UTF-8 text (lossy). Used by the
/// dashboard's side-by-side diff: it fetches `index` + `worktree` (unstaged
/// view) or `head` + `index` (staged view) and diffs them client-side. A
/// missing path at the requested revision returns empty content, so adds and
/// deletes render without a special case.
async fn file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> Result<Json<FileResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    ensure_safe_relative(&q.path)?;
    let rev = q.rev.as_deref().unwrap_or("worktree");

    let mut bytes: Vec<u8> = match rev {
        // `git show HEAD:path` / `git show :path`. A non-zero exit means the
        // path doesn't exist at that revision (new file) → empty content.
        "head" | "index" => {
            let spec = if rev == "head" {
                format!("HEAD:{}", q.path)
            } else {
                format!(":{}", q.path)
            };
            // A non-zero exit means the path doesn't exist at that revision
            // (new file) → empty content.
            let out = git_in_dir(&host, &cwd, &["show", &spec])
                .await
                .map_err(|e| ApiError::Internal(format!("git show: {e}")))?;
            if out.success { out.stdout } else { Vec::new() }
        }
        "worktree" => {
            // Read the on-disk file from the session's host; a missing file
            // (deleted in the worktree) is empty content, matching head/index.
            let abs = format!("{}/{}", cwd.trim_end_matches('/'), q.path);
            host_runtime::read_file_bytes(&host, &abs)
                .await
                .map_err(|e| ApiError::Internal(format!("read file: {e}")))?
                .unwrap_or_default()
        }
        other => {
            return Err(ApiError::BadRequest(format!(
                "unknown rev '{other}' (expected head|index|worktree)"
            )));
        }
    };

    let truncated = bytes.len() > MAX_FILE_BYTES;
    if truncated {
        bytes.truncate(MAX_FILE_BYTES);
    }
    Ok(Json(FileResp {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    }))
}

/// `POST /api/sessions/{id}/git/stage` — `{ "paths": [...], "unstage": bool }`
///
/// Stages (`git add`) or unstages (`git restore --staged`) the listed paths.
/// Lets the dashboard move files between the staged/unstaged groups without
/// committing, so the user can curate the index before a commit. Returns the
/// refreshed status so the UI updates in one round trip.
async fn stage(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<StageBody>,
) -> Result<Json<GitStatus>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    if body.paths.is_empty() {
        return Err(ApiError::BadRequest("paths is empty".into()));
    }
    for p in &body.paths {
        ensure_safe_relative(p)?;
    }

    // `restore --staged` resets the index entry to HEAD without touching the
    // worktree — the inverse of `add` for the dashboard's toggle.
    let mut args: Vec<&str> = if body.unstage {
        vec!["restore", "--staged", "--"]
    } else {
        vec!["add", "--"]
    };
    args.extend(body.paths.iter().map(String::as_str));
    let out = git_in_dir(&host, &cwd, &args)
        .await
        .map_err(|e| ApiError::Internal(format!("git stage: {e}")))?;
    if !out.success {
        return Err(ApiError::BadRequest(format!(
            "git {} failed: {}",
            if body.unstage {
                "restore --staged"
            } else {
                "add"
            },
            out.stderr
        )));
    }

    // Re-read status so the caller reflects the new staged/unstaged split.
    let st = run_git_bytes(&host, &cwd, &["status", "--porcelain=v1", "-z"]).await?;
    Ok(Json(parse_porcelain_z(&st)))
}

/// `POST /api/sessions/{id}/git/commit`
///
/// Two-step shell out: `git add -- <paths>` then a `git commit -m <msg>`
/// scoped with `-c user.name=agentum-bot -c user.email=agentum@localhost`
/// so a worktree that inherits no identity from the host gitconfig still
/// commits cleanly. Returns the resulting HEAD sha.
async fn commit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CommitBody>,
) -> Result<Json<CommitResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    if body.message.trim().is_empty() {
        return Err(ApiError::BadRequest("commit message is empty".into()));
    }
    if body.paths.is_empty() {
        return Err(ApiError::BadRequest("paths is empty".into()));
    }
    for p in &body.paths {
        ensure_safe_relative(p)?;
    }

    let mut add_args = vec!["add", "--"];
    add_args.extend(body.paths.iter().map(String::as_str));
    let add_out = git_in_dir(&host, &cwd, &add_args)
        .await
        .map_err(|e| ApiError::Internal(format!("git add: {e}")))?;
    if !add_out.success {
        return Err(ApiError::BadRequest(format!(
            "git add failed: {}",
            add_out.stderr
        )));
    }

    // `git commit -m <msg> -- <paths>` commits a snapshot of ONLY the
    // listed paths, independent of whatever else might be staged in the
    // index. Without the pathspec the dashboard's "select these N files"
    // UX would silently include sibling work the user didn't pick.
    let mut commit_args = vec![
        "-c",
        "user.name=agentum-bot",
        "-c",
        "user.email=agentum@localhost",
        "commit",
        "-m",
        body.message.as_str(),
        "--",
    ];
    commit_args.extend(body.paths.iter().map(String::as_str));
    let commit_out = git_in_dir(&host, &cwd, &commit_args)
        .await
        .map_err(|e| ApiError::Internal(format!("git commit: {e}")))?;
    if !commit_out.success {
        return Err(ApiError::BadRequest(format!(
            "git commit failed: {}",
            commit_out.stderr
        )));
    }

    let sha = run_git(&host, &cwd, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_string();

    Ok(Json(CommitResp { sha }))
}

#[derive(Debug, Deserialize)]
struct CommitStagedBody {
    message: String,
}

/// `POST /api/sessions/{id}/git/commit-staged` — commit whatever is currently
/// staged in the index (no `git add`), matching the desktop's "commit staged
/// changes" action. Uses the same `agentum-bot` author fallback as `/commit`.
async fn commit_staged(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CommitStagedBody>,
) -> Result<Json<CommitResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    if body.message.trim().is_empty() {
        return Err(ApiError::BadRequest("commit message is empty".into()));
    }
    run_git(
        &host,
        &cwd,
        &[
            "-c",
            "user.name=agentum-bot",
            "-c",
            "user.email=agentum@localhost",
            "commit",
            "-m",
            body.message.as_str(),
        ],
    )
    .await?;
    let sha = run_git(&host, &cwd, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_string();
    Ok(Json(CommitResp { sha }))
}

// ───────────────────────── desktop local-path parity ─────────────────────────
// These endpoints fill the gaps the desktop's native git commands covered but
// the session API did not, so the desktop source-control panel can run entirely
// on the embedded server (see `ui/src/runtime/server-git-adapter.ts`). All are
// read-only or single-command writes reusing the `run_git`/`cwd_for` plumbing.

#[derive(Debug, Deserialize)]
struct CheckIgnoreBody {
    paths: Vec<String>,
}

/// `POST /api/sessions/{id}/git/check-ignore` — the subset of `paths` git ignores.
/// `git check-ignore` exits 0 (some ignored), 1 (none), >1 (a real error). The
/// `--` guard keeps a path that starts with `-` from being read as an option.
async fn check_ignore(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CheckIgnoreBody>,
) -> Result<Json<Vec<String>>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    if body.paths.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let mut args = vec!["check-ignore", "--"];
    args.extend(body.paths.iter().map(String::as_str));
    // `git check-ignore` exits 0 (some ignored), 1 (none), >1 (a real error).
    let out = git_in_dir(&host, &cwd, &args)
        .await
        .map_err(|e| ApiError::Internal(format!("git check-ignore: {e}")))?;
    if matches!(out.code, Some(code) if code > 1) {
        return Err(ApiError::Internal(format!(
            "git check-ignore failed: {}",
            out.stderr.trim()
        )));
    }
    Ok(Json(
        out.stdout_string().lines().map(str::to_string).collect(),
    ))
}

/// `POST /api/sessions/{id}/git/fast-forward` — `git merge --ff-only @{upstream}`.
/// Advances the current branch to its tracking branch without a merge commit.
async fn fast_forward(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["merge", "--ff-only", "@{upstream}"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

mod file_links_routes;
use file_links_routes::*;


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_porcelain_z() {
        // `git status --porcelain=v1 -z` for a tree with one unstaged
        // modification, one untracked file, and one fully-staged file.
        let input: &[u8] = b" M file.rs\0?? new.txt\0M  staged.rs\0";
        let s = parse_porcelain_z(input);
        assert_eq!(s.unstaged, vec!["file.rs"]);
        assert_eq!(s.untracked, vec!["new.txt"]);
        assert_eq!(s.staged, vec!["staged.rs"]);
    }

    #[test]
    fn paths_with_partial_index_and_worktree_changes_land_in_both() {
        // `MM` = staged change + further unstaged edits to the same file.
        let input: &[u8] = b"MM both.rs\0";
        let s = parse_porcelain_z(input);
        assert_eq!(s.staged, vec!["both.rs"]);
        assert_eq!(s.unstaged, vec!["both.rs"]);
    }

    #[test]
    fn rename_consumes_source_record() {
        // `R  new.rs\0old.rs\0` — the source-path follow-up record must
        // not leak into the staged list as a phantom entry.
        let input: &[u8] = b"R  new.rs\0old.rs\0M  other.rs\0";
        let s = parse_porcelain_z(input);
        assert_eq!(s.staged, vec!["new.rs", "other.rs"]);
        assert!(s.unstaged.is_empty());
    }

    #[test]
    fn ensure_safe_relative_blocks_traversal() {
        assert!(ensure_safe_relative("../etc/passwd").is_err());
        assert!(ensure_safe_relative("a/../b").is_err());
        assert!(ensure_safe_relative("/etc/passwd").is_err());
        assert!(ensure_safe_relative("").is_err());
        assert!(ensure_safe_relative("src/lib.rs").is_ok());
        assert!(ensure_safe_relative("a/b/c.txt").is_ok());
    }

    #[test]
    fn parse_log_lines_splits_unit_separated_fields() {
        let raw = "abc123\u{1f}fix: thing\u{1f}Jane\u{1f}2026-06-02T10:00:00+00:00\n\
                   def456\u{1f}feat: other\u{1f}Bob\u{1f}2026-06-01T09:00:00+00:00\n";
        let entries = parse_log_lines(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].sha, "abc123");
        assert_eq!(entries[0].subject, "fix: thing");
        assert_eq!(entries[0].author, "Jane");
        assert_eq!(entries[1].sha, "def456");
        assert_eq!(entries[1].timestamp, "2026-06-01T09:00:00+00:00");
    }

    #[test]
    fn parse_log_lines_tolerates_missing_trailing_fields() {
        // A subject containing nothing else still yields the SHA, never drops it.
        let entries = parse_log_lines("sha-only\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sha, "sha-only");
        assert_eq!(entries[0].subject, "");
        assert!(parse_log_lines("").is_empty());
    }

    #[test]
    fn parse_name_status_handles_renames_and_plain_changes() {
        // `M\0a.rs\0` (modify), `A\0b.rs\0` (add), `R100\0old.rs\0new.rs\0` (rename).
        let input = b"M\0a.rs\0A\0b.rs\0R100\0old.rs\0new.rs\0";
        let entries = parse_name_status_z(input);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].status, "modified");
        assert_eq!(entries[0].path, "a.rs");
        assert_eq!(entries[1].status, "added");
        assert_eq!(entries[2].status, "renamed");
        assert_eq!(entries[2].path, "new.rs");
        assert_eq!(entries[2].old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn map_change_status_covers_git_letters() {
        assert_eq!(map_change_status(b'A'), "added");
        assert_eq!(map_change_status(b'D'), "deleted");
        assert_eq!(map_change_status(b'R'), "renamed");
        assert_eq!(map_change_status(b'C'), "copied");
        assert_eq!(map_change_status(b'M'), "modified");
        assert_eq!(map_change_status(b'T'), "modified");
    }

    #[test]
    fn parse_status_entries_splits_areas_and_untracked() {
        // ` M file.rs` → unstaged modify; `?? new.txt` → untracked;
        // `M  staged.rs` → staged modify; `MM both.rs` → staged + unstaged.
        let input: &[u8] = b" M file.rs\0?? new.txt\0M  staged.rs\0MM both.rs\0";
        let e = parse_status_entries(input);
        assert_eq!(e.len(), 5);
        assert!(e.contains(&StatusEntry {
            path: "file.rs".into(),
            status: "modified".into(),
            area: "unstaged".into(),
            old_path: None
        }));
        assert!(e.contains(&StatusEntry {
            path: "new.txt".into(),
            status: "untracked".into(),
            area: "untracked".into(),
            old_path: None
        }));
        // `both.rs` appears in both staged and unstaged.
        assert_eq!(e.iter().filter(|x| x.path == "both.rs").count(), 2);
    }

    #[test]
    fn parse_status_entries_carries_rename_old_path() {
        // `R  old.rs\0new.rs` — staged rename; old path is the source record.
        let input: &[u8] = b"R  new.rs\0old.rs\0";
        let e = parse_status_entries(input);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].status, "renamed");
        assert_eq!(e[0].area, "staged");
        assert_eq!(e[0].path, "new.rs");
        assert_eq!(e[0].old_path.as_deref(), Some("old.rs"));
    }

}
