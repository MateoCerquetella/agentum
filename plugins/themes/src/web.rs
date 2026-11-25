//! Themes's web part: three looks in Settings → Appearance. Their CSS is
//! crates/agentum-web/style/plugins/agentum-themes.css (in the app's stylesheet, not the wasm).
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn themes(&self) -> &'static [(&'static str, &'static str, &'static str)] {
        &[
            ("monokai", "Monokai", "The editor palette on warm charcoal, mono type"),
            ("lavender", "Lavender", "Your BB Lavender: soft violet, Geist"),
            ("glass", "Glass", "Translucent chrome over a soft wallpaper"),
        ]
    }
}
