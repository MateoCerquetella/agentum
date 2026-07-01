//! `GET /api/github/issue` — read a single GitHub issue's title + body so the
//! desktop Tasks "Use" path can seed the spawned agent with the ticket's spec,
//! not just its URL (spec 002, Option B).
//!
//! The desktop already snapshots a Linear issue's `description` into linked
//! context (`lib/linear-linked-work-item.ts`); GitHub work items carry no body
//! in memory, so this is the one missing read. It reuses the same `gh` runner
//! (`gh_in_dir`) and slug resolver (`resolve_github_slug`) the board
//! issue-creation path uses, so there is no new shell/auth surface: `gh` runs on
//! the local host with the user's own auth from a neutral cwd (`$HOME`),
//! addressed by `--repo <slug>` exactly like issue creation.

use agentum_core::LOCAL_HOST_ID;
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/github/issue", get(get_issue))
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

async fn get_issue(
    State(state): State<AppState>,
    Query(q): Query<IssueQuery>,
) -> Result<Json<IssueBody>, ApiError> {
    let number = q.number.trim();
    if !is_numeric_issue_id(number) {
        return Err(ApiError::BadRequest(
            "issue `number` must be a positive integer".into(),
        ));
    }
    let workdir = q.workdir.trim();
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
    let slug = super::board_goals::resolve_github_slug(&host, workdir, q.slug.as_deref())
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
            "title,body",
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
    Ok(Json(IssueBody {
        title: v["title"].as_str().unwrap_or_default().to_string(),
        body: v["body"].as_str().unwrap_or_default().to_string(),
    }))
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
}
