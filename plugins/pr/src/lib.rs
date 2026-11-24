//! Pull request: The branch's GitHub pull request: checks, review, merge.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "pr",
    name: "Pull request",
    description: "The branch's GitHub pull request: checks, review, merge.",
    icon: "pr",
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
