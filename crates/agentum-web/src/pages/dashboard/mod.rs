//! The Dashboard: your widgets in columns (small, full, small), one tab per page, as Glance drew
//! them, now native. Layout and widget settings live on the hub (GET /api/dashboard, PUT
//! /api/dashboard/layout, PUT /api/widgets). "Edit layout" (the pencil in the pane's header):
//! drag widgets within or between columns, remove them (×), change one's settings (gear), and add
//! more from the panel (+ Add widget: drag in, or +; or create a new one of any type).
mod chart;
mod editor;
mod widgets;

use super::Pane;
use agentum_ui::prelude::*;
use agentum_ui::store;
use editor::Editor;
use std::collections::HashMap;
use wasm_bindgen::JsCast;

const DRAG: &str = "text/agentum-widget";
const PAGE_KEY: &str = "cerq.dash.page";

pub fn page(pane: Pane) -> AnyView {
    view! { <Dashboard pane=pane/> }.into_any()
}

type Cols = Vec<(String, Vec<String>)>;

#[component]
fn Dashboard(pane: Pane) -> impl IntoView {
    let data = RwSignal::new(None::<Value>);
    let err = RwSignal::new(String::new());
    let page = RwSignal::new(store::get(PAGE_KEY).and_then(|p| p.parse::<usize>().ok()).unwrap_or(0));
    let editing = RwSignal::new(false);
    let drawer = RwSignal::new(false);
    let dragging = RwSignal::new(None::<String>);
    let target = RwSignal::new(None::<(usize, usize)>);
    let editor = RwSignal::new(None::<(String, Option<Value>)>);
    let load = move || {
        spawn_local(async move {
            match api_get::<Value>("/dashboard").await {
                Ok(d) => {
                    data.try_set(Some(d));
                }
                Err(e) => {
                    err.try_set(e.message);
                }
            }
        })
    };
    load();

    let pages = Memo::new(move |_| data.with(|d| d.as_ref().and_then(|d| d["layout"]["pages"].as_array().cloned()).unwrap_or_default()));
    let pg = Memo::new(move |_| page.get().min(pages.with(|p| p.len().saturating_sub(1))));
    let cols = Memo::new(move |_| -> Cols {
        pages.with(|p| {
            p.get(pg.get())
                .and_then(|p| p["columns"].as_array())
                .map(|cs| cs.iter().map(|c| (c["size"].as_str().unwrap_or("small").to_string(), c["widgets"].as_array().map(|w| w.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default())).collect())
                .unwrap_or_default()
        })
    });
    let by_id = Memo::new(move |_| -> HashMap<String, Value> { data.with(|d| d.as_ref().and_then(|d| d["widgets"].as_array().map(|a| a.iter().map(|w| (w["id"].as_str().unwrap_or("").to_string(), w.clone())).collect())).unwrap_or_default()) });
    let types = Memo::new(move |_| data.with(|d| d.as_ref().map(|d| d["types"].clone()).unwrap_or_default()));

    // Layout changes: this page's columns, saved on the hub.
    let save_cols = move |c: Cols| {
        let i = pg.get_untracked();
        let mut layout = data.with_untracked(|d| d.as_ref().map(|d| d["layout"].clone()).unwrap_or_default());
        if let Some(p) = layout["pages"].get_mut(i) {
            p["columns"] = json!(c.iter().map(|(size, ids)| json!({"size": size, "widgets": ids})).collect::<Vec<_>>());
        }
        data.update(|d| {
            if let Some(d) = d {
                d["layout"] = layout.clone();
            }
        });
        spawn_local(async move {
            if let Err(e) = api_put::<Value>("/dashboard/layout", &layout).await {
                toast_bad(e.message);
            }
        });
    };
    let place = move |id: String, col: usize, at: usize| {
        let mut c = cols.get_untracked();
        if col >= c.len() {
            return;
        }
        let mut at = at;
        for (ci, (_, ids)) in c.iter_mut().enumerate() {
            if let Some(p) = ids.iter().position(|x| *x == id) {
                ids.remove(p);
                if ci == col && p < at {
                    at -= 1;
                }
            }
        }
        let n = c[col].1.len();
        c[col].1.insert(at.min(n), id);
        save_cols(c);
    };
    let add = move |id: String| {
        let c = cols.get_untracked();
        let col = c.iter().position(|(s, _)| s == "full").unwrap_or(0);
        let n = c.get(col).map(|x| x.1.len()).unwrap_or(0);
        place(id, col, n);
    };
    let remove = move |id: String| {
        let mut c = cols.get_untracked();
        for (_, ids) in c.iter_mut() {
            ids.retain(|x| *x != id);
        }
        save_cols(c);
    };
    let finish_drag = move || {
        dragging.set(None);
        target.set(None);
    };

    // The pane header's pencil.
    let tools = RwSignal::new_local(None::<web_sys::Element>);
    Effect::new(move |_| {
        let id = if pane == Pane::Left { "pane-tools-left" } else { "pane-tools-right" };
        tools.set(web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)));
    });

    let widget = move |ci: usize, id: String, w: Value| {
        let title = w["title"].as_str().unwrap_or("").to_string();
        let ty = w["type"].as_str().unwrap_or("").to_string();
        let editable = types.with_untracked(|t| t.get(&ty).is_some());
        let (id1, id2, id3, id4) = (id.clone(), id.clone(), id.clone(), id.clone());
        let w2 = w.clone();
        let ty_c = format!("{ty}{}{}", if widgets::frameless(&w) { " frameless" } else { "" }, if widgets::headless(&w) { " headless" } else { "" });
        view! {
            <section class=move || format!("dw dwt-{ty_c}{}{}{}", if editing.get() { " editing" } else { "" }, if dragging.get().as_deref() == Some(id1.as_str()) { " dragging" } else { "" },
                    if target.get().is_some_and(|(c, i)| c == ci && cols.with(|cs| cs.get(c).and_then(|x| x.1.get(i)) == Some(&id1))) { " drop-before" } else { "" })
                draggable=move || if editing.get() { "true" } else { "false" }
                on:dragstart=move |e| {
                    if !editing.get_untracked() { return }
                    if let Some(dt) = e.data_transfer() { let _ = dt.set_data(DRAG, &id2); dt.set_effect_allowed("move"); }
                    dragging.set(Some(id2.clone()));
                }
                on:dragend=move |_| finish_drag()
                on:dragover=move |e| {
                    if dragging.get_untracked().is_none() { return }
                    e.prevent_default();
                    e.stop_propagation();
                    let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
                    let r = el.get_bounding_client_rect();
                    let i = cols.with_untracked(|cs| cs.get(ci).and_then(|c| c.1.iter().position(|x| *x == id3))).unwrap_or(0);
                    let at = if (e.client_y() as f64) < r.top() + r.height() / 2.0 { i } else { i + 1 };
                    if target.get_untracked() != Some((ci, at)) { target.set(Some((ci, at))) }
                }>
                <header class="dw-head">
                    <Show when=move || editing.get()><span class="dw-grip" title="Drag to move"><Icon name="grip" size=14/></span></Show>
                    <h2 class="grow">{title}</h2>
                    <Show when=move || editing.get()>
                        {editable.then(|| { let ty = ty.clone(); let w = w2.clone(); view! {
                            <button class="icon-btn" title="Widget settings" aria-label="Widget settings" on:click=move |_| editor.set(Some((ty.clone(), Some(w.clone()))))><Icon name="settings" size=14/></button>
                        } })}
                        <button class="icon-btn" title="Remove from this page" aria-label="Remove" on:click={ let id = id4.clone(); move |_| remove(id.clone()) }><Icon name="x" size=14/></button>
                    </Show>
                </header>
                <div class="dw-body">{widgets::body(&w)}</div>
            </section>
        }
    };

    let column = move |ci: usize, size: String| {
        view! {
            <div class=move || format!("dash-col {size}{}", if target.get().is_some_and(|(c, i)| c == ci && cols.with(|cs| cs.get(c).is_some_and(|x| i >= x.1.len()))) { " drop-end" } else { "" })
                on:dragover=move |e| {
                    if dragging.get_untracked().is_none() { return }
                    e.prevent_default();
                    let n = cols.with_untracked(|cs| cs.get(ci).map(|c| c.1.len()).unwrap_or(0));
                    if target.get_untracked().is_none_or(|(c, _)| c != ci) || e.target() == e.current_target() {
                        if target.get_untracked() != Some((ci, n)) { target.set(Some((ci, n))) }
                    }
                }
                on:drop=move |e| {
                    e.prevent_default();
                    if let (Some(id), Some((c, at))) = (dragging.get_untracked(), target.get_untracked()) { place(id, c, at) }
                    finish_drag();
                }>
                <For each=move || {
                    let ids = cols.with(|cs| cs.get(ci).map(|c| c.1.clone()).unwrap_or_default());
                    by_id.with(|m| ids.into_iter().filter_map(|id| m.get(&id).map(|w| (id, w.clone()))).collect::<Vec<_>>())
                } key=|(id, w)| format!("{id}|{w}") let:item>
                    {widget(ci, item.0, item.1)}
                </For>
                <Show when=move || editing.get() && cols.with(|cs| cs.get(ci).is_some_and(|c| c.1.is_empty()))>
                    <div class="dash-empty-col">"Drop widgets here"</div>
                </Show>
            </div>
        }
    };

    let reset = move || {
        spawn_local(async move {
            if !confirm("Reset the layout?", "Every page goes back to the default widgets and order. Widget settings stay.", "Reset", true).await {
                return;
            }
            match api_post::<Value>("/dashboard/layout/reset", &json!({})).await {
                Ok(_) => load(),
                Err(e) => toast_bad(e.message),
            }
        });
    };

    view! {
        <div class=move || if editing.get() { "dash adash editing" } else { "dash adash" }>
            {move || tools.get().map(|el| view! {
                <leptos::portal::Portal mount=el>
                    <button class=move || if editing.get() { "icon-btn on" } else { "icon-btn" } title=move || if editing.get() { "Done editing" } else { "Edit layout" }
                        on:click=move |_| { editing.update(|e| *e = !*e); if !editing.get_untracked() { drawer.set(false) } }>
                        <Icon name=Signal::derive(move || if editing.get() { "check".to_string() } else { "edit".to_string() })/>
                    </button>
                </leptos::portal::Portal>
            })}
            <Show when=move || { pages.with(|p| p.len()) > 1 || editing.get() }>
                <div class="dash-bar">
                    <div class="dash-tabs" role="tablist">
                        {move || pages.get().into_iter().enumerate().map(|(i, p)| view! {
                            <button role="tab" aria-selected=move || (pg.get() == i).to_string() class=move || if pg.get() == i { "on" } else { "" }
                                on:click=move |_| { page.set(i); store::set(PAGE_KEY, &i.to_string()) }>{p["name"].as_str().unwrap_or("Page").to_string()}</button>
                        }).collect_view()}
                    </div>
                    <span class="grow"/>
                    <Show when=move || editing.get()>
                        <span class="small subdue">"Drag widgets to move them"</span>
                        <button class=move || if drawer.get() { "btn on" } else { "btn" } aria-expanded=move || drawer.get().to_string() on:click=move |_| drawer.update(|d| *d = !*d)><Icon name="plus" size=14/>" Add widget"</button>
                        <button class="btn" on:click=move |_| reset()><Icon name="undo" size=14/>" Reset"</button>
                        <button class="btn primary" on:click=move |_| { editing.set(false); drawer.set(false) }>"Done"</button>
                    </Show>
                </div>
            </Show>
            <Show when=move || !err.get().is_empty()><div class="small err" style="padding:8px 14px">{move || err.get()}</div></Show>
            {move || data.with(|d| d.is_none()).then(|| view! { <div class="empty">"Loading…"</div> })}
            <div class="dash-cols">
                {move || { pg.track(); cols.with(|c| c.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>()) }.into_iter().enumerate().map(|(i, s)| column(i, s)).collect_view()}
            </div>
            <Show when=move || drawer.get()>
                <aside class="dash-drawer" aria-label="Add widgets">
                    <div class="row">
                        <span class="hi grow">"Add widgets"</span>
                        <button class="icon-btn" aria-label="Close" on:click=move |_| drawer.set(false)><Icon name="x" size=14/></button>
                    </div>
                    <p class="small subdue">"Drag onto the page, or press +."</p>
                    <div class="dash-sec">"Create new"</div>
                    <div class="dash-create">
                        {move || types.get().as_object().map(|o| o.iter().map(|(k, t)| {
                            let k = k.clone();
                            view! { <button class="btn" on:click=move |_| editor.set(Some((k.clone(), None)))><Icon name="plus" size=12/>{t["label"].as_str().unwrap_or("").to_string()}</button> }
                        }).collect_view())}
                    </div>
                    <div class="dash-sec">"Not on this page"</div>
                    {move || {
                        let here: Vec<String> = cols.with(|c| c.iter().flat_map(|x| x.1.clone()).collect());
                        let rest: Vec<Value> = data.with(|d| d.as_ref().and_then(|d| d["widgets"].as_array().cloned()).unwrap_or_default()).into_iter().filter(|w| !here.iter().any(|h| w["id"].as_str() == Some(h.as_str()))).collect();
                        if rest.is_empty() {
                            return view! { <p class="small subdue">"Every widget is already on this page."</p> }.into_any();
                        }
                        rest.into_iter().map(|w| {
                            let id = w["id"].as_str().unwrap_or("").to_string();
                            let ty = w["type"].as_str().unwrap_or("").to_string();
                            let (id1, id2) = (id.clone(), id.clone());
                            let (ty1, w1) = (ty.clone(), w.clone());
                            view! {
                                <div class="dash-item-row" draggable="true"
                                    on:dragstart=move |e| { if let Some(dt) = e.data_transfer() { let _ = dt.set_data(DRAG, &id1); } dragging.set(Some(id1.clone())) }
                                    on:dragend=move |_| finish_drag()>
                                    <Icon name="grip" size=14 class="subdue"/>
                                    <span class="grow" style="min-width:0"><span class="hi truncate" style="display:block">{w["title"].as_str().unwrap_or("").to_string()}</span><small class="subdue">{ty.replace('-', " ")}</small></span>
                                    <button class="icon-btn" title="Widget settings" on:click=move |_| editor.set(Some((ty1.clone(), Some(w1.clone()))))><Icon name="settings" size=14/></button>
                                    <button class="icon-btn" title="Add to page" on:click=move |_| add(id2.clone())><Icon name="plus" size=14/></button>
                                </div>
                            }
                        }).collect_view().into_any()
                    }}
                </aside>
            </Show>
            <chart::ChartLayer/>
            {move || editor.get().map(|(ty, w)| view! {
                <Editor ty=ty types=types.get_untracked() widget=w on_close=Callback::new(move |_| editor.set(None))
                    on_done=Callback::new(move |(id, item): (String, Option<Value>)| {
                        editor.set(None);
                        match item {
                            Some(item) => {
                                let fresh = !by_id.with_untracked(|m| m.contains_key(&id));
                                data.update(|d| if let Some(a) = d.as_mut().and_then(|d| d["widgets"].as_array_mut()) {
                                    match a.iter_mut().find(|x| x["id"].as_str() == Some(id.as_str())) { Some(x) => *x = item.clone(), None => a.push(item.clone()) }
                                });
                                if fresh { add(id) }
                                toast("Saved");
                            }
                            None => {
                                data.update(|d| if let Some(d) = d.as_mut() {
                                    if let Some(a) = d["widgets"].as_array_mut() { a.retain(|x| x["id"].as_str() != Some(id.as_str())) }
                                    if let Some(ps) = d["layout"]["pages"].as_array_mut() {
                                        for p in ps { if let Some(cs) = p["columns"].as_array_mut() { for c in cs { if let Some(w) = c["widgets"].as_array_mut() { w.retain(|x| x.as_str() != Some(id.as_str())) } } } }
                                    }
                                });
                                let layout = data.with_untracked(|d| d.as_ref().map(|d| d["layout"].clone()).unwrap_or_default());
                                spawn_local(async move { let _ = api_put::<Value>("/dashboard/layout", &layout).await; });
                                toast("Widget deleted");
                            }
                        }
                    })/>
            })}
        </div>
    }
}
