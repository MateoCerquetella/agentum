//! `/api/board` — kanban CRUD + atomic CAS claim.

use std::collections::BTreeMap;

use agentum_core::{BoardItem, BoardPatch, ClaimRequest, Event, NewBoardItem};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::Serialize;
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/board", get(list).post(create))
        .route("/api/board/{id}", get(get_one).patch(patch).delete(delete))
        .route("/api/board/{id}/claim", post(claim))
}

#[derive(Serialize)]
struct GroupedBoard {
    /// Status name → items, in insertion (created_at) order.
    columns: BTreeMap<String, Vec<BoardItem>>,
    /// Distinct columns we know about — guarantees todo/doing/done are
    /// present even when empty.
    column_order: Vec<String>,
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
    Ok(Json(GroupedBoard {
        columns,
        column_order: order,
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
    let _ = state.bus.send(
        Event::new("board.deleted").with_payload(json!({"id": id})),
    );
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
            let _ = state.bus.send(
                Event::new("board.claimed").with_payload(json!({
                    "id": item.id,
                    "key": item.key,
                    "claimed_by": item.claimed_by,
                })),
            );
            Ok(Json(item))
        }
        None => Err(ApiError::Conflict(format!(
            "board item {id} is already claimed"
        ))),
    }
}
