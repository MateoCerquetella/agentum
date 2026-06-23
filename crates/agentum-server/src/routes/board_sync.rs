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
use crate::linear;
use crate::routes::forge::{ForgeKind, classify_remote, forge_get, forge_send, token_for};

/// Map a Linear client error (plain `String`) to an upstream 502, matching how
/// `forge_get`/`forge_send` surface forge failures.
fn linear_err(e: String) -> ApiError {
    ApiError::Custom(StatusCode::BAD_GATEWAY, json!({ "error": e }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board/bindings", post(create_binding).get(list_bindings))
        .route("/api/board/bindings/{id}", delete(delete_binding))
        .route("/api/board/sync", post(sync))
        .route("/api/board/{id}/push", post(push_card))
}

// ── Pure sync core (unit-tested; no I/O) ─────────────────────────────────────

/// A tracker issue normalized to the fields the board cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalIssue {
    /// Provider-native id (GitHub issue number / Linear identifier, as text).
    pub external_id: String,
    pub title: String,
    pub body: Option<String>,
    pub url: String,
    /// Board column the tracker's state maps to (`todo`/`doing`/`done`),
    /// resolved per-provider before reconcile so the diff is provider-agnostic.
    pub column: String,
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
fn reconcile_status(local: &str, column: &str) -> String {
    if column == "done" {
        "done".to_string()
    } else if local == "done" {
        // reopened upstream → take the tracker's column back
        column.to_string()
    } else {
        local.to_string()
    }
}

/// Diff incoming issues against the cards already mirroring this provider.
/// `existing` is `(card_id, external_id, local_status)`; each issue's `column`
/// is already mapped from its provider state.
pub(crate) fn reconcile(
    existing: &[(i64, String, String)],
    issues: &[ExternalIssue],
) -> Vec<SyncAction> {
    issues
        .iter()
        .map(|issue| match existing.iter().find(|(_, ext, _)| ext == &issue.external_id) {
            Some((card_id, _, local_status)) => SyncAction::Update {
                card_id: *card_id,
                status: reconcile_status(local_status, &issue.column),
                issue: issue.clone(),
            },
            None => SyncAction::Create {
                status: issue.column.clone(),
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
                .unwrap_or("open");
            Some(ExternalIssue {
                external_id: number.to_string(),
                title,
                body,
                url,
                column: state_to_status(state).to_string(),
            })
        })
        .collect()
}

/// Board column → tracker issue state for push-back (spec 014b). Inverse of
/// the pull-side `state_to_status`: only the `done` column closes an issue;
/// everything else (todo/doing/custom) keeps it open.
fn status_to_state(status: &str) -> &'static str {
    if status == "done" { "closed" } else { "open" }
}

/// Derive `owner/repo` from a GitHub issue URL
/// (`https://github.com/owner/repo/issues/123`). The card stores its issue
/// URL but not the repo, so push-back of a linked card recovers the repo from
/// here rather than guessing a binding.
fn parse_repo_from_issue_url(url: &str) -> Option<String> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    Some(format!("{owner}/{repo}"))
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
    let project = body.project.trim();
    match provider.as_str() {
        "github" => {
            if project.is_empty() || !project.contains('/') {
                return Err(ApiError::BadRequest(
                    "github project must be 'owner/repo'".into(),
                ));
            }
        }
        // Linear: project = a team key (e.g. ENG), or empty/"*" for the sole team.
        "linear" => {}
        other => {
            return Err(ApiError::BadRequest(format!(
                "provider '{other}' not supported (github | linear)"
            )));
        }
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
    // Fetch the tracker's issues, normalized to ExternalIssue (column pre-mapped
    // per provider so the reconcile below is provider-agnostic).
    let issues: Vec<ExternalIssue> = match binding.provider.as_str() {
        "github" => {
            let remote = classify_remote("github.com", binding.project.clone()).ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "could not build a github remote for {}",
                    binding.project
                ))
            })?;
            let token = token_for(ForgeKind::Github)?;
            // state=all so closed issues map to done. 100-cap; pagination is 014e.
            let url = format!(
                "{}/repos/{}/issues?state=all&per_page=100",
                remote.api_base, remote.project
            );
            parse_github_issues(&forge_get(&remote, &token, &url).await?)
        }
        "linear" => linear::pull_issues(&binding.project)
            .await
            .map_err(linear_err)?
            .into_iter()
            .map(|li| ExternalIssue {
                external_id: li.identifier,
                title: li.title,
                body: li.body,
                url: li.url,
                column: li.column,
            })
            .collect(),
        other => {
            return Err(ApiError::BadRequest(format!(
                "sync for provider '{other}' not supported"
            )));
        }
    };

    let existing = state.store.list_external_refs(&binding.provider).await?;
    let actions = reconcile(&existing, &issues);
    let synced_at = now_rfc3339()?;

    let (mut created, mut updated) = (0usize, 0usize);
    for action in actions {
        let (issue, status, is_create) = match action {
            SyncAction::Create { issue, status } => (issue, status, true),
            SyncAction::Update { issue, status, .. } => (issue, status, false),
        };
        state
            .store
            .upsert_external_card(
                &binding.provider,
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
        provider: binding.provider.clone(),
        project: binding.project.clone(),
        created,
        updated,
    })
}

// ── Push (board → tracker, spec 014b) ────────────────────────────────────────

#[derive(Deserialize, Default)]
struct PushBody {
    /// Target repo for a NATIVE card with no external ref yet (`owner/repo`).
    #[serde(default)]
    project: Option<String>,
    /// Or pick the target from an existing binding.
    #[serde(default)]
    binding_id: Option<i64>,
}

/// `POST /api/board/{id}/push` — write one card to GitHub (014b).
///
/// - linked card (has external ref) → PATCH the issue (title/body/state).
/// - native card → create an issue in the target repo, stamp the ref back
///   onto the card (stable identity, so the next pull updates not re-creates),
///   then close it if the card is already `done`.
///
/// Fail-loud: missing token/target/repo or a forge error aborts.
async fn push_card(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<PushBody>>,
) -> Result<Json<Value>, ApiError> {
    let card = state
        .store
        .get_board_item(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("card {id}")))?;

    match (card.external_provider.as_deref(), card.external_id.as_deref()) {
        // Linked GitHub card → PATCH the existing issue.
        (Some("github"), Some(number)) => {
            let token = token_for(ForgeKind::Github)?;
            let target_state = status_to_state(&card.status);
            let url = card.external_url.as_deref().unwrap_or_default();
            let project = parse_repo_from_issue_url(url).ok_or_else(|| {
                ApiError::BadRequest(format!("cannot derive owner/repo from issue url: {url}"))
            })?;
            let remote = classify_remote("github.com", project.clone())
                .ok_or_else(|| ApiError::BadRequest(format!("bad github project: {project}")))?;
            let api = format!("{}/repos/{}/issues/{}", remote.api_base, project, number);
            let payload = json!({ "title": card.title, "body": card.body, "state": target_state });
            forge_send(&remote, &token, reqwest::Method::PATCH, &api, &payload).await?;

            // Refresh the sync marker so the next pull treats it as reconciled.
            let synced = now_rfc3339()?;
            state
                .store
                .set_card_external_ref(id, "github", number, url, &synced)
                .await?;
            let _ = state.bus.send(Event::new("board.push.completed").with_payload(json!({
                "id": id, "action": "updated", "provider": "github",
                "project": project, "external_id": number, "state": target_state,
            })));
            Ok(Json(json!({
                "action": "updated", "provider": "github", "project": project,
                "external_id": number, "state": target_state,
            })))
        }
        // Linked Linear card → update title/body + transition workflow state.
        (Some("linear"), Some(identifier)) => {
            // The team key is the identifier prefix (ENG-42 → ENG).
            let project = identifier.split('-').next().unwrap_or_default().to_string();
            let body_str = card.body.as_deref().unwrap_or_default();
            linear::update_issue(&project, identifier, &card.title, body_str, &card.status)
                .await
                .map_err(linear_err)?;
            let synced = now_rfc3339()?;
            let url = card.external_url.as_deref().unwrap_or_default();
            state
                .store
                .set_card_external_ref(id, "linear", identifier, url, &synced)
                .await?;
            let payload = json!({
                "id": id, "action": "updated", "provider": "linear",
                "project": project, "external_id": identifier, "column": card.status,
            });
            let _ = state
                .bus
                .send(Event::new("board.push.completed").with_payload(payload.clone()));
            Ok(Json(payload))
        }
        // Any other external provider.
        (Some(p), _) => Err(ApiError::BadRequest(format!(
            "push for provider '{p}' not supported (github | linear)"
        ))),
        // Native card → create a new issue in the resolved target.
        (None, _) => {
            let (provider, project) = resolve_push_target(&state, body).await?;
            match provider.as_str() {
                "github" => {
                    let token = token_for(ForgeKind::Github)?;
                    let target_state = status_to_state(&card.status);
                    let remote = classify_remote("github.com", project.clone()).ok_or_else(|| {
                        ApiError::BadRequest(format!("bad github project: {project}"))
                    })?;
                    let create_url = format!("{}/repos/{}/issues", remote.api_base, project);
                    let payload = json!({ "title": card.title, "body": card.body });
                    let created =
                        forge_send(&remote, &token, reqwest::Method::POST, &create_url, &payload)
                            .await?;
                    let number = created.get("number").and_then(|n| n.as_i64()).ok_or_else(|| {
                        ApiError::Internal("github create issue: no number".into())
                    })?;
                    let html_url = created
                        .get("html_url")
                        .and_then(|u| u.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let synced = now_rfc3339()?;
                    state
                        .store
                        .set_card_external_ref(id, "github", &number.to_string(), &html_url, &synced)
                        .await?;
                    // A done card's freshly-created issue opens open — close to match.
                    if target_state == "closed" {
                        let close_url =
                            format!("{}/repos/{}/issues/{}", remote.api_base, project, number);
                        forge_send(
                            &remote,
                            &token,
                            reqwest::Method::PATCH,
                            &close_url,
                            &json!({ "state": "closed" }),
                        )
                        .await?;
                    }
                    let payload = json!({
                        "id": id, "action": "created", "provider": "github",
                        "project": project, "external_id": number.to_string(),
                        "url": html_url, "state": target_state,
                    });
                    let _ = state
                        .bus
                        .send(Event::new("board.push.completed").with_payload(payload.clone()));
                    Ok(Json(payload))
                }
                "linear" => {
                    let body_str = card.body.as_deref().unwrap_or_default();
                    let (identifier, url) = linear::create_issue(&project, &card.title, body_str)
                        .await
                        .map_err(linear_err)?;
                    let synced = now_rfc3339()?;
                    state
                        .store
                        .set_card_external_ref(id, "linear", &identifier, &url, &synced)
                        .await?;
                    // New Linear issues open in the team's default state; move a
                    // non-todo card to match its column.
                    if card.status != "todo" {
                        linear::update_issue(&project, &identifier, &card.title, body_str, &card.status)
                            .await
                            .map_err(linear_err)?;
                    }
                    let payload = json!({
                        "id": id, "action": "created", "provider": "linear",
                        "project": project, "external_id": identifier,
                        "url": url, "column": card.status,
                    });
                    let _ = state
                        .bus
                        .send(Event::new("board.push.completed").with_payload(payload.clone()));
                    Ok(Json(payload))
                }
                other => Err(ApiError::BadRequest(format!(
                    "push to provider '{other}' not supported"
                ))),
            }
        }
    }
}

/// Resolve which repo a native card pushes to: explicit `project`, then
/// `binding_id`, then the sole github binding. Ambiguity (0 or many) errors
/// rather than guessing.
async fn resolve_push_target(
    state: &AppState,
    body: Option<Json<PushBody>>,
) -> Result<(String, String), ApiError> {
    if let Some(Json(b)) = &body {
        if let Some(bid) = b.binding_id {
            return state
                .store
                .list_tracker_bindings()
                .await?
                .into_iter()
                .find(|x| x.id == bid)
                .map(|x| (x.provider, x.project))
                .ok_or_else(|| ApiError::NotFound(format!("tracker binding {bid}")));
        }
        if let Some(p) = b.project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            // Infer provider from shape: owner/repo → github, else Linear team key.
            let provider = if p.contains('/') { "github" } else { "linear" };
            return Ok((provider.to_string(), p.to_string()));
        }
    }
    let bindings = state.store.list_tracker_bindings().await?;
    match bindings.len() {
        1 => {
            let b = bindings.into_iter().next().unwrap();
            Ok((b.provider, b.project))
        }
        0 => Err(ApiError::BadRequest(
            "no tracker binding — specify project/binding_id or POST /api/board/bindings first"
                .into(),
        )),
        _ => Err(ApiError::BadRequest(
            "multiple bindings — specify binding_id or project".into(),
        )),
    }
}

fn now_rfc3339() -> Result<String, ApiError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str, title: &str, column: &str) -> ExternalIssue {
        ExternalIssue {
            external_id: id.into(),
            title: title.into(),
            body: None,
            url: format!("https://github.com/o/r/issues/{id}"),
            column: column.into(),
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
        // tracker column "done" → done regardless of the local column.
        assert_eq!(reconcile_status("doing", "done"), "done");
        // reopened (was done, tracker now non-terminal) → take the tracker column.
        assert_eq!(reconcile_status("done", "todo"), "todo");
        assert_eq!(reconcile_status("done", "doing"), "doing");
        // tracker still non-terminal → preserve a local in-progress move.
        assert_eq!(reconcile_status("doing", "todo"), "doing");
        assert_eq!(reconcile_status("todo", "todo"), "todo");
    }

    #[test]
    fn reconcile_creates_unseen_issue() {
        let actions = reconcile(&[], &[issue("12", "Add login", "todo")]);
        assert_eq!(
            actions,
            vec![SyncAction::Create {
                issue: issue("12", "Add login", "todo"),
                status: "todo".into(),
            }]
        );
    }

    #[test]
    fn reconcile_updates_known_issue_and_keeps_card_id() {
        let existing = vec![(7i64, "12".to_string(), "doing".to_string())];
        // tracker column "todo" (open) must NOT yank a locally in-progress card.
        let actions = reconcile(&existing, &[issue("12", "Add login (edited)", "todo")]);
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
        let actions = reconcile(&existing, &[issue("12", "Add login", "done")]);
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
        assert_eq!(issues[0].column, "todo", "open → todo column");
        assert_eq!(issues[1].external_id, "3");
        assert_eq!(issues[1].body, None, "empty body normalizes to None");
        assert_eq!(issues[1].column, "done", "closed → done column");
    }

    #[test]
    fn parse_github_issues_non_array_is_empty() {
        assert!(parse_github_issues(&json!({ "message": "Not Found" })).is_empty());
    }

    #[test]
    fn status_to_state_only_done_closes() {
        assert_eq!(status_to_state("done"), "closed");
        assert_eq!(status_to_state("todo"), "open");
        assert_eq!(status_to_state("doing"), "open");
        assert_eq!(status_to_state("review"), "open"); // custom column stays open
    }

    #[test]
    fn status_round_trips_with_pull_side() {
        // Push then pull must be stable: done→closed→done, todo→open→todo.
        assert_eq!(state_to_status(status_to_state("done")), "done");
        assert_eq!(state_to_status(status_to_state("todo")), "todo");
    }

    #[test]
    fn parse_repo_from_issue_url_extracts_owner_repo() {
        assert_eq!(
            parse_repo_from_issue_url("https://github.com/acme/api/issues/42").as_deref(),
            Some("acme/api")
        );
        assert_eq!(
            parse_repo_from_issue_url("https://github.com/o/r/issues/1").as_deref(),
            Some("o/r")
        );
    }

    #[test]
    fn parse_repo_from_issue_url_rejects_non_github_or_malformed() {
        assert!(parse_repo_from_issue_url("https://gitlab.com/o/r/issues/1").is_none());
        assert!(parse_repo_from_issue_url("https://github.com/onlyowner").is_none());
        assert!(parse_repo_from_issue_url("not a url").is_none());
    }
}
