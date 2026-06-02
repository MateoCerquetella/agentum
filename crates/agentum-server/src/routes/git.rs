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
        .route("/api/sessions/{id}/git/file", get(file))
        .route("/api/sessions/{id}/git/stage", post(stage))
        .route("/api/sessions/{id}/git/commit", post(commit))
        .route("/api/sessions/{id}/git/branches", get(branches))
        .route("/api/sessions/{id}/git/log", get(log))
        .route("/api/sessions/{id}/git/fetch", post(fetch))
        .route("/api/sessions/{id}/git/pull", post(pull))
        .route("/api/sessions/{id}/git/push", post(push))
}

/// Run `git -C <cwd> <args...>`; return stdout (lossy UTF-8) on success.
async fn run_git(cwd: &StdPath, args: &[&str]) -> Result<String, ApiError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git {}: {e}", args.join(" "))))?;
    if !out.status.success() {
        return Err(ApiError::Internal(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
    let cwd = cwd_for(&state, id).await?;
    let current = run_git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "HEAD");
    let raw = run_git(&cwd, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]).await?;
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
    let cwd = cwd_for(&state, id).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let limit_arg = format!("-{limit}");
    // \x1f (unit separator) won't appear in commit metadata — a safe field delimiter.
    let fmt_arg = "--format=%H%x1f%s%x1f%an%x1f%aI";
    let raw = run_git(&cwd, &["log", &limit_arg, fmt_arg]).await?;
    let entries = raw
        .lines()
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
        .collect();
    Ok(Json(entries))
}

/// `POST /api/sessions/{id}/git/fetch` — `git fetch --all --prune`.
async fn fetch(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let cwd = cwd_for(&state, id).await?;
    run_git(&cwd, &["fetch", "--all", "--prune"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/pull` — fast-forward-only pull.
async fn pull(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let cwd = cwd_for(&state, id).await?;
    run_git(&cwd, &["pull", "--ff-only"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/push` — push the current branch, setting upstream
/// on first push so a fresh worktree branch publishes without extra ceremony.
async fn push(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let cwd = cwd_for(&state, id).await?;
    run_git(&cwd, &["push", "--set-upstream", "origin", "HEAD"]).await?;
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
    let cwd = cwd_for(&state, id).await?;
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
            let out = Command::new("git")
                .arg("-C")
                .arg(&cwd)
                .args(["show", &spec])
                .output()
                .await
                .map_err(|e| ApiError::Internal(format!("git show: {}", e)))?;
            if out.status.success() {
                out.stdout
            } else {
                Vec::new()
            }
        }
        "worktree" => {
            // Read straight off disk; a missing file (deleted in the worktree)
            // is empty content, matching the head/index behavior above.
            match tokio::fs::read(cwd.join(&q.path)).await {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(ApiError::Internal(format!("read file: {}", e))),
            }
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
    let cwd = cwd_for(&state, id).await?;
    if body.paths.is_empty() {
        return Err(ApiError::BadRequest("paths is empty".into()));
    }
    for p in &body.paths {
        ensure_safe_relative(p)?;
    }

    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&cwd);
    if body.unstage {
        // `restore --staged` resets the index entry to HEAD without touching
        // the worktree — the inverse of `add` for the dashboard's toggle.
        cmd.args(["restore", "--staged", "--"]);
    } else {
        cmd.args(["add", "--"]);
    }
    for p in &body.paths {
        cmd.arg(p);
    }
    let out = cmd
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git stage: {}", e)))?;
    if !out.status.success() {
        return Err(ApiError::BadRequest(format!(
            "git {} failed: {}",
            if body.unstage {
                "restore --staged"
            } else {
                "add"
            },
            String::from_utf8_lossy(&out.stderr)
        )));
    }

    // Re-read status so the caller reflects the new staged/unstaged split.
    let st = Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .args(["status", "--porcelain=v1", "-z"])
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("git status: {}", e)))?;
    Ok(Json(parse_porcelain_z(&st.stdout)))
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

    // `git commit -m <msg> -- <paths>` commits a snapshot of ONLY the
    // listed paths, independent of whatever else might be staged in the
    // index. Without the pathspec the dashboard's "select these N files"
    // UX would silently include sibling work the user didn't pick.
    let mut commit = Command::new("git");
    commit
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
        .arg("--");
    for p in &body.paths {
        commit.arg(p);
    }
    let commit_out = commit
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
