//! Drafts: What you're typing is kept on the hub, for every device.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "drafts",
    name: "Drafts",
    description: "What you're typing is kept on the hub, for every device.",
    icon: "edit",
    group: Group::Thread,
    default_on: true,
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
