//! `/api/board/bindings` + `/api/board/sync` — spec 014a: pull a bound
//! external tracker's issues onto the board as cards.
//!
//! 014a is **one direction, one provider**: GitHub issues → board cards,
//! idempotent on re-sync (matched by `(external_provider, external_id)`).
//! Push-back (board → tracker) is 014b; Linear is 014c. We **reuse**
//! `routes::forge`'s REST + token plumbing rather than reimplement it.
//!
//! Self-hosted ⇒ no inbound webhooks, so sync is manual/poll: the client
//! calls `POST /api/board/sync`.

use agentum_core::{Event, TrackerBinding};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::AppState;
use crate::error::ApiError;
use crate::routes::forge::{ForgeKind, classify_remote, forge_get, token_for};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board/bindings", post(create_binding).get(list_bindings))
        .route("/api/board/bindings/{id}", delete(delete_binding))
        .route("/api/board/sync", post(sync))
}

// ── Pure sync core (unit-tested; no I/O) ─────────────────────────────────────

/// A tracker issue normalized to the fields the board cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalIssue {
    /// Provider-native id (GitHub issue number, as text).
    pub external_id: String,
    pub title: String,
    pub body: Option<String>,
    pub url: String,
    /// `"open"` | `"closed"`.
    pub state: String,
}

/// What a sync should do with one incoming issue, relative to the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncAction {
    Create { issue: ExternalIssue, status: String },
    Update { card_id: i64, issue: ExternalIssue, status: String },
}

/// Initial board column for a brand-new card from an issue's state.
fn state_to_status(state: &str) -> &'static str {
    if state.eq_ignore_ascii_case("closed") {
        "done"
    } else {
        "todo"
    }
}

/// Board column for an existing card on re-sync. The v1 (014a) conflict
/// policy (single source of this rule):
///
/// - closed upstream → `done` (tracker wins on close)
/// - open upstream, card already `done` → `todo` (reopened upstream)
/// - open upstream otherwise → keep the local column (preserve a manual
///   `todo`→`doing` move; don't yank it back)
///
/// Two-sided conflict detection (both changed since last sync) is 014b.
fn reconcile_status(local: &str, state: &str) -> String {
    if state.eq_ignore_ascii_case("closed") {
        "done".to_string()
    } else if local == "done" {
        "todo".to_string()
    } else {
        local.to_string()
    }
}

/// Diff incoming issues against the cards already mirroring this provider.
/// `existing` is `(card_id, external_id, local_status)`.
pub(crate) fn reconcile(
    existing: &[(i64, String, String)],
    issues: &[ExternalIssue],
) -> Vec<SyncAction> {
    issues
        .iter()
        .map(|issue| match existing.iter().find(|(_, ext, _)| ext == &issue.external_id) {
            Some((card_id, _, local_status)) => SyncAction::Update {
                card_id: *card_id,
                status: reconcile_status(local_status, &issue.state),
                issue: issue.clone(),
            },
            None => SyncAction::Create {
                status: state_to_status(&issue.state).to_string(),
                issue: issue.clone(),
            },
        })
        .collect()
}

/// Parse a GitHub `/issues` array. GitHub returns PRs from this endpoint too
/// (they carry a `pull_request` key) — skip them so the board mirrors only
/// real issues. Rows missing `number`/`title` are skipped, not faked.
pub(crate) fn parse_github_issues(v: &Value) -> Vec<ExternalIssue> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            if item.get("pull_request").is_some() {
                return None;
            }
            let number = item.get("number")?.as_i64()?;
            let title = item.get("title")?.as_str()?.to_string();
            let body = item
                .get("body")
                .and_then(|b| b.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let url = item
                .get("html_url")
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string();
            let state = item
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or("open")
                .to_string();
            Some(ExternalIssue {
                external_id: number.to_string(),
                title,
                body,
                url,
                state,
            })
        })
        .collect()
}

// ── Bindings CRUD ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct NewBindingBody {
    provider: String,
    project: String,
}

async fn create_binding(
    State(state): State<AppState>,
    Json(body): Json<NewBindingBody>,
) -> Result<(StatusCode, Json<TrackerBinding>), ApiError> {
    let provider = body.provider.trim().to_ascii_lowercase();
    if provider != "github" {
        return Err(ApiError::BadRequest(format!(
            "provider '{provider}' not yet supported — 014a is GitHub-only (Linear/GitLab land in 014c)"
        )));
    }
    let project = body.project.trim();
    if project.is_empty() || !project.contains('/') {
        return Err(ApiError::BadRequest(
            "project must be 'owner/repo'".into(),
        ));
    }
    let binding = state.store.create_tracker_binding(&provider, project).await?;
    let _ = state.bus.send(Event::new("board.binding.created").with_payload(json!({
        "id": binding.id, "provider": binding.provider, "project": binding.project,
    })));
    Ok((StatusCode::CREATED, Json(binding)))
}

async fn list_bindings(
    State(state): State<AppState>,
) -> Result<Json<Vec<TrackerBinding>>, ApiError> {
    Ok(Json(state.store.list_tracker_bindings().await?))
}

async fn delete_binding(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_tracker_binding(id).await?;
    let _ = state
        .bus
        .send(Event::new("board.binding.deleted").with_payload(json!({ "id": id })));
    Ok(StatusCode::NO_CONTENT)
}

// ── Sync ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct SyncBody {
    /// Sync only this binding; omit to sync all configured bindings.
    #[serde(default)]
    binding_id: Option<i64>,
}

#[derive(Debug, Serialize)]
struct SyncResult {
    provider: String,
    project: String,
    created: usize,
    updated: usize,
}

async fn sync(
    State(state): State<AppState>,
    body: Option<Json<SyncBody>>,
) -> Result<Json<Value>, ApiError> {
    let binding_id = body.and_then(|b| b.0.binding_id);
    let mut bindings = state.store.list_tracker_bindings().await?;
    if let Some(bid) = binding_id {
        bindings.retain(|b| b.id == bid);
        if bindings.is_empty() {
            return Err(ApiError::NotFound(format!("tracker binding {bid}")));
        }
    }
    if bindings.is_empty() {
        return Err(ApiError::BadRequest(
            "no tracker bindings configured — POST /api/board/bindings first".into(),
        ));
    }

    let mut results = Vec::with_capacity(bindings.len());
    for binding in &bindings {
        // Fail loud: a bad token / forge error aborts here rather than
        // reporting a misleading success.
        results.push(sync_one(&state, binding).await?);
    }

    let _ = state.bus.send(Event::new("board.sync.completed").with_payload(json!({
        "results": results.iter().map(|r| json!({
            "provider": r.provider, "project": r.project,
            "created": r.created, "updated": r.updated,
        })).collect::<Vec<_>>(),
    })));
    Ok(Json(json!({ "results": results })))
}

/// Pull one binding's issues and upsert them. GitHub only in 014a.
async fn sync_one(state: &AppState, binding: &TrackerBinding) -> Result<SyncResult, ApiError> {
    let remote = classify_remote("github.com", binding.project.clone()).ok_or_else(|| {
        ApiError::BadRequest(format!("could not build a github remote for {}", binding.project))
    })?;
    let token = token_for(ForgeKind::Github)?;
    // state=all so closed issues map to done. 100-cap; pagination is 014e.
    let url = format!(
        "{}/repos/{}/issues?state=all&per_page=100",
        remote.api_base, remote.project
    );
    let value = forge_get(&remote, &token, &url).await?;
    let issues = parse_github_issues(&value);

    let existing = state.store.list_external_refs("github").await?;
    let actions = reconcile(&existing, &issues);

    let synced_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let (mut created, mut updated) = (0usize, 0usize);
    for action in actions {
        let (issue, status, is_create) = match action {
            SyncAction::Create { issue, status } => (issue, status, true),
            SyncAction::Update { issue, status, .. } => (issue, status, false),
        };
        state
            .store
            .upsert_external_card(
                "github",
                &issue.external_id,
                &issue.title,
                issue.body.as_deref(),
                &issue.url,
                &status,
                &synced_at,
            )
            .await?;
        if is_create {
            created += 1;
        } else {
            updated += 1;
        }
    }

    Ok(SyncResult {
        provider: "github".into(),
        project: binding.project.clone(),
        created,
        updated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str, title: &str, state: &str) -> ExternalIssue {
        ExternalIssue {
            external_id: id.into(),
            title: title.into(),
            body: None,
            url: format!("https://github.com/o/r/issues/{id}"),
            state: state.into(),
        }
    }

    #[test]
    fn state_to_status_maps_open_and_closed() {
        assert_eq!(state_to_status("open"), "todo");
        assert_eq!(state_to_status("closed"), "done");
        assert_eq!(state_to_status("OPEN"), "todo");
    }

    #[test]
    fn reconcile_status_policy() {
        // closed upstream → done regardless of local column.
        assert_eq!(reconcile_status("doing", "closed"), "done");
        // reopened upstream → pull a done card back to todo.
        assert_eq!(reconcile_status("done", "open"), "todo");
        // open upstream → preserve a local in-progress move.
        assert_eq!(reconcile_status("doing", "open"), "doing");
        assert_eq!(reconcile_status("todo", "open"), "todo");
    }

    #[test]
    fn reconcile_creates_unseen_issue() {
        let actions = reconcile(&[], &[issue("12", "Add login", "open")]);
        assert_eq!(
            actions,
            vec![SyncAction::Create {
                issue: issue("12", "Add login", "open"),
                status: "todo".into(),
            }]
        );
    }

    #[test]
    fn reconcile_updates_known_issue_and_keeps_card_id() {
        let existing = vec![(7i64, "12".to_string(), "doing".to_string())];
        let actions = reconcile(&existing, &[issue("12", "Add login (edited)", "open")]);
        match &actions[0] {
            SyncAction::Update { card_id, status, issue } => {
                assert_eq!(*card_id, 7);
                assert_eq!(status, "doing"); // open preserves the local column
                assert_eq!(issue.title, "Add login (edited)");
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn reconcile_closed_known_issue_moves_to_done() {
        let existing = vec![(7i64, "12".to_string(), "doing".to_string())];
        let actions = reconcile(&existing, &[issue("12", "Add login", "closed")]);
        match &actions[0] {
            SyncAction::Update { status, .. } => assert_eq!(status, "done"),
            other => panic!("expected Update→done, got {other:?}"),
        }
    }

    #[test]
    fn parse_github_issues_filters_prs_and_skips_bad_rows() {
        let v = json!([
            { "number": 1, "title": "Real issue", "body": "hi", "html_url": "u1", "state": "open" },
            { "number": 2, "title": "A PR", "html_url": "u2", "state": "open",
              "pull_request": { "url": "x" } },
            { "title": "missing number" },
            { "number": 3, "title": "Closed one", "body": "", "html_url": "u3", "state": "closed" }
        ]);
        let issues = parse_github_issues(&v);
        assert_eq!(issues.len(), 2, "PR + malformed row dropped");
        assert_eq!(issues[0].external_id, "1");
        assert_eq!(issues[0].body.as_deref(), Some("hi"));
        assert_eq!(issues[1].external_id, "3");
        assert_eq!(issues[1].body, None, "empty body normalizes to None");
        assert_eq!(issues[1].state, "closed");
    }

    #[test]
    fn parse_github_issues_non_array_is_empty() {
        assert!(parse_github_issues(&json!({ "message": "Not Found" })).is_empty());
    }
}
