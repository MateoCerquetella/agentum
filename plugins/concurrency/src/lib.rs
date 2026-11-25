//! Concurrency limit: At most N threads running per machine.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};

pub static MANIFEST: Manifest = Manifest {
    id: "concurrency",
    name: "Concurrency limit",
    description: "At most N threads running per machine.",
    icon: "layers",
    group: Group::Usage,
    default_on: false,
    settings: &[SettingDef {
        key: "limit",
        label: "Threads per machine",
        help: "How many threads may run at once on each machine (threads waiting on you don't count). Per-machine limits below override it.",
        kind: SettingKind::Number { min: 1.0, max: 64.0 },
        default: "3",
    }],
};

/// The machine override map's key in the plugin's storage: `{machine: limit}`.
pub const OVERRIDES: &str = "overrides";

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
