//! Generative UI's hub part.
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    async fn instructions(&self, _cx: &Ctx, _thread: &ThreadSummary) -> Option<String> {
        Some(super::INSTRUCTIONS.to_string())
    }
}
