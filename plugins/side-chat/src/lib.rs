//! Side chat: Ask about a thread in a fork beside it, without disturbing it.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "side-chat",
    name: "Side chat",
    description: "Ask about a thread in a fork beside it, without disturbing it.",
    icon: "chat",
    group: Group::Thread,
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
