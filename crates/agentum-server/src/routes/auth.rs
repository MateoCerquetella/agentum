//! Auth endpoints: register / login / logout / me / status.
//!
//! `register` is anonymous while no users exist (first-run bootstrap). Once
//! at least one user exists, the route is closed permanently for that boot
//! — additional accounts are added via `agentum auth add` on the host.
//!
//! `register` and `login` share a per-IP rate limiter to slow down online
//! password guessing.

use std::net::SocketAddr;

use agentum_core::validate_username;
use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::{SESSION_TTL, hash_password, new_token, verify_password};
use crate::error::ApiError;
use crate::ratelimit::Decision;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/status", get(status))
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
}

#[derive(Serialize)]
struct StatusResp {
    /// True when zero users exist — UI shows registration form.
    needs_setup: bool,
    /// True when register can be hit anonymously. Equal to `needs_setup`
    /// in current shape; kept as a separate field for forward compat.
    register_open: bool,
}

async fn status(State(state): State<AppState>) -> Result<Json<StatusResp>, ApiError> {
    let n = state.store.count_users().await?;
    let needs_setup = n == 0;
    Ok(Json(StatusResp {
        needs_setup,
        register_open: needs_setup,
    }))
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct AuthResp {
    token: String,
    username: String,
}

const MIN_PASSWORD_LEN: usize = 8;

fn validate_password(p: &str) -> Result<(), ApiError> {
    if p.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

fn check_rate_limit(state: &AppState, peer: SocketAddr) -> Result<(), ApiError> {
    match state.auth_limiter.check(peer.ip()) {
        Decision::Allowed => Ok(()),
        Decision::Denied { retry_after } => Err(ApiError::TooManyRequests(format!(
            "too many auth attempts; retry in {}s",
            retry_after.as_secs().max(1)
        ))),
    }
}

async fn register(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Credentials>,
) -> Result<(StatusCode, Json<AuthResp>), ApiError> {
    check_rate_limit(&state, peer)?;

    let user_count = state.store.count_users().await?;

    // Once any user exists, anonymous registration is closed permanently
    // for this boot. Operators add additional users via the CLI
    // (`agentum auth add`) which writes directly to the store.
    if user_count > 0 {
        return Err(ApiError::Forbidden(
            "registration is closed — log in or run `agentum auth reset` on the host".into(),
        ));
    }

    let username = body.username.trim().to_lowercase();
    validate_username(&username)
        .map_err(|e| ApiError::BadRequest(format!("invalid username: {e}")))?;
    validate_password(&body.password)?;

    let hash = hash_password(body.password.clone())
        .await
        .map_err(|e| ApiError::Internal(format!("hash failed: {e}")))?;
    let user = state.store.create_user(&username, &hash).await?;

    let token = new_token();
    state
        .store
        .create_auth_session(user.id, &token, SESSION_TTL)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(AuthResp {
            token,
            username: user.username,
        }),
    ))
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Credentials>,
) -> Result<Json<AuthResp>, ApiError> {
    check_rate_limit(&state, peer)?;

    let username = body.username.trim().to_lowercase();
    let Some((user, stored_hash)) = state.store.get_user_by_username(&username).await? else {
        // Mask account-existence: same error shape as wrong password.
        return Err(ApiError::Unauthorized("invalid credentials".into()));
    };
    let ok = verify_password(body.password.clone(), stored_hash)
        .await
        .map_err(|e| ApiError::Internal(format!("verify failed: {e}")))?;
    if !ok {
        return Err(ApiError::Unauthorized("invalid credentials".into()));
    }
    let token = new_token();
    state
        .store
        .create_auth_session(user.id, &token, SESSION_TTL)
        .await?;
    Ok(Json(AuthResp {
        token,
        username: user.username,
    }))
}

async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, ApiError> {
    if let Some(tok) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        state.store.delete_auth_session(tok.trim()).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct MeResp {
    username: String,
}

async fn me(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<MeResp>, ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::Unauthorized("missing token".into()))?;
    let user = state
        .store
        .touch_auth_session(token.trim(), Some(SESSION_TTL))
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid or expired token".into()))?;
    Ok(Json(MeResp {
        username: user.username,
    }))
}
