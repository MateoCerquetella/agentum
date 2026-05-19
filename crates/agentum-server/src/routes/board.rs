//! `/api/board` — kanban CRUD + atomic CAS claim + comments + reorder.

use std::collections::BTreeMap;

use agentum_core::{
    BoardComment, BoardItem, BoardPatch, ClaimRequest, Event, NewBoardComment, NewBoardItem,
    ReorderEntry,
};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

async fn create(
    State(state): State<AppState>,
    Json(payload): Json<NewBoardItem>,
) -> Result<(StatusCode, Json<BoardItem>), ApiError> {
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
