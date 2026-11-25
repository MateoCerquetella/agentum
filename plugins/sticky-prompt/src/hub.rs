//! Sticky prompt's hub part: `last {thread}` → the person's last prompt (or null).
use crate::logic::last_prompt;
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
            "last" => {
                let id = input["thread"].as_str().ok_or("which thread?")?;
                let t = cx.thread(id).await.ok_or("no such thread")?;
                // Recent events first; the whole thread only if no prompt is among them.
                let mut p = last_prompt(&cx.events(id, t.last_seq.saturating_sub(500)).await);
                if p.is_none() && t.last_seq > 500 {
                    p = last_prompt(&cx.events(id, 0).await);
                }
                serde_json::to_value(p).map_err(|e| e.to_string())
            }
            _ => Err(format!("no method {method}")),
        }
    }
}
