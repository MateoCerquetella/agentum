//! Message times: The time (and cost) under every message.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};

pub static MANIFEST: Manifest = Manifest {
    id: "timestamps",
    name: "Message times",
    description: "The time (and cost) under every message.",
    icon: "clock",
    group: Group::Thread,
    default_on: true,
    settings: &[SettingDef {
        key: "clock",
        label: "Clock",
        help: "24-hour (14:05) or 12-hour (2:05 PM).",
        kind: SettingKind::Choice { options: &["24h", "12h"] },
        default: "\"24h\"",
    }],
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
