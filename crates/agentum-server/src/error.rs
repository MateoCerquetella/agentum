use agentum_core::{Host, HostKind};
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
    /// A host owned by another machine was reachable through this daemon, but
    /// the remote lifecycle operation failed. Keeping this distinct from a
    /// daemon-side 500 lets clients tell users that the SSH/tmux/agent failure
    /// happened at the configured host boundary.
    #[error("bad gateway: {0}")]
    BadGateway(String),
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

impl ApiError {
    /// Translate a host-aware runtime failure at the API boundary. A missing
    /// remote prerequisite is something the user can correct in the request or
    /// host setup (400); all other SSH-side failures crossed the configured
    /// upstream boundary (502). Local runtime failures retain their historical
    /// daemon-side 500 behavior.
    pub(crate) fn from_host_runtime(
        host: &Host,
        error: crate::host_runtime::HostRuntimeError,
    ) -> Self {
        let message = error.to_string();
        match (&host.kind, &error) {
            (
                HostKind::Ssh { .. },
                crate::host_runtime::HostRuntimeError::RemotePrerequisite { .. },
            ) => Self::BadRequest(message),
            (HostKind::Ssh { .. }, _) => Self::BadGateway(message),
            (HostKind::Local, _) => Self::Internal(message),
        }
    }
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
            ApiError::BadGateway(m) => (StatusCode::BAD_GATEWAY, m.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn host(kind: HostKind) -> Host {
        Host {
            id: uuid::Uuid::new_v4(),
            name: "test".into(),
            kind,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    fn ssh_host() -> Host {
        host(HostKind::Ssh {
            user: "user".into(),
            hostname: "example.com".into(),
            port: 22,
            auth: agentum_core::SshAuth::Agent,
        })
    }

    #[tokio::test]
    async fn bad_gateway_keeps_standard_error_envelope() {
        let response = ApiError::BadGateway("remote launch failed".into()).into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"error": "remote launch failed"})
        );
    }

    #[test]
    fn remote_prerequisites_are_actionable_bad_requests() {
        let error = crate::host_runtime::HostRuntimeError::RemotePrerequisite {
            stage: "workdir",
            message: "remote workdir is missing".into(),
        };
        assert!(matches!(
            ApiError::from_host_runtime(&ssh_host(), error),
            ApiError::BadRequest(message) if message.contains("workdir")
        ));
    }

    #[test]
    fn remote_runtime_failures_are_bad_gateway_but_local_failures_stay_internal() {
        let remote = ApiError::from_host_runtime(
            &ssh_host(),
            crate::host_runtime::HostRuntimeError::Bootstrap("tunnel failed".into()),
        );
        assert!(matches!(remote, ApiError::BadGateway(message) if message.contains("tunnel")));

        let local = ApiError::from_host_runtime(
            &host(HostKind::Local),
            crate::host_runtime::HostRuntimeError::Bootstrap("tmux failed".into()),
        );
        assert!(matches!(local, ApiError::Internal(message) if message.contains("tmux")));
    }
}
