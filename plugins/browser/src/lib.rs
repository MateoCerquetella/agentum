//! Browser: The agents' browser (Steel), beside your threads.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "browser",
    name: "Browser",
    description: "The agents' browser (Steel), beside your threads.",
    icon: "globe",
    group: Group::Looks,
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
