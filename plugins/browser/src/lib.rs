//! Browser: each project's own browser (Surf). Steel in a container on the project's machine;
//! agents drive it over CDP, you watch (and type into) it beside the thread or at /p/browser.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};

pub static MANIFEST: Manifest = Manifest {
    id: "browser",
    name: "Browser",
    description: "Each project's own browser (Surf): agents drive it, you watch it beside the thread.",
    icon: "globe",
    group: Group::Code,
    default_on: true,
    settings: &[
        SettingDef {
            key: "auto_start",
            label: "Start automatically for new threads",
            help: "A new thread starts its project's browser. Off: a project's browser starts when you start it in the thread's Browser tab, and stays on for its threads.",
            kind: SettingKind::Bool,
            default: "false",
        },
        SettingDef {
            key: "idle_hours",
            label: "Stop unused browsers after (hours)",
            help: "A browser no agent has used and nobody has watched for this long is stopped (its sign-ins are kept); the project's next agent starts it again. 0: never.",
            kind: SettingKind::Number { min: 0.0, max: 168.0 },
            default: "4",
        },
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
