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

/// Identity established by the API authentication boundary. SDD approval
/// handlers consume this extension and never trust a caller-supplied actor id.
#[derive(Debug, Clone)]
pub struct AuthActor {
    pub id: String,
    pub display_name: String,
    trust: AuthTrust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthTrust {
    /// A database-backed user session or the desktop webview's boot-scoped
    /// capability. Only this trust class may drive SDD mutations.
    Human,
    /// A request admitted solely because a standalone daemon was launched with
    /// `--no-auth`. It remains useful for legacy/non-SDD local automation, but
    /// it must never be promoted into a human approval identity.
    UnauthenticatedLocal,
}

impl AuthActor {
    fn human(id: String, display_name: String) -> Self {
        Self {
            id,
            display_name,
            trust: AuthTrust::Human,
        }
    }

    pub(crate) fn unauthenticated_local() -> Self {
        Self {
            id: "unauthenticated:local".into(),
            display_name: "Unauthenticated local caller".into(),
            trust: AuthTrust::UnauthenticatedLocal,
        }
    }

    /// SDD run-control and authorization commands are human interfaces. A
    /// provider process may share loopback networking with Agentum, so
    /// loopback origin alone is deliberately insufficient here.
    pub fn can_mutate_sdd(&self) -> bool {
        self.trust == AuthTrust::Human
    }
}

/// How long a freshly minted bearer token stays valid. Refreshed on every
/// authenticated request (sliding expiry), so an active session stays live
/// indefinitely while idle ones get reaped.
pub const SESSION_TTL: time::Duration = time::Duration::days(30);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("hash: {0}")]
    Hash(String),
    #[error("join: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Generate a 32-byte URL-safe base64 token (43 chars, no padding).
pub fn new_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Argon2id is intentionally CPU-expensive (~100ms+ per call). Run it on the
/// blocking pool so we don't stall the async runtime under concurrent logins.
pub async fn hash_password(plain: String) -> Result<String, AuthError> {
    tokio::task::spawn_blocking(move || hash_password_sync(&plain)).await?
}

pub async fn verify_password(plain: String, stored_hash: String) -> Result<bool, AuthError> {
    Ok(tokio::task::spawn_blocking(move || verify_password_sync(&plain, &stored_hash)).await?)
}

fn hash_password_sync(plain: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    argon
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Hash(e.to_string()))
}

fn verify_password_sync(plain: &str, stored_hash: &str) -> bool {
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
    if matches!(
        path,
        "/api/health"
            | "/api/cert"
            | "/api/cert/fingerprint"
            | "/api/auth/status"
            | "/api/auth/login"
            | "/api/auth/register"
    ) {
        return true;
    }
    // NOTE: `/mcp` (the Agentum MCP server) is deliberately NOT public. It
    // exposes app-control tools, so both a network daemon and the embedded
    // loopback server require the separate MCP bearer. Agent launch wiring
    // injects that bearer into the agent's config as an `Authorization` header;
    // the desktop UI bearer is never shared with an agent process.
    //
    // Hook endpoints use per-session ephemeral tokens, not bearer auth.
    // Agent CLIs don't know the user's bearer token.
    path.starts_with("/api/sessions/") && path.ends_with("/hook")
}

fn is_sdd(path: &str) -> bool {
    path == "/api/sdd" || path.starts_with("/api/sdd/")
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
/// Sliding expiry: each touch extends the row's `expires_at` by `SESSION_TTL`.
/// The desktop's embedded server presents a boot-scoped capability minted in
/// memory and returned only through the Tauri command boundary. It is checked
/// before database sessions and is never persisted or injected into agents.
///
/// When `state.no_auth` is set (via `agentum serve --no-auth`), requests still
/// receive an explicitly untrusted local identity for non-SDD routes. The
/// entire SDD HTTP/WS namespace still requires a human bearer, so
/// `--no-auth` cannot expose spec state or turn a provider into a human
/// approver or delivery actor.
pub async fn require_token(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/") || is_public(path) {
        return next.run(req).await;
    }

    let presented = extract_token(&req);
    if state
        .embedded_ui_token
        .as_deref()
        .zip(presented.as_deref())
        .is_some_and(|(expected, actual)| constant_time_eq(expected, actual))
    {
        req.extensions_mut().insert(AuthActor::human(
            "human:local-desktop".into(),
            "Local desktop user".into(),
        ));
        return next.run(req).await;
    }

    if let Some(token) = presented {
        match state
            .store
            .touch_auth_session(&token, Some(SESSION_TTL))
            .await
        {
            Ok(Some(user)) => {
                req.extensions_mut().insert(AuthActor::human(
                    format!("human:user:{}", user.id),
                    user.username,
                ));
                return next.run(req).await;
            }
            Ok(None) if !state.no_auth || is_sdd(path) => {
                return unauthorized("invalid or expired bearer token");
            }
            Ok(None) => {}
            Err(e) if !state.no_auth || is_sdd(path) => {
                tracing::warn!(error = %e, "auth lookup failed");
                return unauthorized("auth lookup failed");
            }
            Err(e) => tracing::warn!(error = %e, "auth lookup failed on no-auth server"),
        }
    }

    if state.no_auth {
        if is_sdd(path) {
            return unauthorized("SDD requires an authenticated human capability");
        }
        req.extensions_mut()
            .insert(AuthActor::unauthenticated_local());
        return next.run(req).await;
    }

    unauthorized("missing bearer token")
}

/// Length-checked constant-time comparison. Token length is not secret, while
/// avoiding an early byte mismatch keeps the boot-scoped capability from
/// becoming a useful loopback timing oracle.
fn constant_time_eq(expected: &str, actual: &str) -> bool {
    let (expected, actual) = (expected.as_bytes(), actual.as_bytes());
    if expected.len() != actual.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in expected.iter().zip(actual) {
        difference |= left ^ right;
    }
    difference == 0
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
    use agentum_core::Event;
    use axum::http::Request;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    #[test]
    fn token_format() {
        let t = new_token();
        assert_eq!(t.len(), 43);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[tokio::test]
    async fn hash_roundtrip() {
        let h = hash_password("hunter2".to_string()).await.unwrap();
        assert!(
            verify_password("hunter2".to_string(), h.clone())
                .await
                .unwrap()
        );
        assert!(!verify_password("wrong".to_string(), h).await.unwrap());
    }

    #[test]
    fn url_decode_basic() {
        assert_eq!(urldecode("abc"), "abc");
        assert_eq!(urldecode("abc%20def"), "abc def");
        assert_eq!(urldecode("a+b"), "a b");
    }

    #[tokio::test]
    async fn no_auth_never_exposes_sdd_without_a_human_capability() {
        const UI_TOKEN: &str = "test-only-boot-scoped-ui-capability";
        const DB_TOKEN: &str = "test-only-database-human-session";
        let directory = tempfile::tempdir().unwrap();
        let store = agentum_store::Store::open(&directory.path().join("auth.sqlite"))
            .await
            .unwrap();
        let user = store
            .create_user("test-human", "unused-test-hash")
            .await
            .unwrap();
        store
            .create_auth_session(user.id, DB_TOKEN, SESSION_TTL)
            .await
            .unwrap();
        let (bus, _) = broadcast::channel::<Event>(16);
        let mut state = AppState::new(store, bus);
        state.no_auth = true;
        state.embedded_ui_token = Some(Arc::new(UI_TOKEN.into()));
        let app = crate::router(state);

        // Read/list/durable-events + WS upgrade path, creation/import/Jira,
        // approval, and both delivery authorization commands. Invalid bodies
        // are deliberate: authentication must run before route parsing.
        let cases = [
            ("GET", "/api/sdd/repos/missing/specs", ""),
            ("GET", "/api/sdd/runs/missing", ""),
            ("GET", "/api/sdd/runs/missing/events?after=0", ""),
            ("GET", "/api/sdd/events?repoId=missing&after=0", ""),
            ("POST", "/api/sdd/repos/missing/specs", "{}"),
            (
                "POST",
                "/api/sdd/specs/SPC-00000000000000000000000000/runs",
                "{}",
            ),
            ("POST", "/api/sdd/repos/missing/sources/preview", "{}"),
            ("POST", "/api/sdd/integrations/jira/oauth/start", "{}"),
            (
                "POST",
                "/api/sdd/runs/missing/commands",
                r#"{"type":"decideApproval","requestId":"probe-approval","expectedRevision":0,"approvalId":"missing","digest":"missing","decision":"approve"}"#,
            ),
            (
                "POST",
                "/api/sdd/runs/missing/commands",
                r#"{"type":"previewDelivery","requestId":"probe-preview","expectedRevision":0,"actions":[{"type":"commit","message":"probe"}]}"#,
            ),
            (
                "POST",
                "/api/sdd/runs/missing/commands",
                r#"{"type":"confirmDelivery","requestId":"probe-confirm","expectedRevision":0,"previewToken":"missing","actions":["missing"]}"#,
            ),
        ];

        for (method, path, body) in cases {
            let unauthenticated = Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let response = app.clone().oneshot(unauthenticated).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );

            let provider_guess = Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", "Bearer provider-does-not-know-ui-token")
                .body(Body::from(body))
                .unwrap();
            let response = app.clone().oneshot(provider_guess).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );

            let ui = Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {UI_TOKEN}"))
                .body(Body::from(body))
                .unwrap();
            let response = app.clone().oneshot(ui).await.unwrap();
            assert_ne!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path}"
            );
            assert_ne!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
        }

        let database_human = Request::get("/api/sdd/runs/missing")
            .header("authorization", format!("Bearer {DB_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(database_human).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
