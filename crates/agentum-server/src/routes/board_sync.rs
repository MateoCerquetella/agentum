//! `/api/board/bindings` + `/api/board/bindings/{id}/sync` — spec 016a:
//! server-side PULL of a bound GitHub repo's issues onto the board as cards.
//!
//! 016a is **one direction, one provider**: GitHub issues → board cards,
//! idempotent on re-sync (matched by `(external_provider, external_id)`).
//! Push-back (board → tracker) is 016b; Linear is 016c. We **reuse**
//! `routes::forge`'s REST + token plumbing rather than reimplement it.
//!
//! The shipped client-supplied mirror (#58) lives on `POST /api/board/sync`
//! with an `{items:[…]}` body and is owned by `routes::board` — this module
//! deliberately hangs the server pull off the binding **resource**
//! (`POST /api/board/bindings/{id}/sync`) so the two contracts can never clash.
//!
//! Self-hosted ⇒ no inbound webhooks, so sync is manual/poll: the client
//! triggers `POST /api/board/bindings/{id}/sync`.

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
use crate::routes::forge::{ForgeKind, Remote, classify_remote, forge_get, forge_send, token_for};
use crate::task_sink::TrackerPhase;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/board/bindings",
            post(create_binding).get(list_bindings),
        )
        .route("/api/board/bindings/{id}", delete(delete_binding))
        // The server PULL trigger. Distinct from #58's `POST /api/board/sync`.
        .route("/api/board/bindings/{id}/sync", post(sync_binding))
        // Board → tracker push-back (spec 016b), on top of 016a's bindings.
        .route("/api/board/{id}/push", post(push_card))
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
    /// Board column the tracker's state maps to (`todo`/`doing`/`done`),
    /// resolved per-provider before reconcile so the diff is provider-agnostic.
    pub column: String,
}

/// What a sync should do with one incoming issue, relative to the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncAction {
    Create {
        issue: ExternalIssue,
        status: String,
    },
    Update {
        card_id: i64,
        issue: ExternalIssue,
        status: String,
    },
}

/// Initial board column for a brand-new card from an issue's state.
fn state_to_status(state: &str) -> &'static str {
    if state.eq_ignore_ascii_case("closed") {
        "done"
    } else {
        "todo"
    }
}

/// Board column for an existing card on re-sync. The 016a conflict policy
/// (single source of this rule):
///
/// - closed upstream → `done` (tracker wins on close)
/// - open upstream, card already `done` → take the tracker column (reopened)
/// - open upstream otherwise → keep the local column (preserve a manual
///   `todo`→`doing` move; don't yank it back)
///
/// Two-sided conflict detection (both changed since last sync) is 016b; a pull
/// alone can't conflict with itself, so 016a reports only `{created, updated}`.
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
        .map(|issue| {
            match existing
                .iter()
                .find(|(_, ext, _)| ext == &issue.external_id)
            {
                Some((card_id, _, local_status)) => SyncAction::Update {
                    card_id: *card_id,
                    status: reconcile_status(local_status, &issue.column),
                    issue: issue.clone(),
                },
                None => SyncAction::Create {
                    status: issue.column.clone(),
                    issue: issue.clone(),
                },
            }
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
            let state = item.get("state").and_then(|s| s.as_str()).unwrap_or("open");
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

// ── Bindings CRUD ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct NewBindingBody {
    provider: String,
    project: String,
}

/// `POST /api/board/bindings` — create/idempotent-rebind a durable binding.
/// 016a accepts **github only** (Linear is 016c); the project must be
/// `owner/repo`.
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
        // Linear/GitLab pull is a later slice — reject here so 016a is strictly
        // GitHub. (The reference branch accepted `linear` too.)
        other => {
            return Err(ApiError::BadRequest(format!(
                "provider '{other}' not supported in 016a (github only)"
            )));
        }
    }
    let binding = state
        .store
        .create_tracker_binding(&provider, project)
        .await?;
    let _ = state
        .bus
        .send(Event::new("board.binding.created").with_payload(json!({
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

// ── Sync (server-side PULL) ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SyncResult {
    provider: String,
    project: String,
    created: usize,
    updated: usize,
}

/// `POST /api/board/bindings/{id}/sync` — pull the bound GitHub repo's issues
/// and upsert them as cards (idempotent, matched by external ref).
///
/// Crucial fails-loud ordering: **all network I/O completes before any store
/// write.** A missing token (`400`), an unknown binding (`404`), or a forge
/// error (`502`/`500`) short-circuits with `?` *before* the upsert loop — so a
/// failed sync makes zero board changes (AC). GitHub only in 016a.
async fn sync_binding(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    // Resolve the binding by path id; unknown → 404, no writes.
    let binding = state
        .store
        .list_tracker_bindings()
        .await?
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("tracker binding {id}")))?;

    let result = sync_one(&state, &binding).await?;

    let _ = state
        .bus
        .send(Event::new("board.sync.completed").with_payload(json!({
            "provider": result.provider, "project": result.project,
            "created": result.created, "updated": result.updated,
        })));
    Ok(Json(json!({
        "provider": result.provider, "project": result.project,
        "created": result.created, "updated": result.updated,
    })))
}

/// Pull one binding's issues and upsert them. GitHub only in 016a.
async fn sync_one(state: &AppState, binding: &TrackerBinding) -> Result<SyncResult, ApiError> {
    // Fetch the tracker's issues, normalized to ExternalIssue (column pre-mapped
    // per provider so the reconcile below is provider-agnostic).
    let issues: Vec<ExternalIssue> = match binding.provider.as_str() {
        "github" => {
            let remote =
                classify_remote("github.com", binding.project.clone()).ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "could not build a github remote for {}",
                        binding.project
                    ))
                })?;
            // Fail loud: a missing token aborts here (400) before any write.
            let token = token_for(ForgeKind::Github)?;
            // state=all so closed issues map to done. 100-cap; pagination deferred.
            let url = format!(
                "{}/repos/{}/issues?state=all&per_page=100",
                remote.api_base, remote.project
            );
            // forge_get surfaces a non-2xx forge response as a 502 ApiError and a
            // transport error as a 500 — either short-circuits before the upsert.
            parse_github_issues(&forge_get(&remote, &token, &url).await?)
        }
        other => {
            return Err(ApiError::BadRequest(format!(
                "sync for provider '{other}' not supported in 016a (github only)"
            )));
        }
    };

    // All network I/O is done. Only now do we touch the store.
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

fn now_rfc3339() -> Result<String, ApiError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| ApiError::Internal(e.to_string()))
}

// ── Push (board → tracker, spec 016b) ────────────────────────────────────────
// Layered on 016a's bindings: a linked card updates its issue; a native card
// creates one in the resolved binding and gets its external link stamped.
// Reuses `forge` (GitHub REST) + `crate::linear` (transition_issue/create_issue)
// — no parallel tracker client, no second bindings route.

/// Board column → Linear pipeline phase, reusing the 012
/// `transition_issue`/`LinearStateMap` machinery instead of a new mapping.
fn column_to_phase(column: &str) -> TrackerPhase {
    match column {
        "done" => TrackerPhase::Done,
        "doing" => TrackerPhase::InProgress,
        "review" => TrackerPhase::ReadyToTest,
        _ => TrackerPhase::Todo,
    }
}

/// GitHub issue state for a board column (only `done` closes).
fn github_state(column: &str) -> &'static str {
    if column == "done" { "closed" } else { "open" }
}

/// `owner/repo` + issue number from a GitHub issue URL.
fn parse_github_issue(url: &str) -> Option<(String, i64)> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    if parts.next()? != "issues" {
        return None;
    }
    let num: i64 = parts.next()?.parse().ok()?;
    Some((format!("{owner}/{repo}"), num))
}

/// Linear issue identifier (e.g. `ENG-42`) from a Linear issue URL.
fn parse_linear_identifier(url: &str) -> Option<String> {
    let rest = url.split("/issue/").nth(1)?;
    rest.split('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Best-effort provider guess from a link, when `external_provider` is unset.
fn infer_provider_from_url(url: &str) -> &'static str {
    if url.contains("github.com") {
        "github"
    } else if url.contains("linear.app") {
        "linear"
    } else {
        ""
    }
}

fn github_remote(repo: &str) -> Result<Remote, ApiError> {
    classify_remote("github.com", repo.to_string())
        .ok_or_else(|| ApiError::BadRequest(format!("bad github project: {repo}")))
}

/// Map a Linear (`anyhow`) error to an upstream 502, like forge failures.
fn linear_err(e: anyhow::Error) -> ApiError {
    ApiError::Custom(StatusCode::BAD_GATEWAY, json!({ "error": e.to_string() }))
}

fn emit_push(state: &AppState, payload: Value) -> Json<Value> {
    let _ = state
        .bus
        .send(Event::new("board.push.completed").with_payload(payload.clone()));
    Json(payload)
}

#[derive(Deserialize, Default)]
struct PushBody {
    /// Target for a NATIVE card with no link yet.
    #[serde(default)]
    binding_id: Option<i64>,
    #[serde(default)]
    project: Option<String>,
}

/// `POST /api/board/{id}/push` — write a card to its tracker.
/// Linked card → update the existing issue; native card → create one in the
/// resolved binding and stamp its external link. Fail-loud on token/forge/linear.
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
    let column = card.status.clone();
    let body_text = card.body.clone().unwrap_or_default();

    match card.external_url.as_deref() {
        // Linked card → update the existing issue.
        Some(url) => {
            let provider = match card.external_provider.as_deref() {
                Some(p) if !p.trim().is_empty() => p.trim().to_ascii_lowercase(),
                _ => infer_provider_from_url(url).to_string(),
            };
            match provider.as_str() {
                "github" => {
                    let (repo, num) = parse_github_issue(url).ok_or_else(|| {
                        ApiError::BadRequest(format!("cannot parse github issue url: {url}"))
                    })?;
                    let token = token_for(ForgeKind::Github)?;
                    let remote = github_remote(&repo)?;
                    let api = format!("{}/repos/{}/issues/{}", remote.api_base, repo, num);
                    forge_send(
                        &remote,
                        &token,
                        reqwest::Method::PATCH,
                        &api,
                        &json!({ "title": card.title, "body": body_text, "state": github_state(&column) }),
                    )
                    .await?;
                    Ok(emit_push(
                        &state,
                        json!({
                            "id": id, "action": "updated", "provider": "github",
                            "external_url": url, "state": github_state(&column),
                        }),
                    ))
                }
                "linear" => {
                    let ident = parse_linear_identifier(url).ok_or_else(|| {
                        ApiError::BadRequest(format!("cannot parse linear identifier: {url}"))
                    })?;
                    let outcome = linear::transition_issue(
                        &ident,
                        column_to_phase(&column),
                        &linear::LinearStateMap::from_env(),
                    )
                    .await
                    .map_err(linear_err)?;
                    Ok(emit_push(
                        &state,
                        json!({
                            "id": id, "action": "updated", "provider": "linear",
                            "external_url": url, "identifier": ident,
                            "outcome": format!("{outcome:?}"),
                        }),
                    ))
                }
                other => Err(ApiError::BadRequest(format!(
                    "push for provider '{other}' not supported (github | linear)"
                ))),
            }
        }
        // Native card → create an issue in the resolved target, stamp the link.
        None => {
            let (provider, project) = resolve_target(&state, body).await?;
            match provider.as_str() {
                "github" => {
                    let token = token_for(ForgeKind::Github)?;
                    let remote = github_remote(&project)?;
                    let created = forge_send(
                        &remote,
                        &token,
                        reqwest::Method::POST,
                        &format!("{}/repos/{}/issues", remote.api_base, project),
                        &json!({ "title": card.title, "body": body_text }),
                    )
                    .await?;
                    let html_url = created
                        .get("html_url")
                        .and_then(|u| u.as_str())
                        .ok_or_else(|| {
                            ApiError::Internal("github create issue: no html_url".into())
                        })?
                        .to_string();
                    state
                        .store
                        .set_card_external_link(id, &html_url, "github")
                        .await?;
                    // A done card's freshly-created (open) issue → close to match.
                    if github_state(&column) == "closed" {
                        if let Some(n) = created.get("number").and_then(|n| n.as_i64()) {
                            let api = format!("{}/repos/{}/issues/{}", remote.api_base, project, n);
                            forge_send(
                                &remote,
                                &token,
                                reqwest::Method::PATCH,
                                &api,
                                &json!({ "state": "closed" }),
                            )
                            .await?;
                        }
                    }
                    Ok(emit_push(
                        &state,
                        json!({
                            "id": id, "action": "created", "provider": "github",
                            "project": project, "external_url": html_url,
                        }),
                    ))
                }
                "linear" => {
                    let (ident, url_opt) = linear::create_issue(&card.title, &body_text)
                        .await
                        .map_err(linear_err)?;
                    if let Some(url) = url_opt.as_deref() {
                        state
                            .store
                            .set_card_external_link(id, url, "linear")
                            .await?;
                    }
                    if column != "todo" && !column.is_empty() {
                        linear::transition_issue(
                            &ident,
                            column_to_phase(&column),
                            &linear::LinearStateMap::from_env(),
                        )
                        .await
                        .map_err(linear_err)?;
                    }
                    Ok(emit_push(
                        &state,
                        json!({
                            "id": id, "action": "created", "provider": "linear",
                            "project": project, "identifier": ident, "external_url": url_opt,
                        }),
                    ))
                }
                other => Err(ApiError::BadRequest(format!(
                    "push to provider '{other}' not supported (github | linear)"
                ))),
            }
        }
    }
}

/// Resolve a native card's push target: explicit `binding_id`, then `project`
/// (shape-inferred provider), then the sole binding. Ambiguity errors.
async fn resolve_target(
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
        if let Some(p) = b
            .project
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
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

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_store::Store;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn issue(id: &str, title: &str, column: &str) -> ExternalIssue {
        ExternalIssue {
            external_id: id.into(),
            title: title.into(),
            body: None,
            url: format!("https://github.com/o/r/issues/{id}"),
            column: column.into(),
        }
    }

    // ── Push-back (016b) pure-function tests ─────────────────────────────────
    #[test]
    fn github_state_only_done_closes() {
        assert_eq!(github_state("done"), "closed");
        assert_eq!(github_state("todo"), "open");
        assert_eq!(github_state("doing"), "open");
    }

    #[test]
    fn column_to_phase_maps_board_columns() {
        assert!(matches!(column_to_phase("done"), TrackerPhase::Done));
        assert!(matches!(column_to_phase("doing"), TrackerPhase::InProgress));
        assert!(matches!(
            column_to_phase("review"),
            TrackerPhase::ReadyToTest
        ));
        assert!(matches!(column_to_phase("todo"), TrackerPhase::Todo));
        assert!(matches!(column_to_phase("anything"), TrackerPhase::Todo));
    }

    #[test]
    fn parse_github_issue_extracts_repo_and_number() {
        assert_eq!(
            parse_github_issue("https://github.com/acme/api/issues/42"),
            Some(("acme/api".to_string(), 42))
        );
        assert_eq!(
            parse_github_issue("https://github.com/acme/api/pull/42"),
            None
        );
        assert_eq!(parse_github_issue("https://gitlab.com/o/r/issues/1"), None);
        assert_eq!(parse_github_issue("nope"), None);
    }

    #[test]
    fn parse_linear_identifier_extracts_ident() {
        assert_eq!(
            parse_linear_identifier("https://linear.app/acme/issue/ENG-42/add-login").as_deref(),
            Some("ENG-42")
        );
        assert_eq!(
            parse_linear_identifier("https://github.com/o/r/issues/1"),
            None
        );
    }

    #[test]
    fn infer_provider_from_url_detects_host() {
        assert_eq!(
            infer_provider_from_url("https://github.com/o/r/issues/1"),
            "github"
        );
        assert_eq!(
            infer_provider_from_url("https://linear.app/x/issue/E-1"),
            "linear"
        );
        assert_eq!(infer_provider_from_url("https://example.com/x"), "");
    }

    // ── Pure-function unit tests (ported verbatim from the reference) ─────────

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
            SyncAction::Update {
                card_id,
                status,
                issue,
            } => {
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

    // ── Integration tests (handler-level, offline) ───────────────────────────

    async fn fresh_state() -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir); // keep the tempdir alive for the test
        let store = Store::open(&p).await.unwrap();
        let (bus, _rx) = broadcast::channel(16);
        AppState {
            store: Arc::new(store),
            bus,
            started_at: std::time::Instant::now(),
            version: "test",
            auth_limiter: Arc::new(crate::ratelimit::RateLimiter::new(
                8,
                std::time::Duration::from_secs(60),
            )),
            cert_fingerprint: Arc::new(String::new()),
            transcripts: crate::TranscriptStore::new(broadcast::channel(16).0),
            stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            wiki_keys: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: "test".to_string(),
            no_auth: true,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus: broadcast::channel(64).0,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            mcp_token: Arc::new(String::from("test-mcp-token")),
            api_base_url: None,
            desktop_bridge: None,
            harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
            sdd_loops: Default::default(),
            events_ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// AC: binding creation accepts github `owner/repo`, idempotently rebinds,
    /// and rejects a non-github provider (016a is GitHub-only).
    #[tokio::test]
    async fn create_binding_accepts_github_and_rejects_others() {
        let state = fresh_state().await;
        let (code, b) = create_binding(
            State(state.clone()),
            Json(NewBindingBody {
                provider: "github".into(),
                project: "o/r".into(),
            }),
        )
        .await
        .expect("github owner/repo binding must be accepted");
        assert_eq!(code, StatusCode::CREATED);
        assert_eq!(b.0.provider, "github");
        assert_eq!(b.0.project, "o/r");

        // Re-bind same repo → same row id (idempotent).
        let (_c2, b2) = create_binding(
            State(state.clone()),
            Json(NewBindingBody {
                provider: "github".into(),
                project: "o/r".into(),
            }),
        )
        .await
        .expect("idempotent rebind");
        assert_eq!(b2.0.id, b.0.id);

        // Bad github project (no slash) → 400.
        assert!(
            create_binding(
                State(state.clone()),
                Json(NewBindingBody {
                    provider: "github".into(),
                    project: "no-slash".into(),
                }),
            )
            .await
            .is_err(),
            "github project without owner/repo must be rejected"
        );

        // Non-github provider → 400 (Linear/GitLab are later slices).
        assert!(
            create_binding(
                State(state.clone()),
                Json(NewBindingBody {
                    provider: "linear".into(),
                    project: "ENG".into(),
                }),
            )
            .await
            .is_err(),
            "non-github provider must be rejected in 016a"
        );
    }

    /// **AC (fails-loud → zero mutation):** a sync against an unreachable /
    /// no-token GitHub returns a non-success status AND leaves the board card
    /// count + contents unchanged. We force the no-token path deterministically
    /// (and offline) by pointing the forge-token store at an empty `AGENTUM_HOME`.
    // The env guard MUST span the awaits: `AGENTUM_HOME` is set for the whole
    // body (and restored at the end under the same guard), and `sync_binding`'s
    // `token_for(Github)` reads it mid-flight. `TEST_ENV_LOCK` is a crate-wide
    // `std::sync::Mutex` shared with non-async env tests, so an async mutex would
    // break that cross-test serialization — holding it across `.await` is correct.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn sync_with_no_token_is_non_success_and_writes_nothing() {
        // Share the crate-wide env lock so this serialises against the other
        // AGENTUM_HOME-mutating tests (profiles / planner / board_goals) in the
        // same `--lib` binary — a per-module lock would not, and would race the
        // planner-config read in board_goals::tests.
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("AGENTUM_HOME");
        let empty_home = tempfile::tempdir().unwrap();
        // SAFETY: env access is serialized by ENV_LOCK for the env-touching tests.
        unsafe {
            std::env::set_var("AGENTUM_HOME", empty_home.path());
        }

        let state = fresh_state().await;

        // Seed one pre-existing native card so we can prove the failed sync
        // doesn't add, remove, or mutate any board rows.
        let seeded = state
            .store
            .create_board_item(agentum_core::NewBoardItem {
                title: "pre-existing card".into(),
                body: Some("untouched".into()),
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        let before = state.store.list_board_items().await.unwrap();
        assert_eq!(before.len(), 1);

        // Bind a github repo, then sync — the empty token store makes
        // token_for(Github) fail BEFORE any forge call or store write.
        let (_c, binding) = create_binding(
            State(state.clone()),
            Json(NewBindingBody {
                provider: "github".into(),
                project: "o/r".into(),
            }),
        )
        .await
        .unwrap();

        let result = sync_binding(State(state.clone()), Path(binding.0.id)).await;
        assert!(
            result.is_err(),
            "sync with no token must return a non-success result"
        );

        // Zero board mutation: same count, same single card, contents intact.
        let after = state.store.list_board_items().await.unwrap();
        assert_eq!(after.len(), 1, "failed sync must not change the card count");
        assert_eq!(after[0].id, seeded.id, "the pre-existing card is untouched");
        assert_eq!(after[0].title, "pre-existing card");
        assert_eq!(after[0].body.as_deref(), Some("untouched"));
        assert_eq!(after[0].status, "todo");
        // No external card was created.
        assert!(
            state
                .store
                .list_external_refs("github")
                .await
                .unwrap()
                .is_empty(),
            "failed sync must not create an external card"
        );

        // Restore the prior env so other tests are unaffected.
        // SAFETY: still under ENV_LOCK.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("AGENTUM_HOME", v),
                None => std::env::remove_var("AGENTUM_HOME"),
            }
        }
    }

    /// Sync against an unknown binding id is a 404 and writes nothing.
    #[tokio::test]
    async fn sync_unknown_binding_is_not_found() {
        let state = fresh_state().await;
        let result = sync_binding(State(state.clone()), Path(999_999)).await;
        assert!(result.is_err(), "unknown binding id must 404");
        assert!(
            state.store.list_board_items().await.unwrap().is_empty(),
            "an unknown-binding sync must not write any board rows"
        );
    }
}
