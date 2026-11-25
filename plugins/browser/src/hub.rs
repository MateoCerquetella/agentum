//! Browser's hub part: asks Steel for its sessions (the page can't: Steel is on another origin and
//! usually on the machine's network only).
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use serde_json::{json, Value};
use std::time::Duration;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(8)).build().unwrap_or_default())
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    /// `sessions` → `{sessions: [Steel's session objects]}`.
    async fn rpc(&self, cx: &Ctx, method: &str, _input: Value) -> Result<Value, String> {
        if method != "sessions" {
            return Err(format!("no method {method}"));
        }
        let base = cx.setting("api_url").await.as_str().unwrap_or("").trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return Ok(json!({ "sessions": [], "configured": false }));
        }
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err("the Steel API address starts with http:// or https://".into());
        }
        let res = client().get(format!("{base}/v1/sessions")).send().await.map_err(|e| format!("Steel at {base} didn't answer: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("Steel at {base} answered {}", res.status()));
        }
        let v: Value = res.json().await.map_err(|e| format!("Steel's answer isn't JSON: {e}"))?;
        let sessions = v.get("sessions").cloned().or_else(|| v.is_array().then(|| v.clone())).unwrap_or_else(|| json!([]));
        Ok(json!({ "sessions": sessions, "configured": true }))
    }
}
