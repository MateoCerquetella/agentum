//! Concurrency limit: At most N threads running per machine.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "concurrency",
    name: "Concurrency limit",
    description: "At most N threads running per machine.",
    icon: "layers",
    group: Group::Usage,
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
