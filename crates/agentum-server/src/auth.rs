//! Bearer-token auth.
//!
//! Single-value bearer token stored at `$XDG_DATA_HOME/agentum/auth_token`
//! (chmod 0600). Generated lazily on first serve. Rotation = overwrite the
//! file via `agentum auth rotate`; the running server picks it up because
//! the middleware re-reads the file every request.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("path: {0}")]
    Path(#[from] agentum_store::paths::PathError),
}

/// Generate a 32-byte URL-safe base64 token.
pub fn new_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Read the current token, generating + writing one if the file is missing.
pub fn ensure_token() -> Result<String, AuthError> {
    let path = agentum_store::paths::auth_token_path()?;
    if let Ok(s) = std::fs::read_to_string(&path) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let token = new_token();
    write_token(&path, &token)?;
    Ok(token)
}

/// Overwrite the auth token file with a freshly generated value. Returns
/// the new token.
pub fn rotate_token() -> Result<String, AuthError> {
    let path = agentum_store::paths::auth_token_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let token = new_token();
    write_token(&path, &token)?;
    Ok(token)
}

pub fn token_path() -> Result<PathBuf, AuthError> {
    Ok(agentum_store::paths::auth_token_path()?)
}

fn write_token(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{token}\n"))?;
    set_mode_0600(path)
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    perm.set_mode(0o600);
    std::fs::set_permissions(path, perm)
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Read the current on-disk token (no caching). Returns `None` if missing
/// or empty so the middleware can short-circuit to 401.
fn read_current_token() -> Option<String> {
    let path = agentum_store::paths::auth_token_path().ok()?;
    let s = std::fs::read_to_string(path).ok()?;
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// Paths that bypass auth: liveness probe + cert-download (the cert server
/// is on a separate port but we exempt /api/cert here too in case both
/// listeners share the same router, e.g. `--no-tls`).
fn is_public(path: &str) -> bool {
    matches!(path, "/api/health" | "/api/cert")
}

/// Pull the bearer token from `Authorization: Bearer …` or, for WS upgrades
/// where browsers can't set custom headers, from the `?token=` query param.
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
    // Lightweight: `%XX` and `+`. Token is base64-url so usually no encoding,
    // but be defensive.
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

/// axum middleware. Apply AFTER routes are merged so it covers all `/api/*`.
pub async fn require_token(req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/") || is_public(path) {
        return next.run(req).await;
    }

    let Some(expected) = read_current_token() else {
        return unauthorized("auth token not configured on server");
    };
    let Some(provided) = extract_token(&req) else {
        return unauthorized("missing bearer token");
    };

    // Constant-time compare. Tokens are short enough that a naive compare is
    // fine for a local-only single-user tool, but use subtle-style anyway.
    if eq_const_time(provided.as_bytes(), expected.as_bytes()) {
        next.run(req).await
    } else {
        unauthorized("invalid bearer token")
    }
}

fn eq_const_time(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn unauthorized(msg: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, "Bearer realm=\"agentum\"")],
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
        // 32 bytes -> base64-url-no-pad → 43 chars
        assert_eq!(t.len(), 43);
        // URL-safe alphabet only
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn const_time_eq() {
        assert!(eq_const_time(b"abc", b"abc"));
        assert!(!eq_const_time(b"abc", b"abd"));
        assert!(!eq_const_time(b"abc", b"abcd"));
    }

    #[test]
    fn url_decode_basic() {
        assert_eq!(urldecode("abc"), "abc");
        assert_eq!(urldecode("abc%20def"), "abc def");
        assert_eq!(urldecode("a+b"), "a b");
    }

    #[test]
    fn public_paths() {
        assert!(is_public("/api/health"));
        assert!(is_public("/api/cert"));
        assert!(!is_public("/api/sessions"));
    }
}
