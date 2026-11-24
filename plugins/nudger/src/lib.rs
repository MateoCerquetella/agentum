//! Nudger: Keeps a thread going: sends “continue” when it stops early.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "nudger",
    name: "Nudger",
    description: "Keeps a thread going: sends “continue” when it stops early.",
    icon: "refresh",
    group: Group::Thread,
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
