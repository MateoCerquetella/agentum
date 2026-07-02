//! `GET /api/github/issue` — read a single GitHub issue's title + body so the
//! desktop Tasks "Use" path can seed the spawned agent with the ticket's spec,
//! not just its URL (spec 002, Option B).
//!
//! `POST /api/github/issues` — file a new issue from the New Workspace composer
//! (spec 004 F3): the composer creates the issue *before* any worktree exists
//! and links the response as the workspace's `linkedWorkItem`. Thin over the
//! existing `TaskSink::Github` create path — same `gh` CLI, same auth surface.
//!
//! The desktop already snapshots a Linear issue's `description` into linked
//! context (`lib/linear-linked-work-item.ts`); GitHub work items carry no body
//! in memory, so the read is the one missing piece. Both routes reuse the same
//! `gh` runner (`gh_in_dir` / `TaskSink`) and slug resolver
//! (`resolve_github_slug`) the board issue-creation path uses, so there is no
//! new shell/auth surface: `gh` runs on the local host with the user's own auth
//! from a neutral cwd (`$HOME`), addressed by `--repo <slug>` exactly like
//! issue creation.

use agentum_core::LOCAL_HOST_ID;
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;
use crate::task_sink::{NewFeature, SinkCtx, TaskSink};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/github/issue", get(get_issue))
        .route("/api/github/issues", post(create_issue))
        .route("/api/github/labels", get(list_labels))
}

#[derive(Debug, Deserialize)]
pub struct IssueQuery {
    /// Issue number, as a string for query-param ergonomics. Validated numeric.
    pub number: String,
    /// `owner/repo` hint parsed from the work item's URL. A valid hint lets
    /// `resolve_github_slug` skip the `origin` read entirely.
    pub slug: Option<String>,
    /// Project dir, used to resolve the slug from `origin` when no valid `slug`
    /// hint is supplied. Required so we never read `origin` from the server's own
    /// cwd by accident.
    pub workdir: String,
}

#[derive(Debug, Serialize)]
pub struct IssueBody {
    pub title: String,
    pub body: String,
}

/// True for a bare positive issue number (digits only). Guards the value before
/// it becomes a `gh` argv token so a crafted `number` can never inject a flag.
fn is_numeric_issue_id(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// One GitHub issue as fetched by `gh issue view --json title,body,url`.
/// `url` comes from `gh` itself (authoritative, GHES-correct) — never
/// string-assembled. `slug` is the resolved `owner/repo` the fetch targeted —
/// part of the fetch contract (spec 004 §5) though no in-tree caller reads it
/// yet (`tracker_url` carries the slug downstream), hence the targeted allow.
pub(crate) struct FetchedIssue {
    pub title: String,
    pub body: String,
    pub url: String,
    #[allow(dead_code)]
    pub slug: String,
}

/// Fetch a single issue's title + body + URL via the local `gh` from a neutral
/// cwd (`$HOME`), addressed by `--repo <slug>` — identical mechanics to issue
/// creation. Shared by `GET /api/github/issue` and the spec-from-issue scaffold
/// (spec 004 F4), which needs the server-authoritative body as the transform's
/// input rather than trusting a client-supplied one.
pub(crate) async fn fetch_github_issue(
    state: &AppState,
    workdir: &str,
    number: &str,
    slug_hint: Option<&str>,
) -> Result<FetchedIssue, ApiError> {
    let number = number.trim();
    if !is_numeric_issue_id(number) {
        return Err(ApiError::BadRequest(
            "issue `number` must be a positive integer".into(),
        ));
    }
    let workdir = workdir.trim();
    if workdir.is_empty() {
        return Err(ApiError::BadRequest("`workdir` is required".into()));
    }

    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;

    // Prefer the `owner/repo` hint (zero I/O); fall back to the project's
    // `origin` read. Reuses the exact resolver the board issue path uses.
    let slug = super::board_goals::resolve_github_slug(&host, workdir, slug_hint)
        .await
        .map_err(|reason| {
            ApiError::BadRequest(format!("could not resolve a GitHub repo: {reason:?}"))
        })?;

    // `gh` is addressed by `--repo <slug>` and run from a neutral cwd ($HOME) so
    // a stray `.git`/`GH_REPO` can't redirect it — identical to issue creation.
    let cwd = crate::task_sink::neutral_cwd();
    let cwd = cwd.to_string_lossy();
    let out = crate::host_runtime::gh_in_dir(
        &host,
        &cwd,
        &[
            "issue",
            "view",
            number,
            "--repo",
            slug.as_str(),
            "--json",
            "title,body,url",
        ],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("could not run `gh`: {e}")))?;

    if !out.success {
        // `gh` owns its own auth, so its stderr carries no agentum-held secret;
        // still, log it server-side and return a generic message — the desktop
        // falls back to the title+URL prompt on any error (never breaks "Use").
        tracing::warn!(stderr = %out.stderr, slug = %slug, number = %number, "gh issue view failed");
        return Err(ApiError::BadRequest("`gh issue view` failed".into()));
    }

    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| ApiError::Internal(format!("could not parse `gh` output: {e}")))?;
    Ok(FetchedIssue {
        title: v["title"].as_str().unwrap_or_default().to_string(),
        body: v["body"].as_str().unwrap_or_default().to_string(),
        url: v["url"].as_str().unwrap_or_default().to_string(),
        slug,
    })
}

async fn get_issue(
    State(state): State<AppState>,
    Query(q): Query<IssueQuery>,
) -> Result<Json<IssueBody>, ApiError> {
    let issue = fetch_github_issue(&state, &q.workdir, &q.number, q.slug.as_deref()).await?;
    // The wire shape predates the shared fetch — `url` is fetched but not
    // returned here, so existing clients see no change.
    Ok(Json(IssueBody {
        title: issue.title,
        body: issue.body,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateIssueBody {
    /// Trimmed non-empty, else 400.
    title: String,
    #[serde(default)]
    body: Option<String>,
    /// Project dir for the `origin` read when no `slug` hint is supplied.
    workdir: String,
    /// `owner/repo` fast path (skips the origin read when well-formed).
    #[serde(default)]
    slug: Option<String>,
    /// Spec 006 F1: labels applied at creation via the existing `gh --label`
    /// plumbing (task_sink.rs). Absent = today's behavior, byte-identical.
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateIssueResponse {
    provider: &'static str,
    number: i64,
    url: String,
    slug: String,
    /// Spec 006 F4 (D3): the authenticated `gh` login — the creator. Best
    /// effort: None on any failure, never an error (additive; serializes as
    /// `"author":null`, which old clients ignore).
    author: Option<String>,
}

/// Parse the created issue's number out of `FeatureRef.id`. `parse_gh_issue_url`
/// guarantees digits on success, so a non-numeric id here is an internal
/// contract violation (500), not a client error.
fn issue_number_from_ref_id(id: &str) -> Result<i64, ApiError> {
    id.trim()
        .parse::<i64>()
        .map_err(|_| ApiError::Internal(format!("gh returned a non-numeric issue id: {id:?}")))
}

/// `POST /api/github/issues` — file one issue through the existing
/// `TaskSink::Github` path (spec 004 F3, AC 1). Mirrors the Chat-issues
/// handler's shape minus the LLM: resolve the slug (hint → host-aware `origin`
/// read; miss → typed 422 `no_github_repo`), then `gh issue create --repo
/// <slug>` from `$HOME`.
async fn create_issue(
    State(state): State<AppState>,
    Json(body): Json<CreateIssueBody>,
) -> Result<Json<CreateIssueResponse>, ApiError> {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(ApiError::BadRequest(
            "issue `title` must not be blank".into(),
        ));
    }
    let workdir = body.workdir.trim();
    if workdir.is_empty() {
        return Err(ApiError::BadRequest("`workdir` is required".into()));
    }

    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;

    let slug = match super::board_goals::resolve_github_slug(&host, workdir, body.slug.as_deref())
        .await
    {
        Ok(slug) => slug,
        // Same typed envelope as the Chat-issues route so the UI branches on
        // one `no_github_repo` code for both entry points.
        Err(_) => {
            return Err(ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": { "code": "no_github_repo", "message": "no GitHub repo resolved for this project" } }),
            ));
        }
    };

    let feature = NewFeature {
        title: title.to_string(),
        body: body.body.clone().filter(|b| !b.trim().is_empty()),
        labels: body.labels.clone(),
    };
    let fref = TaskSink::Github
        .create_feature(
            &SinkCtx {
                store: &state.store,
                // The explicit-slug GitHub arm runs `gh` from `$HOME`; workdir
                // is passed for shape only (same note as the Chat path).
                workdir: std::path::Path::new(workdir),
                parent_goal_id: None,
                slug: Some(&slug),
            },
            &feature,
        )
        .await
        .map_err(|e| super::board_goals::map_sink_error(TaskSink::Github, &e))?;

    let number = issue_number_from_ref_id(&fref.id)?;
    // Spec 006 F4: fetched AFTER the successful create — a login failure must
    // never fail a created issue, and a failed create wastes no `gh` call.
    let author = authenticated_github_login(&host).await;
    Ok(Json(CreateIssueResponse {
        provider: "github",
        number,
        url: fref.url.unwrap_or_default(),
        slug,
        author,
    }))
}

#[derive(Debug, Deserialize)]
pub struct LabelsQuery {
    /// Project dir for the `origin` read when no `slug` hint is supplied.
    pub workdir: String,
    /// `owner/repo` fast path (skips the origin read when well-formed).
    pub slug: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LabelsResponse {
    pub labels: Vec<String>,
}

/// Pure: map `gh label list --json name` output (`[{"name": …}, …]`) to names —
/// skip nameless entries, sort case-insensitively, dedup.
fn parse_label_names(stdout: &[u8]) -> anyhow::Result<Vec<String>> {
    let entries: Vec<serde_json::Value> = serde_json::from_slice(stdout)?;
    let mut names: Vec<String> = entries
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .map(str::to_string)
        .collect();
    // Case-insensitive sort keeps exact duplicates adjacent (stable sort), so
    // the plain dedup below removes them.
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    Ok(names)
}

/// `GET /api/github/labels` — the repo's existing label names, for the
/// composer's label picker (spec 006 F1, D2). Same shape as issue fetch/create:
/// slug via `resolve_github_slug` (typed 422 `no_github_repo` on miss, so the
/// UI branches on one code for all three entry points), `gh` from the neutral
/// cwd. A `gh` failure is a plain 400 — the picker treats ANY error as "use the
/// static fallback", so no typed envelope is needed there.
async fn list_labels(
    State(state): State<AppState>,
    Query(q): Query<LabelsQuery>,
) -> Result<Json<LabelsResponse>, ApiError> {
    let workdir = q.workdir.trim();
    if workdir.is_empty() {
        return Err(ApiError::BadRequest("`workdir` is required".into()));
    }

    let host = state
        .store
        .get_host(LOCAL_HOST_ID)
        .await?
        .ok_or_else(|| ApiError::Internal("local host missing".into()))?;

    let slug = match super::board_goals::resolve_github_slug(&host, workdir, q.slug.as_deref())
        .await
    {
        Ok(slug) => slug,
        Err(_) => {
            return Err(ApiError::Custom(
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": { "code": "no_github_repo", "message": "no GitHub repo resolved for this project" } }),
            ));
        }
    };

    let cwd = crate::task_sink::neutral_cwd();
    let cwd = cwd.to_string_lossy();
    let out = crate::host_runtime::gh_in_dir(
        &host,
        &cwd,
        &[
            "label",
            "list",
            "--repo",
            slug.as_str(),
            "--json",
            "name",
            "--limit",
            "100",
        ],
    )
    .await
    .map_err(|e| ApiError::Internal(format!("could not run `gh`: {e}")))?;

    if !out.success {
        tracing::warn!(stderr = %out.stderr, slug = %slug, "gh label list failed");
        return Err(ApiError::BadRequest("`gh label list` failed".into()));
    }

    let labels = parse_label_names(&out.stdout)
        .map_err(|e| ApiError::Internal(format!("could not parse `gh` output: {e}")))?;
    Ok(Json(LabelsResponse { labels }))
}

/// Pure: a login is the trimmed, non-empty stdout of `gh api user --jq .login`.
fn parse_gh_login(stdout: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(stdout);
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// `gh api user --jq .login` from the neutral cwd — best-effort by contract
/// (spec 006 F4): any failure (offline, unauthenticated, old `gh`) yields
/// `None`, never an error. No cache — a create is click-frequency, and a cache
/// would go stale across `gh auth switch`.
async fn authenticated_github_login(host: &agentum_core::Host) -> Option<String> {
    let cwd = crate::task_sink::neutral_cwd();
    let cwd = cwd.to_string_lossy();
    let out = crate::host_runtime::gh_in_dir(host, &cwd, &["api", "user", "--jq", ".login"])
        .await
        .ok()?;
    if !out.success {
        tracing::warn!(stderr = %out.stderr, "gh api user failed; issue author omitted");
        return None;
    }
    parse_gh_login(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_issue_id_accepts_digits_only() {
        assert!(is_numeric_issue_id("42"));
        assert!(is_numeric_issue_id(" 7 "));
        assert!(!is_numeric_issue_id(""));
        assert!(!is_numeric_issue_id("--repo"));
        assert!(!is_numeric_issue_id("1a"));
        assert!(!is_numeric_issue_id("-5"));
    }

    #[test]
    fn create_issue_rejects_blank_title() {
        // The handler's title gate is a plain trim + empty check; pin the
        // deserialized shape + the rejection contract here (a full handler
        // round-trip needs an AppState; the gate itself is pure).
        let body: CreateIssueBody = serde_json::from_value(serde_json::json!({
            "title": "   ",
            "workdir": "/tmp/repo"
        }))
        .unwrap();
        assert!(
            body.title.trim().is_empty(),
            "blank title must trip the 400 gate"
        );

        let ok: CreateIssueBody = serde_json::from_value(serde_json::json!({
            "title": "Add a widget",
            "body": "details",
            "workdir": "/tmp/repo",
            "slug": "acme/widgets"
        }))
        .unwrap();
        assert_eq!(ok.title.trim(), "Add a widget");
        assert_eq!(ok.slug.as_deref(), Some("acme/widgets"));
    }

    #[test]
    fn issue_number_parses_digits_and_rejects_junk() {
        assert_eq!(issue_number_from_ref_id("42").unwrap(), 42);
        assert_eq!(issue_number_from_ref_id(" 7 ").unwrap(), 7);
        assert!(issue_number_from_ref_id("abc").is_err());
        assert!(issue_number_from_ref_id("").is_err());
        assert!(issue_number_from_ref_id("12a").is_err());
    }

    #[test]
    fn create_issue_body_labels_default_empty() {
        // Spec 006 F1 (AC 1): absent labels deserialize to an empty Vec — the
        // wire stays byte-identical to pre-006 requests (the argv half is
        // pinned by task_sink's gh_create_argv_* tests).
        let body: CreateIssueBody = serde_json::from_value(serde_json::json!({
            "title": "Add a widget",
            "workdir": "/tmp/repo"
        }))
        .unwrap();
        assert!(body.labels.is_empty(), "absent labels must default to []");

        let body: CreateIssueBody = serde_json::from_value(serde_json::json!({
            "title": "Add a widget",
            "workdir": "/tmp/repo",
            "labels": ["type/feat", "priority/p1"]
        }))
        .unwrap();
        assert_eq!(body.labels, vec!["type/feat", "priority/p1"]);
    }

    #[test]
    fn parse_label_names_maps_sorts_and_skips_nameless() {
        let stdout = br#"[{"name":"b"},{"name":"A"},{}]"#;
        assert_eq!(parse_label_names(stdout).unwrap(), vec!["A", "b"]);
        // Exact duplicates collapse; junk input errors (the route maps it to 500).
        let stdout = br#"[{"name":"x"},{"name":"x"}]"#;
        assert_eq!(parse_label_names(stdout).unwrap(), vec!["x"]);
        assert!(parse_label_names(b"not json").is_err());
    }

    #[test]
    fn create_issue_response_serializes_author_present_and_null() {
        // Spec 006 F4: the widening is additive — `author` is a plain nullable
        // field old clients ignore.
        let with_author = serde_json::to_string(&CreateIssueResponse {
            provider: "github",
            number: 232,
            url: "https://github.com/o/r/issues/232".into(),
            slug: "o/r".into(),
            author: Some("mateo".into()),
        })
        .unwrap();
        assert!(with_author.contains(r#""author":"mateo""#));

        let without_author = serde_json::to_string(&CreateIssueResponse {
            provider: "github",
            number: 232,
            url: "https://github.com/o/r/issues/232".into(),
            slug: "o/r".into(),
            author: None,
        })
        .unwrap();
        assert!(without_author.contains(r#""author":null"#));
    }

    #[test]
    fn parse_gh_login_trims_and_rejects_empty() {
        assert_eq!(parse_gh_login(b"mateo\n"), Some("mateo".to_string()));
        assert_eq!(parse_gh_login(b"  \n"), None);
        assert_eq!(parse_gh_login(b""), None);
    }
}
