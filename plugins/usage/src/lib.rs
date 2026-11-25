//! Usage: Each agent's limits, tokens and cost by day and project.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "usage",
    name: "Usage",
    description: "Each agent's limits, tokens and cost by day and project.",
    icon: "chart",
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
