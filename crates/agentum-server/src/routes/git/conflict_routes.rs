//! Worktree conflict/merge state routes: detect conflicts, rebase, abort
//! merge/rebase, discard changes, and report upstream divergence.
use super::*;

/// In-progress conflict operation in a worktree.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConflictOp {
    Merge,
    Rebase,
    CherryPick,
    None,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConflictResp {
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
pub(crate) async fn conflict(
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
pub(crate) struct RebaseBody {
    base_ref: String,
}

/// `POST /api/sessions/{id}/git/rebase` — `git rebase <base_ref>`. On conflict
/// git exits non-zero; the error carries git's stderr so the UI can prompt to
/// resolve or abort.
pub(crate) async fn rebase(
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
pub(crate) async fn abort_merge(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["merge", "--abort"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/abort-rebase` — `git rebase --abort`.
pub(crate) async fn abort_rebase(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["rebase", "--abort"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscardBody {
    paths: Vec<String>,
}

/// Split `paths` into `(tracked, untracked)`. "Untracked" mirrors exactly what
/// `git status` reports as `??` — files git neither tracks nor ignores — listed
/// here with `ls-files --others --exclude-standard`. A requested path counts as
/// untracked when git lists it (an untracked file) or lists a child under it (an
/// untracked directory such as `.playwright-mcp/`, whose entries appear as
/// `.playwright-mcp/<file>`). Everything else — modified, staged, even a staged
/// deletion — defaults to tracked so `git restore` can undo it.
async fn partition_discard_paths(
    host: &Host,
    cwd: &str,
    paths: &[String],
) -> Result<(Vec<String>, Vec<String>), ApiError> {
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
    args.extend(paths.iter().map(String::as_str));
    let listed = run_git(host, cwd, &args).await?;
    // `-z` NUL-separates entries so paths with spaces/newlines stay intact.
    let others: Vec<&str> = listed.split('\0').filter(|s| !s.is_empty()).collect();

    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for p in paths {
        let trimmed = p.trim_end_matches('/');
        let dir_prefix = format!("{trimmed}/");
        let is_untracked = others
            .iter()
            .any(|o| *o == trimmed || o.starts_with(dir_prefix.as_str()));
        if is_untracked {
            untracked.push(p.clone());
        } else {
            tracked.push(p.clone());
        }
    }
    Ok((tracked, untracked))
}

/// `POST /api/sessions/{id}/git/discard` — drop changes to the given paths.
/// Tracked paths are restored to HEAD (staged + worktree). Untracked paths are
/// deleted from the worktree (`git clean -fd`) — the UI's "Discard" on an
/// untracked row means "remove this new file/dir", and `git restore` errors on
/// a pathspec git doesn't know (`did not match any file(s) known to git`),
/// aborting the whole batch. So a single untracked path (e.g. `.playwright-mcp/`)
/// must be partitioned out, or it blocks discarding the tracked paths too.
pub(crate) async fn discard(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DiscardBody>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    for p in &body.paths {
        ensure_safe_relative(p)?;
    }
    if body.paths.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }

    let (tracked, untracked) = partition_discard_paths(&host, &cwd, &body.paths).await?;

    if !tracked.is_empty() {
        let mut args = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
        args.extend(tracked.iter().map(String::as_str));
        run_git(&host, &cwd, &args).await?;
    }
    if !untracked.is_empty() {
        // `-f` forces the removal (git refuses without it); `-d` recurses into
        // untracked directories. No `-x`: we only delete what git already
        // reports as untracked, never gitignored files the user didn't select.
        let mut args = vec!["clean", "-f", "-d", "--"];
        args.extend(untracked.iter().map(String::as_str));
        run_git(&host, &cwd, &args).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub(crate) struct UpstreamStatus {
    /// Upstream ref (e.g. `origin/main`), or null when none is set.
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
}

/// `GET /api/sessions/{id}/git/upstream` — tracking-branch + ahead/behind counts.
pub(crate) async fn upstream(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_runtime::git_in_dir;
    use agentum_core::{HostKind, LOCAL_HOST_ID};

    fn local_host() -> Host {
        Host {
            id: LOCAL_HOST_ID,
            name: "local".into(),
            kind: HostKind::Local,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    async fn git(host: &Host, cwd: &str, args: &[&str]) {
        let out = git_in_dir(host, cwd, args).await.unwrap();
        assert!(out.success, "git {args:?} failed: {}", out.stderr);
    }

    /// Repo with one committed-then-modified tracked file, one untracked loose
    /// file, and one untracked directory holding a file — the shape that
    /// surfaced the `.playwright-mcp/` discard 500.
    async fn seed_repo(host: &Host, cwd: &str) {
        git(host, cwd, &["init", "-q"]).await;
        git(host, cwd, &["config", "user.email", "t@example.com"]).await;
        git(host, cwd, &["config", "user.name", "t"]).await;
        // Windows CI defaults to core.autocrlf=true, which checks tracked files
        // back out with CRLF — but the exact-string assertions below expect the
        // LF bytes we write here. Pin the temp repo to LF so these git tests are
        // deterministic across platforms.
        git(host, cwd, &["config", "core.autocrlf", "false"]).await;
        std::fs::write(format!("{cwd}/tracked.txt"), "v1\n").unwrap();
        git(host, cwd, &["add", "tracked.txt"]).await;
        git(host, cwd, &["commit", "-q", "-m", "init"]).await;
        // Dirty the tracked file, then add untracked things.
        std::fs::write(format!("{cwd}/tracked.txt"), "v2\n").unwrap();
        std::fs::write(format!("{cwd}/loose.txt"), "new\n").unwrap();
        std::fs::create_dir(format!("{cwd}/.playwright-mcp")).unwrap();
        std::fs::write(format!("{cwd}/.playwright-mcp/shot.png"), "x").unwrap();
    }

    #[tokio::test]
    async fn partition_splits_tracked_from_untracked_file_and_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let host = local_host();
        seed_repo(&host, &cwd).await;

        let paths = vec![
            "tracked.txt".to_string(),
            "loose.txt".to_string(),
            ".playwright-mcp/".to_string(),
        ];
        let (tracked, untracked) = partition_discard_paths(&host, &cwd, &paths).await.unwrap();
        assert_eq!(tracked, vec!["tracked.txt".to_string()]);
        assert_eq!(
            untracked,
            vec!["loose.txt".to_string(), ".playwright-mcp/".to_string()]
        );
    }

    #[tokio::test]
    async fn staged_deletion_classifies_as_tracked_not_untracked() {
        // A staged `git rm` removes the path from the index, so it's neither
        // tracked-in-index nor "other" — it must still default to tracked so
        // `git restore` can bring it back, not get swept by `git clean`.
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let host = local_host();
        seed_repo(&host, &cwd).await;
        // `-f` because seed_repo leaves a local modification; the result is a
        // staged deletion either way (index entry removed).
        git(&host, &cwd, &["rm", "-q", "-f", "tracked.txt"]).await;

        let paths = vec!["tracked.txt".to_string()];
        let (tracked, untracked) = partition_discard_paths(&host, &cwd, &paths).await.unwrap();
        assert_eq!(tracked, vec!["tracked.txt".to_string()]);
        assert!(untracked.is_empty());
    }

    /// Mirror the route's restore-tracked + clean-untracked body and assert the
    /// worktree ends in the state the user expects: tracked file reverted,
    /// untracked file and directory deleted — no 500 on the untracked dir.
    #[tokio::test]
    async fn discard_reverts_tracked_and_removes_untracked() {
        let dir = tempfile::TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let host = local_host();
        seed_repo(&host, &cwd).await;

        let paths = vec![
            "tracked.txt".to_string(),
            "loose.txt".to_string(),
            ".playwright-mcp/".to_string(),
        ];
        let (tracked, untracked) = partition_discard_paths(&host, &cwd, &paths).await.unwrap();

        let mut restore = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
        restore.extend(tracked.iter().map(String::as_str));
        git(&host, &cwd, &restore).await;

        let mut clean = vec!["clean", "-f", "-d", "--"];
        clean.extend(untracked.iter().map(String::as_str));
        git(&host, &cwd, &clean).await;

        assert_eq!(
            std::fs::read_to_string(format!("{cwd}/tracked.txt")).unwrap(),
            "v1\n",
            "tracked file should be reverted to HEAD"
        );
        assert!(
            !std::path::Path::new(&format!("{cwd}/loose.txt")).exists(),
            "untracked loose file should be removed"
        );
        assert!(
            !std::path::Path::new(&format!("{cwd}/.playwright-mcp")).exists(),
            "untracked directory should be removed"
        );
    }
}
