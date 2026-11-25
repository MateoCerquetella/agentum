//! Agentum's web app: Leptos in the browser (WebAssembly), served by the hub.
mod app;
mod code;
mod dockside;
mod launcher;
mod pages;
mod palette;
mod plugins;
mod side;

fn main() {
    console_error_panic_hook::set_once();
    pages::theme::apply_saved();
    let root = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("root")).map(wasm_bindgen::JsCast::unchecked_into::<web_sys::HtmlElement>);
    match root {
        Some(r) => leptos::mount::mount_to(r, app::App).forget(),
        None => leptos::mount::mount_to_body(app::App),
    }
    register_service_worker();
}

/// Push notifications and the home-screen app need the service worker (https only).
fn register_service_worker() {
    let Some(w) = web_sys::window() else { return };
    if w.location().protocol().unwrap_or_default() != "https:" {
        return;
    }
    let sw = w.navigator().service_worker();
    let _ = sw.register("/sw.js");
}
