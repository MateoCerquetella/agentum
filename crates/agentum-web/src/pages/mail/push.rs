//! Mail › Web Push for this device: the hub (push.rs) sends new mail to the browser, so
//! notifications arrive with Agentum closed (Safari, the iPhone home-screen app, Chrome, Firefox).
use agentum_ui::prelude::*;
use agentum_ui::store;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

pub const NOTIFY_KEY: &str = "cerq-mail-notify";
pub fn notify_on() -> bool {
    store::get(NOTIFY_KEY).as_deref() != Some("off")
}
fn has(obj: &JsValue, k: &str) -> bool {
    js_sys::Reflect::has(obj, &k.into()).unwrap_or(false)
}
pub fn pushable() -> bool {
    let Some(w) = web_sys::window() else { return false };
    has(&w.navigator(), "serviceWorker") && has(&w, "PushManager") && has(&w, "Notification")
}
fn ua() -> String {
    web_sys::window().and_then(|w| w.navigator().user_agent().ok()).unwrap_or_default()
}
fn ios_like() -> bool {
    let u = ua();
    u.contains("iPhone") || u.contains("iPad") || u.contains("iPod") || (web_sys::window().is_some_and(|w| w.navigator().platform().ok().as_deref() == Some("MacIntel") && w.navigator().max_touch_points() > 1))
}
fn standalone() -> bool {
    web_sys::window().and_then(|w| w.match_media("(display-mode: standalone)").ok().flatten()).is_some_and(|m| m.matches())
}
fn device_name() -> &'static str {
    let u = ua();
    if u.contains("iPhone") {
        "iPhone"
    } else if ios_like() {
        "iPad"
    } else if u.contains("Edg/") {
        "Edge"
    } else if u.contains("Chrome/") {
        "Chrome"
    } else if u.contains("Firefox/") {
        "Firefox"
    } else if u.contains("Safari/") {
        "Safari (Mac)"
    } else {
        "Browser"
    }
}
fn b64bytes(s: &str) -> js_sys::Uint8Array {
    let t: String = s.replace('-', "+").replace('_', "/");
    let pad = (4 - t.len() % 4) % 4;
    let bin = web_sys::window().and_then(|w| w.atob(&format!("{t}{}", "=".repeat(pad))).ok()).unwrap_or_default();
    let bytes: Vec<u8> = bin.chars().map(|c| c as u32 as u8).collect();
    js_sys::Uint8Array::from(bytes.as_slice())
}
async fn registration() -> Option<web_sys::ServiceWorkerRegistration> {
    let sw = web_sys::window()?.navigator().service_worker();
    JsFuture::from(sw.get_registration()).await.ok().filter(|r| !r.is_undefined()).map(|r| r.unchecked_into())
}
async fn get_sub() -> Option<web_sys::PushSubscription> {
    if !pushable() {
        return None;
    }
    let reg = registration().await?;
    let p = reg.push_manager().ok()?.get_subscription().ok()?;
    JsFuture::from(p).await.ok().filter(|s| !s.is_null() && !s.is_undefined()).map(|s| s.unchecked_into())
}
fn sub_json(sub: &web_sys::PushSubscription) -> Value {
    let s: String = js_sys::JSON::stringify(sub).ok().and_then(|s| s.as_string()).unwrap_or_default();
    let mut v: Value = serde_json::from_str(&s).unwrap_or(json!({}));
    v["device"] = json!(device_name());
    v
}
fn granted() -> bool {
    web_sys::Notification::permission() == web_sys::NotificationPermission::Granted
}

thread_local! {
    static CHECKED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}
/// Is push on for this device? The first call also re-posts the subscription so the hub's copy stays current.
pub async fn push_state() -> bool {
    if let Some(v) = CHECKED.with(|c| c.get()) {
        return v;
    }
    let sub = get_sub().await;
    let on = sub.is_some() && pushable() && granted();
    if let (true, Some(s)) = (on, sub) {
        let body = sub_json(&s);
        spawn_local(async move {
            let _ = api_post::<Value>("/push/subscribe", &body).await;
        });
    }
    CHECKED.with(|c| c.set(Some(on)));
    on
}

/// The service worker, registered now if it isn't yet; the real reason when the browser refuses it.
async fn worker() -> Result<web_sys::ServiceWorkerRegistration, String> {
    let loc = web_sys::window().map(|w| w.location()).ok_or("no window")?;
    if loc.protocol().unwrap_or_default() != "https:" {
        return Err(format!("Notifications need Agentum over HTTPS: open https://{} instead.", loc.hostname().unwrap_or_default()));
    }
    if let Some(r) = registration().await {
        return Ok(r);
    }
    let sw = web_sys::window().ok_or("no window")?.navigator().service_worker();
    match JsFuture::from(sw.register("/sw.js")).await {
        Ok(_) => JsFuture::from(sw.ready().map_err(|_| "no service worker")?).await.map(|r| r.unchecked_into()).map_err(|e| format!("{e:?}")),
        Err(e) => {
            let msg = js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_default();
            let name = js_sys::Reflect::get(&e, &"name".into()).ok().and_then(|m| m.as_string()).unwrap_or_default();
            let all = format!("{name} {msg}");
            Err(if all.contains("SSL") || all.contains("certificate") || all.contains("Security") {
                "This device doesn't trust Agentum's certificate, so the browser won't run its notifications. Install the certificate (Settings → Notifications), restart the browser, then try again.".into()
            } else {
                format!("The browser refused Agentum's service worker: {msg}")
            })
        }
    }
}

pub async fn enable_push() -> Result<(), String> {
    if !pushable() {
        return Err(if ios_like() && !standalone() { "On iPhone: add Agentum to the Home Screen first, open it from there, then tap the bell.".into() } else { "This browser can't do push notifications.".into() });
    }
    // Safari only asks if this runs straight from the tap: nothing awaited before it.
    let p = web_sys::Notification::request_permission().map_err(|_| "Notifications are blocked".to_string())?;
    let perm = JsFuture::from(p).await.ok().and_then(|v| v.as_string()).unwrap_or_default();
    if perm != "granted" {
        return Err("Notifications are blocked for this site. Allow them in the browser's settings, then tap the bell again.".into());
    }
    let reg = worker().await?;
    let k: Value = api_get("/push/key").await.map_err(|e| e.message)?;
    let pm = reg.push_manager().map_err(|_| "no PushManager")?;
    let existing = JsFuture::from(pm.get_subscription().map_err(|_| "push")?).await.ok().filter(|s| !s.is_null() && !s.is_undefined());
    let sub: web_sys::PushSubscription = match existing {
        Some(s) => s.unchecked_into(),
        None => {
            let o = web_sys::PushSubscriptionOptionsInit::new();
            o.set_user_visible_only(true);
            o.set_application_server_key(&b64bytes(k["key"].as_str().unwrap_or("")));
            JsFuture::from(pm.subscribe_with_options(&o).map_err(|_| "push")?).await.map_err(|e| format!("{e:?}"))?.unchecked_into()
        }
    };
    api_post::<Value>("/push/subscribe", &sub_json(&sub)).await.map_err(|e| e.message)?;
    store::set(NOTIFY_KEY, "on");
    CHECKED.with(|c| c.set(Some(true)));
    Ok(())
}

pub async fn disable_push() {
    if let Some(sub) = get_sub().await {
        let _ = api_post::<Value>("/push/unsubscribe", &json!({"endpoint": sub.endpoint()})).await;
        if let Ok(p) = sub.unsubscribe() {
            let _ = JsFuture::from(p).await;
        }
    }
    store::set(NOTIFY_KEY, "off");
    CHECKED.with(|c| c.set(Some(false)));
}
