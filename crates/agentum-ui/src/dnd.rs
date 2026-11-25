//! Drag something (a thread, a page, a panel tab) onto the left or right half of the app to open it
//! there. Drag sources call [`start`]; the shell shows drop zones while [`dragging`] is on.
use leptos::prelude::*;
use std::collections::BTreeMap;

pub const MIME: &str = "text/agentum-open";

thread_local! {
    static DRAGGING: RwSignal<bool> = crate::global(false);
}

/// Something is being dragged that the panes can take.
pub fn dragging() -> RwSignal<bool> {
    DRAGGING.with(|d| *d)
}

/// What a drop opens: a page and its params.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Open {
    pub page: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

/// Call from `on:dragstart`.
pub fn start(e: &web_sys::DragEvent, page: &str, params: &[(&str, &str)]) {
    let o = Open { page: page.to_string(), params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
    if let Some(dt) = e.data_transfer() {
        let _ = dt.set_data(MIME, &serde_json::to_string(&o).unwrap_or_default());
        dt.set_effect_allowed("copyMove");
    }
    // Not during dragstart itself: Chrome cancels a drag whose source gets covered right away.
    gloo_timers::callback::Timeout::new(0, || dragging().set(true)).forget();
}

/// Read a drop.
pub fn read(e: &web_sys::DragEvent) -> Option<Open> {
    let s = e.data_transfer()?.get_data(MIME).ok()?;
    serde_json::from_str(&s).ok()
}

/// The drag ended (dropped anywhere, or cancelled).
pub fn end() {
    dragging().set(false);
}
