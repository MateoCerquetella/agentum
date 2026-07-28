//! `/api/sessions/{id}/forge/*` + `/api/forge/token` — in-app GitHub/GitLab
//! integration for PRs, issues, and checks.
//!
//! Per session we detect the forge from the repo's `origin` remote, then call
//! the forge REST API directly with `reqwest` (already a server dep). The cwd
//! is resolved via [`crate::routes::git::cwd_for`] — the session's worktree
//! when present, else its workdir — so a worktree-isolated session reports the
//! PRs/issues/checks for its own branch.
//!
//! Endpoints (all auth-gated by the standard bearer middleware):
//!   * `GET  /api/sessions/{id}/forge/info`               → detected forge + repo + branch
//!   * `GET  /api/sessions/{id}/forge/prs`                → open PRs/MRs (normalized)
//!   * `GET  /api/sessions/{id}/forge/issues`             → open issues (normalized)
//!   * `GET  /api/sessions/{id}/forge/checks?ref=<ref>`   → CI checks/pipelines for a ref
//!   * `POST /api/sessions/{id}/forge/pr`                 → open a PR/MR from the current branch
//!   * `GET  /api/forge/token?forge=github`               → `{ "has_token": bool }` (never echoes the token)
//!   * `PUT  /api/forge/token`                            → `{ "forge": "...", "token": "..." }`
//!
//! Token storage: `<data_dir>/forge.json` written 0600 (a global, single-user,
//! local-only secret — mirrors `preferences.rs`'s file-based config rather than
//! a SQLite column, since unlike the per-host SSH secret it isn't tied to a
//! table row). The token never leaves the daemon: clients only ever learn
//! whether one is set.

use std::collections::BTreeMap;
use std::path::{Path as StdPath, PathBuf};

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::routes::git::cwd_for;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions/{id}/forge/info", get(info))
        .route("/api/sessions/{id}/forge/prs", get(prs))
        .route("/api/sessions/{id}/forge/issues", get(issues))
        .route("/api/sessions/{id}/forge/checks", get(checks))
        .route("/api/sessions/{id}/forge/pr", post(create_pr))
        .route("/api/forge/token", get(get_token).put(put_token))
}

// ── Forge detection ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ForgeKind {
    Github,
    Gitlab,
}

impl ForgeKind {
    fn as_str(self) -> &'static str {
        match self {
            ForgeKind::Github => "github",
            ForgeKind::Gitlab => "gitlab",
        }
    }
}

/// A parsed public-cloud `origin` remote. Forge credentials are currently
/// global per provider, so only the exact public origins they are issued for
/// are accepted. Self-hosted support requires origin-bound credentials and is
/// deliberately fail-closed until that storage model exists.
#[derive(Debug, Clone)]
pub(crate) struct Remote {
    kind: ForgeKind,
    /// `owner/repo` (GitHub) or full project path incl. nested groups (GitLab).
    pub(crate) project: String,
}

impl Remote {
    /// True when this remote points at GitHub. The `kind`
    /// field is private; Chat's slug resolver (spec 019) only needs "is this a
    /// GitHub target?", so expose that question rather than the whole enum.
    pub(crate) fn is_github(&self) -> bool {
        matches!(self.kind, ForgeKind::Github)
    }

    /// Build a project-scoped API URL from a compile-time trusted origin.
    /// Every dynamic value is added as a URL path segment or query pair, so it
    /// can never affect the destination scheme, authority, or port.
    fn project_api_url(
        &self,
        resource: &[&str],
        query: &[(&str, &str)],
    ) -> Result<TrustedForgeUrl, ApiError> {
        let base = match self.kind {
            ForgeKind::Github => "https://api.github.com/",
            ForgeKind::Gitlab => "https://gitlab.com/api/v4/",
        };
        let mut url = Url::parse(base).expect("compiled forge API origin is valid");
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|()| ApiError::Internal("invalid forge API base".into()))?;
            segments.pop_if_empty();
            match self.kind {
                ForgeKind::Github => {
                    let mut project = self.project.split('/');
                    let owner = project.next().ok_or_else(|| {
                        ApiError::BadRequest("invalid GitHub project path".into())
                    })?;
                    let repository = project.next().ok_or_else(|| {
                        ApiError::BadRequest("invalid GitHub project path".into())
                    })?;
                    if project.next().is_some() {
                        return Err(ApiError::BadRequest("invalid GitHub project path".into()));
                    }
                    segments.push("repos").push(owner).push(repository);
                }
                ForgeKind::Gitlab => {
                    // Pushing the complete project as one segment produces the
                    // `%2F` encoding required by GitLab's project API.
                    segments.push("projects").push(&self.project);
                }
            }
            for segment in resource {
                segments.push(segment);
            }
        }
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        TrustedForgeUrl::new(self.kind, url)
    }
}

/// A URL whose scheme, host, port, and API path prefix have been checked
/// against the selected forge. Its constructor is private so request helpers
/// cannot accidentally accept route input or an arbitrary string.
#[derive(Debug, Clone)]
struct TrustedForgeUrl(Url);

impl TrustedForgeUrl {
    fn new(kind: ForgeKind, url: Url) -> Result<Self, ApiError> {
        let trusted = url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && match kind {
                ForgeKind::Github => {
                    url.host_str() == Some("api.github.com") && url.path().starts_with("/repos/")
                }
                ForgeKind::Gitlab => {
                    url.host_str() == Some("gitlab.com")
                        && url.path().starts_with("/api/v4/projects/")
                }
            };
        if !trusted {
            return Err(ApiError::Internal(
                "refused untrusted forge request target".into(),
            ));
        }
        Ok(Self(url))
    }

    fn as_url(&self) -> &Url {
        &self.0
    }
}

/// Normalize a git remote URL to `(host, owner/repo)`.
///
/// Handles `git@host:owner/repo(.git)`, `ssh://git@host/owner/repo(.git)`,
/// and `https://host/owner/repo(.git)`. GitLab nested groups keep their full
/// path (`group/sub/repo`); the `.git` suffix and a trailing slash are
/// stripped.
pub(crate) fn parse_remote_url(url: &str) -> Option<(String, String)> {
    let raw = url.trim();
    let (host, path) = if let Some(rest) = raw.strip_prefix("git@") {
        // scp-like: git@host:owner/repo
        let (host, path) = rest.split_once(':')?;
        if !valid_remote_host(host) {
            return None;
        }
        (host.to_string(), path.to_string())
    } else {
        let parsed = Url::parse(raw).ok()?;
        if !matches!(parsed.scheme(), "https" | "ssh")
            || parsed.password().is_some()
            || parsed.port().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || (parsed.scheme() == "https" && !parsed.username().is_empty())
            || (parsed.scheme() == "ssh"
                && !parsed.username().is_empty()
                && parsed.username() != "git")
        {
            return None;
        }
        let host = parsed.host_str()?;
        if !valid_remote_host(host) {
            return None;
        }
        (
            host.to_string(),
            parsed.path().trim_start_matches('/').to_string(),
        )
    };

    let project = path
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if !valid_project_path(&project) {
        return None;
    }
    Some((host, project))
}

fn valid_remote_host(host: &str) -> bool {
    !host.is_empty()
        && !host.contains(['@', ':', '/', '\\'])
        && !host.chars().any(char::is_whitespace)
}

fn valid_project_path(project: &str) -> bool {
    let mut count = 0usize;
    for segment in project.split('/') {
        if segment.is_empty()
            || matches!(segment, "." | "..")
            || segment.contains(['\\', '%', '?', '#'])
            || segment.chars().any(char::is_control)
        {
            return false;
        }
        count += 1;
    }
    count >= 2
}

pub(crate) fn classify_remote(host: &str, project: String) -> Option<Remote> {
    let h = host.to_ascii_lowercase();
    if !valid_remote_host(host) || !valid_project_path(&project) {
        return None;
    }
    if (h == "github.com" || h == "www.github.com") && project.split('/').count() == 2 {
        Some(Remote {
            kind: ForgeKind::Github,
            project,
        })
    } else if h == "gitlab.com" {
        Some(Remote {
            kind: ForgeKind::Gitlab,
            project,
        })
    } else {
        None
    }
}

async fn git_origin_url(cwd: &StdPath) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

async fn git_current_branch(cwd: &StdPath) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty() && b != "HEAD").then_some(b)
}

/// Resolve the session's cwd → origin remote → parsed [`Remote`]. Maps the
/// "not a repo / no origin / unsupported host" cases to a 400 so the dashboard
/// can render a clear "connect a remote" state instead of a 500.
async fn remote_for(state: &AppState, id: Uuid) -> Result<(PathBuf, Remote), ApiError> {
    let cwd = cwd_for(state, id).await?;
    let url = git_origin_url(&cwd)
        .await
        .ok_or_else(|| ApiError::BadRequest("no git 'origin' remote for this session".into()))?;
    let (host, project) = parse_remote_url(&url)
        .ok_or_else(|| ApiError::BadRequest("could not parse origin remote".into()))?;
    let remote = classify_remote(&host, project)
        .ok_or_else(|| ApiError::BadRequest(format!("unsupported forge host: {host}")))?;
    Ok((cwd, remote))
}

// ── Token store (<data_dir>/forge.json, 0600) ────────────────────────────────

fn forge_tokens_path() -> Option<PathBuf> {
    let dir = agentum_store::paths::data_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("forge.json"))
}

fn read_tokens() -> BTreeMap<String, String> {
    let Some(path) = forge_tokens_path() else {
        return BTreeMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write_tokens(tokens: &BTreeMap<String, String>) -> std::io::Result<()> {
    let Some(path) = forge_tokens_path() else {
        return Ok(());
    };
    let body = serde_json::to_string_pretty(tokens).unwrap_or_else(|_| "{}".into());
    std::fs::write(&path, body)?;
    // Tighten perms to 0600 — this file holds API tokens. Best-effort on
    // non-unix (no-op).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub(crate) fn token_for(kind: ForgeKind) -> Result<String, ApiError> {
    read_tokens()
        .get(kind.as_str())
        .filter(|t| !t.trim().is_empty())
        .cloned()
        .ok_or_else(|| ApiError::BadRequest(format!("no {} token configured", kind.as_str())))
}

// ── HTTP helpers ─────────────────────────────────────────────────────────────

/// One reqwest GET to the forge, returning parsed JSON. Adds the right auth
/// header per forge and a `User-Agent` (GitHub rejects requests without one).
fn forge_client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder()
        // Never forward forge credentials through an upstream redirect. Each
        // request must terminate at the exact origin bound by TrustedForgeUrl.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ApiError::Internal("could not initialize forge client".into()))
}

async fn forge_get(remote: &Remote, token: &str, url: &TrustedForgeUrl) -> Result<Value, ApiError> {
    let client = forge_client()?;
    let mut req = client
        .get(url.as_url().clone())
        .header("User-Agent", "agentum");
    req = match remote.kind {
        ForgeKind::Github => req
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json"),
        ForgeKind::Gitlab => req.header("PRIVATE-TOKEN", token),
    };
    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("forge request failed: {e}")))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ApiError::Custom(
            axum::http::StatusCode::BAD_GATEWAY,
            json!({ "error": format!("forge returned {status}") }),
        ));
    }
    serde_json::from_str(&body).map_err(|e| ApiError::Internal(format!("forge JSON parse: {e}")))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

// ── Endpoints ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ForgeInfo {
    forge: Option<&'static str>,
    project: Option<String>,
    branch: Option<String>,
    has_token: bool,
}

fn parse_id(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// `GET /forge/info` — what the dashboard shows before fetching anything:
/// the detected forge, project, current branch, and whether a token exists.
/// Never 500s on a non-forge repo; reports `forge: null` instead.
async fn info(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ForgeInfo>, ApiError> {
    let id = parse_id(&id)?;
    let cwd = cwd_for(&state, id).await?;
    let branch = git_current_branch(&cwd).await;
    let remote = match git_origin_url(&cwd).await {
        Some(url) => parse_remote_url(&url).and_then(|(h, p)| classify_remote(&h, p)),
        None => None,
    };
    let (forge, project, has_token) = match remote {
        Some(r) => (
            Some(r.kind.as_str()),
            Some(r.project),
            token_for(r.kind).is_ok(),
        ),
        None => (None, None, false),
    };
    Ok(Json(ForgeInfo {
        forge,
        project,
        branch,
        has_token,
    }))
}

/// Normalized pull/merge request row the dashboard renders.
#[derive(Debug, Serialize)]
struct PrRow {
    number: i64,
    title: String,
    url: String,
    state: String,
    author: Option<String>,
    branch: Option<String>,
    draft: bool,
}

async fn prs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<PrRow>>, ApiError> {
    let id = parse_id(&id)?;
    let (_cwd, remote) = remote_for(&state, id).await?;
    let token = token_for(remote.kind)?;

    let rows = match remote.kind {
        ForgeKind::Github => {
            let url =
                remote.project_api_url(&["pulls"], &[("state", "open"), ("per_page", "50")])?;
            let arr = forge_get(&remote, &token, &url).await?;
            arr.as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|p| PrRow {
                            number: p.get("number").and_then(|n| n.as_i64()).unwrap_or(0),
                            title: str_field(p, "title").unwrap_or_default(),
                            url: str_field(p, "html_url").unwrap_or_default(),
                            state: str_field(p, "state").unwrap_or_default(),
                            author: p.get("user").and_then(|u| str_field(u, "login")),
                            branch: p.get("head").and_then(|h| str_field(h, "ref")),
                            draft: p.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        ForgeKind::Gitlab => {
            let url = remote.project_api_url(
                &["merge_requests"],
                &[("state", "opened"), ("per_page", "50")],
            )?;
            let arr = forge_get(&remote, &token, &url).await?;
            arr.as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|p| PrRow {
                            number: p.get("iid").and_then(|n| n.as_i64()).unwrap_or(0),
                            title: str_field(p, "title").unwrap_or_default(),
                            url: str_field(p, "web_url").unwrap_or_default(),
                            state: str_field(p, "state").unwrap_or_default(),
                            author: p.get("author").and_then(|u| str_field(u, "username")),
                            branch: str_field(p, "source_branch"),
                            draft: p.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    };
    Ok(Json(rows))
}

#[derive(Debug, Serialize)]
struct IssueRow {
    number: i64,
    title: String,
    url: String,
    state: String,
    author: Option<String>,
}

async fn issues(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<IssueRow>>, ApiError> {
    let id = parse_id(&id)?;
    let (_cwd, remote) = remote_for(&state, id).await?;
    let token = token_for(remote.kind)?;

    let rows = match remote.kind {
        ForgeKind::Github => {
            let url =
                remote.project_api_url(&["issues"], &[("state", "open"), ("per_page", "50")])?;
            let arr = forge_get(&remote, &token, &url).await?;
            arr.as_array()
                .map(|items| {
                    items
                        .iter()
                        // GitHub's issues endpoint also returns PRs; the
                        // `pull_request` key marks those. Drop them so the
                        // issues panel shows only real issues.
                        .filter(|i| i.get("pull_request").is_none())
                        .map(|i| IssueRow {
                            number: i.get("number").and_then(|n| n.as_i64()).unwrap_or(0),
                            title: str_field(i, "title").unwrap_or_default(),
                            url: str_field(i, "html_url").unwrap_or_default(),
                            state: str_field(i, "state").unwrap_or_default(),
                            author: i.get("user").and_then(|u| str_field(u, "login")),
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        ForgeKind::Gitlab => {
            let url =
                remote.project_api_url(&["issues"], &[("state", "opened"), ("per_page", "50")])?;
            let arr = forge_get(&remote, &token, &url).await?;
            arr.as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|i| IssueRow {
                            number: i.get("iid").and_then(|n| n.as_i64()).unwrap_or(0),
                            title: str_field(i, "title").unwrap_or_default(),
                            url: str_field(i, "web_url").unwrap_or_default(),
                            state: str_field(i, "state").unwrap_or_default(),
                            author: i.get("author").and_then(|u| str_field(u, "username")),
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    };
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct ChecksQuery {
    /// Branch or SHA to report checks for. Defaults to the session's current
    /// branch when omitted.
    #[serde(default)]
    r#ref: Option<String>,
}

#[derive(Debug, Serialize)]
struct CheckRow {
    name: String,
    /// Normalized: `success` | `failure` | `pending` | `<raw>`.
    status: String,
    url: Option<String>,
}

async fn checks(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ChecksQuery>,
) -> Result<Json<Vec<CheckRow>>, ApiError> {
    let id = parse_id(&id)?;
    let (cwd, remote) = remote_for(&state, id).await?;
    let token = token_for(remote.kind)?;
    let git_ref = match q.r#ref {
        Some(r) if !r.trim().is_empty() => r,
        _ => git_current_branch(&cwd)
            .await
            .ok_or_else(|| ApiError::BadRequest("no ref given and no current branch".into()))?,
    };

    let rows = match remote.kind {
        ForgeKind::Github => {
            let url = remote.project_api_url(&["commits", &git_ref, "check-runs"], &[])?;
            let body = forge_get(&remote, &token, &url).await?;
            body.get("check_runs")
                .and_then(|c| c.as_array())
                .map(|items| {
                    items
                        .iter()
                        .map(|c| {
                            // GitHub: status in_progress/queued/completed +
                            // conclusion success/failure/… Normalize to one field.
                            let concl = str_field(c, "conclusion");
                            let status = match concl.as_deref() {
                                Some("success") => "success",
                                Some("failure") | Some("timed_out") | Some("cancelled") => {
                                    "failure"
                                }
                                _ => "pending",
                            }
                            .to_string();
                            CheckRow {
                                name: str_field(c, "name").unwrap_or_default(),
                                status,
                                url: str_field(c, "html_url"),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        ForgeKind::Gitlab => {
            let url = remote.project_api_url(
                &["pipelines"],
                &[("ref", git_ref.as_str()), ("per_page", "20")],
            )?;
            let arr = forge_get(&remote, &token, &url).await?;
            arr.as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|p| {
                            let raw = str_field(p, "status").unwrap_or_default();
                            let status = match raw.as_str() {
                                "success" => "success",
                                "failed" => "failure",
                                _ => "pending",
                            }
                            .to_string();
                            CheckRow {
                                name: format!(
                                    "pipeline #{}",
                                    p.get("id").and_then(|n| n.as_i64()).unwrap_or(0)
                                ),
                                status,
                                url: str_field(p, "web_url"),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    };
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct CreatePrBody {
    title: String,
    #[serde(default)]
    body: String,
    /// Target branch (e.g. `main`). The source is the session's current branch.
    base: String,
}

async fn create_pr(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<CreatePrBody>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&id)?;
    let (cwd, remote) = remote_for(&state, id).await?;
    let token = token_for(remote.kind)?;
    if input.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title is empty".into()));
    }
    if input.base.trim().is_empty() {
        return Err(ApiError::BadRequest("base branch is empty".into()));
    }
    let head = git_current_branch(&cwd)
        .await
        .ok_or_else(|| ApiError::BadRequest("session is not on a named branch".into()))?;

    let client = forge_client()?;
    let (url, payload, auth_header) = match remote.kind {
        ForgeKind::Github => (
            remote.project_api_url(&["pulls"], &[])?,
            json!({ "title": input.title, "head": head, "base": input.base, "body": input.body }),
            ("Authorization", format!("Bearer {token}")),
        ),
        ForgeKind::Gitlab => (
            remote.project_api_url(&["merge_requests"], &[])?,
            json!({
                "source_branch": head,
                "target_branch": input.base,
                "title": input.title,
                "description": input.body,
            }),
            ("PRIVATE-TOKEN", token.clone()),
        ),
    };

    let mut req = client
        .post(url.as_url().clone())
        .header("User-Agent", "agentum")
        .header(auth_header.0, auth_header.1)
        .json(&payload);
    if remote.kind == ForgeKind::Github {
        req = req.header("Accept", "application/vnd.github+json");
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("create PR request failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ApiError::Custom(
            axum::http::StatusCode::BAD_GATEWAY,
            json!({ "error": format!("forge returned {status}") }),
        ));
    }
    let created: Value = serde_json::from_str(&text).unwrap_or(json!({}));
    // Normalize the one field the dashboard needs to link straight to it.
    let html_url = str_field(&created, "html_url").or_else(|| str_field(&created, "web_url"));
    Ok(Json(json!({ "url": html_url })))
}

// ── Token endpoints ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenQuery {
    forge: String,
}

async fn get_token(Query(q): Query<TokenQuery>) -> Result<Json<Value>, ApiError> {
    let has = read_tokens()
        .get(&q.forge)
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);
    Ok(Json(json!({ "forge": q.forge, "has_token": has })))
}

#[derive(Debug, Deserialize)]
struct PutTokenBody {
    forge: String,
    /// The PAT. An empty string clears the stored token.
    token: String,
}

async fn put_token(Json(body): Json<PutTokenBody>) -> Result<Json<Value>, ApiError> {
    if body.forge != "github" && body.forge != "gitlab" {
        return Err(ApiError::BadRequest(format!(
            "unknown forge: {} (expected github|gitlab)",
            body.forge
        )));
    }
    let mut tokens = read_tokens();
    if body.token.trim().is_empty() {
        tokens.remove(&body.forge);
    } else {
        tokens.insert(body.forge.clone(), body.token);
    }
    write_tokens(&tokens).map_err(|e| ApiError::Internal(format!("write forge tokens: {e}")))?;
    Ok(Json(
        json!({ "forge": body.forge, "has_token": tokens.contains_key(&body.forge) }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scp_like_github() {
        let (host, project) = parse_remote_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(project, "owner/repo");
    }

    #[test]
    fn parses_https_gitlab_nested_groups() {
        let (host, project) = parse_remote_url("https://gitlab.com/group/sub/repo.git").unwrap();
        assert_eq!(host, "gitlab.com");
        assert_eq!(project, "group/sub/repo");
    }

    #[test]
    fn parses_ssh_url_with_user() {
        let (host, project) = parse_remote_url("ssh://git@github.com/owner/repo").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(project, "owner/repo");
    }

    #[test]
    fn rejects_non_repo_urls() {
        assert!(parse_remote_url("https://github.com/owner").is_none());
        assert!(parse_remote_url("not a url").is_none());
        assert!(parse_remote_url("http://github.com/owner/repo").is_none());
        assert!(parse_remote_url("https://github.com:444/owner/repo").is_none());
        assert!(parse_remote_url("https://github.com@evil.example/owner/repo").is_none());
        assert!(parse_remote_url("ssh://root@github.com/owner/repo").is_none());
        assert!(parse_remote_url("git@github.com@evil.example:owner/repo").is_none());
    }

    #[test]
    fn classifies_only_exact_public_hosts() {
        assert_eq!(
            classify_remote("github.com", "o/r".into()).unwrap().kind,
            ForgeKind::Github
        );
        assert_eq!(
            classify_remote("gitlab.com", "o/r".into()).unwrap().kind,
            ForgeKind::Gitlab
        );
        for hostile in [
            "github.evil",
            "evilgithub.com",
            "github.com.evil.example",
            "gitlab.internal.corp",
            "gitlab.com.evil.example",
            "evilgitlab.com",
        ] {
            assert!(
                classify_remote(hostile, "o/r".into()).is_none(),
                "unexpected classification for {hostile}"
            );
        }
    }

    #[test]
    fn api_urls_keep_dynamic_values_out_of_the_authority() {
        let github = classify_remote("github.com", "owner/repo".into()).unwrap();
        let github_url = github
            .project_api_url(
                &["commits", "feature/x?redirect=https://evil", "check-runs"],
                &[],
            )
            .unwrap();
        assert_eq!(github_url.as_url().scheme(), "https");
        assert_eq!(github_url.as_url().host_str(), Some("api.github.com"));
        assert_eq!(
            github_url.as_url().path(),
            "/repos/owner/repo/commits/feature%2Fx%3Fredirect=https:%2F%2Fevil/check-runs"
        );

        let gitlab = classify_remote("gitlab.com", "group/sub/repo".into()).unwrap();
        let gitlab_url = gitlab
            .project_api_url(&["pipelines"], &[("ref", "feature/x&owned=true")])
            .unwrap();
        assert_eq!(gitlab_url.as_url().scheme(), "https");
        assert_eq!(gitlab_url.as_url().host_str(), Some("gitlab.com"));
        assert_eq!(
            gitlab_url.as_url().path(),
            "/api/v4/projects/group%2Fsub%2Frepo/pipelines"
        );
        assert_eq!(
            gitlab_url.as_url().query(),
            Some("ref=feature%2Fx%26owned%3Dtrue")
        );
    }

    #[test]
    fn github_rejects_nested_or_ambiguous_project_paths() {
        assert!(classify_remote("github.com", "group/sub/repo".into()).is_none());
        assert!(classify_remote("github.com", "owner/../repo".into()).is_none());
        assert!(classify_remote("github.com", "owner/%2e%2e".into()).is_none());
    }
}
