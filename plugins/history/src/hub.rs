//! History's hub part.
use agentum_plugin::hub::HubPlugin;
use agentum_plugin::Manifest;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
}
