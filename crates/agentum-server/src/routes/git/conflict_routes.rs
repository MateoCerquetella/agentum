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

/// `POST /api/sessions/{id}/git/discard` — restore the given tracked paths to
/// HEAD (drops staged + worktree changes). Untracked files are left untouched.
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
    if !body.paths.is_empty() {
        let mut args = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
        args.extend(body.paths.iter().map(String::as_str));
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
