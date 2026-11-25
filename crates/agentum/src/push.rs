//! Web Push (owner: mail backend; RFC 8030 / 8291 / 8292). The browser
//! subscribes at its vendor's push service; each message is encrypted for
//! that browser (aes128gcm) and signed with CERQ's VAPID key, so the push
//! service only relays bytes it can't read. The key is data/vapid.pem
//! (imported from cerq-mail so existing subscriptions keep working); the
//! subscriptions live in kv "push.subs". Endpoints must be HTTPS on a known
//! push service, so a subscription can't make the server call arbitrary URLs.
use crate::{state::App, util::*};
use aes_gcm::{aead::{Aead, KeyInit}, Aes128Gcm, Nonce};
use axum::{extract::State, routing::{get, post}, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hkdf::Hkdf;
use p256::{ecdsa::{signature::Signer, Signature, SigningKey}, elliptic_curve::sec1::ToEncodedPoint, pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding}, PublicKey, SecretKey};
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::OnceLock;

const KEY: &str = "push.subs";
const HOSTS: [&str; 4] = ["web.push.apple.com", "fcm.googleapis.com", "updates.push.services.mozilla.com", "notify.windows.com"];

static VAPID: OnceLock<SecretKey> = OnceLock::new();

fn random_key() -> SecretKey {
    loop {
        if let Ok(k) = SecretKey::from_slice(&rand::random::<[u8; 32]>()) { return k; }
    }
}

/// The VAPID key, created (0600) on first use.
fn vapid(app: &App) -> &'static SecretKey {
    VAPID.get_or_init(|| {
        let path = format!("{}/vapid.pem", app.cfg.data_dir);
        if let Some(k) = std::fs::read_to_string(&path).ok().and_then(|p| SecretKey::from_pkcs8_pem(&p).ok()) { return k; }
        let k = random_key();
        match k.to_pkcs8_pem(LineEnding::LF) {
            Ok(pem) => {
                if let Err(e) = std::fs::write(&path, pem.as_bytes()) { tracing::error!("can't save {path}: {e}"); }
                #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)); }
            }
            Err(e) => tracing::error!("vapid key: {e}"),
        }
        k
    })
}

fn raw_public(k: &SecretKey) -> Vec<u8> { k.public_key().to_encoded_point(false).as_bytes().to_vec() }

/// b64url of the uncompressed public point (the browser's applicationServerKey).
pub fn public_key(app: &App) -> String { B64.encode(raw_public(vapid(app))) }

fn unb64(s: &str) -> Vec<u8> { B64.decode(s.trim_end_matches('=')).unwrap_or_default() }

pub async fn subs(app: &App) -> Vec<Value> {
    app.db.kv_get(KEY).await.ok().flatten().and_then(|v| v.as_array().cloned()).unwrap_or_default()
}
async fn save(app: &App, subs: Vec<Value>) -> anyhow::Result<()> { app.db.kv_set(KEY, &Value::Array(subs)).await }

/// A subscription from the browser, checked: a known push service, real keys.
pub fn clean(d: &Value) -> Result<Value, String> {
    let ep = s(d, "endpoint");
    let u = url::Url::parse(&ep).map_err(|_| "not a known push service endpoint")?;
    let host = u.host_str().unwrap_or("").to_lowercase();
    if u.scheme() != "https" || !HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) || ep.len() > 1000 {
        return Err("not a known push service endpoint".into());
    }
    let (p256dh, auth) = (unb64(&s(&d["keys"], "p256dh")), unb64(&s(&d["keys"], "auth")));
    if p256dh.len() != 65 || p256dh[0] != 4 || auth.len() != 16 { return Err("bad subscription keys".into()); }
    Ok(json!({"endpoint": ep, "keys": {"p256dh": B64.encode(&p256dh), "auth": B64.encode(&auth)}, "device": s(d, "device").chars().take(80).collect::<String>(), "added": now_ts()}))
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) { Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, out).expect("hkdf length"); }

/// RFC 8291 aes128gcm, one record, with a given ephemeral key and salt.
pub fn encrypt_with(ua_public: &[u8], auth: &[u8], payload: &[u8], eph: &SecretKey, salt: [u8; 16]) -> anyhow::Result<Vec<u8>> {
    let ua = PublicKey::from_sec1_bytes(ua_public)?;
    let as_public = raw_public(eph);
    let shared = p256::ecdh::diffie_hellman(eph.to_nonzero_scalar(), ua.as_affine());
    let mut ikm = [0u8; 32];
    hkdf(auth, shared.raw_secret_bytes(), &[b"WebPush: info\0".as_slice(), ua_public, &as_public].concat(), &mut ikm);
    let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
    hkdf(&salt, &ikm, b"Content-Encoding: aes128gcm\0", &mut cek);
    hkdf(&salt, &ikm, b"Content-Encoding: nonce\0", &mut nonce);
    let body = Aes128Gcm::new(&cek.into()).encrypt(Nonce::from_slice(&nonce), [payload, &[2u8]].concat().as_slice()).map_err(|_| anyhow::anyhow!("encrypt"))?;
    Ok([&salt[..], &4096u32.to_be_bytes(), &[as_public.len() as u8], &as_public, &body].concat())
}

/// ES256 VAPID JWT for the endpoint's origin, valid 12 h.
fn jwt(app: &App, endpoint: &str) -> anyhow::Result<String> {
    let u = url::Url::parse(endpoint)?;
    let contact = std::env::var("CERQ_PUSH_CONTACT").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "mailto:cerq@example.com".into());
    let head = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(json!({"aud": u.origin().ascii_serialization(), "exp": now_ts() + 12 * 3600, "sub": contact}).to_string());
    let signing = format!("{head}.{claims}");
    let sig: Signature = SigningKey::from(vapid(app)).sign(signing.as_bytes());
    Ok(format!("{signing}.{}", B64.encode(sig.to_bytes())))
}

/// One push; returns the HTTP status (0 on a network error).
async fn send(app: &App, sub: &Value, message: &Value) -> u16 {
    let ep = s(sub, "endpoint");
    let mut payload = message.to_string().into_bytes();
    payload.truncate(3000);
    let body = match encrypt_with(&unb64(&s(&sub["keys"], "p256dh")), &unb64(&s(&sub["keys"], "auth")), &payload, &random_key(), rand::random()) {
        Ok(b) => b,
        Err(e) => { tracing::warn!("push: {e:#}"); return 0; }
    };
    let Ok(token) = jwt(app, &ep) else { return 0 };
    let r = app.http.post(&ep).header("Authorization", format!("vapid t={token}, k={}", public_key(app)))
        .header("Content-Encoding", "aes128gcm").header("Content-Type", "application/octet-stream").header("TTL", "86400").header("Urgency", "high")
        .body(body).timeout(std::time::Duration::from_secs(15)).send().await;
    match r {
        Ok(r) => r.status().as_u16(),
        Err(e) => { tracing::warn!("push: network error {e}"); 0 }
    }
}

/// Push to every device; subscriptions the browser dropped (404/410) go.
/// Returns (sent, removed).
pub async fn broadcast(app: &App, message: &Value) -> (usize, usize) {
    // Dev copies of the data must not buzz the owner's real devices.
    if std::env::var("CERQ_NO_PUSH").is_ok() { tracing::info!("push suppressed (CERQ_NO_PUSH): {}", message["title"]); return (0, 0); }
    let (mut sent, mut gone, all) = (0, vec![], subs(app).await);
    if all.is_empty() { tracing::info!("push: no device has notifications on, dropped: {}", message["title"]); return (0, 0); }
    for sub in all {
        match send(app, &sub, message).await {
            200..=299 => sent += 1,
            404 | 410 => gone.push(s(&sub, "endpoint")),
            code => tracing::warn!("push: HTTP {code} from {}", url::Url::parse(&s(&sub, "endpoint")).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default()),
        }
    }
    if !gone.is_empty() {
        tracing::info!("push: removed {} device(s) the browser dropped", gone.len());
        let keep = subs(app).await.into_iter().filter(|x| !gone.contains(&s(x, "endpoint"))).collect();
        if let Err(e) = save(app, keep).await { tracing::warn!("push: {e:#}"); }
    }
    (sent, gone.len())
}

/// Send {title, body, tag, url} to every subscribed device. Returns devices reached.
pub async fn send_all(app: &App, payload: Value) -> usize { broadcast(app, &payload).await.0 }

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/push/key", get(|State(app): State<App>| async move { Json(json!({"key": public_key(&app), "devices": subs(&app).await.len()})) }))
        .route("/api/push/subscribe", post(subscribe))
        .route("/api/push/unsubscribe", post(unsubscribe))
        .route("/api/push/test", post(|State(app): State<App>| async move {
            let (sent, removed) = broadcast(&app, &json!({"title": "cerq", "body": "Mail notifications are on for this device.", "tag": "cerq-test", "url": "/?open=mail"})).await;
            Json(json!({"sent": sent, "removed": removed}))
        }))
}

async fn subscribe(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let sub = clean(&d).map_err(bad)?;
    let ep = s(&sub, "endpoint");
    let mut all: Vec<Value> = subs(&app).await.into_iter().filter(|x| s(x, "endpoint") != ep).collect();
    all.push(sub.clone());
    if all.len() > 20 { all.drain(..all.len() - 20); }
    let n = all.len();
    save(&app, all).await?;
    Ok(Json(json!({"ok": true, "devices": n, "device": sub["device"]})))
}

async fn unsubscribe(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let ep = s(&d, "endpoint");
    let all: Vec<Value> = subs(&app).await.into_iter().filter(|x| s(x, "endpoint") != ep).collect();
    let n = all.len();
    save(&app, all).await?;
    Ok(Json(json!({"ok": true, "devices": n})))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8291 Appendix A.
    #[test]
    fn rfc8291_vector() {
        let as_private = SecretKey::from_slice(&unb64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let ua_public = unb64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let auth = unb64("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = unb64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let out = encrypt_with(&ua_public, &auth, b"When I grow up, I want to be a watermelon", &as_private, salt).unwrap();
        assert_eq!(B64.encode(out), "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN");
    }

    #[test]
    fn endpoint_allowlist() {
        let keys = json!({"p256dh": "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4", "auth": "BTBZMqHH6r4Tts7J_aSIgg"});
        assert!(clean(&json!({"endpoint": "https://web.push.apple.com/abc", "keys": keys})).is_ok());
        assert!(clean(&json!({"endpoint": "https://fcm.googleapis.com/fcm/send/x", "keys": keys})).is_ok());
        assert!(clean(&json!({"endpoint": "https://evil.com/fcm.googleapis.com", "keys": keys})).is_err());
        assert!(clean(&json!({"endpoint": "http://fcm.googleapis.com/x", "keys": keys})).is_err());
        assert!(clean(&json!({"endpoint": "https://notfcm.googleapis.com.evil.io/x", "keys": keys})).is_err());
        assert!(clean(&json!({"endpoint": "https://fcm.googleapis.com/x", "keys": {"p256dh": "AAAA", "auth": "BTBZMqHH6r4Tts7J_aSIgg"}})).is_err());
    }
}
