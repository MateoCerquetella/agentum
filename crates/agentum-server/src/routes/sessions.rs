use std::str::FromStr;

use agentum_core::{NewSession, Session, Status};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list).post(create))
        .route("/api/sessions/{id}", get(get_one).delete(delete))
        .route("/api/sessions/{id}/start", post(start))
        .route("/api/sessions/{id}/stop", post(stop))
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Session>>, ApiError> {
    let status = match q.status.as_deref() {
        Some(s) => Some(Status::from_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))?),
        None => None,
    };
    let rows = state.store.list_sessions(status).await?;
    Ok(Json(rows))
}

async fn create(
    State(state): State<AppState>,
    Json(payload): Json<NewSession>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    let s = state.store.create_session(payload).await?;
    Ok((StatusCode::CREATED, Json(s)))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let s = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(Json(s))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state.store.delete_session(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    transition(&state, &id, Status::Running).await
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    transition(&state, &id, Status::Stopped).await
}

async fn transition(
    state: &AppState,
    id: &str,
    target: Status,
) -> Result<Json<Session>, ApiError> {
    let id = Uuid::parse_str(id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    state.store.update_status(id, target).await?;
    let s = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(Json(s))
}
