//! The Coding sidebar, after Dockside (the author's BB plugin): projects stay put, roots keep their
//! creation order (pinned first), and status lives inside each row, so nothing moves under the
//! cursor when an agent finishes. Clear the list with two verbs: **snooze** (until tomorrow 09:00;
//! it wakes early if it works or asks) and **settle** (archive; shown for a day). Side chats sit
//! under their thread. Drag a card to a half to open it there; ⌘-click opens it on the right.
use crate::code::store::{self, overview};
use crate::code::{agent_icon, patch_thread, tomorrow_9};
use agentum_proto::api::{PatchThread, ThreadSummary};
use agentum_proto::{Attention, ThreadState};
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;

fn project_name(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

fn snoozed(t: &ThreadSummary, now: i64) -> bool {
    !t.archived && t.snoozed_until.is_some_and(|u| u > now)
}

/// Open roots by project: projects ordered by their newest thread's creation, roots pinned
/// first then newest created first. Children (side chats) go with their root.
pub struct Groups {
    pub projects: Vec<(String, Vec<(ThreadSummary, Vec<ThreadSummary>)>)>,
    pub snoozed: Vec<ThreadSummary>,
    pub settled: Vec<ThreadSummary>,
}

pub fn group(all: &[ThreadSummary], now: i64, q: &str) -> Groups {
    let q = q.trim().to_lowercase();
    let hit = |t: &ThreadSummary| q.is_empty() || t.title.to_lowercase().contains(&q) || t.project.to_lowercase().contains(&q) || t.branch.as_deref().unwrap_or("").to_lowercase().contains(&q);
    let open: Vec<&ThreadSummary> = all.iter().filter(|t| !t.archived && !snoozed(t, now)).collect();
    let roots: Vec<&ThreadSummary> = open.iter().copied().filter(|t| t.parent.is_none() || !open.iter().any(|p| Some(&p.id) == t.parent.as_ref())).collect();
    let mut projects: Vec<String> = vec![];
    let mut newest: Vec<(String, i64)> = vec![];
    for t in &roots {
        match newest.iter_mut().find(|(p, _)| *p == t.project) {
            Some(x) => x.1 = x.1.max(t.created_at),
            None => newest.push((t.project.clone(), t.created_at)),
        }
    }
    newest.sort_by(|a, b| b.1.cmp(&a.1));
    projects.extend(newest.into_iter().map(|(p, _)| p));
    let mut out = vec![];
    for p in projects {
        let mut fam: Vec<(ThreadSummary, Vec<ThreadSummary>)> = roots
            .iter()
            .filter(|t| t.project == p)
            .map(|r| {
                let mut kids: Vec<ThreadSummary> = open.iter().filter(|c| c.parent.as_ref() == Some(&r.id)).map(|c| (*c).clone()).collect();
                kids.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                ((*r).clone(), kids)
            })
            .filter(|(r, kids)| hit(r) || kids.iter().any(hit))
            .collect();
        fam.sort_by(|a, b| b.0.pinned.cmp(&a.0.pinned).then(b.0.created_at.cmp(&a.0.created_at)));
        if !fam.is_empty() {
            out.push((p, fam));
        }
    }
    let mut snoozed_v: Vec<ThreadSummary> = all.iter().filter(|t| snoozed(t, now) && hit(t)).cloned().collect();
    snoozed_v.sort_by_key(|t| t.snoozed_until);
    let mut settled: Vec<ThreadSummary> = all.iter().filter(|t| t.archived && hit(t)).cloned().collect();
    settled.sort_by(|a, b| b.archived_at.cmp(&a.archived_at));
    Groups { projects: out, snoozed: snoozed_v, settled }
}

/// The visible order (roots and their children), for ⌘1–9 and ⌘⇧[ ].
pub fn order(all: &[ThreadSummary], now: i64) -> Vec<String> {
    group(all, now, "").projects.into_iter().flat_map(|(_, fam)| fam.into_iter().flat_map(|(r, kids)| std::iter::once(r.id).chain(kids.into_iter().map(|k| k.id)))).collect()
}

fn working(t: &ThreadSummary) -> bool {
    t.state.busy() && t.asking.is_none()
}
fn needs_you(t: &ThreadSummary) -> bool {
    t.attention == Attention::NeedsYou
}

/// Dockside's working spinner. The list is rebuilt on every thread update (and on a click), so a
/// plain CSS spin restarted at 0° each time and looked stuck or jumped. Once the span is in the
/// page its animation is pinned to the page timeline's origin, so every spinner shares one phase
/// and a rebuilt one carries on exactly where the old one was.
#[component]
fn Spinner(#[prop(into)] class: String, size: u32) -> impl IntoView {
    let el = NodeRef::<leptos::html::Span>::new();
    // After the list's rebuild is in the page (a microtask, so before it paints); once more a frame
    // later if it was not attached yet.
    el.on_load(|el| {
        let el: web_sys::Element = el.into();
        spawn_local(async move {
            if !pin_to_timeline(&el) {
                request_animation_frame(move || {
                    pin_to_timeline(&el);
                });
            }
        })
    });
    view! { <span node_ref=el class=class title="Working"><Icon name="loading" size=size/></span> }
}

/// Whether it found an animation to pin.
fn pin_to_timeline(el: &web_sys::Element) -> bool {
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::{JsCast, JsValue};
    let Some(get) = Reflect::get(el, &"getAnimations".into()).ok().and_then(|f| f.dyn_into::<Function>().ok()) else { return true };
    let Some(list) = get.call0(el).ok().and_then(|l| l.dyn_into::<Array>().ok()) else { return true };
    for a in list.iter() {
        let _ = Reflect::set(&a, &"startTime".into(), &JsValue::from_f64(0.0));
    }
    list.length() > 0
}

/// The glyph before a title, Dockside's: failed (circle-x), needs you (circle-question), working
/// (spins), unread (an eye), else the agent.
#[component]
fn Glyph(t: ThreadSummary) -> impl IntoView {
    if needs_you(&t) {
        let failed = t.state == ThreadState::Error;
        let (class, icon, title) = if failed { ("dk-g dk-g-fail", "x-circle", "Stopped with an error") } else { ("dk-g dk-g-ask", "help", "Needs you") };
        view! { <span class=class title=title><Icon name=icon size=14/></span> }.into_any()
    } else if working(&t) {
        view! { <Spinner class="dk-g dk-g-work" size=14/> }.into_any()
    } else if t.unread {
        view! { <span class="dk-g dk-g-unread" title="Unread"><Icon name="eye" size=14/></span> }.into_any()
    } else {
        view! { <span class="dk-g dk-g-agent"><Icon name=agent_icon(t.agent) size=12/></span> }.into_any()
    }
}

/// Dockside's coloured status pill under the time: (class, label).
fn pill(t: &ThreadSummary) -> Option<(&'static str, &'static str)> {
    if needs_you(t) {
        Some(if t.state == ThreadState::Error { ("dk-pill fail", "Failed") } else { ("dk-pill ask", "Needs you") })
    } else if working(t) {
        Some(("dk-pill work", "Working"))
    } else if t.unread {
        Some(("dk-pill unread", "Unread"))
    } else {
        None
    }
}

/// Dockside's automatic project badge colours (FNV-1a of the project), with the text colour that
/// reads best on each.
fn badge_colors(project: &str) -> (&'static str, &'static str) {
    const PALETTE: [(&str, &str); 12] = [
        ("#2563EB", "#FFFFFF"), ("#7C3AED", "#FFFFFF"), ("#C026D3", "#FFFFFF"), ("#DB2777", "#FFFFFF"),
        ("#DC2626", "#FFFFFF"), ("#EA580C", "#000000"), ("#CA8A04", "#000000"), ("#16A34A", "#000000"),
        ("#059669", "#000000"), ("#0891B2", "#000000"), ("#0284C7", "#000000"), ("#4F46E5", "#FFFFFF"),
    ];
    let mut h: u32 = 0x811c9dc5;
    for c in project.encode_utf16() {
        h ^= c as u32;
        h = h.wrapping_mul(0x01000193);
    }
    PALETTE[h as usize % PALETTE.len()]
}

fn menu_for(t: &ThreadSummary) -> Vec<PaneAction> {
    let (a, b, c, d, e, f) = (t.id.clone(), t.id.clone(), t.id.clone(), t.id.clone(), t.id.clone(), t.id.clone());
    let (unread, pinned, archived, title) = (t.unread, t.pinned, t.archived, t.title.clone());
    let mut v = vec![
        PaneAction::new("split", "Open beside", move || navigate("code", Where::Right, &[("thread", &a)])),
        PaneAction::new("terminal", "Its terminal beside", move || navigate("panel", Where::Right, &[("tab", "terminal"), ("thread", &f)])),
        PaneAction::new("eye", if unread { "Mark as read" } else { "Mark as unread" }, move || patch_thread(&b, PatchThread { read: Some(unread), ..Default::default() })),
        PaneAction::new("pin", if pinned { "Unpin" } else { "Pin" }, move || patch_thread(&c, PatchThread { pinned: Some(!pinned), ..Default::default() })),
    ];
    if archived {
        v.push(PaneAction::new("refresh", "Unsettle", move || patch_thread(&d, PatchThread { archived: Some(false), ..Default::default() })));
    } else {
        let d2 = d.clone();
        v.push(PaneAction::new("alarm", "Snooze until tomorrow", move || patch_thread(&d2, PatchThread { snoozed_until: Some(Some(tomorrow_9())), ..Default::default() })));
        v.push(PaneAction::new("check-circle", "Settle", move || patch_thread(&d, PatchThread { archived: Some(true), ..Default::default() })));
    }
    v.push(
        PaneAction::new("trash", "Delete", move || {
            let (id, title) = (e.clone(), title.clone());
            spawn_local(async move {
                if confirm("Delete this thread?", title, "Delete", true).await {
                    if let Err(err) = api_delete::<Value>(&format!("/code/threads/{id}")).await {
                        toast_bad(err.message);
                    }
                }
            });
        })
        .danger(),
    );
    v
}

fn open_thread(e: &leptos::ev::MouseEvent, id: &str) {
    navigate("code", if e.meta_key() || e.ctrl_key() || e.alt_key() { Where::Right } else { Where::Replace }, &[("thread", id)]);
}

/// The row link under a pointer event, and its thread.
fn hit_thread(e: &leptos::ev::PointerEvent) -> Option<String> {
    use wasm_bindgen::JsCast;
    e.target()?.dyn_into::<web_sys::Element>().ok()?.closest("a.dk-hit[data-thread]").ok()??.get_attribute("data-thread")
}

/// A row's link only handles the keyboard (Enter). A mouse click on a row is caught by the list
/// itself in pointerdown/pointerup: the list redraws on every thread update, so a working row is
/// often replaced between press and release and the browser drops the click.
fn row_click(e: leptos::ev::MouseEvent, id: &str) {
    e.prevent_default();
    if e.detail() == 0 {
        open_thread(&e, id)
    }
}

/// One root thread (two lines) and its side chats.
#[component]
fn Card(t: ThreadSummary, kids: Vec<ThreadSummary>, on: Signal<Option<String>>, on_r: Signal<Option<String>>) -> impl IntoView {
    let id = t.id.clone();
    let is_on = {
        let id = id.clone();
        Signal::derive(move || on.get().as_deref() == Some(id.as_str()) || on_r.get().as_deref() == Some(id.as_str()))
    };
    // Anything not running or asking can be parked (a failed thread too).
    let can_park = !working(&t) && t.asking.is_none() && t.state != ThreadState::Waiting;
    let title = if t.title.is_empty() { "New thread".to_string() } else { t.title.clone() };
    let mut meta = vec![];
    if let Some(b) = &t.branch {
        meta.push(view! { <span class="dk-branch">{b.clone()}</span> }.into_any());
    }
    meta.push(view! { <span>{t.machine.clone()}</span> }.into_any());
    let badges = crate::plugins::thread_row(&t);
    let hover = RwSignal::new(false);
    let timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    let t_hover = t.clone();
    let (id_click, id_drag, id_snooze, id_settle) = (id.clone(), id.clone(), id.clone(), id.clone());
    let t_menu = t.clone();
    let has_kids = !kids.is_empty();
    let kids_open = RwSignal::new(true);
    view! {
        <li class="dk-li">
            <div class=move || format!("dk-card{}{}", if is_on.get() { " on" } else { "" }, if t.unread || needs_you(&t) { " unread" } else { "" })
                draggable="true"
                on:dragstart=move |e| agentum_ui::dnd::start(&e, "code", &[("thread", &id_drag)])
                on:contextmenu=move |e| context_menu(&e, menu_for(&t_menu))
                on:mouseenter=move |_| timer.set_value(Some(gloo_timers::callback::Timeout::new(500, move || { let _ = hover.try_set(true); })))
                on:mouseleave=move |_| { let _ = timer.try_set_value(None); let _ = hover.try_set(false); }>
                <a class="dk-hit" href=format!("/code?thread={}", id_click) data-thread=id_click.clone() aria-label=title.clone() draggable="false"
                    on:click=move |e: leptos::ev::MouseEvent| row_click(e, &id_click)/>
                <Glyph t=t.clone()/>
                <div class="dk-body">
                    <div class="dk-l1">
                        <span class="dk-title">{title.clone()}</span>
                        {t.pinned.then(|| view! { <Icon name="pin" size=11 class="dk-pin"/> })}
                    </div>
                    <div class="dk-l2">{meta}{badges}{has_kids.then(|| view! { <span class="dk-kidsn" title="Side chats">{kids.len()}</span> })}</div>
                </div>
                <div class="dk-right">
                    <span class=if can_park { "dk-status can-park" } else { "dk-status" }>{ago_ms(t.updated_at)}</span>
                    {can_park.then(|| view! {
                        <span class="dk-park">
                            <button class="icon-btn" title="Snooze until tomorrow 09:00" on:click=move |_| patch_thread(&id_snooze, PatchThread { snoozed_until: Some(Some(tomorrow_9())), ..Default::default() })><Icon name="alarm" size=13/></button>
                            <button class="icon-btn" title="Settle (done; archived, shown for a day)" on:click=move |_| patch_thread(&id_settle, PatchThread { archived: Some(true), ..Default::default() })><Icon name="check" size=13/></button>
                        </span>
                    })}
                    {pill(&t).map(|(class, label)| view! { <span class=class>{label}</span> })}
                </div>
                {move || hover.get().then(|| {
                    let views = crate::plugins::thread_hover(&t_hover);
                    (!views.is_empty()).then(|| view! { <div class="hover-card">{views}</div> })
                })}
            </div>
            {has_kids.then(|| view! {
                <Show when=move || kids_open.get()>
                    <ul class="dk-kids">{kids.clone().into_iter().map(|k| view! { <Slim t=k on=on on_r=on_r/> }).collect_view()}</ul>
                </Show>
            })}
        </li>
    }
}

/// A one-line row: a side chat, or a parked thread on a shelf.
#[component]
fn Slim(t: ThreadSummary, on: Signal<Option<String>>, on_r: Signal<Option<String>>, #[prop(optional)] restore: Option<&'static str>) -> impl IntoView {
    let id = t.id.clone();
    let is_on = {
        let id = id.clone();
        Signal::derive(move || on.get().as_deref() == Some(id.as_str()) || on_r.get().as_deref() == Some(id.as_str()))
    };
    let (id_c, id_d, id_r) = (id.clone(), id.clone(), id.clone());
    let t_menu = t.clone();
    let when = match restore {
        Some("snooze") => t.snoozed_until.map(|u| format!("{}", agentum_ui::fmt::clock(u))).unwrap_or_default(),
        _ => ago_ms(t.archived_at.unwrap_or(t.updated_at)),
    };
    view! {
        <li class=move || if is_on.get() { "dk-slim on" } else { "dk-slim" } draggable="true"
            on:dragstart=move |e| agentum_ui::dnd::start(&e, "code", &[("thread", &id_d)])
            on:contextmenu=move |e| context_menu(&e, menu_for(&t_menu))>
            <a class="dk-hit" href=format!("/code?thread={}", id_c) data-thread=id_c.clone() draggable="false"
                on:click=move |e: leptos::ev::MouseEvent| row_click(e, &id_c)/>
            <Glyph t=t.clone()/>
            <span class="dk-title">{if t.title.is_empty() { "New thread".to_string() } else { t.title.clone() }}</span>
            <span class="dk-status">{when}</span>
            {restore.map(|r| view! {
                <button class="icon-btn dk-restore" title=if r == "snooze" { "Wake it now" } else { "Unsettle" }
                    on:click=move |_| patch_thread(&id_r, if r == "snooze" { PatchThread { snoozed_until: Some(None), ..Default::default() } } else { PatchThread { archived: Some(false), ..Default::default() } })>
                    <Icon name="refresh" size=12/>
                </button>
            })}
        </li>
    }
}

#[component]
fn Project(path: String, fam: Vec<(ThreadSummary, Vec<ThreadSummary>)>, on: Signal<Option<String>>, on_r: Signal<Option<String>>, force: bool) -> impl IntoView {
    let key = format!("agentum.dk.fold.{path}");
    let open = RwSignal::new(agentum_ui::store::get(&key).as_deref() != Some("1"));
    let name = project_name(&path);
    let all: Vec<&ThreadSummary> = fam.iter().flat_map(|(r, k)| std::iter::once(r).chain(k.iter())).collect();
    let (asks, work, unread) = (all.iter().any(|t| needs_you(t)), all.iter().any(|t| working(t)), all.iter().any(|t| t.unread));
    let n = fam.len();
    let machine = fam.first().map(|(r, _)| r.machine.clone()).unwrap_or_default();
    let (p2, m2) = (path.clone(), machine.clone());
    let badge = { let (bg, fg) = badge_colors(&path); format!("background:{bg};color:{fg}") };
    let letter = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "•".into());
    view! {
        <section class="dk-proj">
            <div class="dk-proj-h">
                <button class="dk-proj-btn" aria-expanded=move || open.get().to_string() title=path.clone()
                    on:click=move |_| { open.update(|o| *o = !*o); agentum_ui::store::set(&key, if open.get_untracked() { "" } else { "1" }) }>
                    <span class="dk-init" style=badge>{letter}</span>
                    <span class="dk-proj-name">{name.clone()}</span>
                    <span class="dk-count">{n}</span>
                    <span class="dk-dots">
                        {asks.then(|| view! { <span class="dk-dot ask"/> })}
                        {work.then(|| view! { <Spinner class="dk-dot work" size=11/> })}
                        {unread.then(|| view! { <span class="dk-dot unread"/> })}
                    </span>
                    <Icon name="chevron" size=12 class=Signal::derive(move || if open.get() || force { "dk-chev open".to_string() } else { "dk-chev".to_string() })/>
                </button>
                <button class="icon-btn dk-proj-new" title=format!("New thread in {name}") on:click=move |_| navigate("code", Where::Replace, &[("thread", ""), ("machine", &m2), ("project", &p2)])><Icon name="plus" size=13/></button>
            </div>
            <Show when=move || open.get() || force>
                <ul class="dk-list">{fam.clone().into_iter().map(|(r, kids)| view! { <Card t=r kids=kids on=on on_r=on_r/> }).collect_view()}</ul>
            </Show>
        </section>
    }
}

#[component]
fn Shelf(label: &'static str, items: Vec<ThreadSummary>, on: Signal<Option<String>>, on_r: Signal<Option<String>>, restore: &'static str) -> impl IntoView {
    let open = RwSignal::new(false);
    let n = items.len();
    (n > 0).then(|| view! {
        <section class="dk-shelf">
            <button class="dk-shelf-h" aria-expanded=move || open.get().to_string() on:click=move |_| open.update(|o| *o = !*o)>
                <span>{move || if open.get() { label.to_string() } else { format!("{label} ({n})") }}</span>
                <span class="dk-rule"/>
                <Icon name="chevron" size=12 class=Signal::derive(move || if open.get() { "dk-chev open".to_string() } else { "dk-chev".to_string() })/>
            </button>
            <Show when=move || open.get()>
                <ul class="dk-list">{items.clone().into_iter().map(|t| view! { <Slim t=t on=on on_r=on_r restore=restore/> }).collect_view()}</ul>
            </Show>
        </section>
    })
}

/// The Coding side of the sidebar.
#[component]
pub fn Dockside(collapsed: RwSignal<bool>, #[prop(into)] mail_open: Signal<bool>) -> impl IntoView {
    store::start();
    let ov = overview();
    let (l_thread, r_thread) = (use_param("thread"), use_param("r_thread"));
    let current = Signal::derive(move || l_thread.get().filter(|t| !t.is_empty()).or_else(|| r_thread.get()));
    // Minute clock: snoozes wake on time.
    let now = RwSignal::new(now_ms());
    let tick = StoredValue::new_local(Some(gloo_timers::callback::Interval::new(60_000, move || now.set(now_ms()))));
    on_cleanup(move || tick.set_value(None));
    // ⌘1–9 open the visible threads in order; ⌘⇧[ / ] the previous and next.
    let visible = Memo::new(move |_| ov.with(|o| o.as_ref().map(|o| order(&o.threads, now.get())).unwrap_or_default()));
    agentum_ui::on_window(leptos::ev::keydown, move |e| {
        if !(e.meta_key() || e.ctrl_key()) {
            return;
        }
        let ids = visible.get_untracked();
        let k = e.key();
        if let Some(n) = k.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
            if let Some(id) = ids.get(n - 1) {
                e.prevent_default();
                navigate("code", Where::Replace, &[("thread", id)]);
            }
        } else if e.shift_key() && matches!(k.as_str(), "[" | "]" | "{" | "}") {
            let cur = current.get_untracked().unwrap_or_default();
            let i = ids.iter().position(|x| *x == cur);
            let next = match (i, k == "]" || k == "}") {
                (Some(i), true) => ids.get(i + 1),
                (Some(i), false) => i.checked_sub(1).and_then(|j| ids.get(j)),
                (None, _) => ids.first(),
            };
            if let Some(id) = next {
                e.prevent_default();
                navigate("code", Where::Replace, &[("thread", id)]);
            }
        }
    });
    let press = StoredValue::new(None::<(String, i32, i32)>);
    let on = Signal::derive(move || l_thread.get());
    let on_r = Signal::derive(move || r_thread.get());
    let dock_on = move || crate::plugins::is_on("dockside");
    let list = move || {
        let (q, searching) = (String::new(), false);
        ov.with(|o| {
            let Some(o) = o else { return view! { <p class="dk-empty">"Connecting to your machines…"</p> }.into_any() };
            if !dock_on() {
                // The plain list: pinned, then everything open by latest activity.
                let ql = q.to_lowercase();
                let mut v: Vec<ThreadSummary> = o.threads.iter().filter(|t| !t.archived && (ql.is_empty() || t.title.to_lowercase().contains(&ql) || t.project.to_lowercase().contains(&ql))).cloned().collect();
                v.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated_at.cmp(&a.updated_at)));
                let empty = v.is_empty();
                return view! {
                    <ul class="dk-list">{v.into_iter().map(|t| view! { <Card t=t kids=vec![] on=on on_r=on_r/> }).collect_view()}</ul>
                    {empty.then(|| view! { <p class="dk-empty">"No threads yet."</p> })}
                }.into_any();
            }
            let g = group(&o.threads, now.get(), &q);
            let total = g.projects.len() + g.snoozed.len() + g.settled.len();
            view! {
                {g.projects.into_iter().map(|(p, fam)| view! { <Project path=p fam=fam on=on on_r=on_r force=searching/> }).collect_view()}
                <Shelf label="Snoozed" items=g.snoozed on=on on_r=on_r restore="snooze"/>
                <Shelf label="Settled" items=g.settled on=on on_r=on_r restore="settle"/>
                {(total == 0).then(|| view! { <p class="dk-empty">{if searching { "No threads found." } else { "No threads yet." }}</p> })}
            }
            .into_any()
        })
    };
    view! {
        <Show when=move || !collapsed.get() fallback=move || view! {
            <button class="nav" title="New thread" on:click=move |_| navigate("code", Where::Replace, &[("thread", "")])><Icon name="plus"/></button>
        }>
            <div class="dk">
                <div class="dk-top">
                    <button class="dk-new" title="New thread" on:click=move |_| navigate("code", Where::Replace, &[("thread", "")])>
                        <Icon name="plus-circle" size=16/><span>"New thread"</span>
                    </button>
                    <span class="dk-machines" title="Machines online">{move || ov.with(|o| o.as_ref().map(|o| format!("{}/{}", o.machines.iter().filter(|m| m.online).count(), o.machines.len())).unwrap_or_default())}</span>
                    <button class="icon-btn" title="Go to… a thread, what was said, a page (⌘K)" on:click=move |_| crate::palette::shown().set(true)><Icon name="search" size=15/></button>
                </div>
                // Mail open beside a thread: its folders here too (one sidebar, whichever side).
                <Show when=move || mail_open.get()>
                    <div class="dk-mail">
                        <button class="nav on" title="Mail" on:click=move |_| navigate("mail", Where::Replace, &[])><Icon name="mail"/><span>"Mail"</span></button>
                        <crate::pages::mail::side::Folders/>
                    </div>
                </Show>
                <div class="dk-head"><Icon name="folder" size=13/><span>{move || if dock_on() { "Projects" } else { "Threads" }}</span>
                    <span class="dk-count">{move || ov.with(|o| o.as_ref().map(|o| group(&o.threads, now.get(), "").projects.len()).unwrap_or(0))}</span>
                </div>
                <div class="dk-scroll"
                    on:pointerdown=move |e| press.set_value((e.button() == 0).then(|| hit_thread(&e).map(|id| (id, e.client_x(), e.client_y()))).flatten())
                    on:pointerup=move |e| {
                        // A press and release on the same row, without dragging, opens it.
                        if let Some((id, x, y)) = press.get_value() {
                            press.set_value(None);
                            if e.button() == 0 && hit_thread(&e).as_deref() == Some(id.as_str()) && (e.client_x() - x).abs() + (e.client_y() - y).abs() < 6 {
                                open_thread(&e, &id)
                            }
                        }
                    }
                    on:pointercancel=move |_| press.set_value(None)>
                    {list}
                </div>
            </div>
        </Show>
    }
}
