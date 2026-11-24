//! Terminal: Shells on the thread's machine, in its directory.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "terminal",
    name: "Terminal",
    description: "Shells on the thread's machine, in its directory.",
    icon: "terminal",
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
