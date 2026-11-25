//! The pane header's + launcher, as the user's BB Action Topbar: a searchable list of what can
//! open (this thread's tools: Files, Git, Terminal…; the plugins' pages: Usage, Browser…). A
//! click opens one (a thread tool in its side panel, a page beside); dragging one onto a half of
//! the window opens it there.
use agentum_ui::nav::{PaneCtx, Where};
use agentum_ui::prelude::*;

#[derive(Clone)]
struct Row {
    label: String,
    icon: &'static str,
    page: &'static str,
    params: Vec<(String, String)>,
    open: Option<Callback<()>>,
    group: &'static str,
}

fn open_beside(right: bool, r: &Row) {
    let params: Vec<(&str, &str)> = r.params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    navigate(r.page, if right { Where::Left } else { Where::Right }, &params);
}

#[component]
pub fn Launcher(ctx: PaneCtx) -> impl IntoView {
    let open = RwSignal::new(false);
    let q = RwSignal::new(String::new());
    let sel = RwSignal::new(0usize);
    let root = NodeRef::<leptos::html::Div>::new();
    let input = NodeRef::<leptos::html::Input>::new();
    let rows = Signal::derive(move || {
        let ql = q.get().to_lowercase();
        let mut v: Vec<Row> = ctx.tools.get().into_iter().map(|t| Row { label: t.label, icon: t.icon, page: t.page, params: t.params, open: t.open, group: "This thread" }).collect();
        v.extend(crate::plugins::pages().into_iter().map(|d| Row { label: d.title.to_string(), icon: d.icon, page: d.id, params: vec![], open: None, group: "Pages" }));
        v.into_iter().filter(|r| ql.is_empty() || r.label.to_lowercase().contains(&ql)).collect::<Vec<_>>()
    });
    // A drag from the list keeps it (hidden) until the drag ends: Chrome drops a drag whose source
    // node goes away.
    let dragging = agentum_ui::dnd::dragging();
    Effect::new(move |was: Option<bool>| {
        let now = dragging.get();
        if was == Some(true) && !now {
            open.set(false);
        }
        now
    });
    agentum_ui::on_window(leptos::ev::mousedown, move |e| {
        let inside = root.get_untracked().zip(e.target()).is_some_and(|(r, t)| {
            use wasm_bindgen::JsCast;
            t.dyn_ref::<web_sys::Node>().is_some_and(|n| r.contains(Some(n)))
        });
        if open.get_untracked() && !inside {
            open.set(false);
        }
    });
    Effect::new(move |_| {
        if open.get() {
            q.set(String::new());
            sel.set(0);
            if let Some(i) = input.get() {
                let _ = i.focus();
            }
        }
    });
    let run = move |r: &Row| {
        match r.open {
            Some(cb) => cb.run(()),
            None => open_beside(ctx.right, r),
        }
        open.set(false);
    };
    let key = move |e: leptos::ev::KeyboardEvent| match e.key().as_str() {
        "ArrowDown" => {
            e.prevent_default();
            sel.update(|s| *s = (*s + 1).min(rows.get_untracked().len().saturating_sub(1)))
        }
        "ArrowUp" => {
            e.prevent_default();
            sel.update(|s| *s = s.saturating_sub(1))
        }
        "Enter" => {
            e.prevent_default();
            if let Some(r) = rows.get_untracked().get(sel.get_untracked()) {
                run(r)
            }
        }
        "Escape" => {
            e.prevent_default();
            open.set(false)
        }
        _ => {}
    };
    view! {
        <div class="pk launcher" node_ref=root on:keydown=key>
            <button type="button" class=move || if open.get() { "icon-btn on" } else { "icon-btn" } title="Open a tool or page · drag one to a side of the window" aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)>
                <Icon name="plus"/>
            </button>
            <Show when=move || open.get()>
                <div class=move || if dragging.get() { "pk-pop launcher-pop away" } else { "pk-pop launcher-pop" } role="listbox">
                    <input class="pk-search" node_ref=input placeholder="Search tools and pages" prop:value=q on:input=move |e| { q.set(event_target_value(&e)); sel.set(0) }/>
                    <div class="pk-list">
                        {move || {
                            let v = rows.get();
                            if v.is_empty() {
                                return view! { <div class="pk-empty">"Nothing matches."</div> }.into_any();
                            }
                            let mut last = "";
                            v.into_iter().enumerate().map(|(i, r)| {
                                let head = (r.group != last).then_some(r.group);
                                last = r.group;
                                let (page, params) = (r.page, r.params.clone());
                                let r2 = r.clone();
                                view! {
                                    {head.map(|g| view! { <div class="pk-group">{g}</div> })}
                                    <button type="button" draggable="true" class=move || if sel.get() == i { "pk-item sel" } else { "pk-item" }
                                        on:mouseenter=move |_| sel.set(i)
                                        on:dragstart=move |e| { let p: Vec<(&str, &str)> = params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(); agentum_ui::dnd::start(&e, page, &p) }
                                        on:dragend=move |_| { dragging.set(false); open.set(false) }
                                        on:click=move |_| run(&r2)>
                                        <Icon name=r.icon size=14/>
                                        <span class="pk-label">{r.label.clone()}</span>
                                        <span class="pk-sub">"Drag"</span>
                                    </button>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                </div>
            </Show>
        </div>
    }
}
