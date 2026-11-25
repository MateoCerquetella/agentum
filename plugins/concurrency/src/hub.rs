//! Concurrency limit's hub part: before a turn starts, count the threads already running on its
//! machine and refuse when the limit is reached.
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::ThreadState;
use serde_json::{json, Value};

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

/// Running for the limit: starting or working. A thread waiting on a question holds no turn.
fn running(t: &ThreadSummary) -> bool {
    matches!(t.state, ThreadState::Starting | ThreadState::Running)
}

async fn overrides(cx: &Ctx) -> serde_json::Map<String, Value> {
    cx.kv_get(super::OVERRIDES).await.and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

async fn limit_for(cx: &Ctx, machine: &str) -> u64 {
    if let Some(n) = overrides(cx).await.get(machine).and_then(Value::as_f64) {
        return n.max(1.0) as u64;
    }
    cx.setting("limit").await.as_f64().map(|n| n.max(1.0) as u64).unwrap_or(3)
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn before_turn(&self, cx: &Ctx, thread: &ThreadSummary, _text: &str) -> Result<(), String> {
        let limit = limit_for(cx, &thread.machine).await;
        let n = cx.threads().await.iter().filter(|t| t.machine == thread.machine && t.id != thread.id && running(t)).count() as u64;
        if n >= limit {
            let what = if n == 1 { "1 thread is".to_string() } else { format!("{n} threads are") };
            return Err(format!("{what} already running on {}. Stop one, or raise the limit in Settings → Plugins.", thread.machine));
        }
        Ok(())
    }

    /// `overrides` → `{overrides: {machine: n}}`; `set` `{machine, limit: n | null}`.
    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        match method {
            "overrides" => Ok(json!({ "overrides": overrides(cx).await })),
            "set" => {
                let machine = input["machine"].as_str().filter(|m| !m.is_empty()).ok_or("which machine?")?;
                let mut o = overrides(cx).await;
                match input["limit"].as_f64() {
                    Some(n) if (1.0..=64.0).contains(&n) => {
                        o.insert(machine.to_string(), json!(n as u64));
                    }
                    Some(_) => return Err("a limit is 1 to 64".into()),
                    None => {
                        o.remove(machine);
                    }
                }
                cx.kv_set(super::OVERRIDES, Value::Object(o.clone())).await;
                Ok(json!({ "overrides": o }))
            }
            _ => Err(format!("no method {method}")),
        }
    }
}
