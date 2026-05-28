//! `/api/hosts` — machines controlled directly by this daemon.

use agentum_core::{Host, HostKind, NewHost, SshAuth};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::host_runtime::HostProbe;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/hosts", get(list).post(create))
        .route("/api/hosts/{id}", get(get_one).delete(remove))
        .route("/api/hosts/{id}/test", post(test))
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<Host>>, ApiError> {
    Ok(Json(state.store.list_hosts().await?))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Host>, ApiError> {
    let id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(Json(host))
}

async fn create(
    State(state): State<AppState>,
    Json(new): Json<NewHost>,
) -> Result<(StatusCode, Json<Host>), ApiError> {
    let name = new.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::BadRequest("host name is required".into()));
    }
    let kind = match new.kind {
        HostKind::Local => {
            return Err(ApiError::BadRequest(
                "additional local hosts are not supported".into(),
            ));
        }
        HostKind::Ssh {
            user,
            hostname,
            port,
            auth,
        } => {
            let user = user.trim().to_string();
            let hostname = hostname.trim().to_string();
            if user.is_empty() {
                return Err(ApiError::BadRequest("ssh user is required".into()));
            }
            if hostname.is_empty() {
                return Err(ApiError::BadRequest("ssh hostname is required".into()));
            }
            if port == 0 {
                return Err(ApiError::BadRequest(
                    "ssh port must be between 1 and 65535".into(),
                ));
            }
            let auth = match auth {
                SshAuth::Key { path } if path.trim().is_empty() => SshAuth::Agent,
                SshAuth::Key { path } => SshAuth::Key {
                    path: path.trim().to_string(),
                },
                SshAuth::Agent => SshAuth::Agent,
            };
            HostKind::Ssh {
                user,
                hostname,
                port,
                auth,
            }
        }
    };
    let new = NewHost { name, kind };
    let host = state.store.create_host(new).await?;
    Ok((StatusCode::CREATED, Json(host)))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    if state.store.delete_host(id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(id.to_string()))
    }
}

async fn test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<HostProbe>, ApiError> {
    let id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let probe = crate::host_runtime::probe(&host).await;
    if probe.ok {
        let _ = state.store.update_host_seen(id).await;
    }
    Ok(Json(probe))
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::BadRequest(format!("invalid uuid: {s}")))
}
