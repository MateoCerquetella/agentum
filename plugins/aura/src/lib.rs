//! Aura: A slow animated colour field behind the app.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};

pub static MANIFEST: Manifest = Manifest {
    id: "aura",
    name: "Aura",
    description: "A slow animated colour field behind the app.",
    icon: "sparkle",
    group: Group::Looks,
    default_on: false,
    settings: &[
        SettingDef { key: "palette", label: "Colours", help: "\"theme\" follows your look's accent; \"custom\" uses the three colours below.", kind: SettingKind::Choice { options: &["aurora", "sunset", "ocean", "forest", "theme", "custom"] }, default: "\"aurora\"" },
        SettingDef { key: "intensity", label: "Intensity", help: "5 (a hint) to 100 (vivid).", kind: SettingKind::Number { min: 5.0, max: 100.0 }, default: "35" },
        SettingDef { key: "motion", label: "Drift", help: "Slowly move the colours (never when your system asks for reduced motion).", kind: SettingKind::Bool, default: "true" },
        SettingDef { key: "colors", label: "Custom colours", help: "Three CSS colours, comma-separated.", kind: SettingKind::Text, default: "\"#7c5cff, #00c2ff, #ff5ca8\"" },
    ],
};

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
