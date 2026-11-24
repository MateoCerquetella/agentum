//! Accounts and Google sign-in. Google's OAuth client is a "desktop app",
//! so it only sends you back to a loopback address: after consent your
//! browser lands on http://127.0.0.1:8765/?code=… (a page that doesn't
//! load) and you paste that address into CERQ, which finishes the sign-in.
//! One consent covers mail (when that account signs in to IMAP with Google)
//! and calendars.
use crate::{secrets, state::App, util::*};
use axum::{extract::State, routing::{get, post}, Json, Router};
use base64::Engine;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::Digest;
use std::{collections::HashMap, sync::Mutex};

const REDIRECT: &str = "http://127.0.0.1:8765/";
const MAIL_SCOPE: &str = "https://mail.google.com/";
const CAL_SCOPE: &str = "https://www.googleapis.com/auth/calendar";

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/accounts", get(list))
        .route("/api/accounts/google/start", post(start))
        .route("/api/accounts/google/finish", post(finish))
}

struct Pending { verifier: String, email: String, at: i64 }
static PENDING: Mutex<Option<HashMap<String, Pending>>> = Mutex::new(None);

/// Every account with what it's used for and its last sync error.
async fn list(State(app): State<App>) -> R<Json<Value>> {
    let mail: Vec<(String, String, String, String)> = app.db.call(|c| {
        let mut st = c.prepare("SELECT id, coalesce(name,''), lower(coalesce(imap_user,'')), coalesce(oauth_provider,'') FROM email_accounts")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect()
    }).await?;
    let cals: Vec<(String, String, String, String)> = app.db.call(|c| {
        let mut st = c.prepare("SELECT id, coalesce(label,''), lower(coalesce(username,'')), coalesce(kind,'') FROM calendar_accounts")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect()
    }).await?;
    let runs = app.db.kv_get("mail.state").await?.and_then(|s| s["runs"].as_array().and_then(|r| r.last().cloned())).unwrap_or(json!({}));
    let cal_result = crate::calendar::last_sync_result();
    let cal_checked = !cal_result.is_null();
    let cal_errors: Vec<String> = cal_result["errors"].as_array().cloned().unwrap_or_default().iter().filter_map(|e| e.as_str().map(String::from)).collect();
    let mut by_email: Vec<Value> = vec![];
    for (_, name, email, oauth) in &mail {
        let i = entry(&mut by_email, email);
        let err = runs["accounts"][name]["error"].as_str().map(String::from);
        by_email[i]["mail"] = json!({"name": name, "auth": if oauth.is_empty() { "password" } else { oauth.as_str() }, "error": err});
        by_email[i]["name"] = json!(name);
    }
    for (_, label, email, kind) in &cals {
        let i = entry(&mut by_email, email);
        let err = cal_errors.iter().find(|e| e.starts_with(&format!("{label}:"))).map(|e| e[label.len() + 1..].trim().to_string());
        by_email[i]["calendar"] = json!({"label": label, "kind": kind, "error": err, "checked": cal_checked});
        if by_email[i]["name"].is_null() { by_email[i]["name"] = json!(label.split(" (").next().unwrap_or(label)); }
    }
    for e in by_email.iter_mut() {
        let google = e["mail"]["auth"] == "google" || e["calendar"]["kind"] == "google";
        e["google"] = json!(google);
    }
    Ok(Json(json!({"accounts": by_email, "google_ready": !env("GOOGLE_OAUTH_CLIENT_ID").is_empty() && !env("GOOGLE_OAUTH_CLIENT_SECRET").is_empty()})))
}

fn entry(list: &mut Vec<Value>, email: &str) -> usize {
    if let Some(i) = list.iter().position(|e| e["email"] == email) { return i; }
    list.push(json!({"email": email, "mail": null, "calendar": null}));
    list.len() - 1
}

fn env(k: &str) -> String { std::env::var(k).unwrap_or_default() }
fn b64(b: &[u8]) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b) }

/// POST {email} → {url}: open it, sign in, then paste where you land.
async fn start(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    if env("GOOGLE_OAUTH_CLIENT_ID").is_empty() { return Err(bad("GOOGLE_OAUTH_CLIENT_ID isn't set on the server (/etc/cerq.env).")); }
    let email = s(&b, "email").trim().to_lowercase();
    let e2 = email.clone();
    let google_mail: bool = app.db.call(move |c| c.query_row("SELECT count(*) FROM email_accounts WHERE lower(imap_user)=?1 AND oauth_provider='google'", [e2], |r| r.get::<_, i64>(0))).await? > 0;
    let scope = if google_mail { format!("{MAIL_SCOPE} {CAL_SCOPE} email openid") } else { format!("{CAL_SCOPE} email openid") };
    let verifier = b64(&rand::random::<[u8; 32]>());
    let challenge = b64(&sha2::Sha256::digest(verifier.as_bytes()));
    let state = hex::encode(rand::random::<[u8; 12]>());
    {
        let mut g = PENDING.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        m.retain(|_, p| now_ts() - p.at < 1800);
        m.insert(state.clone(), Pending { verifier, email: email.clone(), at: now_ts() });
    }
    let mut u = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth")?;
    u.query_pairs_mut().append_pair("client_id", &env("GOOGLE_OAUTH_CLIENT_ID")).append_pair("redirect_uri", REDIRECT)
        .append_pair("response_type", "code").append_pair("scope", &scope).append_pair("access_type", "offline")
        .append_pair("prompt", "consent").append_pair("state", &state).append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    if !email.is_empty() { u.query_pairs_mut().append_pair("login_hint", &email); }
    Ok(Json(json!({"url": u.to_string(), "mail": google_mail})))
}

/// POST {url: "http://127.0.0.1:8765/?state=…&code=…"} → stores the new
/// refresh token for that account's mail and calendars, then syncs.
async fn finish(State(app): State<App>, Json(b): Json<Value>) -> R<Json<Value>> {
    let pasted = s(&b, "url");
    let u = url::Url::parse(pasted.trim()).map_err(|_| bad("Paste the whole address, starting with http://127.0.0.1"))?;
    let q: HashMap<String, String> = u.query_pairs().into_owned().collect();
    if let Some(e) = q.get("error") { return Err(bad(format!("Google said: {e}"))); }
    let (code, state) = (q.get("code").ok_or_else(|| bad("That address has no code in it."))?, q.get("state").cloned().unwrap_or_default());
    let p = PENDING.lock().unwrap().get_or_insert_with(HashMap::new).remove(&state).ok_or_else(|| bad("This sign-in expired or was already used. Start again."))?;
    let r = app.http.post("https://oauth2.googleapis.com/token").form(&[
        ("client_id", env("GOOGLE_OAUTH_CLIENT_ID")), ("client_secret", env("GOOGLE_OAUTH_CLIENT_SECRET")), ("code", code.clone()),
        ("code_verifier", p.verifier), ("redirect_uri", REDIRECT.into()), ("grant_type", "authorization_code".into())]).send().await?;
    let st = r.status();
    let tok: Value = r.json().await.unwrap_or(json!({}));
    if !st.is_success() { return Err(bad(format!("Google refused the sign-in: {} {}", s(&tok, "error"), s(&tok, "error_description")))); }
    let refresh = s(&tok, "refresh_token");
    if refresh.is_empty() { return Err(bad("Google sent no refresh token. Start again (CERQ asks for consent each time).")); }
    let info: Value = app.http.get("https://openidconnect.googleapis.com/v1/userinfo").bearer_auth(s(&tok, "access_token")).send().await?.json().await.unwrap_or(json!({}));
    let email = s(&info, "email").to_lowercase();
    if email.is_empty() { return Err(bad("Couldn't read which Google account this is.")); }
    if !p.email.is_empty() && p.email != email { return Err(bad(format!("You signed in as {email}, not {}. Start again and pick the right account.", p.email))); }
    let scopes = s(&tok, "scope");
    let enc = secrets::encrypt(&refresh);
    let (e2, enc2, sc) = (email.clone(), enc.clone(), scopes.clone());
    let updated: Vec<String> = app.db.call(move |c| {
        let mut out = vec![];
        let now = chrono::Utc::now().to_rfc3339();
        if sc.contains(MAIL_SCOPE) {
            let n = c.execute("UPDATE email_accounts SET oauth_refresh_token=?1, oauth_access_token=NULL, enabled=1, updated_at=?3 WHERE lower(imap_user)=?2 AND oauth_provider='google'", rusqlite::params![enc2, e2, now])?;
            if n > 0 { out.push("mail".to_string()); }
        }
        if sc.contains(CAL_SCOPE) {
            let id: Option<String> = c.query_row("SELECT id FROM calendar_accounts WHERE lower(username)=?1 AND kind='google'", [&e2], |r| r.get(0)).optional()?;
            match id {
                Some(id) => { c.execute("UPDATE calendar_accounts SET password=?1 WHERE id=?2", [&enc2, &id])?; }
                None => {
                    let label = format!("{} (Google)", e2.split('@').next().unwrap_or(&e2));
                    c.execute("INSERT INTO calendar_accounts(id,label,kind,url,username,password,created_at) VALUES(?1,?2,'google',?3,?4,?5,?6)",
                        rusqlite::params![uuid::Uuid::new_v4().to_string(), label, format!("https://apidata.googleusercontent.com/caldav/v2/{e2}/user"), e2, enc2, now])?;
                }
            }
            out.push("calendar".to_string());
        }
        Ok(out)
    }).await?;
    tracing::info!("google sign-in for {email}: {}", updated.join(", "));
    // Sync both right away so the status turns green.
    let a = app.clone();
    tokio::spawn(async move { crate::mail::sync_mail_now(&a).await; crate::calendar::sync_now(&a, true).await; a.emit(json!({"kind": "accounts"})); });
    Ok(Json(json!({"ok": true, "email": email, "updated": updated})))
}
