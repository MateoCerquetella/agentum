//! The PR plugin's hub part: each thread's PR cached in storage, refreshed after every turn.
//!
//! - `get {thread, refresh?}` → `{pr, at}` (the cached one if younger than 5 minutes)
//! - `badges` → `{thread: badge}` for every thread with a PR
//! - `merge {thread, method?}` → `{output, pr, at}`
use super::badge;
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::machine::{GitReq, Req};
use agentum_proto::{Event, EventKind};
use serde_json::{json, Value};

pub struct Plugin {
    /// One writer of the `badges` map at a time.
    badges: tokio::sync::Mutex<()>,
}
pub static PLUGIN: Plugin = Plugin { badges: tokio::sync::Mutex::const_new(()) };

const FRESH_MS: i64 = 5 * 60 * 1000;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Plugin {
    /// Ask gh (through agentd) and remember the answer.
    async fn fetch(&self, cx: &Ctx, t: &ThreadSummary) -> Result<Value, String> {
        let pr = cx.machine(&t.machine, Req::Git(GitReq::Pr { cwd: t.cwd.clone() })).await?;
        let at = now_ms();
        let out = json!({"pr": pr, "at": at});
        cx.kv_set(&format!("t:{}", t.id), out.clone()).await;
        let b = (!pr.is_null()).then(|| badge(&pr));
        {
            let _one = self.badges.lock().await;
            let mut all = cx.kv_get("badges").await.filter(Value::is_object).unwrap_or_else(|| json!({}));
            let before = all.get(&t.id).cloned();
            match &b {
                Some(b) => all[&t.id] = b.clone(),
                None => {
                    all.as_object_mut().map(|m| m.remove(&t.id));
                }
            }
            if all.get(&t.id) != before.as_ref() {
                cx.kv_set("badges", all).await;
            }
        }
        cx.publish(json!({"thread": t.id, "badge": b, "pr": pr, "at": at}));
        Ok(out)
    }
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        let thread = || async {
            let id = input["thread"].as_str().ok_or("thread?")?;
            cx.thread(id).await.ok_or_else(|| "no such thread".to_string())
        };
        match method {
            "badges" => Ok(cx.kv_get("badges").await.unwrap_or_else(|| json!({}))),
            "get" => {
                let t = thread().await?;
                if !input["refresh"].as_bool().unwrap_or(false) {
                    if let Some(c) = cx.kv_get(&format!("t:{}", t.id)).await {
                        if now_ms() - c["at"].as_i64().unwrap_or(0) < FRESH_MS {
                            return Ok(c);
                        }
                    }
                }
                self.fetch(cx, &t).await
            }
            "merge" => {
                let t = thread().await?;
                let method = input["method"].as_str().filter(|m| ["squash", "merge", "rebase"].contains(m)).map(str::to_string);
                let r = cx.machine(&t.machine, Req::Git(GitReq::PrMerge { cwd: t.cwd.clone(), method })).await?;
                let mut out = self.fetch(cx, &t).await.unwrap_or_else(|_| json!({}));
                out["output"] = r["output"].clone();
                Ok(out)
            }
            _ => Err(format!("no method {method}")),
        }
    }

    async fn on_event(&self, cx: &Ctx, t: &ThreadSummary, e: &Event) {
        if matches!(e.kind, EventKind::TurnDone { .. }) {
            let _ = self.fetch(cx, t).await;
        }
    }
}
