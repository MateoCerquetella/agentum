//! The pages. Each is `fn(Pane) -> AnyView` in its own module; the shell shows one or two side
//! by side. `Side` says which half of the sidebar a page belongs to.
use agentum_ui::prelude::*;

pub mod calendar;
pub mod chat;
pub mod dashboard;
pub mod home;
pub mod login;
pub mod mail;
pub mod notes;
pub mod settings;
pub mod theme;
pub mod today;

/// Which pane a page is in (pages keep their own URL params; both panes share the URL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Left,
    Right,
}

/// Personal (your pages and private chats) or Coding (threads on your machines).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Life,
    Work,
}

#[derive(Clone, Copy)]
pub struct PageDef {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    pub side: Option<Side>,
    pub view: fn(Pane) -> AnyView,
    /// A plugin's page (its view doesn't take the pane).
    pub plugin: Option<fn() -> AnyView>,
}
impl PageDef {
    pub fn render(&self, pane: Pane) -> AnyView {
        match self.plugin {
            Some(f) => f(),
            None => (self.view)(pane),
        }
    }
}
fn none(_: Pane) -> AnyView {
    ().into_any()
}
/// A plugin page's definition.
pub fn plugin_page(id: &'static str, title: &'static str, icon: &'static str, f: fn() -> AnyView) -> PageDef {
    PageDef { id, title, icon, side: Some(Side::Work), view: none, plugin: Some(f) }
}

pub const PAGES: &[PageDef] = &[
    PageDef { id: "today", title: "Today", icon: "today", side: Some(Side::Life), view: today::page, plugin: None },
    PageDef { id: "dashboard", title: "Dashboard", icon: "dashboard", side: Some(Side::Life), view: dashboard::page, plugin: None },
    PageDef { id: "mail", title: "Mail", icon: "mail", side: Some(Side::Life), view: mail::page, plugin: None },
    PageDef { id: "calendar", title: "Calendar", icon: "calendar", side: Some(Side::Life), view: calendar::page, plugin: None },
    PageDef { id: "notes", title: "Notes", icon: "notes", side: Some(Side::Life), view: notes::page, plugin: None },
    PageDef { id: "chat", title: "Secretary", icon: "lock", side: Some(Side::Life), view: chat::page, plugin: None },
    PageDef { id: "home", title: "Home", icon: "house", side: Some(Side::Life), view: home::page, plugin: None },
    PageDef { id: "code", title: "Code", icon: "bot", side: Some(Side::Work), view: crate::code::page, plugin: None },
    PageDef { id: "settings", title: "Settings", icon: "settings", side: None, view: settings::page, plugin: None },
];

/// The sidebar's default order of personal pages (Today opens from the Dashboard's brief; the
/// Secretary has its own card).
pub const NAV: &[&str] = &["dashboard", "mail", "calendar", "notes"];

pub fn def(id: &str) -> Option<PageDef> {
    PAGES.iter().copied().find(|p| p.id == id).or_else(|| crate::plugins::page(id))
}

/// A page that isn't built yet.
pub fn todo(name: &'static str) -> AnyView {
    view! { <div class="empty">{name}" is on its way."</div> }.into_any()
}
