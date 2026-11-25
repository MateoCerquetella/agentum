//! Themes: More looks: Monokai, Lavender, Glass.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "themes",
    name: "Themes",
    description: "More looks: Monokai, Lavender, Glass.",
    icon: "palette",
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
