//! Auth endpoints: register / login / logout / me / status.
//!
//! `register` is open while no users exist (first-run bootstrap). Once at
//! least one user exists, the route requires a logged-in caller — letting an
//! existing operator add additional users without exposing a public sign-up.

use agentum_core::validate_username;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::{hash_password, new_token, verify_password};
use crate::error::ApiError;

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
    /// True when register/login can be hit anonymously.
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

async fn register(
    State(state): State<AppState>,
    Json(body): Json<Credentials>,
) -> Result<(StatusCode, Json<AuthResp>), ApiError> {
    let user_count = state.store.count_users().await?;

    // Once any user exists, register is closed via this anonymous endpoint.
    // (We can re-open it for authenticated callers later, but keep it tight
    // for now — most installs are single-user.)
    if user_count > 0 {
        return Err(ApiError::Forbidden(
            "registration is closed — log in or run `agentum auth reset` on the host".into(),
        ));
    }

    let username = body.username.trim().to_lowercase();
    validate_username(&username)
        .map_err(|e| ApiError::BadRequest(format!("invalid username: {e}")))?;
    validate_password(&body.password)?;

    let hash = hash_password(&body.password)
        .map_err(|e| ApiError::Internal(format!("hash failed: {e}")))?;
    let user = state.store.create_user(&username, &hash).await?;
    let token = new_token();
    state.store.create_auth_session(user.id, &token).await?;

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
    Json(body): Json<Credentials>,
) -> Result<Json<AuthResp>, ApiError> {
    let username = body.username.trim().to_lowercase();
    let Some((user, stored_hash)) = state.store.get_user_by_username(&username).await? else {
        // Mask account-existence: same error shape as wrong password.
        return Err(ApiError::Unauthorized("invalid credentials".into()));
    };
    if !verify_password(&body.password, &stored_hash) {
        return Err(ApiError::Unauthorized("invalid credentials".into()));
    }
    let token = new_token();
    state.store.create_auth_session(user.id, &token).await?;
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
        .touch_auth_session(token.trim())
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid token".into()))?;
    Ok(Json(MeResp {
        username: user.username,
    }))
}
