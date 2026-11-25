//! Dockside: the project-first Coding sidebar, after the author's BB plugin. Projects stay put,
//! threads keep their creation order (pinned first), status lives inside each row, and two verbs
//! clear the list: snooze (until tomorrow 09:00, or until it works or asks) and settle (archive,
//! shown for a day). The sidebar itself is drawn by the app (crates/agentum-web/src/dockside.rs);
//! turning this plugin off gives the plain list instead.
use agentum_plugin::{Group, Manifest};

pub static MANIFEST: Manifest = Manifest {
    id: "dockside",
    name: "Dockside",
    description: "A project-first sidebar: stable order, snooze and settle, side chats under their thread. Off: a plain list by recent activity.",
    icon: "layout",
    group: Group::Thread,
    default_on: true,
    settings: &[],
};

#[cfg(feature = "hub")]
mod hub {
    use agentum_plugin::hub::HubPlugin;
    use agentum_plugin::Manifest;
    pub struct Plugin;
    pub static PLUGIN: Plugin = Plugin;
    #[async_trait::async_trait]
    impl HubPlugin for Plugin {
        fn manifest(&self) -> &'static Manifest {
            &super::MANIFEST
        }
    }
}
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web {
    use agentum_plugin::web::WebPlugin;
    use agentum_plugin::Manifest;
    pub struct Plugin;
    pub static PLUGIN: Plugin = Plugin;
    impl WebPlugin for Plugin {
        fn manifest(&self) -> &'static Manifest {
            &super::MANIFEST
        }
    }
}
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;
