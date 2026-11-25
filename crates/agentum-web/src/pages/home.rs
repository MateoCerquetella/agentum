//! Home Assistant, framed (Settings → app → Home Assistant), in Agentum's look: its bridge script
//! (cerq-ha.js on the HA side) gets the scheme and Glance-named colours from our tokens, and is
//! told again on every change of look and whenever it asks.
use super::Pane;
use agentum_ui::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

const TOKENS: [(&str, &str); 11] = [
    ("background", "bg"),
    ("widget-background", "widget"),
    ("widget-background-highlight", "hl"),
    ("widget-content-border", "border"),
    ("text-highlight", "hi"),
    ("text-paragraph", "text"),
    ("text-base", "base"),
    ("text-subdue", "subdue"),
    ("primary", "primary"),
    ("positive", "positive"),
    ("negative", "negative"),
];

/// A colour as plain rgb(), whatever it is written in (Studio's tokens are oklch and color-mix).
pub fn rgb_of(css: &str) -> String {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return String::new() };
    let Some(root) = doc.document_element() else { return String::new() };
    let Ok(probe) = doc.create_element("i") else { return String::new() };
    let _ = probe.set_attribute("style", &format!("color:{css}"));
    let _ = root.append_child(&probe);
    let computed = web_sys::window().and_then(|w| w.get_computed_style(&probe).ok().flatten()).and_then(|s| s.get_property_value("color").ok()).unwrap_or_default();
    probe.remove();
    // A 1×1 canvas turns any colour syntax into bytes.
    let px = doc.create_element("canvas").ok().and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok()).and_then(|c| {
        c.set_width(1);
        c.set_height(1);
        c.get_context("2d").ok().flatten()?.dyn_into::<web_sys::CanvasRenderingContext2d>().ok()
    });
    let Some(g) = px else { return computed };
    g.set_fill_style_str("#000");
    g.set_fill_style_str(&computed);
    g.fill_rect(0.0, 0.0, 1.0, 1.0);
    match g.get_image_data(0.0, 0.0, 1.0, 1.0) {
        Ok(d) => {
            let b = d.data();
            format!("rgb({}, {}, {})", b[0], b[1], b[2])
        }
        Err(_) => computed,
    }
}

fn look() -> JsValue {
    let o = js_sys::Object::new();
    let set = |o: &js_sys::Object, k: &str, v: JsValue| {
        let _ = js_sys::Reflect::set(o, &k.into(), &v);
    };
    let el = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element());
    set(&o, "type", "cerq:theme".into());
    set(&o, "look", el.as_ref().and_then(|e| e.get_attribute("data-theme")).unwrap_or_default().into());
    set(&o, "scheme", if el.and_then(|e| e.get_attribute("data-scheme")).as_deref() == Some("light") { "light" } else { "dark" }.into());
    let colors = js_sys::Object::new();
    for (name, token) in TOKENS {
        set(&colors, name, rgb_of(&format!("var(--{token})")).into());
    }
    set(&o, "colors", colors.into());
    o.into()
}

pub fn page(_pane: Pane) -> AnyView {
    view! { <Home/> }.into_any()
}

#[component]
fn Home() -> impl IntoView {
    let url = RwSignal::new(None::<String>);
    spawn_local(async move {
        let u = api_get::<Value>("/settings").await.ok().and_then(|d| d["ui"]["ha_url"].as_str().map(str::to_string)).unwrap_or_default();
        url.try_set(Some(u));
    });
    let frame = NodeRef::<leptos::html::Iframe>::new();
    let origin = Memo::new(move |_| url.get().and_then(|u| web_sys::Url::new(&u).ok()).map(|u| u.origin()));
    let loaded = RwSignal::new(false);
    let tell = move || {
        if !loaded.get_untracked() {
            return;
        }
        if let (Some(f), Some(o)) = (frame.get_untracked(), origin.get_untracked()) {
            if let Some(w) = f.content_window() {
                let _ = w.post_message(&look(), &o);
            }
        }
    };
    // Once it has loaded, and on every change of look.
    let look_sig = crate::pages::theme::look_signal();
    Effect::new(move |_| {
        look_sig.track();
        tell();
    });
    let asked = window_event_listener(leptos::ev::message, move |e| {
        let from_ha = origin.get_untracked().is_some_and(|o| o == e.origin());
        let kind = js_sys::Reflect::get(&e.data(), &"type".into()).ok().and_then(|v| v.as_string());
        if from_ha && kind.as_deref() == Some("cerq:theme-request") {
            tell();
        }
    });
    on_cleanup(move || asked.remove());
    move || match url.get() {
        None => ().into_any(),
        Some(u) if u.is_empty() => view! { <div class="empty">"Add your Home Assistant address in Settings → App."</div> }.into_any(),
        Some(u) => view! {
            <iframe node_ref=frame src=u title="Home Assistant" style="width:100%;height:100%;border:0;display:block" allow="fullscreen" on:load=move |_| { loaded.set(true); tell() }/>
        }
        .into_any(),
    }
}
