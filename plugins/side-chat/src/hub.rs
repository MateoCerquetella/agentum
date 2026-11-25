//! Side chat's hub part: `list {thread}` → its side chats (archived ones too), and every ten
//! minutes the side chats with nothing new for a day are archived.
use crate::logic::{of, quiet};
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::PatchThread;
use agentum_proto::Ms;
use serde_json::{json, Value};
use std::time::Duration;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

fn now() -> Ms {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as Ms).unwrap_or(0)
}

/// Archive the quiet ones; their ids.
pub async fn sweep(cx: &Ctx) -> Vec<String> {
    let mut done = vec![];
    for id in quiet(&cx.threads().await, now()) {
        if cx.patch_thread(&id, PatchThread { archived: Some(true), ..Default::default() }).await.is_ok() {
            done.push(id);
        }
    }
    if !done.is_empty() {
        cx.publish(json!({ "archived": done }));
    }
    done
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        match method {
            "list" => {
                let id = input["thread"].as_str().ok_or("which thread?")?;
                serde_json::to_value(of(&cx.threads().await, id)).map_err(|e| e.to_string())
            }
            "sweep" => Ok(json!({ "archived": sweep(cx).await })),
            _ => Err(format!("no method {method}")),
        }
    }

    fn start(&self, cx: Ctx) {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            loop {
                if cx.enabled().await {
                    sweep(&cx).await;
                }
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
        });
    }
}
