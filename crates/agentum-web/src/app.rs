//! The app: sign-in gate, then the shell (sidebar + one or two panes).
//!
//! URL = `/<left page>[?side=<right page>&<params>]`. Opening a page that is already on screen
//! keeps it where it is and updates its params; Alt/⌘-click opens it on the right.
use crate::pages::{self, login::Login, theme, Pane, Side};
use agentum_ui::nav::{self, Where};
use agentum_ui::prelude::*;
use agentum_ui::store;
use agentum_ui::ui::UiLayer;
use std::collections::BTreeMap;

#[component]
pub fn App() -> impl IntoView {
    // None: asking; Some(None): signed out; Some(Some(me)): signed in.
    let me = RwSignal::new(None::<Option<Value>>);
    agentum_ui::api::on_logout(move || me.set(Some(None)));
    spawn_local(async move {
        match api_get::<Value>("/auth/me").await {
            Ok(m) => me.set(Some(Some(m))),
            Err(_) => me.set(Some(None)),
        }
    });
    view! {
        {move || match me.get() {
            None => ().into_any(),
            Some(None) => view! { <Login on_done=Callback::new(move |u| me.set(Some(Some(u))))/> }.into_any(),
            Some(Some(_)) => view! { <Shell/> }.into_any(),
        }}
        <UiLayer/>
        {crate::plugins::overlays()}
    }
}

fn read_url() -> (String, Option<String>) {
    let path = web_sys::window().and_then(|w| w.location().pathname().ok()).unwrap_or_default();
    let seg = path.trim_start_matches('/').split('/').next().unwrap_or("").to_string();
    // A plugin's page: /p/<id>.
    let seg = if seg == "p" { path.trim_start_matches('/').splitn(3, '/').take(2).collect::<Vec<_>>().join("/") } else { seg };
    let q = nav::read();
    let legacy = q.get("open").cloned().filter(|p| pages::def(p).is_some());
    let left = legacy.unwrap_or_else(|| if pages::def(&seg).is_some() { seg } else { "dashboard".into() });
    // The Code page may be on both sides (two threads); other pages once.
    let right = q.get("side").cloned().filter(|s| pages::def(s).is_some() && (*s != left || s == "code"));
    (left, right)
}

fn side_of(p: &str) -> Option<Side> {
    pages::def(p).and_then(|d| d.side)
}

#[component]
fn Shell() -> impl IntoView {
    let (l0, r0) = read_url();
    let left = RwSignal::new(l0);
    let right = RwSignal::new(r0);
    let maxed = RwSignal::new(None::<Pane>);
    let ratio = RwSignal::new(store::get("cerq.split").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.5));
    let collapsed = RwSignal::new(store::get("cerq.sidebar").as_deref() == Some("collapsed"));
    let hidden = RwSignal::new(store::flag("cerq.sidebar.hidden"));
    let side_w = RwSignal::new(store::get("cerq.sidebar.w").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0));
    let mobile_open = RwSignal::new(false);
    let palette = RwSignal::new(false);
    let side = RwSignal::new(side_of(&left.get_untracked()).unwrap_or(if store::get("cerq.side").as_deref() == Some("work") { Side::Work } else { Side::Life }));
    let brand = RwSignal::new("Agentum".to_string());
    let ui_prefs = RwSignal::new(Value::Null);
    let counts = RwSignal::new(BTreeMap::<String, u64>::new());

    let write = move |l: String, r: Option<String>, params: BTreeMap<String, String>, push: bool| {
        let mut q = params;
        if let Some(r) = &r {
            q.insert("side".into(), r.clone());
        }
        let url = format!("/{l}{}", nav::query(&q));
        if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
            let _ = if push { h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url)) } else { h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url)) };
        }
        left.set(l);
        right.set(r);
        maxed.set(None);
        nav::refresh();
    };
    let open = move |p: &str, at: Where, params: BTreeMap<String, String>| {
        mobile_open.set(false);
        let (cl, cr) = (left.get_untracked(), right.get_untracked());
        let p = p.to_string();
        // A Code page on the right keeps its own params, prefixed "r_" (its thread, machine…).
        let right_code_kept = |r: &Option<String>, fresh: &BTreeMap<String, String>| -> BTreeMap<String, String> {
            let mut q = fresh.clone();
            if r.as_deref() == Some("code") && cr.as_deref() == Some("code") {
                for (k, v) in nav::read().into_iter().filter(|(k, _)| k.starts_with("r_")) {
                    q.entry(k).or_insert(v);
                }
            }
            q
        };
        if p == "code" {
            let right_side = matches!(at, Where::Right) || (matches!(at, Where::Replace) && cl != "code" && cr.as_deref() == Some("code"));
            if right_side {
                // Into the right pane; the left keeps its page and params.
                let mut q: BTreeMap<String, String> = nav::read().into_iter().filter(|(k, _)| !k.starts_with("r_") && k != "side").collect();
                for (k, v) in params {
                    q.insert(format!("r_{k}"), v);
                }
                return write(cl, Some(p), q, true);
            }
            let r = if cl == "code" { cr.clone() } else if matches!(at, Where::Left) { cr.clone().or(Some(cl.clone())) } else { cr.clone() };
            let q = right_code_kept(&r, &params);
            return write(p, r, q, true);
        }
        let params = right_code_kept(&cr, &params);
        match at {
            Where::Left => {
                let r = if cl == p { cr } else if cr.as_deref() == Some(&p) { Some(cl) } else { cr.or(Some(cl)) };
                write(p, r, params, true)
            }
            Where::Right => {
                let l = if cl == p { cr.clone().unwrap_or_else(|| "dashboard".into()) } else { cl };
                write(l, Some(p), params, true)
            }
            Where::Replace => {
                if cl == p || cr.as_deref() == Some(&p) { write(cl, cr, params, true) } else { write(p, cr, params, true) }
            }
        }
    };
    nav::set_opener(open);

    // The sidebar's side follows the left page, and remembers the page it was on.
    Effect::new(move |_| {
        let l = left.get();
        if let Some(s) = side_of(&l) {
            side.set(s);
            store::set("cerq.side", if s == Side::Work { "work" } else { "life" });
            store::set(if s == Side::Work { "cerq.side.work" } else { "cerq.side.life" }, &l);
        }
    });

    let _pop = window_event_listener(leptos::ev::popstate, move |_| {
        let (l, r) = read_url();
        left.set(l);
        right.set(r);
        nav::refresh();
    });

    let load_settings = move || {
        spawn_local(async move {
            if let Ok(d) = api_get::<Value>("/settings").await {
                brand.set(d["brand"].as_str().filter(|b| !b.is_empty() && !b.eq_ignore_ascii_case("cerq")).unwrap_or("Agentum").to_string());
                theme::adopt_look(&d["ui"]["look"]);
                ui_prefs.set(d["ui"].clone());
            }
        });
    };
    load_settings();
    let load_counts = move || {
        spawn_local(async move {
            if let Ok(s) = api_get::<Value>("/summary").await {
                counts.update(|c| {
                    c.insert("mail".into(), s["to_reply"].as_u64().unwrap_or(0));
                    c.insert("today".into(), s["today_open"].as_u64().unwrap_or(0));
                });
            }
        });
    };
    load_counts();
    subscribe(&["app"]);
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            match payload["kind"].as_str() {
                Some("settings") => load_settings(),
                Some("mail") | Some("today") => load_counts(),
                _ => {}
            }
        }
    });

    // ⌘K the palette, ⌘B the sidebar, "/" the Secretary beside what's open.
    let _keys = window_event_listener(leptos::ev::keydown, move |e| {
        let k = e.key().to_lowercase();
        let m = e.meta_key() || e.ctrl_key();
        if m && k == "k" {
            e.prevent_default();
            palette.update(|p| *p = !*p);
            return;
        }
        if m && k == "b" {
            e.prevent_default();
            hidden.update(|h| *h = !*h);
            store::set("cerq.sidebar.hidden", if hidden.get_untracked() { "1" } else { "" });
            return;
        }
        let in_field = e.target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok()).is_some_and(|el| el.closest("input,textarea,select,[contenteditable]").ok().flatten().is_some());
        if in_field {
            return;
        }
        if k == "/" && !m && left.get_untracked() != "mail" && right.get_untracked().as_deref() != Some("mail") {
            e.prevent_default();
            open("chat", if left.get_untracked() == "chat" { Where::Replace } else { Where::Right }, BTreeMap::new());
        }
    });

    let close = move |p: Pane| {
        let (l, r) = (left.get_untracked(), right.get_untracked());
        match p {
            Pane::Right => write(l, None, nav::read().into_iter().filter(|(k, _)| k != "side").collect(), true),
            Pane::Left => write(r.unwrap_or_else(|| "dashboard".into()), None, BTreeMap::new(), true),
        }
    };

    let pane = move |p: Pane| {
        let id = Memo::new(move |_| if p == Pane::Left { left.get() } else { right.get().unwrap_or_default() });
        let def = move || pages::def(&id.get());
        view! {
            <section class="pane" data-side=if p == Pane::Left { "left" } else { "right" }
                style=move || if right.get().is_some() && maxed.get().is_none() { format!("flex:{}", if p == Pane::Left { ratio.get() } else { 1.0 - ratio.get() }) } else { String::new() }>
                <header class="pane-head">
                    <button class="icon-btn mobile-bar" aria-label="Menu" on:click=move |_| mobile_open.set(true)><Icon name="menu"/></button>
                    {(p == Pane::Left).then(|| view! {
                        <button class="icon-btn side-toggle" title=move || if hidden.get() { "Show the sidebar (⌘B)" } else { "Hide the sidebar (⌘B)" }
                            on:click=move |_| { hidden.update(|h| *h = !*h); store::set("cerq.sidebar.hidden", if hidden.get_untracked() { "1" } else { "" }) }>
                            <Icon name="sidebar"/></button>
                    })}
                    <Icon name=Signal::derive(move || def().map(|d| d.icon).unwrap_or("dot").to_string()) class="muted"/>
                    <span class="title grow">{move || def().map(|d| d.title).unwrap_or("")}</span>
                    <div id=if p == Pane::Left { "pane-tools-left" } else { "pane-tools-right" } class="row"/>
                    <Show when=move || right.get().is_some()>
                        <button class="icon-btn" title=move || if maxed.get().is_some() { "Restore" } else { "Maximize" }
                            on:click=move |_| maxed.update(|m| *m = if m.is_some() { None } else { Some(p) })>
                            <Icon name=Signal::derive(move || if maxed.get().is_some() { "minimize".to_string() } else { "maximize".to_string() })/></button>
                    </Show>
                    <Show when=move || right.get().is_none() && left.get() != "dashboard">
                        <button class="icon-btn" title="Dock to the right" on:click=move |_| write("dashboard".into(), Some(left.get_untracked()), nav::read(), true)><Icon name="dock"/></button>
                    </Show>
                    <Show when=move || right.get().is_some() || id.get() != "dashboard">
                        <button class="icon-btn" title="Close" on:click=move |_| close(p)><Icon name="x"/></button>
                    </Show>
                </header>
                <div class="pane-body">{move || def().map(|d| d.render(p)).unwrap_or_else(|| pages::todo("This page"))}</div>
            </section>
        }
    };

    let main_ref = NodeRef::<leptos::html::Main>::new();
    let start_resize = move |e: leptos::ev::PointerEvent| {
        e.prevent_default();
        let Some(m) = main_ref.get_untracked() else { return };
        let b = m.get_bounding_client_rect();
        let (x0, w) = (b.left(), b.width());
        let mv = window_event_listener(leptos::ev::pointermove, move |ev| {
            let r = ((ev.client_x() as f64 - x0) / w).clamp(0.2, 0.8);
            ratio.set(r);
            store::set("cerq.split", &r.to_string());
        });
        let mv = std::cell::RefCell::new(Some(mv));
        let up = window_event_listener(leptos::ev::pointerup, move |_| {
            if let Some(h) = mv.borrow_mut().take() {
                h.remove();
            }
        });
        std::mem::forget(up);
    };
    let drag_side = move |e: leptos::ev::PointerEvent| {
        e.prevent_default();
        let x0 = e.client_x();
        let w0 = side_w.get_untracked().max(260);
        let mv = window_event_listener(leptos::ev::pointermove, move |ev| {
            let w = (w0 + ev.client_x() - x0).clamp(220, 480);
            side_w.set(w);
            store::set("cerq.sidebar.w", &w.to_string());
        });
        let mv = std::cell::RefCell::new(Some(mv));
        let up = window_event_listener(leptos::ev::pointerup, move |_| {
            if let Some(h) = mv.borrow_mut().take() {
                h.remove();
            }
        });
        std::mem::forget(up);
    };

    view! {
        <div class=move || if hidden.get() { "shell side-hidden" } else { "shell" }>
            <Show when=move || mobile_open.get()><div class="scrim" on:click=move |_| mobile_open.set(false)/></Show>
            <nav class=move || format!("sidebar{}{}", if collapsed.get() { " collapsed" } else { "" }, if mobile_open.get() { " open" } else { "" })
                style=move || { if side_w.get() > 0 && !collapsed.get() { format!("--side-w:{}px", side_w.get()) } else { String::new() } }>
                <Show when=move || !collapsed.get()>
                    <div class="side-resize" role="separator" aria-orientation="vertical" title="Drag to resize · double-click to reset"
                        on:pointerdown=drag_side on:dblclick=move |_| { side_w.set(0); store::remove("cerq.sidebar.w") }/>
                </Show>
                <div class="brand">
                    <button class="icon-btn" aria-label="Narrow the sidebar" on:click=move |_| {
                        collapsed.update(|c| *c = !*c);
                        store::set("cerq.sidebar", if collapsed.get_untracked() { "collapsed" } else { "" })
                    }><Icon name="menu"/></button>
                    <Show when=move || !collapsed.get()><img src="/icon-192.png" alt=""/>" "{move || brand.get()}</Show>
                </div>
                <crate::side::SideSwitch side=side collapsed=collapsed on_set=Callback::new(move |s: Side| {
                    side.set(s);
                    let key = if s == Side::Work { "cerq.side.work" } else { "cerq.side.life" };
                    if side_of(&left.get_untracked()) != Some(s) {
                        let last = store::get(key).filter(|p| side_of(p) == Some(s)).unwrap_or_else(|| if s == Side::Work { "code".into() } else { "dashboard".into() });
                        open(&last, Where::Replace, BTreeMap::new());
                    }
                })/>
                <button class="nav nav-search" title="Go to… (⌘K)" on:click=move |_| palette.set(true)>
                    <Icon name="search"/>
                    <Show when=move || !collapsed.get()><span>"Go to…"</span><span class="kbd" style="margin-left:auto">"⌘K"</span></Show>
                </button>
                {move || match side.get() {
                    Side::Life => view! { <crate::side::Personal left=left right=right collapsed=collapsed counts=counts prefs=ui_prefs/> }.into_any(),
                    Side::Work => view! { <crate::side::Coding collapsed=collapsed/> }.into_any(),
                }}
                <div class="grow"/>
                <crate::side::SideFoot collapsed=collapsed settings_on=Signal::derive(move || left.get() == "settings" || right.get().as_deref() == Some("settings"))/>
            </nav>
            <Show when=move || palette.get()>
                <crate::palette::Palette on_close=Callback::new(move |_| palette.set(false))/>
            </Show>
            <main class="main" node_ref=main_ref>
                <Show when=move || maxed.get().is_none_or(|m| m == Pane::Left)>{pane(Pane::Left)}</Show>
                <Show when=move || right.get().is_some() && maxed.get().is_none()>
                    <div class="splitter" on:pointerdown=start_resize on:dblclick=move |_| { ratio.set(0.5); store::remove("cerq.split") }/>
                </Show>
                <Show when=move || right.get().is_some() && maxed.get().is_none_or(|m| m == Pane::Right)>{pane(Pane::Right)}</Show>
            </main>
        </div>
    }
}
