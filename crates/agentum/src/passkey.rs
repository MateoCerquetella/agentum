//! Touch ID / Face ID: passkeys (WebAuthn). Sign in without the password,
//! and unlock Settings for 10 minutes; while a passkey exists, changing
//! settings, accounts, GitHub tokens, the password or the passkeys needs that
//! unlock (auth.rs guard → locked()). ES256 only (what Apple, Google and
//! Windows platform authenticators make), user verification required,
//! attestation "none". Credentials in kv "auth.passkeys"; the relying party
//! is the host CERQ is opened on (your hub's host name, or localhost in development).
use crate::{auth, state::App, util::*};
use axum::{extract::{Path, State}, http::{HeaderMap, Method, StatusCode}, response::Response, routing::{delete, get, post}, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use ciborium::Value as Cbor;
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Mutex};

const KEY: &str = "auth.passkeys";
const UNLOCK_SECS: i64 = 600;
static CHALLENGES: Mutex<Vec<(String, &'static str, String, i64)>> = Mutex::new(Vec::new());
static UNLOCKED: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/auth/passkey/state", get(state))
        .route("/api/auth/passkey/register/start", post(reg_start))
        .route("/api/auth/passkey/register/finish", post(reg_finish))
        .route("/api/auth/passkey/login/start", post(login_start))
        .route("/api/auth/passkey/login/finish", post(login_finish))
        .route("/api/auth/passkey/unlock/start", post(unlock_start))
        .route("/api/auth/passkey/unlock/finish", post(unlock_finish))
        .route("/api/auth/passkey/unlock/password", post(unlock_password))
        .route("/api/auth/passkey/{id}", delete(remove))
}

async fn creds(app: &App) -> Vec<Value> { app.db.kv_get(KEY).await.ok().flatten().and_then(|v| v.as_array().cloned()).unwrap_or_default() }
fn forbidden(m: &str) -> AppError { AppError(StatusCode::FORBIDDEN, m.into()) }

/// The relying party and the origin a ceremony must come from.
fn rp(h: &HeaderMap) -> (String, String) {
    let host = h.get("x-forwarded-host").or(h.get("host")).and_then(|v| v.to_str().ok()).unwrap_or("localhost").to_string();
    let id = host.split(':').next().unwrap_or("localhost").to_string();
    let origin = h.get("origin").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    (id, origin)
}
fn challenge(purpose: &'static str, user: &str) -> String {
    let c = B64.encode(rand::random::<[u8; 32]>());
    let mut g = CHALLENGES.lock().unwrap();
    let now = now_ts();
    g.retain(|(_, _, _, exp)| *exp > now);
    g.push((c.clone(), purpose, user.to_string(), now + 300));
    c
}
/// Take (once) a live challenge of this purpose: its user.
fn take(c: &str, purpose: &str) -> Option<String> {
    let mut g = CHALLENGES.lock().unwrap();
    let i = g.iter().position(|(x, p, _, exp)| x == c && *p == purpose && *exp > now_ts())?;
    Some(g.remove(i).2)
}
fn unlocked_until(token: &str) -> i64 { UNLOCKED.lock().unwrap().as_ref().and_then(|m| m.get(token).copied()).filter(|t| *t > now_ts()).unwrap_or(0) }

/// A change that needs the Touch ID unlock (only once a passkey exists).
pub async fn locked(app: &App, method: &Method, path: &str, h: &HeaderMap) -> bool {
    let write = method != Method::GET;
    let guarded = write && (path == "/api/settings" || path.starts_with("/api/accounts") || path.starts_with("/api/github") || path == "/api/auth/password"
        || path == "/api/journal/config" || path.starts_with("/api/auth/passkey/register") || (method == Method::DELETE && path.starts_with("/api/auth/passkey/")));
    if !guarded || creds(app).await.is_empty() { return false; }
    auth::token_from(h).map(|t| unlocked_until(&t) == 0).unwrap_or(true)
}

fn b64(v: &Value, k: &str) -> anyhow::Result<Vec<u8>> { Ok(B64.decode(v[k].as_str().unwrap_or("").trim_end_matches('='))?) }

/// clientDataJSON: the type, the challenge we issued, the origin we're on.
fn client_data(raw: &[u8], typ: &str, purpose: &'static str, h: &HeaderMap) -> R<String> {
    let cd: Value = serde_json::from_slice(raw).map_err(|_| bad("bad clientDataJSON"))?;
    if s(&cd, "type") != typ { return Err(bad("wrong ceremony")); }
    let (rp_id, origin) = rp(h);
    let o = s(&cd, "origin");
    let host = url::Url::parse(&o).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
    if host != rp_id || (!origin.is_empty() && origin != o) { return Err(forbidden("This passkey request came from another site.")); }
    take(&s(&cd, "challenge"), purpose).ok_or_else(|| bad("That request expired; try again."))
}
struct Auth { flags: u8, count: u32, cred: Option<(Vec<u8>, Vec<u8>)> }
fn auth_data(d: &[u8], rp_id: &str) -> R<Auth> {
    if d.len() < 37 { return Err(bad("short authenticator data")); }
    if d[..32] != Sha256::digest(rp_id.as_bytes())[..] { return Err(forbidden("This passkey is for another site.")); }
    let flags = d[32];
    if flags & 0x01 == 0 || flags & 0x04 == 0 { return Err(forbidden("Touch ID / Face ID didn't verify you.")); }
    let count = u32::from_be_bytes([d[33], d[34], d[35], d[36]]);
    let cred = if flags & 0x40 != 0 && d.len() > 55 {
        let n = u16::from_be_bytes([d[53], d[54]]) as usize;
        if d.len() < 55 + n { return Err(bad("short credential")); }
        Some((d[55..55 + n].to_vec(), d[55 + n..].to_vec()))
    } else { None };
    Ok(Auth { flags, count, cred })
}
fn int(v: &Cbor) -> Option<i128> { if let Cbor::Integer(i) = v { Some(i128::from(*i)) } else { None } }
/// A COSE EC2 P-256 key (alg -7) as SEC1 bytes.
fn cose_p256(cose: &[u8]) -> R<Vec<u8>> {
    let v: Cbor = ciborium::from_reader(cose).map_err(|_| bad("bad public key"))?;
    let Cbor::Map(m) = v else { return Err(bad("bad public key")) };
    let get = |k: i128| m.iter().find(|(a, _)| int(a) == Some(k)).map(|(_, b)| b);
    if get(1).and_then(int) != Some(2) || get(3).and_then(int) != Some(-7) || get(-1).and_then(int) != Some(1) { return Err(bad("Only ES256 passkeys are supported.")); }
    let (Some(Cbor::Bytes(x)), Some(Cbor::Bytes(y))) = (get(-2), get(-3)) else { return Err(bad("bad public key")) };
    let mut out = vec![4u8];
    out.extend(x); out.extend(y);
    VerifyingKey::from_sec1_bytes(&out).map_err(|_| bad("bad public key"))?;
    Ok(out)
}

async fn state(State(app): State<App>, h: HeaderMap) -> Json<Value> {
    let cs = creds(&app).await;
    let until = auth::token_from(&h).map(|t| unlocked_until(&t)).unwrap_or(0);
    Json(json!({"count": cs.len(), "unlocked_until": until, "passkeys": cs.iter().map(|c| json!({"id": c["id"], "name": c["name"], "created": c["created"], "last_used": c["last_used"]})).collect::<Vec<_>>()}))
}

async fn reg_start(State(app): State<App>, h: HeaderMap) -> R<Json<Value>> {
    let user = auth::user_of(&app, &h).await.ok_or_else(|| forbidden("sign in first"))?;
    let (rp_id, _) = rp(&h);
    let exclude: Vec<Value> = creds(&app).await.iter().map(|c| json!({"type": "public-key", "id": c["id"]})).collect();
    Ok(Json(json!({"publicKey": {"rp": {"id": rp_id, "name": "CERQ"}, "user": {"id": B64.encode(user.as_bytes()), "name": user, "displayName": user},
        "challenge": challenge("register", &user), "pubKeyCredParams": [{"type": "public-key", "alg": -7}], "timeout": 60000, "attestation": "none",
        "authenticatorSelection": {"residentKey": "required", "requireResidentKey": true, "userVerification": "required"}, "excludeCredentials": exclude}})))
}
async fn reg_finish(State(app): State<App>, h: HeaderMap, Json(b): Json<Value>) -> R<Json<Value>> {
    let r = &b["response"];
    let user = client_data(&b64(r, "clientDataJSON")?, "webauthn.create", "register", &h)?;
    let att: Cbor = ciborium::from_reader(&b64(r, "attestationObject")?[..]).map_err(|_| bad("bad attestation"))?;
    let Cbor::Map(m) = att else { return Err(bad("bad attestation")) };
    let Some(Cbor::Bytes(ad)) = m.iter().find(|(k, _)| matches!(k, Cbor::Text(t) if t == "authData")).map(|(_, v)| v) else { return Err(bad("bad attestation")) };
    let (rp_id, _) = rp(&h);
    let a = auth_data(ad, &rp_id)?;
    let (id, cose) = a.cred.ok_or_else(|| bad("no credential"))?;
    let pk = cose_p256(&cose)?;
    let name: String = { let n = s(&b, "name").trim().to_string(); if n.is_empty() { "Passkey".into() } else { n.chars().take(40).collect() } };
    let mut cs = creds(&app).await;
    let idb = B64.encode(&id);
    cs.retain(|c| s(c, "id") != idb);
    cs.push(json!({"id": idb, "name": name, "user": user, "pk": hex::encode(pk), "count": a.count, "flags": a.flags, "created": now_ts(), "last_used": 0, "rp": rp_id}));
    app.db.kv_set(KEY, &json!(cs)).await?;
    Ok(Json(json!({"ok": true, "count": cs.len()})))
}

/// Check an assertion against a stored passkey; bump its counter. Returns the passkey's user.
async fn assert(app: &App, h: &HeaderMap, b: &Value, purpose: &'static str) -> R<String> {
    let r = &b["response"];
    let cdj = b64(r, "clientDataJSON")?;
    let ch_user = client_data(&cdj, "webauthn.get", purpose, h)?;
    let mut cs = creds(app).await;
    let id = B64.encode(b64(b, "rawId").or_else(|_| b64(b, "id"))?);
    let i = cs.iter().position(|c| s(c, "id") == id).ok_or_else(|| forbidden("CERQ doesn't know this passkey."))?;
    let c = cs[i].clone();
    if !ch_user.is_empty() && s(&c, "user") != ch_user { return Err(forbidden("That passkey belongs to someone else.")); }
    let ad = b64(r, "authenticatorData")?;
    let (rp_id, _) = rp(h);
    let a = auth_data(&ad, &rp_id)?;
    let vk = VerifyingKey::from_sec1_bytes(&hex::decode(s(&c, "pk")).map_err(|_| bad("bad stored key"))?).map_err(|_| bad("bad stored key"))?;
    let sig = Signature::from_der(&b64(r, "signature")?).map_err(|_| bad("bad signature"))?;
    let mut msg = ad.clone();
    msg.extend(Sha256::digest(&cdj));
    vk.verify(&msg, &sig).map_err(|_| forbidden("Touch ID / Face ID didn't match."))?;
    let stored = c["count"].as_u64().unwrap_or(0) as u32;
    if a.count != 0 && a.count <= stored { return Err(forbidden("This passkey looks copied; sign in with your password.")); }
    cs[i]["count"] = json!(a.count);
    cs[i]["last_used"] = json!(now_ts());
    app.db.kv_set(KEY, &json!(cs)).await?;
    Ok(s(&c, "user"))
}

async fn login_start(State(app): State<App>, h: HeaderMap) -> R<Json<Value>> {
    if creds(&app).await.is_empty() { return Err(bad("No passkey yet: sign in with your password and add one in Settings.")); }
    let (rp_id, _) = rp(&h);
    Ok(Json(json!({"publicKey": {"challenge": challenge("login", ""), "rpId": rp_id, "timeout": 60000, "userVerification": "required", "allowCredentials": []}})))
}
async fn login_finish(State(app): State<App>, h: HeaderMap, Json(b): Json<Value>) -> R<Response> {
    let user = assert(&app, &h, &b, "login").await?;
    auth::new_session(&app, &user, &h).await
}

async fn unlock_start(State(app): State<App>, h: HeaderMap) -> R<Json<Value>> {
    let user = auth::user_of(&app, &h).await.ok_or_else(|| forbidden("sign in first"))?;
    let allow: Vec<Value> = creds(&app).await.iter().filter(|c| s(c, "user") == user).map(|c| json!({"type": "public-key", "id": c["id"]})).collect();
    if allow.is_empty() { return Err(bad("No passkey for you yet.")); }
    let (rp_id, _) = rp(&h);
    Ok(Json(json!({"publicKey": {"challenge": challenge("unlock", &user), "rpId": rp_id, "timeout": 60000, "userVerification": "required", "allowCredentials": allow}})))
}
async fn unlock_finish(State(app): State<App>, h: HeaderMap, Json(b): Json<Value>) -> R<Json<Value>> {
    assert(&app, &h, &b, "unlock").await?;
    let token = auth::token_from(&h).ok_or_else(|| forbidden("sign in first"))?;
    let until = now_ts() + UNLOCK_SECS;
    let mut g = UNLOCKED.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    m.retain(|_, t| *t > now_ts());
    m.insert(token, until);
    Ok(Json(json!({"ok": true, "unlocked_until": until})))
}
/// The fallback when the device with the passkey isn't at hand: the account password unlocks too.
async fn unlock_password(State(app): State<App>, h: HeaderMap, Json(b): Json<Value>) -> R<Json<Value>> {
    let user = auth::user_of(&app, &h).await.ok_or_else(|| forbidden("sign in first"))?;
    let token = auth::token_from(&h).ok_or_else(|| forbidden("sign in first"))?;
    if !auth::password_ok(&app, &user, &s(&b, "password")).await { return Err(forbidden("Wrong password.")); }
    let until = now_ts() + UNLOCK_SECS;
    UNLOCKED.lock().unwrap().get_or_insert_with(HashMap::new).insert(token, until);
    Ok(Json(json!({"ok": true, "unlocked_until": until})))
}
async fn remove(State(app): State<App>, Path(id): Path<String>) -> R<Json<Value>> {
    let mut cs = creds(&app).await;
    cs.retain(|c| s(c, "id") != id);
    app.db.kv_set(KEY, &json!(cs)).await?;
    Ok(Json(json!({"ok": true, "count": cs.len()})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{signature::Signer, SigningKey};
    #[test]
    fn ceremony_parts() {
        let rp_id = "hub.example.lan";
        let sk = SigningKey::from_bytes(&[7u8; 32].into()).unwrap();
        let pt = sk.verifying_key().to_encoded_point(false);
        // A COSE key and an authenticator data with it, as a platform authenticator makes them.
        let cose = { let mut b = vec![]; ciborium::into_writer(&Cbor::Map(vec![(Cbor::Integer(1.into()), Cbor::Integer(2.into())), (Cbor::Integer(3.into()), Cbor::Integer((-7).into())),
            (Cbor::Integer((-1).into()), Cbor::Integer(1.into())), (Cbor::Integer((-2).into()), Cbor::Bytes(pt.x().unwrap().to_vec())), (Cbor::Integer((-3).into()), Cbor::Bytes(pt.y().unwrap().to_vec()))]), &mut b).unwrap(); b };
        let mut ad = Sha256::digest(rp_id.as_bytes()).to_vec();
        ad.push(0x45); ad.extend(0u32.to_be_bytes()); ad.extend([0u8; 16]); ad.extend(3u16.to_be_bytes()); ad.extend([7, 8, 9]); ad.extend(&cose);
        let a = auth_data(&ad, rp_id).ok().unwrap();
        let (id, key) = a.cred.unwrap();
        assert_eq!(id, vec![7, 8, 9]);
        let pk = cose_p256(&key).ok().unwrap();
        assert!(auth_data(&ad, "evil.example").is_err());                  // another site
        let mut no_uv = ad.clone(); no_uv[32] = 0x41;
        assert!(auth_data(&no_uv, rp_id).is_err());                         // no user verification
        // A signature over authData || sha256(clientDataJSON) verifies with the stored key.
        let cdj = br#"{"type":"webauthn.get","challenge":"x","origin":"https://hub.example.lan"}"#;
        let mut msg = ad[..37].to_vec(); msg.extend(Sha256::digest(cdj));
        let sig: Signature = sk.sign(&msg);
        let vk = VerifyingKey::from_sec1_bytes(&pk).unwrap();
        assert!(vk.verify(&msg, &Signature::from_der(sig.to_der().as_bytes()).unwrap()).is_ok());
        msg[40] ^= 1;
        assert!(vk.verify(&msg, &sig).is_err());
    }
}
