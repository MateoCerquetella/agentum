//! `/api/notes` — REST CRUD per PRD §7.

use agentum_core::{NewNote, Note, NotePatch};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/notes", get(list).post(create))
        .route("/api/notes/{id}", get(get_one).patch(patch).delete(delete))
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<Note>>, ApiError> {
    let v = state.store.list_notes().await?;
    Ok(Json(v))
}

async fn create(
    State(state): State<AppState>,
    Json(payload): Json<NewNote>,
) -> Result<(StatusCode, Json<Note>), ApiError> {
    let n = state.store.create_note(payload).await?;
    Ok((StatusCode::CREATED, Json(n)))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Note>, ApiError> {
    let n = state
        .store
        .get_note(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("note {id}")))?;
    Ok(Json(n))
}

async fn patch(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(patch): Json<NotePatch>,
) -> Result<Json<Note>, ApiError> {
    let n = state.store.patch_note(id, patch).await?;
    Ok(Json(n))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_note(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
