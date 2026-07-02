//! `GET .../git/remote-file-url` (build external web links to a file) and
//! `GET .../git/blob` (fetch staged/worktree blob content) + their helpers.
use super::*;

#[derive(Debug, Deserialize)]
pub(crate) struct RemoteFileUrlQuery {
    /// Repo-relative path.
    path: String,
    line: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct RemoteFileUrlResp {
    /// Web URL for the file/line on origin's host, or null when there's no
    /// `origin` remote or its URL couldn't be parsed.
    url: Option<String>,
}

/// Convert a git remote URL (scp-like, `ssh://`, or `http(s)://`) to
/// `(web_base, host)`. Mirrors the desktop's `git_url_to_web_base`.
fn git_url_to_web_base(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    if let Some(rest) = url.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            return Some((format!("https://{host}/{path}"), host.to_string()));
        }
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        if let Some((host, path)) = rest.split_once('/') {
            return Some((format!("https://{host}/{path}"), host.to_string()));
        }
    }
    for prefix in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
            if let Some((host, path)) = rest.split_once('/') {
                return Some((format!("https://{host}/{path}"), host.to_string()));
            }
        }
    }
    None
}

/// Build a host-specific blob URL. Mirrors the desktop's `build_file_url`.
fn build_file_url(web_base: &str, host: &str, reference: &str, path: &str, line: i64) -> String {
    let host = host.to_lowercase();
    if host.contains("gitlab") {
        format!("{web_base}/-/blob/{reference}/{path}#L{line}")
    } else if host.contains("bitbucket") {
        format!("{web_base}/src/{reference}/{path}#lines-{line}")
    } else {
        format!("{web_base}/blob/{reference}/{path}#L{line}")
    }
}

/// `GET /api/sessions/{id}/git/remote-file-url?path=…&line=N` — a web URL to the
/// given file/line on origin's host (GitHub/GitLab/Bitbucket URL shapes).
pub(crate) async fn remote_file_url(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RemoteFileUrlQuery>,
) -> Result<Json<RemoteFileUrlResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    let remote = match run_git(&host, &cwd, &["remote", "get-url", "origin"]).await {
        Ok(s) => s.trim().to_string(),
        Err(_) => return Ok(Json(RemoteFileUrlResp { url: None })),
    };
    let Some((web_base, web_host)) = git_url_to_web_base(&remote) else {
        return Ok(Json(RemoteFileUrlResp { url: None }));
    };
    // Prefer the branch name; fall back to the commit oid for detached HEAD.
    let branch = run_git(&host, &cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()
        .map(|s| s.trim().to_string());
    let reference = match branch {
        Some(name) if !name.is_empty() && name != "HEAD" => name,
        _ => run_git(&host, &cwd, &["rev-parse", "HEAD"])
            .await
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "HEAD".to_string()),
    };
    Ok(Json(RemoteFileUrlResp {
        url: Some(build_file_url(
            &web_base, &web_host, &reference, &q.path, q.line,
        )),
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct BlobQuery {
    /// Repo-relative path. Rejected if absolute or contains `..`.
    path: String,
    /// Revision to read at (commit oid or ref); `git show <commit>:<path>`.
    commit: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlobResp {
    /// Base64 of the blob's bytes (binary-safe). Empty when the path is absent
    /// at that revision (e.g. a file added after `commit`) — so adds/deletes
    /// render cleanly as one empty side of the pair.
    content: String,
    /// True when the bytes contain a NUL; the desktop then renders the diff as a
    /// binary preview (image/PDF) from the base64 rather than as text.
    is_binary: bool,
    truncated: bool,
}

/// `GET /api/sessions/{id}/git/blob?path=…&commit=<rev>` — one file's bytes at an
/// arbitrary revision, base64-encoded. Powers the desktop's commit/branch diff
/// (content-pair) views, which fetch two blobs and diff them client-side.
pub(crate) async fn blob(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BlobQuery>,
) -> Result<Json<BlobResp>, ApiError> {
    let id = parse_uuid(&id)?;
    let (host, cwd) = host_and_cwd_for(&state, id).await?;
    ensure_safe_relative(&q.path)?;
    // A commit/ref never starts with '-'; reject so the rev can't smuggle a
    // `git show` option.
    if q.commit.starts_with('-') {
        return Err(ApiError::BadRequest("invalid commit ref".into()));
    }
    let spec = format!("{}:{}", q.commit, q.path);
    let out = git_in_dir(&host, &cwd, &["show", &spec])
        .await
        .map_err(|e| ApiError::Internal(format!("git show: {e}")))?;
    // A non-zero exit means the path doesn't exist at that revision → empty.
    let mut bytes = if out.success { out.stdout } else { Vec::new() };
    let is_binary = bytes.contains(&0);
    let truncated = bytes.len() > MAX_FILE_BYTES;
    if truncated {
        bytes.truncate(MAX_FILE_BYTES);
    }
    let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(Json(BlobResp {
        content,
        is_binary,
        truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_url_to_web_base_handles_scp_ssh_and_https() {
        // scp-like (git@host:path).
        assert_eq!(
            git_url_to_web_base("git@github.com:owner/repo.git"),
            Some(("https://github.com/owner/repo".into(), "github.com".into()))
        );
        // ssh:// with user.
        assert_eq!(
            git_url_to_web_base("ssh://git@gitlab.com/group/repo.git"),
            Some(("https://gitlab.com/group/repo".into(), "gitlab.com".into()))
        );
        // https with embedded token (stripped).
        assert_eq!(
            git_url_to_web_base("https://x-token:abc@github.com/o/r"),
            Some(("https://github.com/o/r".into(), "github.com".into()))
        );
        assert_eq!(git_url_to_web_base("not a url"), None);
    }

    #[test]
    fn build_file_url_is_host_specific() {
        assert_eq!(
            build_file_url(
                "https://github.com/o/r",
                "github.com",
                "main",
                "src/a.rs",
                12
            ),
            "https://github.com/o/r/blob/main/src/a.rs#L12"
        );
        assert_eq!(
            build_file_url(
                "https://gitlab.com/g/r",
                "gitlab.com",
                "main",
                "src/a.rs",
                12
            ),
            "https://gitlab.com/g/r/-/blob/main/src/a.rs#L12"
        );
        assert_eq!(
            build_file_url(
                "https://bitbucket.org/o/r",
                "bitbucket.org",
                "main",
                "a.rs",
                5
            ),
            "https://bitbucket.org/o/r/src/main/a.rs#lines-5"
        );
    }
}
