//! Nudger's hub part. A turn that ends normally in a thread where Nudger is on starts a countdown
//! (the `delay` setting); when it runs out and the thread is still idle, it reads the turn and,
//! if it looks unfinished, sends the `text` setting. At most `max` times per thread.
//!
//! RPC: `get {thread}`, `set {thread, on}`, `cancel {thread}` → the thread's status, also
//! published on `plugin:nudger` whenever it changes.
use crate::logic::{assess, moved_on, ThreadNudge, Verdict};
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::{SendBody, ThreadSummary};
use agentum_proto::{Event, EventKind, Ms, StopReason};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

/// Countdowns running: thread → (its number, when it sends).
static PENDING: LazyLock<Mutex<HashMap<String, (u64, Ms)>>> = LazyLock::new(Default::default);
static NEXT: AtomicU64 = AtomicU64::new(1);

fn now() -> Ms {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as Ms).unwrap_or(0)
}

async fn load(cx: &Ctx, id: &str) -> ThreadNudge {
    match cx.kv_get(&format!("t:{id}")).await.and_then(|v| serde_json::from_value(v).ok()) {
        Some(st) => st,
        // A thread never touched: on when "On for every thread" is.
        None => ThreadNudge { on: cx.setting("auto").await.as_bool().unwrap_or(true), ..Default::default() },
    }
}
async fn save(cx: &Ctx, id: &str, st: &ThreadNudge) {
    cx.kv_set(&format!("t:{id}"), serde_json::to_value(st).unwrap_or(Value::Null)).await;
}

struct Settings {
    text: String,
    max: u32,
    delay: u64,
}
async fn settings(cx: &Ctx) -> Settings {
    let text = cx.setting("text").await.as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("Continue.").to_string();
    let max = cx.setting("max").await.as_f64().unwrap_or(5.0).clamp(1.0, 50.0) as u32;
    let delay = cx.setting("delay").await.as_f64().unwrap_or(30.0).clamp(3.0, 600.0) as u64;
    Settings { text, max, delay }
}

async fn status(cx: &Ctx, id: &str) -> Value {
    let st = load(cx, id).await;
    let s = settings(cx).await;
    let pending = PENDING.lock().unwrap().get(id).map(|p| p.1);
    json!({ "thread": id, "on": st.on, "count": st.count, "max": s.max, "pending_at": pending, "delay": s.delay, "why": st.why, "nudged": st.nudged, "at": st.at })
}

async fn publish(cx: &Ctx, id: &str) -> Value {
    let s = status(cx, id).await;
    cx.publish(s.clone());
    s
}

/// Stop a countdown; true if there was one.
fn cancel(id: &str) -> bool {
    PENDING.lock().unwrap().remove(id).is_some()
}

/// Start the countdown after a turn that ended at `done_seq`.
async fn arm(cx: &Ctx, t: &ThreadSummary, done_seq: u64) {
    let mut st = load(cx, &t.id).await;
    if !st.on {
        return;
    }
    let s = settings(cx).await;
    if st.count >= s.max {
        st.why = Some(format!("it has nudged {} times here, its limit", st.count));
        st.nudged = false;
        save(cx, &t.id, &st).await;
        publish(cx, &t.id).await;
        return;
    }
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    PENDING.lock().unwrap().insert(t.id.clone(), (n, now() + s.delay as Ms * 1000));
    publish(cx, &t.id).await;
    let (cx, id) = (cx.clone(), t.id.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(s.delay)).await;
        // Still this countdown (not cancelled, not replaced by a newer turn)?
        let mine = {
            let mut p = PENDING.lock().unwrap();
            if p.get(&id).is_some_and(|x| x.0 == n) {
                p.remove(&id);
                true
            } else {
                false
            }
        };
        if mine {
            act(&cx, &id, done_seq, &s.text).await;
            publish(&cx, &id).await;
        }
    });
}

/// The countdown ran out: look again, and nudge if it still looks unfinished.
async fn act(cx: &Ctx, id: &str, done_seq: u64, text: &str) {
    if !cx.enabled().await {
        return;
    }
    let mut st = load(cx, id).await;
    if !st.on {
        return;
    }
    let Some(t) = cx.thread(id).await else { return };
    if t.state.busy() || t.asking.is_some() || t.archived {
        return;
    }
    let evs = cx.events(id, done_seq.saturating_sub(400)).await;
    if moved_on(&evs, done_seq) {
        return;
    }
    st.at = Some(now());
    match assess(&evs, done_seq) {
        Verdict::Nudge(why) => match cx.send(id, SendBody { text: text.to_string(), ..Default::default() }).await {
            Ok(()) => {
                st.count += 1;
                st.nudged = true;
                st.why = Some(why.to_string());
            }
            Err(e) => {
                st.nudged = false;
                st.why = Some(format!("it couldn't send: {e}"));
            }
        },
        Verdict::Leave(why) => {
            st.nudged = false;
            st.why = Some(why.to_string());
        }
    }
    save(cx, id, &st).await;
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        let id = input["thread"].as_str().ok_or("which thread?")?;
        cx.thread(id).await.ok_or("no such thread")?;
        match method {
            "get" => Ok(status(cx, id).await),
            "set" => {
                let on = input["on"].as_bool().ok_or("on?")?;
                let mut st = load(cx, id).await;
                if on && !st.on {
                    st = ThreadNudge { on: true, ..Default::default() };
                }
                st.on = on;
                if !on {
                    cancel(id);
                }
                save(cx, id, &st).await;
                Ok(publish(cx, id).await)
            }
            "cancel" => {
                if cancel(id) {
                    let mut st = load(cx, id).await;
                    st.why = Some("you cancelled the last nudge".into());
                    st.nudged = false;
                    save(cx, id, &st).await;
                }
                Ok(publish(cx, id).await)
            }
            _ => Err(format!("no method {method}")),
        }
    }

    async fn on_event(&self, cx: &Ctx, t: &ThreadSummary, e: &Event) {
        match &e.kind {
            EventKind::TurnDone { stop: StopReason::End, .. } => arm(cx, t, e.seq).await,
            // The thread moved on by itself: no countdown.
            EventKind::User { .. } | EventKind::TurnStarted { .. } | EventKind::Ask { .. } => {
                if cancel(&t.id) {
                    publish(cx, &t.id).await;
                }
            }
            _ => {}
        }
    }
}
