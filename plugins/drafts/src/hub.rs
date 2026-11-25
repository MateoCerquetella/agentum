//! Drafts's hub part: `get {thread}` → the draft or null; `set {thread, text, at}` (an empty
//! text deletes it). Kept in the plugin's storage, `d:<thread>`.
use crate::logic::Draft;
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
        let id = input["thread"].as_str().ok_or("which thread?")?;
        let key = format!("d:{id}");
        match method {
            "get" => Ok(cx.kv_get(&key).await.unwrap_or(Value::Null)),
            "set" => {
                let text = input["text"].as_str().unwrap_or("");
                let at = input["at"].as_i64().ok_or("when?")?;
                if text.trim().is_empty() {
                    cx.kv_delete(&key).await;
                } else {
                    // A late save from another device doesn't overwrite a newer draft.
                    let old: Option<Draft> = cx.kv_get(&key).await.and_then(|v| serde_json::from_value(v).ok());
                    if old.is_some_and(|o| o.at > at) {
                        return Ok(json!({ "ok": false, "newer": true }));
                    }
                    cx.kv_set(&key, serde_json::to_value(Draft { text: text.chars().take(100_000).collect(), at }).unwrap_or(Value::Null)).await;
                }
                Ok(json!({ "ok": true }))
            }
            _ => Err(format!("no method {method}")),
        }
    }
}
