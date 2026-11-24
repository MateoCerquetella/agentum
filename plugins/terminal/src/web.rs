//! Terminal's web part.
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
}
