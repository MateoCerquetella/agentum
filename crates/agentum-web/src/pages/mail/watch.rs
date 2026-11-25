//! New mail while Agentum is open: one poller for the whole app, not only the Mail page (a toast
//! also appears on the Dashboard, Today or Calendar), independent of Web Push. Clicking the toast
//! opens the message.
use super::lib::Msg;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use agentum_ui::store;

const SINCE_KEY: &str = "cerq-mail-notified-until";

fn mail_on_screen() -> bool {
    let path = web_sys::window().and_then(|w| w.location().pathname().ok()).unwrap_or_default();
    path.trim_start_matches('/').split('/').next() == Some("mail") || agentum_ui::nav::read().get("side").map(String::as_str) == Some("mail")
}
fn visible() -> bool {
    web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.visibility_state() == web_sys::VisibilityState::Visible)
}

async fn check() {
    if !visible() {
        return;
    }
    let since: i64 = store::get(SINCE_KEY).and_then(|v| v.parse().ok()).unwrap_or(0);
    let Ok(r) = api_get::<Value>(&format!("/mail/new?since={since}")).await else { return };
    let now = r["now"].as_i64().unwrap_or_else(|| (js_sys::Date::now() / 1000.0) as i64);
    // Seed from the hub's clock, and correct a mark a device clock left in the future.
    if since == 0 || since > now {
        store::set(SINCE_KEY, &now.to_string());
        return;
    }
    let fresh: Vec<Msg> = Msg::list(&r["messages"]).into_iter().filter(|m| m.ts > since).collect();
    if fresh.is_empty() {
        return;
    }
    // A future-dated mail must not push the mark past real mail arriving now.
    let top = fresh.iter().map(|m| m.ts).max().unwrap_or(now).min(now);
    store::set(SINCE_KEY, &top.to_string());
    if !super::push::notify_on() || mail_on_screen() {
        return;
    }
    let latest = fresh[0].clone();
    if fresh.len() == 1 {
        let text = format!("{}: {}", latest.who(), if latest.subject.is_empty() { "(no subject)" } else { &latest.subject });
        let id = format!("{}:{}", latest.account, latest.uid);
        agentum_ui::ui::toast_action(text, move || navigate("mail", Where::Replace, &[("msg", &id), ("folder", "INBOX")]));
    } else {
        agentum_ui::ui::toast_action(format!("{} new emails", fresh.len()), || navigate("mail", Where::Replace, &[]));
    }
}

/// Start the poller (once; the shell calls it after sign-in).
pub fn start() {
    thread_local! { static ON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if ON.with(|o| o.replace(true)) {
        return;
    }
    gloo_timers::callback::Timeout::new(4000, || spawn_local(check())).forget();
    gloo_timers::callback::Interval::new(30_000, || spawn_local(check())).forget();
    if let Some(d) = web_sys::window().and_then(|w| w.document()) {
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(|| {
            if visible() {
                spawn_local(check())
            }
        });
        let _ = d.add_event_listener_with_callback("visibilitychange", wasm_bindgen::JsCast::unchecked_ref(cb.as_ref()));
        cb.forget();
    }
}
