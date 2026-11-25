//! Files: Browse the thread's working directory.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "files",
    name: "Files",
    description: "Browse the thread's working directory.",
    icon: "folder",
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
