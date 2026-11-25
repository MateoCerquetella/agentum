//! Send later's hub part. The queue lives in the plugin's storage (key `queue`); `start()` checks
//! it every 5 seconds and sends what is due with `cx.send`.
//!
//! RPC: `schedule {thread, text, at}` → the item; `list {thread}` → its items, soonest first;
//! `cancel {id}`. Changes are published on `plugin:scheduled-send` as `{thread}`.
use crate::logic::{check, retry, take_due, Scheduled};
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::SendBody;
use agentum_proto::Ms;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const KEY: &str = "queue";
/// One change to the queue at a time.
static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);
static SEQ: AtomicU64 = AtomicU64::new(0);

fn now() -> Ms {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as Ms).unwrap_or(0)
}

async fn queue(cx: &Ctx) -> Vec<Scheduled> {
    cx.kv_get(KEY).await.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}
async fn store(cx: &Ctx, q: &[Scheduled]) {
    cx.kv_set(KEY, serde_json::to_value(q).unwrap_or(Value::Null)).await;
}

/// Send what is due; failures wait a minute and try again, up to MAX_TRIES.
async fn tick(cx: &Ctx) {
    let due = {
        let _g = LOCK.lock().await;
        let (due, rest) = take_due(queue(cx).await, now());
        if due.is_empty() {
            return;
        }
        store(cx, &rest).await;
        due
    };
    let mut again = vec![];
    for s in due {
        let thread = s.thread.clone();
        if cx.thread(&thread).await.is_none() {
            continue;
        }
        match cx.send(&thread, SendBody { text: s.text.clone(), ..Default::default() }).await {
            Ok(()) => {}
            Err(e) => match retry(s.clone(), e.clone(), now()) {
                Some(r) => again.push(r),
                None => cx.notify("A scheduled message wasn't sent", &format!("{e}: “{}”", s.text.chars().take(80).collect::<String>()), &format!("/code?thread={thread}")).await,
            },
        }
        cx.publish(json!({ "thread": thread }));
    }
    if !again.is_empty() {
        let _g = LOCK.lock().await;
        let mut q = queue(cx).await;
        q.extend(again);
        store(cx, &q).await;
    }
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        match method {
            "schedule" => {
                let thread = input["thread"].as_str().ok_or("which thread?")?.to_string();
                let text = input["text"].as_str().unwrap_or("").trim().to_string();
                let at = input["at"].as_i64().ok_or("when?")?;
                let n = now();
                check(&text, at, n)?;
                cx.thread(&thread).await.ok_or("no such thread")?;
                let s = Scheduled { id: format!("s{n:x}{:x}", SEQ.fetch_add(1, Ordering::Relaxed)), thread: thread.clone(), text, at, created: n, tries: 0, error: None };
                {
                    let _g = LOCK.lock().await;
                    let mut q = queue(cx).await;
                    q.push(s.clone());
                    store(cx, &q).await;
                }
                cx.publish(json!({ "thread": thread }));
                serde_json::to_value(s).map_err(|e| e.to_string())
            }
            "list" => {
                let thread = input["thread"].as_str().ok_or("which thread?")?;
                let mut v: Vec<Scheduled> = queue(cx).await.into_iter().filter(|s| s.thread == thread).collect();
                v.sort_by_key(|s| s.at);
                serde_json::to_value(v).map_err(|e| e.to_string())
            }
            "cancel" => {
                let id = input["id"].as_str().ok_or("which one?")?;
                let gone = {
                    let _g = LOCK.lock().await;
                    let mut q = queue(cx).await;
                    let i = q.iter().position(|s| s.id == id).ok_or("It was already sent.")?;
                    let s = q.remove(i);
                    store(cx, &q).await;
                    s
                };
                cx.publish(json!({ "thread": gone.thread }));
                Ok(json!({ "ok": true }))
            }
            _ => Err(format!("no method {method}")),
        }
    }

    fn start(&self, cx: Ctx) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if cx.enabled().await {
                    tick(&cx).await;
                }
            }
        });
    }
}
