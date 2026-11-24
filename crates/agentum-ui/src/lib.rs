//! Agentum's web kit, shared by the app and every plugin's web part.
//!
//! - [`api`]: JSON calls to the hub (`/api/...`), plugin RPC.
//! - [`ws`]: the page's one WebSocket (`/api/ws`): channel subscriptions as signals.
//! - [`ui`]: toasts, confirm dialogs, modals.
//! - [`nav`]: opening pages and reading/writing URL params from anywhere.
//! - [`icons`]: the icon set.
//! - [`fmt`]: times and sizes for people.
//! - [`store`]: localStorage.
pub mod api;
pub mod fmt;
pub mod icons;
pub mod nav;
pub mod store;
pub mod ui;
pub mod ws;

pub use icons::Icon;

/// The common imports of a page or plugin.
pub mod prelude {
    pub use crate::api::{api, api_delete, api_get, api_post, api_put, rpc, ApiError};
    pub use crate::fmt::{ago, ago_ms, clock, now_ms};
    pub use crate::icons::Icon;
    pub use crate::nav::{navigate, set_param, use_param, use_params, Where};
    pub use crate::ui::{confirm, toast, toast_bad, Modal};
    pub use crate::ws::{on_ws, subscribe};
    pub use leptos::prelude::*;
    pub use leptos::task::spawn_local;
    pub use serde_json::{json, Value};
}
