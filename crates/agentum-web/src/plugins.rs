//! Every plugin's web part, and the slots the app draws them in. A plugin that is turned off in
//! Settings → Plugins draws nothing.
use crate::pages::PageDef;
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_proto::api::{EventView, ThreadSummary};
use agentum_ui::prelude::*;

/// The plugins built into this app (their hub parts are in crates/agentum/src/plugins.rs).
pub fn all() -> &'static [&'static dyn WebPlugin] {
    &[]
}

thread_local! {
    /// Plugin ids turned off (from /api/plugins).
    static OFF: RwSignal<Vec<String>> = RwSignal::new(vec![]);
    static LOADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub fn off() -> RwSignal<Vec<String>> {
    let s = OFF.with(|o| *o);
    if !LOADED.with(|l| l.replace(true)) {
        spawn_local(async move {
            if let Ok(v) = api_get::<Value>("/plugins").await {
                let off: Vec<String> = v.as_array().into_iter().flatten().filter(|p| p["enabled"] == false).filter_map(|p| p["id"].as_str().map(str::to_string)).collect();
                s.set(off);
            }
        });
    }
    s
}
fn on() -> Vec<&'static dyn WebPlugin> {
    let off = off();
    all().iter().copied().filter(|p| off.with(|o| !o.iter().any(|x| x == p.manifest().id))).collect()
}

pub fn rail_tabs() -> Vec<RailTab> {
    on().into_iter().flat_map(|p| p.rail_tabs().iter().copied()).collect()
}
pub fn thread_header(cx: ThreadCx) -> impl IntoView {
    move || on().into_iter().filter_map(|p| p.thread_header(cx.clone())).collect_view()
}
pub fn thread_row(t: &ThreadSummary) -> impl IntoView + use<> {
    on().into_iter().filter_map(|p| p.thread_row(t)).collect_view()
}
pub fn thread_hover(t: &ThreadSummary) -> Vec<AnyView> {
    on().into_iter().filter_map(|p| p.thread_hover(t)).collect()
}
pub fn message_meta(e: &EventView) -> impl IntoView + use<> {
    on().into_iter().filter_map(|p| p.message_meta(e)).collect_view()
}
pub fn composer_actions(cx: agentum_plugin::web::ComposerCx) -> impl IntoView {
    move || on().into_iter().filter_map(|p| p.composer_actions(cx.clone())).collect_view()
}
/// A fenced code block drawn by a plugin (```ui → genui), if one takes this language.
pub fn code_fence(lang: &str, code: &str) -> Option<AnyView> {
    on().into_iter().find_map(|p| p.code_fence(lang, code))
}
pub fn overlays() -> impl IntoView {
    move || {
        let css: String = on().into_iter().filter_map(|p| p.css()).collect::<Vec<_>>().join("\n");
        view! {
            <style>{css}</style>
            {on().into_iter().filter_map(|p| p.overlay()).collect_view()}
        }
    }
}
pub fn settings_sections() -> Vec<(&'static agentum_plugin::Manifest, Option<AnyView>)> {
    all().iter().map(|p| (p.manifest(), p.settings())).collect()
}

/// Plugin pages: /p/<id>.
pub fn pages() -> Vec<PageDef> {
    all().iter().filter_map(|p| p.nav()).map(|n| crate::pages::plugin_page(n.path.trim_start_matches('/'), n.title, n.icon, n.view)).collect()
}
pub fn page(id: &str) -> Option<PageDef> {
    pages().into_iter().find(|p| p.id == id)
}
/// Plugin pages listed in the Personal sidebar (none: plugin pages are reached from Coding and ⌘K).
pub fn nav_pages() -> Vec<&'static str> {
    vec![]
}
