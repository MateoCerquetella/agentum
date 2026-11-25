//! Secrets' hub part: the tab's calls, for the thread's machine and working directory (never a
//! path from the browser). Values pass through to agentd and are neither kept nor logged.
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::dotenv::valid_key;
use agentum_proto::machine::{Req, SecretsReq};
use serde_json::Value;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    /// `keys {thread}`, `set {thread, key, value}`, `delete {thread, key}` → {keys, exists, ignored,
    /// tracked, gitignore_added, dir}.
    async fn rpc(&self, cx: &Ctx, method: &str, input: Value) -> Result<Value, String> {
        let id = input["thread"].as_str().ok_or("thread?")?;
        let t = cx.thread(id).await.ok_or("no such thread")?;
        let dir = t.cwd.clone();
        let key = || -> Result<String, String> {
            let k = input["key"].as_str().unwrap_or("").trim().to_string();
            if valid_key(&k) { Ok(k) } else { Err(format!("{k:?} isn't a variable name (letters, digits and _, not starting with a digit)")) }
        };
        let req = match method {
            "keys" => SecretsReq::Keys { dir },
            "set" => SecretsReq::Set { dir, key: key()?, value: input["value"].as_str().ok_or("value?")?.to_string() },
            "delete" => SecretsReq::Delete { dir, key: key()? },
            _ => return Err(format!("no method {method}")),
        };
        cx.machine(&t.machine, Req::Secrets(req)).await
    }

    async fn instructions(&self, _cx: &Ctx, t: &ThreadSummary) -> Option<String> {
        Some(super::instructions(&t.cwd))
    }
}
