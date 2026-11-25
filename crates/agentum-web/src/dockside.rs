//! The Coding sidebar, after Dockside (the author's BB plugin): projects stay put, roots keep their
//! creation order (pinned first), and status lives inside each row, so nothing moves under the
//! cursor when an agent finishes. Clear the list with two verbs: **snooze** (until tomorrow 09:00;
//! it wakes early if it works or asks) and **settle** (archive; shown for a day). Side chats sit
//! under their thread. Drag a card to a half to open it there; ⌘-click opens it on the right.
use crate::code::store::{self, overview};
use crate::code::{agent_icon, patch_thread, tomorrow_9};
use agentum_proto::api::{PatchThread, SearchHit, ThreadSummary};
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

/// The glyph before a title: working (spins), needs you (filled), unread (a ring), else the agent.
#[component]
fn Glyph(t: ThreadSummary) -> impl IntoView {
    if needs_you(&t) {
        let failed = t.state == ThreadState::Error;
        view! { <span class=if failed { "dk-g dk-g-fail" } else { "dk-g dk-g-ask" } title=if failed { "Stopped with an error" } else { "Needs you" }/> }.into_any()
    } else if working(&t) {
        view! { <span class="dk-g dk-g-work" title="Working"/> }.into_any()
    } else if t.unread {
        view! { <span class="dk-g dk-g-unread" title="Unread"/> }.into_any()
    } else {
        view! { <span class="dk-g dk-g-agent"><Icon name=agent_icon(t.agent) size=12/></span> }.into_any()
    }
}

fn status_label(t: &ThreadSummary) -> String {
    if needs_you(t) {
        if t.state == ThreadState::Error { "Failed".into() } else { "Needs you".into() }
    } else if working(t) {
        "Working".into()
    } else {
        ago_ms(t.updated_at)
    }
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
                <a class="dk-hit" href=format!("/code?thread={}", id_click) aria-label=title.clone() draggable="false"
                    on:click=move |e: leptos::ev::MouseEvent| { if e.button() == 0 { e.prevent_default(); open_thread(&e, &id_click) } }/>
                <Glyph t=t.clone()/>
                <div class="dk-body">
                    <div class="dk-l1">
                        <span class="dk-title">{title.clone()}</span>
                        {t.pinned.then(|| view! { <Icon name="pin" size=11 class="dk-pin"/> })}
                    </div>
                    <div class="dk-l2">{meta}{badges}{has_kids.then(|| view! { <span class="dk-kidsn" title="Side chats">{kids.len()}</span> })}</div>
                </div>
                <div class="dk-right">
                    <span class=if can_park { "dk-status can-park" } else { "dk-status" }>{status_label(&t)}</span>
                    {can_park.then(|| view! {
                        <span class="dk-park">
                            <button class="icon-btn" title="Snooze until tomorrow 09:00" on:click=move |_| patch_thread(&id_snooze, PatchThread { snoozed_until: Some(Some(tomorrow_9())), ..Default::default() })><Icon name="alarm" size=13/></button>
                            <button class="icon-btn" title="Settle (done; archived, shown for a day)" on:click=move |_| patch_thread(&id_settle, PatchThread { archived: Some(true), ..Default::default() })><Icon name="check" size=13/></button>
                        </span>
                    })}
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
            <a class="dk-hit" href=format!("/code?thread={}", id_c) draggable="false"
                on:click=move |e: leptos::ev::MouseEvent| { if e.button() == 0 { e.prevent_default(); open_thread(&e, &id_c) } }/>
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
    let letter = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "•".into());
    view! {
        <section class="dk-proj">
            <div class="dk-proj-h">
                <button class="dk-proj-btn" aria-expanded=move || open.get().to_string() title=path.clone()
                    on:click=move |_| { open.update(|o| *o = !*o); agentum_ui::store::set(&key, if open.get_untracked() { "" } else { "1" }) }>
                    <span class="dk-init">{letter}</span>
                    <span class="dk-proj-name">{name.clone()}</span>
                    <span class="dk-count">{n}</span>
                    <span class="dk-dots">
                        {asks.then(|| view! { <span class="dk-dot ask"/> })}
                        {work.then(|| view! { <span class="dk-dot work"/> })}
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
pub fn Dockside(collapsed: RwSignal<bool>) -> impl IntoView {
    store::start();
    let ov = overview();
    let (l_thread, r_thread) = (use_param("thread"), use_param("r_thread"));
    let current = Signal::derive(move || l_thread.get().filter(|t| !t.is_empty()).or_else(|| r_thread.get()));
    let find = RwSignal::new(None::<String>);
    let hits = RwSignal::new(Vec::<SearchHit>::new());
    // Minute clock: snoozes wake on time.
    let now = RwSignal::new(now_ms());
    let tick = StoredValue::new_local(Some(gloo_timers::callback::Interval::new(60_000, move || now.set(now_ms()))));
    on_cleanup(move || tick.set_value(None));
    Effect::new(move |_| {
        let q = find.get().unwrap_or_default().trim().to_string();
        if q.len() < 2 {
            hits.set(vec![]);
            return;
        }
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(150).await;
            if find.get_untracked().unwrap_or_default().trim() != q {
                return;
            }
            let enc = js_sys::encode_uri_component(&q);
            if let Ok(h) = api_get::<Vec<SearchHit>>(&format!("/code/search?q={enc}")).await {
                hits.set(h.into_iter().filter(|h| h.seq > 0).collect());
            }
        });
    });
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
    let on = Signal::derive(move || l_thread.get());
    let on_r = Signal::derive(move || r_thread.get());
    let dock_on = move || crate::plugins::is_on("dockside");
    let list = move || {
        let q = find.get().unwrap_or_default();
        let searching = !q.trim().is_empty();
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
                    <button class=move || if find.get().is_some() { "icon-btn on" } else { "icon-btn" } title="Find a thread, or what was said"
                        on:click=move |_| find.update(|f| *f = if f.is_some() { None } else { Some(String::new()) })><Icon name="search" size=15/></button>
                </div>
                <Show when=move || find.get().is_some()>
                    <input class="dock-find" autofocus placeholder="Find a thread or what was said" prop:value=move || find.get().unwrap_or_default()
                        on:input=move |e| find.set(Some(event_target_value(&e))) on:keydown=move |e| if e.key() == "Escape" { find.set(None) }/>
                </Show>
                // Dockside's top list: the plugins' pages (Usage, Browser…), as BB shows them.
                <div class="dk-nav">
                    {move || crate::plugins::pages().into_iter().map(|d| {
                        let id = d.id;
                        view! {
                            <button class="dk-navrow" draggable="true" title=format!("{} · drag it to a side to open it there", d.title)
                                on:dragstart=move |e| agentum_ui::dnd::start(&e, id, &[])
                                on:click=move |e| navigate(id, if e.meta_key() || e.ctrl_key() { Where::Right } else { Where::Replace }, &[])>
                                <Icon name=d.icon size=15/><span>{d.title}</span>
                            </button>
                        }
                    }).collect_view()}
                </div>
                <div class="dk-head"><Icon name="folder" size=13/><span>{move || if dock_on() { "Projects" } else { "Threads" }}</span>
                    <span class="dk-count">{move || ov.with(|o| o.as_ref().map(|o| group(&o.threads, now.get(), "").projects.len()).unwrap_or(0))}</span>
                </div>
                <div class="dk-scroll">
                    {list}
                    {move || {
                        let h = hits.get();
                        (!h.is_empty()).then(|| view! {
                            <div class="dk-shelf-h static"><span>"Said in threads"</span><span class="dk-rule"/></div>
                            <ul class="dk-list">{h.into_iter().take(20).map(|hit| { let id = hit.thread.clone(); let seq = hit.seq.to_string(); view! {
                                <li class="dk-slim hit">
                                    <button class="dk-hit" on:click=move |_| navigate("code", Where::Replace, &[("thread", &id), ("at", &seq)])/>
                                    <span class="dk-title">{hit.title.clone()}</span>
                                    <span class="dk-excerpt" inner_html=hit.excerpt.clone()/>
                                </li>
                            }}).collect_view()}</ul>
                        })
                    }}
                </div>
            </div>
        </Show>
    }
}
