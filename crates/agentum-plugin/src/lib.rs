//! The Agentum plugin API.
//!
//! A plugin is a crate under `plugins/<id>/` with up to two parts, like a BB plugin:
//!
//! - a **hub part** (feature `hub`): a [`hub::HubPlugin`] that answers RPC calls from its web
//!   part, reacts to thread events and may run background work. It reaches machines, threads,
//!   storage and notifications only through the [`hub::HubCtx`] it is given.
//! - a **web part** (feature `web`): a [`web::WebPlugin`] whose Leptos views the app mounts in
//!   slots (a tab in the thread's side panel, the thread header, a sidebar row, a message, the
//!   composer, a code fence, Settings, a page of its own, an overlay).
//!
//! Both parts share one [`Manifest`]. The hub lists every hub part in `crates/agentum/src/plugins.rs`
//! and the web app every web part in `crates/agentum-web/src/plugins.rs`; Settings → Plugins turns
//! each one on or off.
use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// git, PRs, files, the editor, history, terminals.
    Code,
    /// What happens around a thread: hover cards, timestamps, drafts, nudges, side chats.
    Thread,
    /// Usage, context, limits, notifications.
    Usage,
    /// Themes, aura, generative UI, the browser.
    Looks,
}

#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum SettingKind {
    Bool,
    Number { min: f64, max: f64 },
    Text,
    Choice { options: &'static [&'static str] },
}

#[derive(Serialize, Clone, Copy, Debug)]
pub struct SettingDef {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    #[serde(flatten)]
    pub kind: SettingKind,
    /// As JSON text: "true", "30", "\"auto\"".
    pub default: &'static str,
}

#[derive(Serialize, Clone, Copy, Debug)]
pub struct Manifest {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// An icon name from agentum-ui's icon set.
    pub icon: &'static str,
    pub group: Group,
    /// On for a new install.
    pub default_on: bool,
    pub settings: &'static [SettingDef],
}

#[cfg(feature = "hub")]
pub mod hub {
    use super::Manifest;
    use agentum_proto::api::{NewThread, PatchThread, SendBody, ThreadSummary};
    use agentum_proto::machine::Req;
    use agentum_proto::Event;
    use serde_json::Value;
    use std::sync::Arc;

    pub type Ctx = Arc<dyn HubCtx>;

    /// What a plugin's hub part may do. Each plugin gets its own, namespaced by its id.
    #[async_trait::async_trait]
    pub trait HubCtx: Send + Sync {
        fn plugin(&self) -> &'static str;
        /// Call agentd on a machine (files, git, terminals…).
        async fn machine(&self, machine: &str, req: Req) -> Result<Value, String>;
        async fn thread(&self, id: &str) -> Option<ThreadSummary>;
        async fn threads(&self) -> Vec<ThreadSummary>;
        /// A thread's stored events after `after` (0: all), oldest first.
        async fn events(&self, id: &str, after: u64) -> Vec<Event>;
        async fn send(&self, id: &str, body: SendBody) -> Result<(), String>;
        async fn new_thread(&self, body: NewThread) -> Result<ThreadSummary, String>;
        /// Change a thread the way the person can (rename, pin, archive…).
        async fn patch_thread(&self, _id: &str, _patch: PatchThread) -> Result<ThreadSummary, String> {
            Err("not supported".into())
        }
        /// The plugin's own key-value storage.
        async fn kv_get(&self, key: &str) -> Option<Value>;
        async fn kv_set(&self, key: &str, value: Value);
        async fn kv_delete(&self, key: &str);
        /// A setting from the manifest, as saved (or its default).
        async fn setting(&self, key: &str) -> Value;
        /// Is this plugin turned on?
        async fn enabled(&self) -> bool;
        /// To every open page subscribed to "plugin:<id>".
        fn publish(&self, payload: Value);
        /// A push notification to your devices (and a toast in open pages).
        async fn notify(&self, title: &str, body: &str, url: &str);
        /// The hub's HTTP client (for plugins that call outside services).
        fn http(&self) -> Option<Value> {
            None
        }
    }

    #[async_trait::async_trait]
    pub trait HubPlugin: Send + Sync + 'static {
        fn manifest(&self) -> &'static Manifest;
        /// `POST /api/p/<id>/<method>` with a JSON body.
        async fn rpc(&self, _cx: &Ctx, method: &str, _input: Value) -> Result<Value, String> {
            Err(format!("no method {method}"))
        }
        /// Every stored event of every thread, after it is saved (only when the plugin is on).
        async fn on_event(&self, _cx: &Ctx, _thread: &ThreadSummary, _event: &Event) {}
        /// Once at boot: start background work (check `cx.enabled()` as you go).
        fn start(&self, _cx: Ctx) {}
        /// Before a thread starts a turn (a new thread, or a send): Err(why) refuses it, and the
        /// person sees why (a concurrency limit, a budget).
        async fn before_turn(&self, _cx: &Ctx, _thread: &ThreadSummary, _text: &str) -> Result<(), String> {
            Ok(())
        }
        /// Text added to the agent's system prompt when a thread's agent starts (after
        /// `before_turn`): how to use what the plugin offers (the project's browser…).
        fn instructions(&self, _cx: &Ctx, _thread: &ThreadSummary) -> Option<String> {
            None
        }
    }
}

#[cfg(feature = "web")]
pub mod web {
    use super::Manifest;
    use agentum_proto::api::{EventView, SendBody, ThreadSummary};
    use leptos::prelude::*;

    /// The thread a slot is shown for.
    #[derive(Clone)]
    pub struct ThreadCx {
        pub id: String,
        /// Its summary, live.
        pub summary: Signal<Option<ThreadSummary>>,
        /// The side panel's open tab (a [`RailTab`] id; None: closed). Set it to show another
        /// tab (Files opens a file in the Editor's).
        pub rail: RwSignal<Option<String>>,
    }

    /// The composer a slot is shown in.
    #[derive(Clone)]
    pub struct ComposerCx {
        /// None: the composer of a new thread.
        pub thread: Option<String>,
        pub text: RwSignal<String>,
        /// Send what is in the composer with these options (the plugin may set `send_at`, `mode`).
        pub send: Callback<SendBody>,
    }

    /// A tab in a thread's side panel (Files, Git, History, Terminal, Browser…).
    #[derive(Clone, Copy)]
    pub struct RailTab {
        pub id: &'static str,
        pub title: &'static str,
        pub icon: &'static str,
        pub view: fn(ThreadCx) -> AnyView,
    }

    /// A page of its own in the sidebar (Usage, Browser…).
    #[derive(Clone, Copy)]
    pub struct NavEntry {
        /// The URL path: `/p/<plugin>`.
        pub path: &'static str,
        pub title: &'static str,
        pub icon: &'static str,
        pub view: fn() -> AnyView,
    }

    pub trait WebPlugin: Send + Sync + 'static {
        fn manifest(&self) -> &'static Manifest;
        fn rail_tabs(&self) -> &'static [RailTab] {
            &[]
        }
        fn thread_header(&self, _cx: ThreadCx) -> Option<AnyView> {
            None
        }
        /// Small badges on a thread's row in the sidebar.
        fn thread_row(&self, _thread: &ThreadSummary) -> Option<AnyView> {
            None
        }
        /// A popover when hovering a thread's row.
        fn thread_hover(&self, _thread: &ThreadSummary) -> Option<AnyView> {
            None
        }
        /// Under a message in the timeline (time, cost…).
        fn message_meta(&self, _event: &EventView) -> Option<AnyView> {
            None
        }
        /// Buttons next to Send.
        fn composer_actions(&self, _cx: ComposerCx) -> Option<AnyView> {
            None
        }
        /// Draws a fenced code block of this language instead of the default (```ui → genui).
        fn code_fence(&self, _lang: &str, _code: &str) -> Option<AnyView> {
            None
        }
        /// Its section in Settings → Plugins.
        fn settings(&self) -> Option<AnyView> {
            None
        }
        /// Drawn once over (or under) the whole app: backgrounds, toasts.
        fn overlay(&self) -> Option<AnyView> {
            None
        }
        fn nav(&self) -> Option<NavEntry> {
            None
        }
        /// Extra CSS for the whole app (themes).
        fn css(&self) -> Option<&'static str> {
            None
        }
        /// Looks it adds to Settings → Appearance: (id, name, description). Their CSS (in `css`)
        /// targets `html[data-theme="<id>"]`.
        fn themes(&self) -> &'static [(&'static str, &'static str, &'static str)] {
            &[]
        }
    }
}
