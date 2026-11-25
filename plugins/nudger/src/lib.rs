//! Nudger: Keeps a thread going: sends “continue” when it stops early.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};

pub static MANIFEST: Manifest = Manifest {
    id: "nudger",
    name: "Nudger",
    description: "Keeps a thread going: sends “continue” when it stops early.",
    icon: "refresh",
    group: Group::Thread,
    default_on: false,
    settings: &[
        SettingDef { key: "text", label: "Message", help: "What it sends to keep a thread going.", kind: SettingKind::Text, default: "\"Continue.\"" },
        SettingDef {
            key: "max",
            label: "Nudges per thread",
            help: "It stops after this many in one thread (turning it off and on there starts the count again).",
            kind: SettingKind::Number { min: 1.0, max: 50.0 },
            default: "5",
        },
        SettingDef {
            key: "delay",
            label: "Wait (seconds)",
            help: "How long after the agent stops before it nudges; you can cancel meanwhile.",
            kind: SettingKind::Number { min: 3.0, max: 600.0 },
            default: "30",
        },
    ],
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
