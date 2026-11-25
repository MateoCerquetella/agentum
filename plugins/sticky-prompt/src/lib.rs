//! Sticky prompt: Your last prompt stays at the top of the thread.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "sticky-prompt",
    name: "Sticky prompt",
    description: "Your last prompt stays at the top of the thread.",
    icon: "pin",
    group: Group::Thread,
    default_on: true,
    settings: &[],
};

pub mod logic;

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
