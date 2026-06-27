//! Mutating git routes: stage/unstage, commit, commit-staged, check-ignore,
//! and fast-forward, plus their request/response DTOs.
use super::*;

#[derive(Debug, Deserialize)]
pub(crate) struct StageBody {
    paths: Vec<String>,
    /// `false` → `git add` (stage); `true` → `git restore --staged` (unstage).
    #[serde(default)]
    unstage: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitBody {
    message: String,
    paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommitResp {
    sha: String,
}

/// `POST /api/sessions/{id}/git/stage` — `{ "paths": [...], "unstage": bool }`
///
/// Stages (`git add`) or unstages (`git restore --staged`) the listed paths.
/// Lets the dashboard move files between the staged/unstaged groups without
/// committing, so the user can curate the index before a commit. Returns the
/// refreshed status so the UI updates in one round trip.
pub(crate) async fn stage(
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
pub(crate) async fn commit(
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
pub(crate) struct CommitStagedBody {
    message: String,
}

/// `POST /api/sessions/{id}/git/commit-staged` — commit whatever is currently
/// staged in the index (no `git add`), matching the desktop's "commit staged
/// changes" action. Uses the same `agentum-bot` author fallback as `/commit`.
pub(crate) async fn commit_staged(
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
pub(crate) struct CheckIgnoreBody {
    paths: Vec<String>,
}

/// `POST /api/sessions/{id}/git/check-ignore` — the subset of `paths` git ignores.
/// `git check-ignore` exits 0 (some ignored), 1 (none), >1 (a real error). The
/// `--` guard keeps a path that starts with `-` from being read as an option.
pub(crate) async fn check_ignore(
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
pub(crate) async fn fast_forward(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["merge", "--ff-only", "@{upstream}"]).await?;
    Ok(StatusCode::NO_CONTENT)
}
