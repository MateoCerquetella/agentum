//! `/api/board` — kanban CRUD + atomic CAS claim + comments + reorder.

use std::collections::BTreeMap;

use agentum_core::{
    BoardComment, BoardItem, BoardPatch, ClaimRequest, Event, NewBoardComment, NewBoardItem,
    ReorderEntry, TransitionCtx,
};
use agentum_store::Store;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board", get(list).post(create))
        // /reorder must come before /{id} so axum doesn't route the
        // bare path through the id-extractor and 400 on "reorder".
        .route("/api/board/reorder", post(reorder))
        .route("/api/board/{id}", get(get_one).patch(patch).delete(delete))
        .route("/api/board/{id}/claim", post(claim))
        .route("/api/board/{id}/release", post(release))
        .route(
            "/api/board/{id}/comments",
            get(list_comments).post(create_comment),
        )
}

#[derive(Serialize)]
struct GroupedBoard {
    /// Status name → items, ordered by priority ASC, created_at ASC.
    columns: BTreeMap<String, Vec<BoardItem>>,
    /// Distinct columns we know about — guarantees todo/doing/done are
    /// present even when empty.
    column_order: Vec<String>,
    /// Per-ticket comment count keyed by board id. Sent alongside the
    /// items so the card-foot 💬N chip doesn't need a second round-trip.
    /// Missing ids implicitly mean zero comments.
    comment_counts: std::collections::HashMap<i64, i64>,
}

const DEFAULT_COLUMNS: &[&str] = &["todo", "doing", "done"];

async fn list(State(state): State<AppState>) -> Result<Json<GroupedBoard>, ApiError> {
    let items = state.store.list_board_items().await?;
    let mut columns: BTreeMap<String, Vec<BoardItem>> = BTreeMap::new();
    for col in DEFAULT_COLUMNS {
        columns.insert((*col).to_string(), Vec::new());
    }
    for it in items {
        columns.entry(it.status.clone()).or_default().push(it);
    }
    let mut order: Vec<String> = DEFAULT_COLUMNS.iter().map(|s| s.to_string()).collect();
    for k in columns.keys() {
        if !order.iter().any(|o| o == k) {
            order.push(k.clone());
        }
    }
    let comment_counts = state.store.count_board_comments().await?;
    Ok(Json(GroupedBoard {
        columns,
        column_order: order,
        comment_counts,
    }))
}

/// Validate a (POST-or-PATCH) transition against the per-status required
/// field matrix in `agentum_core::board_schema`. Returns a 400 with body
/// `{"missing": [...], "status": target}` on gate failure; emits
/// `board.transition.rejected` before returning.
///
/// The handler builds the `TransitionCtx` (POST: from the payload alone;
/// PATCH: by merging the patch's set fields over the existing row). The
/// `done` transition is the only case that needs the comments-existence
/// check — we skip it for every other target so non-`done` patches never
/// touch `board_comments`.
///
/// Returns `ApiError::Custom(400, {missing, status})` on failure rather
/// than `ApiError::BadRequest(String)` because the spec (Decision 5) pins
/// the failure body to `{"missing": [...], "status": "..."}`, not the
/// default `{"error": "..."}` envelope. The `Custom` variant exists for
/// exactly this case — see `crate::error` for the rationale.
async fn enforce_transition(
    store: &Store,
    bus: &broadcast::Sender<Event>,
    id: Option<i64>,
    target_status: &str,
    ctx: &mut TransitionCtx<'_>,
) -> Result<(), ApiError> {
    // Resolve required-fields once per gate evaluation. Slice 2: DB
    // override (if any) replaces the const matrix completely; otherwise
    // we get the slice-1 default. Cow keeps the const path alloc-free.
    let required = crate::rules::resolve_required_fields(store, target_status).await?;

    if target_status == "done" {
        // Resolve the OR-clause: either `session_id` is set (the agent
        // session that produced this), or the parent row has at least
        // one comment (the manual-close path).
        if let Some(real_id) = id {
            ctx.has_comment = store.has_board_comments(real_id).await?;
        }
    }

    match agentum_core::validate_against(&required, ctx) {
        Ok(()) => Ok(()),
        Err(missing) => {
            // Single source of truth for the event payload — emit before
            // returning so listeners see every rejection regardless of
            // whether the handler bails immediately or does further work.
            // POST has no id yet (validation runs before insert), so
            // emit `null` to keep the consumer's branch-free.
            let id_payload = match id {
                Some(real_id) => serde_json::Value::from(real_id),
                None => serde_json::Value::Null,
            };
            let _ = bus.send(Event::new("board.transition.rejected").with_payload(json!({
                "id": id_payload,
                "target_status": target_status,
                "missing": missing,
            })));
            Err(ApiError::Custom(
                StatusCode::BAD_REQUEST,
                json!({ "missing": missing, "status": target_status }),
            ))
        }
    }
}

async fn create(
    State(state): State<AppState>,
    Json(payload): Json<NewBoardItem>,
) -> Result<(StatusCode, Json<BoardItem>), ApiError> {
    // Validate the target status before any work happens. POST has no
    // row yet, so the ctx is built straight from the payload. Per the
    // store, an absent status defaults to `"todo"` (see
    // `create_board_item`), so the gate must check against the same
    // default — otherwise the dashboard could omit `status` to dodge the
    // gate.
    let target_status = payload.status.as_deref().unwrap_or("todo");
    let mut ctx = TransitionCtx {
        title: Some(payload.title.as_str()),
        lbl: payload.lbl.as_deref(),
        workdir: payload.workdir.as_deref(),
        tool: payload.tool.as_deref(),
        // POSTs never carry `claimed_by` on the payload (it's not a
        // field of NewBoardItem — there's a dedicated /claim endpoint),
        // so this is structurally always None on create.
        claimed_by: None,
        session_id: payload.session_id.as_deref(),
        has_comment: false,
    };
    enforce_transition(&state.store, &state.bus, None, target_status, &mut ctx).await?;

    let item = state.store.create_board_item(payload).await?;
    let _ = state.bus.send(
        Event::new("board.created")
            .with_payload(json!({"id": item.id, "key": item.key, "title": item.title})),
    );
    Ok((StatusCode::CREATED, Json(item)))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<BoardItem>, ApiError> {
    let item = state
        .store
        .get_board_item(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("board item {id}")))?;
    Ok(Json(item))
}

async fn patch(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(patch): Json<BoardPatch>,
) -> Result<Json<BoardItem>, ApiError> {
    // Validation gate runs FIRST — before any mutation — and only when
    // the PATCH actually changes the status. PATCHes that omit `status`
    // or set it to the same value the row already holds skip the gate
    // entirely. This is the grandfathering invariant: existing rows in
    // a gated column without all required fields stay editable, and the
    // gate only fires on transitions.
    let current = state
        .store
        .get_board_item(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("board item {id}")))?;

    let status_changing = match patch.status.as_deref() {
        Some(target) => target != current.status,
        None => false,
    };

    if status_changing {
        let target_status = patch.status.as_deref().unwrap_or(&current.status);
        // Merge: incoming patch wins for any `Some(_)` field; otherwise
        // fall back to the existing row. `BoardPatch` uses the
        // double-Option pattern to distinguish "field omitted" from
        // "explicitly null"; an explicit null counts as the user
        // clearing the field, which means the gate should treat it as
        // absent. `as_deref().flatten()` collapses both layers to
        // `Option<&str>` and the validator's `is_set` filters empties.
        let merged_lbl = match patch.lbl.as_ref() {
            Some(v) => v.as_deref(),
            None => current.lbl.as_deref(),
        };
        let merged_tool = match patch.tool.as_ref() {
            Some(v) => v.as_deref(),
            None => current.tool.as_deref(),
        };
        let merged_workdir = match patch.workdir.as_ref() {
            Some(v) => v.as_deref(),
            None => current.workdir.as_deref(),
        };
        let merged_session_id = match patch.session_id.as_ref() {
            Some(v) => v.as_deref(),
            None => current.session_id.as_deref(),
        };
        let merged_title = match patch.title.as_deref() {
            Some(t) => t,
            None => current.title.as_str(),
        };

        let mut ctx = TransitionCtx {
            title: Some(merged_title),
            lbl: merged_lbl,
            workdir: merged_workdir,
            tool: merged_tool,
            // `claimed_by` is owned by /claim + /release, not by PATCH —
            // there's no field on `BoardPatch` for it. The existing row's
            // value is the only source.
            claimed_by: current.claimed_by.as_deref(),
            session_id: merged_session_id,
            has_comment: false,
        };
        enforce_transition(&state.store, &state.bus, Some(id), target_status, &mut ctx).await?;
    }

    let item = state.store.patch_board_item(id, patch).await?;
    let _ = state.bus.send(
        Event::new("board.updated")
            .with_payload(json!({"id": item.id, "key": item.key, "status": item.status})),
    );
    Ok(Json(item))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_board_item(id).await?;
    let _ = state
        .bus
        .send(Event::new("board.deleted").with_payload(json!({"id": id})));
    Ok(StatusCode::NO_CONTENT)
}

async fn claim(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ClaimRequest>,
) -> Result<Json<BoardItem>, ApiError> {
    if req.claimed_by.trim().is_empty() {
        return Err(ApiError::BadRequest("claimed_by must be non-empty".into()));
    }
    match state
        .store
        .claim_board_item(id, req.claimed_by.trim())
        .await?
    {
        Some(item) => {
            let _ = state
                .bus
                .send(Event::new("board.claimed").with_payload(json!({
                    "id": item.id,
                    "key": item.key,
                    "claimed_by": item.claimed_by,
                })));
            Ok(Json(item))
        }
        None => Err(ApiError::Conflict(format!(
            "board item {id} is already claimed"
        ))),
    }
}

/// Symmetric to /claim. Empty `claimed_by` is admin-override (anyone can
/// release); a non-empty value enforces the same actor that holds the
/// claim — anyone else gets 409.
async fn release(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ClaimRequest>,
) -> Result<Json<BoardItem>, ApiError> {
    let actor = req.claimed_by.trim();
    match state.store.release_board_item(id, actor).await? {
        Some(item) => {
            let _ = state.bus.send(
                Event::new("board.released").with_payload(json!({"id": item.id, "key": item.key})),
            );
            Ok(Json(item))
        }
        None => Err(ApiError::Conflict(format!(
            "board item {id} is held by a different actor"
        ))),
    }
}

#[derive(Debug, Deserialize)]
struct ReorderRequest {
    entries: Vec<ReorderEntry>,
}

/// Batch priority rewrite — used by the dashboard's drag-to-reorder
/// drop. The transaction in the store keeps the affected column from
/// flashing into an inconsistent half-state mid-write.
async fn reorder(
    State(state): State<AppState>,
    Json(req): Json<ReorderRequest>,
) -> Result<StatusCode, ApiError> {
    if req.entries.is_empty() {
        return Err(ApiError::BadRequest("entries must be non-empty".into()));
    }
    state.store.reorder_board_items(&req.entries).await?;
    let ids: Vec<i64> = req.entries.iter().map(|e| e.id).collect();
    let _ = state
        .bus
        .send(Event::new("board.reordered").with_payload(json!({"ids": ids})));
    Ok(StatusCode::NO_CONTENT)
}

async fn list_comments(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<BoardComment>>, ApiError> {
    let comments = state.store.list_board_comments(id).await?;
    Ok(Json(comments))
}

async fn create_comment(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<NewBoardComment>,
) -> Result<(StatusCode, Json<BoardComment>), ApiError> {
    if req.author.trim().is_empty() {
        return Err(ApiError::BadRequest("author must be non-empty".into()));
    }
    if req.body.trim().is_empty() {
        return Err(ApiError::BadRequest("body must be non-empty".into()));
    }
    let comment = state.store.create_board_comment(id, req).await?;
    let _ = state
        .bus
        .send(Event::new("board.commented").with_payload(json!({
            "board_id": comment.board_id,
            "comment_id": comment.id,
            "author": comment.author,
        })));
    Ok((StatusCode::CREATED, Json(comment)))
}

/// Test-only re-export of `create` so sibling test modules
/// (`routes::board_rules::tests`) can exercise the full POST handler —
/// including the gate — without duplicating the handler boilerplate.
/// `pub(crate)` so it stays private to the server crate.
#[cfg(test)]
pub(crate) async fn tests_helpers_create(
    state: AppState,
    payload: NewBoardItem,
) -> Result<(StatusCode, Json<BoardItem>), ApiError> {
    create(State(state), Json(payload)).await
}

/// Test-only re-export of `patch` — see `tests_helpers_create`.
#[cfg(test)]
pub(crate) async fn tests_helpers_patch(
    state: AppState,
    id: i64,
    patch_body: BoardPatch,
) -> Result<Json<BoardItem>, ApiError> {
    patch(State(state), Path(id), Json(patch_body)).await
}

#[cfg(test)]
mod tests {
    //! Handler-level tests for the per-status transition gate. These
    //! exercise `enforce_transition` indirectly via `create` and `patch`
    //! so the merge-patch-over-current ctx-building boilerplate gets
    //! covered, not just the validator in `agentum-core`.

    use super::*;
    use agentum_store::Store;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    async fn fresh_state() -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir); // keep tempdir alive for the test
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
            hostname: "test".to_string(),
            no_auth: true,
        }
    }

    /// Render the `ApiError` through its `IntoResponse` impl and drain the
    /// body — convenient for asserting on the gate's 400 status + payload
    /// shape from a single helper.
    async fn err_status_and_body(err: ApiError) -> (StatusCode, serde_json::Value) {
        let resp = err.into_response();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn doing_pass_payload() -> NewBoardItem {
        NewBoardItem {
            title: "ship it".into(),
            body: None,
            status: Some("doing".into()),
            lbl: Some("feat".into()),
            tool: Some("claude".into()),
            workdir: Some("/home/me/foo".into()),
            model: None,
            session_id: None,
            priority: None,
            parent_goal_id: None,
        }
    }

    #[tokio::test]
    async fn create_into_gated_passes() {
        // POST with every required field present for the target column.
        let state = fresh_state().await;
        let (code, json) = create(
            State(state.clone()),
            Json(NewBoardItem {
                // claimed_by isn't a field on NewBoardItem (the dedicated
                // /claim endpoint owns it) — but the gate still demands
                // it on `doing`. So this POST against `doing` should
                // actually FAIL the gate. We use `todo` instead to
                // exercise the create-into-gated-pass path that the
                // dashboard's "new ticket" button realistically hits.
                title: "todo entry".into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            }),
        )
        .await
        .expect("gate should pass on full todo payload");
        assert_eq!(code, StatusCode::CREATED);
        assert_eq!(json.0.status, "todo");
        assert_eq!(json.0.lbl.as_deref(), Some("feat"));
    }

    #[tokio::test]
    async fn create_into_gated_fails_with_missing_list() {
        // POST with `status: "doing"` and several required fields empty
        // should return 400 with the exact missing[] shape the AC
        // pins down: ["workdir", "tool", "claimed_by"].
        let state = fresh_state().await;
        let err = create(
            State(state.clone()),
            Json(NewBoardItem {
                title: "no workdir".into(),
                body: None,
                status: Some("doing".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            }),
        )
        .await
        .expect_err("gate should reject doing without workdir/tool/claimed_by");
        // Convert the typed ApiError to its response form to assert on the
        // wire shape the spec pins down.
        let (status, body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], "doing");
        assert_eq!(
            body["missing"],
            serde_json::json!(["workdir", "tool", "claimed_by"])
        );
    }

    #[tokio::test]
    async fn create_with_default_status_uses_todo() {
        // No `status` in the payload defaults to `todo` per
        // create_board_item. The gate must match that default — otherwise
        // a client could elide `status` to dodge validation.
        let state = fresh_state().await;
        let err = create(
            State(state.clone()),
            Json(NewBoardItem {
                title: "needs lbl".into(),
                body: None,
                status: None,
                lbl: None,
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            }),
        )
        .await
        .expect_err("default `todo` target still demands lbl");
        let (status, body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], "todo");
        assert_eq!(body["missing"], serde_json::json!(["lbl"]));
    }

    #[tokio::test]
    async fn patch_into_gated_fails() {
        // Seed a `todo` row with only the todo-required fields, then try
        // to PATCH it into `doing` without supplying workdir/tool. The
        // gate sees the merged ctx (todo's existing lbl + doing's empty
        // workdir/tool/claimed_by) and rejects.
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "seed".into(),
                body: None,
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

        let err = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("doing".into()),
                ..Default::default()
            }),
        )
        .await
        .expect_err("doing transition without workdir/tool/claimed_by must fail");
        let (status, body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], "doing");
        assert_eq!(
            body["missing"],
            serde_json::json!(["workdir", "tool", "claimed_by"])
        );
    }

    #[tokio::test]
    async fn patch_out_of_gated_passes() {
        // Transitions *out of* a gated column never re-validate — the
        // user is allowed to move a partially-filled card back to `todo`
        // without satisfying the previous column's requirements.
        let state = fresh_state().await;
        // Seed a row directly in `doing` with workdir/tool/claimed_by
        // populated (bypassing the gate — the store doesn't run it on
        // direct writes; that's fine for setting up legacy state).
        let seed = state
            .store
            .create_board_item(doing_pass_payload())
            .await
            .unwrap();
        // Stamp claimed_by via the dedicated path so the row is fully
        // "in doing" from the gate's POV; not strictly needed for this
        // test but mirrors the real lifecycle.
        let _ = state
            .store
            .claim_board_item(seed.id, "actor-A")
            .await
            .unwrap();

        // Move back to todo — the todo gate runs with the merged ctx
        // and passes (title + lbl present), even though some other doing-
        // only fields might or might not still be set.
        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("todo".into()),
                ..Default::default()
            }),
        )
        .await
        .expect("doing -> todo should pass");
        assert_eq!(out.0.status, "todo");
    }

    #[tokio::test]
    async fn patch_without_status_change_skips_gate() {
        // Grandfathering invariant: a PATCH that doesn't touch `status`
        // must skip the gate entirely even if the row's current state
        // would fail validation. Seed a row in `doing` *without*
        // workdir (which we can't do directly through create with the
        // gate on — so use the store to bypass the gate the same way a
        // pre-existing legacy row would have landed). Then PATCH only
        // the title; the gate should never fire.
        let state = fresh_state().await;
        // Bypass: create with `todo` (gate passes for todo), then bump
        // the status field straight via the store's patch_board_item —
        // that's a back-door but mirrors a row that was already in
        // `doing` before the gate was deployed.
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "legacy doing".into(),
                body: None,
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
        let _ = state
            .store
            .patch_board_item(
                seed.id,
                BoardPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // PATCH only the title — no `status` in the body. Even though
        // the row currently sits in `doing` without workdir/tool, this
        // must succeed.
        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                title: Some("renamed".into()),
                ..Default::default()
            }),
        )
        .await
        .expect("title-only patch must skip the gate on a legacy doing row");
        assert_eq!(out.0.title, "renamed");
        assert_eq!(out.0.status, "doing");
    }

    #[tokio::test]
    async fn patch_status_to_same_value_skips_gate() {
        // Variant of grandfathering: PATCH that sets `status` to the
        // same value the row already has must skip the gate. This is
        // the "no-op transition" path — e.g. dashboard re-saves the
        // current state without realizing the row is partially filled.
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "legacy doing".into(),
                body: None,
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
        let _ = state
            .store
            .patch_board_item(
                seed.id,
                BoardPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // PATCH `status: "doing"` on a row already in `doing` — same
        // value, no transition, gate must skip.
        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("doing".into()),
                ..Default::default()
            }),
        )
        .await
        .expect("same-status patch must skip the gate");
        assert_eq!(out.0.status, "doing");
    }

    #[tokio::test]
    async fn custom_column_passthrough_on_patch() {
        // Custom columns (anything outside the matrix) bypass validation
        // entirely. Seed a `todo` row, then PATCH `status: "review"` —
        // no rule defined for `review`, so this must pass without
        // requiring any extra fields.
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "needs review".into(),
                body: None,
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

        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("review".into()),
                ..Default::default()
            }),
        )
        .await
        .expect("custom column must bypass the gate");
        assert_eq!(out.0.status, "review");
    }

    #[tokio::test]
    async fn done_via_comment_path_passes() {
        // The `done` OR-clause: either `session_id IS NOT NULL` OR at
        // least one row in `board_comments`. Seed a row in `todo` with
        // lbl set, drop a comment, then PATCH it straight to `done`.
        // Validation should pass via the comment path even though
        // `session_id` is null.
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "manual close".into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("chore".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        let _ = state
            .store
            .create_board_comment(
                seed.id,
                NewBoardComment {
                    author: "actor-A".into(),
                    body: "won't fix — dup of AG-3".into(),
                },
            )
            .await
            .unwrap();

        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("done".into()),
                ..Default::default()
            }),
        )
        .await
        .expect("done via comment path must pass");
        assert_eq!(out.0.status, "done");
    }

    #[tokio::test]
    async fn done_without_session_or_comment_fails() {
        // Negative: `done` with neither session_id nor a comment returns
        // 400 with missing: ["session_id_or_comment"].
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "no anchor".into(),
                body: None,
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

        let err = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("done".into()),
                ..Default::default()
            }),
        )
        .await
        .expect_err("done without session or comment must fail");
        let (status, body) = err_status_and_body(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], "done");
        assert_eq!(
            body["missing"],
            serde_json::json!(["session_id_or_comment"])
        );
    }

    #[tokio::test]
    async fn patch_done_to_doing_skips_gate_when_done_lacks_anchor() {
        // Grandfathered/inconsistent state: a row sitting in `done` with
        // neither `session_id` nor any comments (technically possible if
        // the row predates the gate, or if comments were deleted). PATCH
        // it `done -> doing` providing all the `doing`-required fields.
        // The gate must run for the *target* status (`doing`), not the
        // source — so it doesn't matter that the row's `done` state would
        // fail validation today. The transition is allowed because the
        // merged ctx satisfies `doing`'s rules.
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "stranded done".into(),
                body: None,
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
        // Back-door into `done` via the store, leaving session_id null and
        // adding no comments — the row is now in a state the current gate
        // would reject if `done` were the *target*. We're testing that the
        // gate runs against the target (`doing`), not the source.
        let _ = state
            .store
            .patch_board_item(
                seed.id,
                BoardPatch {
                    status: Some("done".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // Claim it so the doing-gate's `claimed_by` requirement is met.
        let _ = state
            .store
            .claim_board_item(seed.id, "actor-A")
            .await
            .unwrap();

        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("doing".into()),
                tool: Some(Some("claude".into())),
                workdir: Some(Some("/home/me/foo".into())),
                ..Default::default()
            }),
        )
        .await
        .expect("done -> doing with full target ctx must pass — gate runs on target, not source");
        assert_eq!(out.0.status, "doing");
    }

    #[tokio::test]
    async fn patch_done_to_todo_skips_gate() {
        // Symmetric to the above: `done -> todo` with the row in the same
        // anchorless `done` state. The todo-gate only needs title + lbl,
        // which the merged ctx provides. Proves the source-state `done`
        // never re-runs as a "from" gate even though it'd fail today.
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "stranded done".into(),
                body: None,
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
        let _ = state
            .store
            .patch_board_item(
                seed.id,
                BoardPatch {
                    status: Some("done".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("todo".into()),
                ..Default::default()
            }),
        )
        .await
        .expect("done -> todo must always pass — source gate never re-runs");
        assert_eq!(out.0.status, "todo");
    }

    #[tokio::test]
    async fn done_via_session_only_passes() {
        // The `done` OR-clause via the session arm: `session_id IS NOT
        // NULL` is enough on its own; zero comments. This complements
        // `done_via_comment_path_passes` (comment arm) and
        // `done_without_session_or_comment_fails` (negative). The handler
        // calls `has_board_comments` regardless, but the validator's OR
        // short-circuits on `session_id`.
        let state = fresh_state().await;
        let seed = state
            .store
            .create_board_item(NewBoardItem {
                title: "agent ran it".into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: Some("claude".into()),
                workdir: Some("/home/me/foo".into()),
                model: None,
                // Session id is the anchor we're proving works alone.
                session_id: Some("11111111-1111-1111-1111-111111111111".into()),
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();
        // Move through `doing` first so the doing-gate has claimed_by.
        let _ = state
            .store
            .claim_board_item(seed.id, "actor-A")
            .await
            .unwrap();
        let _ = state
            .store
            .patch_board_item(
                seed.id,
                BoardPatch {
                    status: Some("doing".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = patch(
            State(state.clone()),
            Path(seed.id),
            Json(BoardPatch {
                status: Some("done".into()),
                ..Default::default()
            }),
        )
        .await
        .expect("done via session-only path must pass");
        assert_eq!(out.0.status, "done");
        // Sanity: no comments were created — the session arm was the only
        // thing satisfying the OR.
        let comments = state.store.list_board_comments(seed.id).await.unwrap();
        assert!(
            comments.is_empty(),
            "session-only path must not require comments"
        );
    }

    #[tokio::test]
    async fn rejection_emits_board_transition_rejected_event() {
        // Every gate failure must emit `board.transition.rejected` with
        // payload {id, target_status, missing}. POST has no id yet, so
        // `id` is null — the architecture pinned this contract.
        let state = fresh_state().await;
        let mut rx = state.bus.subscribe();
        let _err = create(
            State(state.clone()),
            Json(NewBoardItem {
                title: "missing fields".into(),
                body: None,
                status: Some("doing".into()),
                lbl: Some("feat".into()),
                tool: None,
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            }),
        )
        .await
        .expect_err("gate fails");

        let ev = rx.recv().await.expect("event must fire");
        assert_eq!(ev.kind, "board.transition.rejected");
        assert!(ev.payload["id"].is_null(), "POST id must be null");
        assert_eq!(ev.payload["target_status"], "doing");
        assert_eq!(
            ev.payload["missing"],
            serde_json::json!(["workdir", "tool", "claimed_by"])
        );
    }
}
