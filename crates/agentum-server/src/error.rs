use agentum_store::StoreError;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("too many requests: {0}")]
    TooManyRequests(String),
    #[error("internal: {0}")]
    Internal(String),
    /// Escape hatch for handlers that need a custom JSON body shape rather
    /// than the default `{"error": msg}` envelope. Used when a wire contract
    /// pins down a specific payload (e.g. validation gates returning
    /// `{"missing": [...], "status": "doing"}`) and adding a single-purpose
    /// variant per such gate would balloon this enum. Keep the structured
    /// variants above for the common path — reach for `Custom` only when
    /// the body shape is genuinely non-default.
    #[error("custom {0}: {1}")]
    Custom(StatusCode, serde_json::Value),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Custom carries its own body shape — short-circuit before the
        // common `{"error": msg}` envelope path. Every other variant
        // produces a single string message wrapped in the standard
        // envelope.
        if let ApiError::Custom(status, body) = self {
            return (status, Json(body)).into_response();
        }
        let (status, msg) = match &self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            ApiError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m.clone()),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m.clone()),
            ApiError::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, m.clone()),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m.clone()),
            ApiError::Custom(..) => unreachable!("handled above"),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(m) => ApiError::NotFound(m),
            StoreError::AlreadyExists(m) => ApiError::Conflict(m),
            StoreError::Core(c) => ApiError::BadRequest(c.to_string()),
            other => {
                tracing::error!(error = %other, "store error");
                ApiError::Internal(other.to_string())
            }
        }
    }
}
