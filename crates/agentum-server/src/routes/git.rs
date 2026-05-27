//! `/api/sessions/{id}/git/*` — minimal git status / diff / commit surface
//! for a session's worktree. ORCA §3 Feature #3 (P1).
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
//!   * `POST /api/sessions/{id}/git/commit`              → `{"sha": "…"}`
//!
//! Commits are authored as `agentum-bot <agentum@localhost>` via
//! `git -c user.name=… -c user.email=…` so a worktree that inherits a
//! missing/incomplete identity from `~/.gitconfig` still commits cleanly.

use std::path::{Component, Path as StdPath, PathBuf};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/{id}/git/status", get(status))
        .route("/api/sessions/{id}/git/diff", get(diff))
        .route("/api/sessions/{id}/git/commit", post(commit))
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

async fn cwd_for(state: &AppState, id: Uuid) -> Result<PathBuf, ApiError> {
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(PathBuf::from(session.effective_cwd()))
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
    let cwd = cwd_for(&state, id).await?;
    if !crate::git::is_git_repo(&cwd).await {
        return Err(ApiError::BadRequest(format!(
            "not a git repository: {}",
            cwd.display()
        )));
    }

    let out = Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .args(["status", "--porcelain=v1", "-z"])
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git status: {}", e)))?;
    if !out.status.success() {
        return Err(ApiError::Internal(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(Json(parse_porcelain_z(&out.stdout)))
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
    let cwd = cwd_for(&state, id).await?;
    ensure_safe_relative(&q.path)?;

    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&cwd).args(["diff", "--no-color"]);
    if q.staged {
        cmd.arg("--cached");
    }
    cmd.arg("--").arg(&q.path);
    let out = cmd
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git diff: {}", e)))?;

    let mut body = String::from_utf8_lossy(&out.stdout).into_owned();

    // Empty diff + worktree side requested + the file exists on disk →
    // very likely an untracked file. `git diff --no-index /dev/null <path>`
    // synthesises a diff against an empty baseline so the UI shows the
    // new content. `--no-index` exits 1 when a diff exists; we ignore
    // status and just read stdout.
    if body.is_empty() && !q.staged && cwd.join(&q.path).exists() {
        let synth = Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .args(["diff", "--no-color", "--no-index", "--", "/dev/null"])
            .arg(&q.path)
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("git diff --no-index: {}", e)))?;
        body = String::from_utf8_lossy(&synth.stdout).into_owned();
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    Ok((StatusCode::OK, headers, body))
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
    let cwd = cwd_for(&state, id).await?;
    if body.message.trim().is_empty() {
        return Err(ApiError::BadRequest("commit message is empty".into()));
    }
    if body.paths.is_empty() {
        return Err(ApiError::BadRequest("paths is empty".into()));
    }
    for p in &body.paths {
        ensure_safe_relative(p)?;
    }

    let mut add = Command::new("git");
    add.arg("-C").arg(&cwd).args(["add", "--"]);
    for p in &body.paths {
        add.arg(p);
    }
    let add_out = add
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git add: {}", e)))?;
    if !add_out.status.success() {
        return Err(ApiError::BadRequest(format!(
            "git add failed: {}",
            String::from_utf8_lossy(&add_out.stderr)
        )));
    }

    let commit_out = Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .args([
            "-c",
            "user.name=agentum-bot",
            "-c",
            "user.email=agentum@localhost",
            "commit",
            "-m",
        ])
        .arg(&body.message)
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git commit: {}", e)))?;
    if !commit_out.status.success() {
        return Err(ApiError::BadRequest(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit_out.stderr)
        )));
    }

    let sha_out = Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .args(["rev-parse", "HEAD"])
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git rev-parse: {}", e)))?;
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

    Ok(Json(CommitResp { sha }))
}

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
}
