//! Message times's hub part: `costs {thread}` → `{"<seq of a turn's last message>": cost}`.
use crate::logic::turn_costs;
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use serde_json::{json, Value};

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        match method {
            "costs" => {
                let id = input["thread"].as_str().ok_or("which thread?")?;
                let t = cx.thread(id).await.ok_or("no such thread")?;
                let evs = cx.events(id, t.last_seq.saturating_sub(3000)).await;
                let m: serde_json::Map<String, Value> = turn_costs(&evs).into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
                Ok(Value::Object(m))
            }
            _ => Err(format!("no method {method}")),
        }
    }
}
