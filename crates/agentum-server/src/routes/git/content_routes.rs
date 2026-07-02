//! Read-only file-content routes: side-by-side `diff` of a path and `file`
//! content at a ref, with their query/response DTOs.
use super::*;

#[derive(Debug, Deserialize)]
pub(crate) struct FileQuery {
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
pub(crate) struct FileResp {
    content: String,
    /// True when the file exceeded `MAX_FILE_BYTES` and was cut — the UI
    /// shows a notice rather than pretending it has the whole file.
    truncated: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiffQuery {
    /// Repo-relative path. Rejected if absolute or contains `..`.
    path: String,
    /// `true` → `git diff --cached` (index vs HEAD). Default `false`
    /// returns the unstaged diff (worktree vs index).
    #[serde(default)]
    staged: bool,
}

/// `GET /api/sessions/{id}/git/diff?path=…&staged=bool`
///
/// Returns the unified diff for a single path as `text/plain`. For
/// untracked files (which `git diff` ignores) we fall back to
/// `git diff --no-index /dev/null <path>` so the dashboard can still
/// render the new content.
pub(crate) async fn diff(
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
pub(crate) async fn file(
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
