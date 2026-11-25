//! Generative UI: Draws ```ui blocks agents write as live cards, charts and tables.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "genui",
    name: "Generative UI",
    description: "Draws ```ui blocks agents write as live cards, charts and tables.",
    icon: "dashboard",
    group: Group::Looks,
    default_on: true,
    settings: &[],
};

pub mod lang;

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
