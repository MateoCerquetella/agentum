//! Sign-in. Same cookie name, bcrypt hashes and session tokens as Odysseus
//! (imported from auth.json / sessions.json), so signed-in devices stay in.
use crate::{state::App, util::*};
use axum::{extract::{Request, State}, http::{header, HeaderMap, StatusCode}, middleware::Next, response::{IntoResponse, Response}, Json};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Mutex, time::Instant};

pub const COOKIE: &str = "odysseus_session";
const TTL_DAYS: i64 = 3650;

pub fn token_from(h: &HeaderMap) -> Option<String> {
    let c = h.get(header::COOKIE)?.to_str().ok()?;
    c.split(';').map(|p| p.trim()).find_map(|p| p.strip_prefix(&format!("{COOKIE}=")).map(String::from))
}

pub async fn user_of(app: &App, h: &HeaderMap) -> Option<String> {
    let t = token_from(h)?;
    if t.len() < 32 { return None; }
    app.db.call(move |c| c.query_row("SELECT user FROM auth_sessions WHERE token=?1 AND expiry>?2", rusqlite::params![t, now_ts()], |r| r.get::<_, String>(0)).optional()).await.ok().flatten()
}

/// The dashboard (Glance, a container on Mercury) reads two widgets with a
/// key instead of a session: X-CERQ-Key = CERQ_WIDGET_KEY from /etc/cerq.env
/// (16+ characters), GET only, nothing else. The PC that runs BB pushes its
/// threads the same way: CERQ_INGEST_KEY, POST /api/journal/ingest only.
pub const WIDGET_PATHS: [&str; 2] = ["/api/summary", "/api/calendar/widget"];
pub fn widget_key_ok(method: &axum::http::Method, path: &str, h: &HeaderMap, want: Option<&str>) -> bool {
    if method != axum::http::Method::GET || !WIDGET_PATHS.contains(&path) { return false; }
    key_ok(h, want)
}
pub fn ingest_key_ok(method: &axum::http::Method, path: &str, h: &HeaderMap, want: Option<&str>) -> bool {
    method == axum::http::Method::POST && path == "/api/journal/ingest" && key_ok(h, want)
}
fn key_ok(h: &HeaderMap, want: Option<&str>) -> bool {
    let Some(want) = want.filter(|w| w.len() >= 16) else { return false };
    let Some(got) = h.get("x-cerq-key").and_then(|v| v.to_str().ok()) else { return false };
    got.len() == want.len() && got.bytes().zip(want.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Every /api route except sign-in needs a session (or, for the dashboard's widgets, the widget key).
pub async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    let p = req.uri().path();
    let open = !p.starts_with("/api/") || matches!(p, "/api/auth/login" | "/api/health" | "/api/auth/passkey/login/start" | "/api/auth/passkey/login/finish" | "/api/code/syntax.css");
    let (wk, ik) = (std::env::var("CERQ_WIDGET_KEY").ok(), std::env::var("CERQ_INGEST_KEY").ok());
    if open || widget_key_ok(req.method(), p, req.headers(), wk.as_deref()) || ingest_key_ok(req.method(), p, req.headers(), ik.as_deref()) { return next.run(req).await; }
    if user_of(&app, req.headers()).await.is_some() {
        // With a passkey set up, changing settings, accounts or tokens needs a Touch ID unlock first.
        if crate::passkey::locked(&app, req.method(), p, req.headers()).await {
            return (StatusCode::FORBIDDEN, Json(json!({"error": "Unlock with Touch ID or Face ID first.", "locked": true}))).into_response();
        }
        return next.run(req).await;
    }
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"}))).into_response()
}

static TRIES: Mutex<Option<HashMap<String, (u32, Instant)>>> = Mutex::new(None);

pub async fn login(State(app): State<App>, headers: HeaderMap, Json(b): Json<Value>) -> R<Response> {
    let ip = headers.get("x-forwarded-for").or(headers.get("x-real-ip")).and_then(|v| v.to_str().ok()).unwrap_or("local").split(',').next().unwrap_or("").trim().to_string();
    {
        let mut g = TRIES.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        if let Some((n, t)) = m.get(&ip) { if *n >= 8 && t.elapsed().as_secs() < 300 { return Err(AppError(StatusCode::TOO_MANY_REQUESTS, "Too many tries. Wait 5 minutes.".into())); } }
    }
    let user = s(&b, "username").trim().to_lowercase();
    let pass = s(&b, "password");
    let u2 = user.clone();
    let hash: Option<String> = app.db.call(move |c| c.query_row("SELECT password_hash FROM users WHERE name=?1", [u2], |r| r.get(0)).optional()).await?;
    let ok = match hash { Some(h) => tokio::task::spawn_blocking(move || bcrypt::verify(pass, &h).unwrap_or(false)).await.unwrap_or(false), None => false };
    if !ok {
        let mut g = TRIES.lock().unwrap();
        let e = g.get_or_insert_with(HashMap::new).entry(ip).or_insert((0, Instant::now()));
        if e.1.elapsed().as_secs() > 300 { *e = (0, Instant::now()); }
        e.0 += 1;
        return Err(AppError(StatusCode::UNAUTHORIZED, "Wrong username or password.".into()));
    }
    new_session(&app, &user, &headers).await
}

/// Is this the user's password? (The Settings unlock's fallback.)
pub async fn password_ok(app: &App, user: &str, pass: &str) -> bool {
    let (u, p) = (user.to_string(), pass.to_string());
    let hash: Option<String> = app.db.call(move |c| c.query_row("SELECT password_hash FROM users WHERE name=?1", [u], |r| r.get(0)).optional()).await.ok().flatten();
    match hash { Some(h) => tokio::task::spawn_blocking(move || bcrypt::verify(p, &h).unwrap_or(false)).await.unwrap_or(false), None => false }
}

/// A signed-in session for `user` (password or passkey): the cookie and {ok, user}.
pub async fn new_session(app: &App, user: &str, headers: &HeaderMap) -> R<Response> {
    let token = hex::encode(rand::random::<[u8; 32]>());
    let (t2, u3) = (token.clone(), user.to_string());
    app.db.call(move |c| c.execute("INSERT INTO auth_sessions(token,user,expiry) VALUES(?1,?2,?3)", rusqlite::params![t2, u3, now_ts() + TTL_DAYS * 86400])).await?;
    let secure = headers.get("x-forwarded-proto").map(|v| v == "https").unwrap_or(false);
    let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}", TTL_DAYS * 86400, if secure { "; Secure" } else { "" });
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({"ok": true, "user": user}))).into_response())
}

pub async fn logout(State(app): State<App>, headers: HeaderMap) -> R<Response> {
    if let Some(t) = token_from(&headers) { app.db.call(move |c| c.execute("DELETE FROM auth_sessions WHERE token=?1", [t])).await?; }
    app.db.kv_set("bb_sessions", &json!({})).await?;
    Ok(([(header::SET_COOKIE, format!("{COOKIE}=; Path=/; Max-Age=0"))], Json(json!({"ok": true}))).into_response())
}

pub async fn me(State(app): State<App>, headers: HeaderMap) -> R<Json<Value>> {
    match user_of(&app, &headers).await {
        Some(u) => Ok(Json(json!({"user": u, "brand": app.db.kv_get("brand").await?.unwrap_or(json!("Agentum"))}))),
        None => Err(AppError(StatusCode::UNAUTHORIZED, "unauthorized".into())),
    }
}

/// POST /api/auth/password {current, new}
pub async fn change_password(State(app): State<App>, headers: HeaderMap, Json(b): Json<Value>) -> R<Json<Value>> {
    let user = user_of(&app, &headers).await.ok_or_else(|| AppError(StatusCode::UNAUTHORIZED, "unauthorized".into()))?;
    let (cur, new) = (s(&b, "current"), s(&b, "new"));
    if new.len() < 8 { return Err(bad("Use at least 8 characters.")); }
    let u2 = user.clone();
    let h: String = app.db.call(move |c| c.query_row("SELECT password_hash FROM users WHERE name=?1", [u2], |r| r.get(0))).await?;
    if !tokio::task::spawn_blocking(move || bcrypt::verify(cur, &h).unwrap_or(false)).await? { return Err(bad("The current password is wrong.")); }
    let nh = tokio::task::spawn_blocking(move || bcrypt::hash(new, 12)).await??;
    app.db.call(move |c| c.execute("UPDATE users SET password_hash=?1 WHERE name=?2", [nh, user])).await?;
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, Method};
    #[test]
    fn widget_key() {
        let mut h = HeaderMap::new();
        h.insert("x-cerq-key", HeaderValue::from_static("0123456789abcdef"));
        let want = Some("0123456789abcdef");
        assert!(widget_key_ok(&Method::GET, "/api/summary", &h, want));
        assert!(widget_key_ok(&Method::GET, "/api/calendar/widget", &h, want));
        assert!(!widget_key_ok(&Method::GET, "/api/brief", &h, want));             // the Brief widget is CERQ's own page, signed in
        assert!(!widget_key_ok(&Method::POST, "/api/summary", &h, want));          // read only
        assert!(!widget_key_ok(&Method::GET, "/api/mail/inbox", &h, want));        // only the widgets
        assert!(!widget_key_ok(&Method::GET, "/api/summary", &h, Some("0123456789abcdeX")));
        assert!(!widget_key_ok(&Method::GET, "/api/summary", &h, Some("short")));  // no weak keys
        assert!(!widget_key_ok(&Method::GET, "/api/summary", &h, None));           // off unless set
        assert!(!widget_key_ok(&Method::GET, "/api/summary", &HeaderMap::new(), want));
        // the ingest key: POST /api/journal/ingest and nothing else
        assert!(ingest_key_ok(&Method::POST, "/api/journal/ingest", &h, want));
        assert!(!ingest_key_ok(&Method::GET, "/api/journal/day", &h, want));
        assert!(!ingest_key_ok(&Method::POST, "/api/settings", &h, want));
    }
}
