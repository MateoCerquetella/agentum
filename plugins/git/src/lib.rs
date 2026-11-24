//! Git: Status, diffs, commit and push for the thread's checkout.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "git",
    name: "Git",
    description: "Status, diffs, commit and push for the thread's checkout.",
    icon: "git",
    group: Group::Code,
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
