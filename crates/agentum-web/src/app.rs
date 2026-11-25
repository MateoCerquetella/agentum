//! The app: sign-in gate, then the shell (sidebar + one or two panes).
//!
//! URL = `/<left page>[?side=<right page>&<params>]`. Opening a page that is already on screen
//! keeps it where it is and updates its params; Alt/⌘-click opens it on the right.
use crate::pages::{self, login::Login, theme, Pane, Side};
use agentum_ui::nav::{self, PaneAction, Where};
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
    // Any page on either side, the same page on both too (two threads).
    let right = q.get("side").cloned().filter(|s| pages::def(s).is_some());
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
    let palette = crate::palette::shown();
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
    // Each pane keeps its own params: the left's plain, the right's "r_"-prefixed.
    let open = move |p: &str, at: Where, params: BTreeMap<String, String>| {
        mobile_open.set(false);
        let (cl, cr) = (left.get_untracked(), right.get_untracked());
        let now = nav::read();
        let (lp, rp) = (nav::pane_params(&now, false), nav::pane_params(&now, true));
        let p = p.to_string();
        let at = match at {
            Where::Replace if cr.as_deref() == Some(p.as_str()) && cl != p => Where::Right,
            Where::Replace => Where::Left,
            w => w,
        };
        let join = |l: BTreeMap<String, String>, r: &BTreeMap<String, String>| {
            let mut q = l;
            for (k, v) in r {
                q.insert(format!("r_{k}"), v.clone());
            }
            q
        };
        match at {
            Where::Right => write(cl, Some(p), join(lp, &params), true),
            _ => write(p, cr, join(params, &rp), true),
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

    agentum_ui::on_window(leptos::ev::popstate, move |_| {
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
    // New mail anywhere in the app: a toast you can click (pages/mail/watch.rs).
    crate::pages::mail::watch::start();
    subscribe(&["app"]);
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            match payload["kind"].as_str() {
                Some("settings") => load_settings(),
                Some("mail") | Some("today") => load_counts(),
                // A plugin's notification (HubCtx::notify), in every open page.
                Some("toast") => {
                    let (title, body) = (payload["title"].as_str().unwrap_or(""), payload["body"].as_str().unwrap_or(""));
                    let text = if body.is_empty() { title.to_string() } else { format!("{title}: {body}") };
                    match payload["url"].as_str().filter(|u| u.starts_with('/')).map(str::to_string) {
                        Some(url) => agentum_ui::ui::toast_action(text, move || {
                            let (page, q) = url.trim_start_matches('/').split_once('?').unwrap_or((url.trim_start_matches('/'), ""));
                            let params: Vec<(&str, &str)> = q.split('&').filter_map(|kv| kv.split_once('=')).collect();
                            navigate(page, Where::Replace, &params);
                        }),
                        None => toast(text),
                    }
                }
                _ => {}
            }
        }
    });

    // ⌘K the palette, ⌘B the sidebar, "/" the Secretary beside what's open.
    agentum_ui::on_window(leptos::ev::keydown, move |e| {
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
        let now = nav::read();
        match p {
            Pane::Right => write(l, None, nav::pane_params(&now, false), true),
            // The right pane moves left, with its params.
            Pane::Left => write(r.unwrap_or_else(|| "dashboard".into()), None, nav::pane_params(&now, true), true),
        }
    };
    // Open what was dropped on a half.
    let drop_on = move |e: leptos::ev::DragEvent, at: Where| {
        e.prevent_default();
        agentum_ui::dnd::end();
        if let Some(o) = agentum_ui::dnd::read(&e) {
            open(&o.page, at, o.params);
        }
    };
    let drop_over = RwSignal::new(None::<Pane>);
    agentum_ui::on_window(leptos::ev::dragend, move |_| {
        agentum_ui::dnd::end();
        drop_over.set(None);
    });
    // "This pane" for handlers without a component context: the one last touched.
    let touch = move |e: &web_sys::Event| {
        let right = e.target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok()).and_then(|el| el.closest(".pane").ok().flatten()).and_then(|p| p.get_attribute("data-side")).is_some_and(|s| s == "right");
        nav::set_active_pane(right);
    };
    agentum_ui::on_window(leptos::ev::pointerdown, move |e| touch(&e));
    agentum_ui::on_window(leptos::ev::focusin, move |e| touch(&e));

    let pane = move |p: Pane| {
        let id = Memo::new(move |_| if p == Pane::Left { left.get() } else { right.get().unwrap_or_default() });
        let def = move || pages::def(&id.get());
        let ctx = nav::PaneCtx::new(p == Pane::Right);
        // A new page in this pane starts with a clean header.
        Effect::new(move |prev: Option<String>| {
            let now = id.get();
            if prev.is_some_and(|x| x != now) {
                ctx.clear();
            }
            now
        });
        // Pages find their pane (and its header) in context.
        provide_context(ctx);
        let body = move || def().map(|d| d.render(p)).unwrap_or_else(|| pages::todo("This page"));
        view! {
            <section class="pane" data-side=if p == Pane::Left { "left" } else { "right" }
                style=move || if right.get().is_some() && maxed.get().is_none() { format!("flex:{}", if p == Pane::Left { ratio.get() } else { 1.0 - ratio.get() }) } else { String::new() }>
                <header class="pane-head">
                    <button class="icon-btn mobile-bar" aria-label="Menu" on:click=move |_| mobile_open.set(true)><Icon name="menu"/></button>
                    {(p == Pane::Left).then(|| view! {
                        <Show when=move || hidden.get()>
                            <button class="icon-btn side-toggle" title="Show the sidebar (⌘B)"
                                on:click=move |_| { hidden.set(false); store::set("cerq.sidebar.hidden", "") }>
                                <Icon name="sidebar"/></button>
                        </Show>
                    })}
                    <span class="title truncate">{move || ctx.title.get().filter(|t| !t.is_empty()).unwrap_or_else(|| def().map(|d| d.title).unwrap_or("").to_string())}</span>
                    {move || ctx.sub.get().map(|s| view! { <span class="pane-sub truncate">{s}</span> })}
                    <span class="grow"/>
                    <div id=if p == Pane::Left { "pane-tools-left" } else { "pane-tools-right" } class="row"/>
                    <crate::launcher::Launcher ctx=ctx/>
                    {move || ctx.actions.get().into_iter().map(|a| {
                        let run = a.run;
                        let on = a.on;
                        view! {
                            <button class=move || format!("icon-btn{}{}", if on.is_some_and(|s| s.get()) { " on" } else { "" }, if a.danger { " danger" } else { "" }) title=a.label.clone() on:click=move |_| run.run(())>
                                <Icon name=a.icon/>
                            </button>
                        }
                    }).collect_view()}
                    <Show when=move || !ctx.menu.with(|m| m.is_empty())>
                        <agentum_ui::picker::Menu items=Signal::derive(move || {
                            let mut m = ctx.menu.get();
                            if right.get_untracked().is_some() {
                                m.push(PaneAction::new(if maxed.get_untracked().is_some() { "minimize" } else { "maximize" }, if maxed.get_untracked().is_some() { "Restore" } else { "Maximize" }, move || maxed.update(|x| *x = if x.is_some() { None } else { Some(p) })));
                            }
                            m
                        }) title="More"/>
                    </Show>
                    <Show when=move || ctx.menu.with(|m| m.is_empty()) && right.get().is_some()>
                        <button class="icon-btn" title=move || if maxed.get().is_some() { "Restore" } else { "Maximize" }
                            on:click=move |_| maxed.update(|m| *m = if m.is_some() { None } else { Some(p) })>
                            <Icon name=Signal::derive(move || if maxed.get().is_some() { "minimize".to_string() } else { "maximize".to_string() })/></button>
                    </Show>
                    <Show when=move || right.get().is_some() || id.get() != "dashboard">
                        <button class="icon-btn" title="Close" on:click=move |_| close(p)><Icon name="x"/></button>
                    </Show>
                </header>
                <div class="pane-body">{body}</div>
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
                // Coding has it beside New thread (Dockside's top row).
                <Show when=move || side.get() == Side::Life>
                    <button class="nav nav-search" title="Go to… (⌘K)" on:click=move |_| palette.set(true)>
                        <Icon name="search"/>
                        <Show when=move || !collapsed.get()><span>"Go to…"</span><span class="kbd" style="margin-left:auto">"⌘K"</span></Show>
                    </button>
                </Show>
                {move || match side.get() {
                    Side::Life => view! { <crate::side::Personal left=left right=right collapsed=collapsed hidden=hidden counts=counts prefs=ui_prefs/> }.into_any(),
                    Side::Work => view! { <crate::dockside::Dockside collapsed=collapsed mail_open=Signal::derive(move || !hidden.get() && (left.get() == "mail" || right.get().as_deref() == Some("mail")))/> }.into_any(),
                    // (Dockside off: the same component draws a plain list.)
                }}
                <div class="grow"/>
                <crate::side::SideFoot collapsed=collapsed settings_on=Signal::derive(move || left.get() == "settings" || right.get().as_deref() == Some("settings"))/>
            </nav>
            <Show when=move || palette.get()>
                <crate::palette::Palette on_close=Callback::new(move |_| palette.set(false))/>
            </Show>
            <main class="main" node_ref=main_ref
                on:dragenter=move |e| {
                    let ours = e.data_transfer().map(|dt| { let t = dt.types(); (0..t.length()).any(|i| t.get(i).as_string().as_deref() == Some(agentum_ui::dnd::MIME)) }).unwrap_or(false);
                    if ours { agentum_ui::dnd::dragging().set(true) }
                }>
                <Show when=move || maxed.get().is_none_or(|m| m == Pane::Left)>{pane(Pane::Left)}</Show>
                <Show when=move || right.get().is_some() && maxed.get().is_none()>
                    <div class="splitter" on:pointerdown=start_resize on:dblclick=move |_| { ratio.set(0.5); store::remove("cerq.split") }/>
                </Show>
                <Show when=move || right.get().is_some() && maxed.get().is_none_or(|m| m == Pane::Right)>{pane(Pane::Right)}</Show>
                <Show when=move || agentum_ui::dnd::dragging().get()>
                    <div class="drop-zones active">
                        {[(Pane::Left, Where::Left, "Open on the left"), (Pane::Right, Where::Right, "Open on the right")].into_iter().map(|(p, at, label)| view! {
                            <div class=move || if drop_over.get() == Some(p) { "over" } else { "" }
                                on:dragover=move |e| { e.prevent_default(); drop_over.set(Some(p)) }
                                on:dragleave=move |_| drop_over.set(None)
                                on:drop=move |e| { drop_over.set(None); drop_on(e, at) }>{label}</div>
                        }).collect_view()}
                    </div>
                </Show>
            </main>
        </div>
    }
}
