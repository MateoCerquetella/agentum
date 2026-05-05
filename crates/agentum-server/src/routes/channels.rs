//! `/api/channels` — list/create/delete + per-channel message append + listing
//!.
//!
//! Live delivery rides on the existing `/api/events` broadcast bus: a POST
//! to `/api/channels/{id}/messages` emits a `message.posted` event with
//! payload `{channel_id, message_id, sender, body, ts}`. Frontends filter
//! by `channel_id`. No per-channel WebSocket needed.

use agentum_core::{Channel, Event, Message, NewChannel, NewMessage};
use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/channels", get(list).post(create))
        .route("/api/channels/{id}", get(get_one).delete(delete))
        .route(
            "/api/channels/{id}/messages",
            get(list_messages).post(post_message),
        )
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<Channel>>, ApiError> {
    Ok(Json(state.store.list_channels().await?))
}

async fn create(
    State(state): State<AppState>,
    Json(payload): Json<NewChannel>,
) -> Result<(StatusCode, Json<Channel>), ApiError> {
    let ch = state.store.create_channel(payload).await?;
    let _ = state
        .bus
        .send(Event::new("channel.created").with_payload(json!({
            "id": ch.id,
            "a_session": ch.a_session,
            "b_session": ch.b_session,
        })));
    Ok((StatusCode::CREATED, Json(ch)))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Channel>, ApiError> {
    let ch = state
        .store
        .get_channel(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("channel {id}")))?;
    Ok(Json(ch))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_channel(id).await?;
    let _ = state
        .bus
        .send(Event::new("channel.deleted").with_payload(json!({"id": id})));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ListMessagesQuery {
    /// Maximum messages to return (oldest-first within the slice).
    limit: Option<i64>,
}

async fn list_messages(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<ListMessagesQuery>,
) -> Result<Json<Vec<Message>>, ApiError> {
    state
        .store
        .get_channel(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("channel {id}")))?;
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    Ok(Json(state.store.list_messages(id, limit).await?))
}

async fn post_message(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<NewMessage>,
) -> Result<(StatusCode, Json<Message>), ApiError> {
    if payload.body.trim().is_empty() {
        return Err(ApiError::BadRequest("body must be non-empty".into()));
    }
    state
        .store
        .get_channel(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("channel {id}")))?;
    let msg = state.store.append_message(id, payload).await?;
    let _ = state
        .bus
        .send(Event::new("message.posted").with_payload(json!({
            "channel_id": msg.channel_id,
            "message_id": msg.id,
            "sender": msg.sender,
            "body": msg.body,
            "ts": msg.ts,
        })));
    Ok((StatusCode::CREATED, Json(msg)))
}
