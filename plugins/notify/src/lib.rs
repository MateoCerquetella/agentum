//! Notifications: A push when a thread needs you or finishes.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "notify",
    name: "Notifications",
    description: "A push when a thread needs you or finishes.",
    icon: "bell",
    group: Group::Usage,
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
