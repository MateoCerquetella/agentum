//! Aura: A slow animated colour field behind the app.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "aura",
    name: "Aura",
    description: "A slow animated colour field behind the app.",
    icon: "sparkle",
    group: Group::Looks,
    default_on: false,
    settings: &[],
};

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
