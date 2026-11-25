//! Agentum's web kit, shared by the app and every plugin's web part.
//!
//! - [`api`]: JSON calls to the hub (`/api/...`), plugin RPC.
//! - [`diff`]: unified diffs, parsed and drawn (git, history).
//! - [`ws`]: the page's one WebSocket (`/api/ws`): channel subscriptions as signals.
//! - [`ui`]: toasts, confirm dialogs, modals.
//! - [`nav`]: opening pages and reading/writing URL params from anywhere.
//! - [`icons`]: the icon set.
//! - [`fmt`]: times and sizes for people.
//! - [`store`]: localStorage.
pub mod api;
pub mod diff;
pub mod fmt;
pub mod icons;
pub mod nav;
pub mod store;
pub mod ui;
pub mod ws;

pub use icons::Icon;

/// A signal that lives as long as the page (app-wide state kept in a `thread_local!`). A plain
/// `RwSignal::new` belongs to whatever view is being built and dies with it.
pub fn global<T: Send + Sync + 'static>(value: T) -> leptos::prelude::RwSignal<T> {
    use leptos::prelude::*;
    thread_local! {
        static ROOT: Owner = Owner::new_root(None);
    }
    ROOT.with(|o| o.with(|| RwSignal::new(value)))
}

/// The common imports of a page or plugin.
pub mod prelude {
    pub use crate::api::{api, api_delete, api_get, api_post, api_put, rpc, ApiError};
    pub use crate::fmt::{ago, ago_ms, clock, now_ms};
    pub use crate::icons::Icon;
    pub use crate::nav::{navigate, set_param, use_param, use_params, Where};
    pub use crate::ui::{confirm, toast, toast_bad, Modal};
    pub use crate::ws::{on_ws, subscribe};
    pub use crate::global;
    pub use leptos::prelude::*;
    pub use leptos::task::spawn_local;
    pub use serde_json::{json, Value};
}
