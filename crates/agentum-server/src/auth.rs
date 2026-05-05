//! Username/password auth.
//!
//! Replaces the old static bearer-token file. Passwords are hashed with
//! Argon2id (default OWASP-aligned params from the `argon2` crate). Each
//! login mints a 32-byte URL-safe random token, stored in `auth_sessions`,
//! and the client presents it as `Authorization: Bearer …` (or `?token=`
//! on WS upgrades, since browsers can't set headers there).

use argon2::Argon2;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng};
use rand::RngCore;

use crate::AppState;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("hash: {0}")]
    Hash(String),
}

/// Generate a 32-byte URL-safe base64 token (43 chars, no padding).
pub fn new_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn hash_password(plain: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    argon
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Hash(e.to_string()))
}

pub fn verify_password(plain: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

/// Endpoints reachable without auth.
///
/// `/api/auth/register` is also public-ish, but only behaves as such when no
/// users exist yet; the route handler enforces that.
fn is_public(path: &str) -> bool {
    matches!(
        path,
        "/api/health" | "/api/cert" | "/api/auth/status" | "/api/auth/login" | "/api/auth/register"
    )
}

fn extract_token(req: &Request<Body>) -> Option<String> {
    if let Some(auth) = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(t) = auth.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    let q = req.uri().query()?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "token" {
            return Some(urldecode(v));
        }
    }
    None
}

fn urldecode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1]);
                let lo = hex_val(bytes[i + 2]);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push(h * 16 + l);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// axum middleware. Looks up the bearer token in `auth_sessions`. If the
/// token resolves to a user, the request continues; otherwise 401.
pub async fn require_token(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/") || is_public(path) {
        return next.run(req).await;
    }

    let Some(token) = extract_token(&req) else {
        return unauthorized("missing bearer token");
    };

    match state.store.touch_auth_session(&token).await {
        Ok(Some(_user)) => next.run(req).await,
        Ok(None) => unauthorized("invalid bearer token"),
        Err(e) => {
            tracing::warn!(error = %e, "auth lookup failed");
            unauthorized("auth lookup failed")
        }
    }
}

fn unauthorized(msg: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            axum::http::header::WWW_AUTHENTICATE,
            "Bearer realm=\"agentum\"",
        )],
        msg,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        let t = new_token();
        assert_eq!(t.len(), 43);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[test]
    fn hash_roundtrip() {
        let h = hash_password("hunter2").unwrap();
        assert!(verify_password("hunter2", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn url_decode_basic() {
        assert_eq!(urldecode("abc"), "abc");
        assert_eq!(urldecode("abc%20def"), "abc def");
        assert_eq!(urldecode("a+b"), "a b");
    }
}
