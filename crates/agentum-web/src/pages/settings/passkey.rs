//! Touch ID / Face ID (passkeys, WebAuthn): sign in, unlock Settings, add one. The hub
//! (crates/agentum/src/passkey.rs) sends the options with base64url fields; these turn them into
//! what the browser wants (bytes) and the answer back.
use agentum_ui::prelude::*;
use js_sys::{Array, ArrayBuffer, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn to_b64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 3);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}
fn from_b64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' | b'+' => Some(62),
        b'_' | b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (vec![], 0u32, 0);
    for v in s.bytes().filter_map(val) {
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn bytes(v: &[u8]) -> JsValue {
    Uint8Array::from(v).buffer().into()
}
fn field_b64(o: &JsValue, k: &str) -> Option<String> {
    let v = Reflect::get(o, &k.into()).ok().filter(|v| !v.is_null() && !v.is_undefined())?;
    let buf: ArrayBuffer = v.dyn_into().ok()?;
    Some(to_b64(&Uint8Array::new(&buf).to_vec()))
}

/// The hub's publicKey options with their ids and challenge as bytes.
fn options(pk: &Value) -> Result<JsValue, String> {
    let o: JsValue = js_sys::JSON::parse(&pk.to_string()).map_err(|_| "bad options".to_string())?;
    let set = |o: &JsValue, k: &str, v: JsValue| {
        let _ = Reflect::set(o, &k.into(), &v);
    };
    set(&o, "challenge", bytes(&from_b64(pk["challenge"].as_str().unwrap_or(""))));
    if let Some(id) = pk["user"]["id"].as_str() {
        let user = Reflect::get(&o, &"user".into()).unwrap_or(JsValue::NULL);
        set(&user, "id", bytes(&from_b64(id)));
    }
    for k in ["excludeCredentials", "allowCredentials"] {
        if let Some(list) = pk[k].as_array() {
            let arr = Array::new();
            for c in list {
                let e = Object::new();
                set(&e, "type", "public-key".into());
                set(&e, "id", bytes(&from_b64(c["id"].as_str().unwrap_or(""))));
                arr.push(&e);
            }
            set(&o, k, arr.into());
        }
    }
    let outer = Object::new();
    set(&outer, "publicKey", o);
    Ok(outer.into())
}

/// A cancelled prompt is Err("") (nothing to show).
fn js_err(e: JsValue) -> String {
    let name = Reflect::get(&e, &"name".into()).ok().and_then(|n| n.as_string()).unwrap_or_default();
    if name == "NotAllowedError" || name == "AbortError" {
        return String::new();
    }
    Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()).filter(|m| !m.is_empty()).unwrap_or_else(|| format!("{e:?}"))
}

fn container() -> Result<web_sys::CredentialsContainer, String> {
    web_sys::window().map(|w| w.navigator().credentials()).ok_or_else(|| "no window".to_string())
}

/// Ask the browser for a passkey (`get`) and send its signature to `finish`.
async fn assert(start: &str, finish: &str) -> Result<Value, String> {
    let s: Value = api_post(start, &json!({})).await.map_err(|e| e.message)?;
    let opts = options(&s["publicKey"])?;
    let p = container()?.get_with_options(opts.unchecked_ref()).map_err(js_err)?;
    let c = JsFuture::from(p).await.map_err(js_err)?;
    let r = Reflect::get(&c, &"response".into()).unwrap_or(JsValue::NULL);
    let body = json!({
        "id": Reflect::get(&c, &"id".into()).ok().and_then(|v| v.as_string()),
        "rawId": field_b64(&c, "rawId"),
        "type": "public-key",
        "response": {
            "clientDataJSON": field_b64(&r, "clientDataJSON"),
            "authenticatorData": field_b64(&r, "authenticatorData"),
            "signature": field_b64(&r, "signature"),
            "userHandle": field_b64(&r, "userHandle"),
        }
    });
    api_post::<Value>(finish, &body).await.map_err(|e| e.message)
}

/// Passkeys need a secure page (https, or localhost) and a browser that has them.
pub fn works() -> bool {
    web_sys::window().is_some_and(|w| w.is_secure_context() && Reflect::has(&w, &"PublicKeyCredential".into()).unwrap_or(false))
}

/// A name for the passkey this device makes.
pub fn device_name() -> &'static str {
    let ua = web_sys::window().and_then(|w| w.navigator().user_agent().ok()).unwrap_or_default();
    if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Mac") {
        "Mac"
    } else if ua.contains("Android") {
        "Android"
    } else {
        "computer"
    }
}

pub async fn sign_in() -> Result<Value, String> {
    assert("/auth/passkey/login/start", "/auth/passkey/login/finish").await
}

pub async fn unlock() -> Result<Value, String> {
    assert("/auth/passkey/unlock/start", "/auth/passkey/unlock/finish").await
}

/// Make a passkey on this device and register it.
pub async fn add(name: &str) -> Result<Value, String> {
    let s: Value = api_post("/auth/passkey/register/start", &json!({})).await.map_err(|e| e.message)?;
    let opts = options(&s["publicKey"])?;
    let p = container()?.create_with_options(opts.unchecked_ref()).map_err(js_err)?;
    let c = JsFuture::from(p).await.map_err(js_err)?;
    let r = Reflect::get(&c, &"response".into()).unwrap_or(JsValue::NULL);
    let body = json!({
        "id": Reflect::get(&c, &"id".into()).ok().and_then(|v| v.as_string()),
        "rawId": field_b64(&c, "rawId"),
        "type": "public-key",
        "name": name,
        "response": { "clientDataJSON": field_b64(&r, "clientDataJSON"), "attestationObject": field_b64(&r, "attestationObject") }
    });
    api_post::<Value>("/auth/passkey/register/finish", &body).await.map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    #[test]
    fn b64_round_trip() {
        for s in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\xff\xfe\x00\x01"] {
            assert_eq!(super::from_b64(&super::to_b64(s)), s);
        }
        assert_eq!(super::to_b64(b"foobar"), "Zm9vYmFy");
        assert_eq!(super::to_b64(b"\xfb\xff"), "-_8");
    }
}
