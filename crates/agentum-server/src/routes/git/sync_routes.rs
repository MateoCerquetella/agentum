//! Branch/remote sync routes: list branches, commit log, and fetch/pull/push.
use super::*;

#[derive(Debug, Serialize)]
pub(crate) struct BranchesResp {
    /// Current branch, or `None` in detached-HEAD.
    current: Option<String>,
    /// Local branch names (refs/heads), short form.
    branches: Vec<String>,
}

/// `GET /api/sessions/{id}/git/branches` — local branches + the current one.
pub(crate) async fn branches(
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
pub(crate) struct LogQuery {
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub(crate) struct LogEntry {
    sha: String,
    subject: String,
    author: String,
    /// Author date, ISO-8601 (`%aI`).
    timestamp: String,
}

/// `GET /api/sessions/{id}/git/log?limit=N` — recent commits (default 50, max 500).
pub(crate) async fn log(
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
pub(crate) async fn fetch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["fetch", "--all", "--prune"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/sessions/{id}/git/pull` — fast-forward-only pull.
pub(crate) async fn pull(
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
pub(crate) async fn push(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    run_git(&host, &cwd, &["push", "--set-upstream", "origin", "HEAD"]).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
