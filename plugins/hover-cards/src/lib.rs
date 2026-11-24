//! Hover cards: Hover a thread in the sidebar to see where it stands.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "hover-cards",
    name: "Hover cards",
    description: "Hover a thread in the sidebar to see where it stands.",
    icon: "eye",
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
