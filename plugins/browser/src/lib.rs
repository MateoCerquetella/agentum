//! Browser: The agents' browser (Steel), beside your threads.
use agentum_plugin::{Group, Manifest, SettingDef, SettingKind};

pub static MANIFEST: Manifest = Manifest {
    id: "browser",
    name: "Browser",
    description: "The agents' browser (Steel), beside your threads.",
    icon: "globe",
    group: Group::Looks,
    default_on: true,
    settings: &[
        SettingDef { key: "api_url", label: "Steel API", help: "Where Steel's API answers, like http://my-desktop:3000 (its /v1/sessions lists the browsers).", kind: SettingKind::Text, default: "\"\"" },
        SettingDef { key: "viewer_url", label: "Viewer", help: "The page that shows the browser, like http://my-desktop:5173 (Steel's UI). Empty: a session's own viewer.", kind: SettingKind::Text, default: "\"\"" },
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
