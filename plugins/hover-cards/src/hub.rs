//! Hover cards's hub part: `card {thread}` reads the thread's recent events for what its summary
//! doesn't carry (since when it is in its state, the agent's last message at length).
use crate::card::card_of;
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use serde_json::Value;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        match method {
            "card" => {
                let id = input["thread"].as_str().ok_or("which thread?")?;
                let t = cx.thread(id).await.ok_or("no such thread")?;
                let evs = cx.events(id, t.last_seq.saturating_sub(300)).await;
                serde_json::to_value(card_of(&evs)).map_err(|e| e.to_string())
            }
            _ => Err(format!("no method {method}")),
        }
    }
}
