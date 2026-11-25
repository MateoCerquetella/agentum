//! Every plugin's web part, and the slots the app draws them in. A plugin that is turned off in
//! Settings → Plugins draws nothing.
use crate::pages::PageDef;
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_proto::api::{EventView, ThreadSummary};
use agentum_ui::prelude::*;

/// The plugins built into this app (their hub parts are in crates/agentum/src/plugins.rs).
static ALL: &[&dyn WebPlugin] = &[
        &agentum_plugin_git::WEB,
        &agentum_plugin_pr::WEB,
        &agentum_plugin_files::WEB,
        &agentum_plugin_editor::WEB,
        &agentum_plugin_history::WEB,
        &agentum_plugin_terminal::WEB,
        &agentum_plugin_hover_cards::WEB,
        &agentum_plugin_timestamps::WEB,
        &agentum_plugin_sticky_prompt::WEB,
        &agentum_plugin_nudger::WEB,
        &agentum_plugin_scheduled_send::WEB,
        &agentum_plugin_side_chat::WEB,
        &agentum_plugin_drafts::WEB,
        &agentum_plugin_usage::WEB,
        &agentum_plugin_context::WEB,
        &agentum_plugin_concurrency::WEB,
        &agentum_plugin_notify::WEB,
        &agentum_plugin_aura::WEB,
        &agentum_plugin_genui::WEB,
        &agentum_plugin_browser::WEB,
        &agentum_plugin_themes::WEB,
    &agentum_plugin_dockside::WEB,
];

pub fn all() -> &'static [&'static dyn WebPlugin] {
    ALL
}

thread_local! {
    /// Plugin ids turned off (from /api/plugins).
    // Until /api/plugins answers, the plugins that are off by default are off.
    static OFF: RwSignal<Vec<String>> = agentum_ui::global(ALL.iter().filter(|p| !p.manifest().default_on).map(|p| p.manifest().id.to_string()).collect());
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
/// Is this plugin turned on (live)?
pub fn is_on(id: &'static str) -> bool {
    off().with(|o| !o.iter().any(|x| x == id))
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
/// Looks added by plugins (id, name, description).
pub fn themes() -> Vec<(&'static str, &'static str, &'static str)> {
    // Runs before the app mounts (the saved look is applied first): read the switch without
    // starting its fetch (no executor yet).
    let off = OFF.with(|o| *o);
    all().iter().filter(|p| off.with_untracked(|o| !o.iter().any(|x| x == p.manifest().id))).flat_map(|p| p.themes().iter().copied()).collect()
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
