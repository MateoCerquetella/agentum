//! The sidebar: the Personal / Coding switch, each side's list, and the footer.
//!
//! Coding lists every thread on every machine the way old Agentum and BB together do it best:
//! Pinned, then **Needs you** (a question, a permission, a failure), **Ready** (a finished turn
//! you haven't read), **Working**, and the rest by project. ⌘1–9 open the visible rows in order.
use crate::code::store::{self, overview};
use crate::pages::{self, theme::ModeSwitch, Side};
use agentum_proto::api::ThreadSummary;
use agentum_proto::{Attention, ThreadState};
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

#[component]
pub fn SideSwitch(side: RwSignal<Side>, collapsed: RwSignal<bool>, on_set: Callback<Side>) -> impl IntoView {
    let ov = overview();
    let asks = move || ov.with(|o| o.as_ref().map(|o| o.threads.iter().filter(|t| !t.archived && t.attention == Attention::NeedsYou).count()).unwrap_or(0));
    view! {
        <div class="side-switch" role="radiogroup" aria-label="Personal or coding">
            {[(Side::Life, "Personal", "lock", "Your pages and private chats"), (Side::Work, "Coding", "bot", "Every coding thread on your machines")].into_iter().map(|(k, label, icon, title)| view! {
                <button role="radio" aria-checked=move || (side.get() == k).to_string() title=title on:click=move |_| on_set.run(k)>
                    <Icon name=icon size=16/>
                    <Show when=move || !collapsed.get()><span>{label}</span></Show>
                    {(k == Side::Work).then(|| view! {
                        <Show when=move || { asks() > 0 }>
                            <i class="side-badge" title=move || format!("{} need you", asks())>{asks}</i>
                        </Show>
                    })}
                </button>
            }).collect_view()}
        </div>
    }
}

/// Personal: the Secretary, then your pages (drag one onto another to reorder).
#[component]
pub fn Personal(left: RwSignal<String>, right: RwSignal<Option<String>>, collapsed: RwSignal<bool>, counts: RwSignal<BTreeMap<String, u64>>, prefs: RwSignal<Value>) -> impl IntoView {
    let order = Memo::new(move |_| {
        let has_ha = prefs.with(|p| p["ha_url"].as_str().is_some_and(|u| !u.is_empty()));
        let mut on: Vec<&'static str> = pages::NAV.to_vec();
        if has_ha {
            on.push("home");
        }
        on.extend(crate::plugins::nav_pages());
        let saved: Vec<String> = prefs.with(|p| p["sidebar"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default());
        let mut out: Vec<&'static str> = saved.iter().filter_map(|s| on.iter().copied().find(|p| p == s)).collect();
        for p in on {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    });
    let dragging = RwSignal::new(None::<&'static str>);
    let over = RwSignal::new(None::<&'static str>);
    let move_nav = move |p: &'static str, before: &'static str| {
        if p == before {
            return;
        }
        let mut next: Vec<&'static str> = order.get_untracked().into_iter().filter(|x| *x != p).collect();
        let i = next.iter().position(|x| *x == before).unwrap_or(next.len());
        next.insert(i, p);
        prefs.update(|u| {
            if !u.is_object() {
                *u = json!({});
            }
            u["sidebar"] = json!(next);
        });
        spawn_local(async move {
            let _ = api_put::<Value>("/settings/sidebar", &json!({"order": next})).await;
        });
    };
    view! {
        <Show when=move || !collapsed.get()>
            <div class="side-bot">
                <button class="side-bot-main" title="The Secretary: your private chats, on your own model" on:click=move |_| navigate("chat", Where::Replace, &[])>
                    <span class="side-bot-text"><b>"Secretary"</b><span class="truncate">"Private · on your model"</span></span>
                </button>
                <button class="icon-btn side-bot-plus" title="New private chat" on:click=move |_| navigate("chat", Where::Replace, &[("chat", "")])><Icon name="plus" size=16/></button>
            </div>
            <crate::pages::chat::side::PersonalChats/>
            <div class="nav-label side-pages">"Pages"</div>
        </Show>
        <For each=move || order.get() key=|p| *p let:id>
            {pages::def(id).map(|d| view! {
                <button draggable="true"
                    class=move || format!("nav{}{}", if left.get() == id || right.get().as_deref() == Some(id) { " on" } else { "" }, if over.get() == Some(id) && dragging.get().is_some_and(|x| x != id) { " drop-before" } else { "" })
                    title=format!("{} · drag onto another item to move it", d.title)
                    on:dragstart=move |e| { if let Some(dt) = e.data_transfer() { let _ = dt.set_data("text/cerq-page", id); } dragging.set(Some(id)) }
                    on:dragend=move |_| { dragging.set(None); over.set(None) }
                    on:dragover=move |e| { if dragging.get_untracked().is_some() { e.prevent_default(); over.set(Some(id)) } }
                    on:drop=move |e| { e.prevent_default(); if let Some(p) = dragging.get_untracked() { move_nav(p, id) } dragging.set(None); over.set(None) }
                    on:click=move |e| navigate(id, if e.alt_key() || e.meta_key() { Where::Right } else { Where::Replace }, &[])>
                    <Icon name=d.icon/>
                    <Show when=move || !collapsed.get()><span>{d.title}</span></Show>
                    <Show when=move || { !collapsed.get() && counts.with(|c| c.get(id).copied().unwrap_or(0)) > 0 }>
                        <span class="count">{move || counts.with(|c| c.get(id).copied().unwrap_or(0))}</span>
                    </Show>
                </button>
            })}
        </For>
    }
}

/// The kind of mark a thread row shows.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Working,
    NeedsYou,
    Failed,
    Unread,
    Idle,
}
fn mark(t: &ThreadSummary) -> Mark {
    match t.attention {
        Attention::NeedsYou if t.state == ThreadState::Error => Mark::Failed,
        Attention::NeedsYou => Mark::NeedsYou,
        Attention::Working => Mark::Working,
        Attention::Ready => Mark::Unread,
        Attention::Settled if t.state == ThreadState::Error => Mark::Failed,
        Attention::Settled => Mark::Idle,
    }
}

#[component]
fn StatusMark(m: Mark) -> impl IntoView {
    match m {
        Mark::Working => view! {
            <svg viewBox="0 0 16 16" role="img" aria-label="Working" class="fac-busy"><circle cx="8" cy="8" r="6" fill="none" stroke-width="2.2" class="fac-busy-track"/><path d="M8 2a6 6 0 0 1 6 6" fill="none" stroke-width="2.2" stroke-linecap="round" class="fac-busy-arc"/></svg>
        }.into_any(),
        Mark::Idle => view! { <span class="dock-mark dock-mark-inactive"><Icon name="clock" size=13/></span> }.into_any(),
        _ => {
            let (cls, inner) = match m {
                Mark::Unread => ("dock-mark dock-mark-unread", view! { <path d="m6.3 10 2.4 2.5 5-5" stroke="var(--dock-cut)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/> }.into_any()),
                Mark::Failed => ("dock-mark dock-mark-failed", view! { <path d="m7 7 6 6m0-6-6 6" stroke="var(--dock-cut)" stroke-width="2" stroke-linecap="round"/> }.into_any()),
                _ => ("dock-mark dock-mark-needs-you", view! { <><path d="M10 5.5v5" stroke="var(--dock-cut)" stroke-width="2" stroke-linecap="round"/><circle cx="10" cy="14" r="1" fill="var(--dock-cut)"/></> }.into_any()),
            };
            view! { <svg aria-hidden="true" viewBox="0 0 20 20" class=cls fill="none"><circle cx="10" cy="10" r="8.5" fill="currentColor"/>{inner}</svg> }.into_any()
        }
    }
}

fn project_name(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

const PALETTE: [&str; 12] = ["#2563EB", "#7C3AED", "#C026D3", "#DB2777", "#DC2626", "#EA580C", "#CA8A04", "#16A34A", "#059669", "#0891B2", "#0284C7", "#4F46E5"];
fn color(id: &str) -> &'static str {
    let mut h: u32 = 0x811c9dc5;
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    PALETTE[(h as usize) % PALETTE.len()]
}

#[component]
fn Row(t: ThreadSummary, on: Signal<bool>) -> impl IntoView {
    let id = t.id.clone();
    let m = mark(&t);
    let meta = t.asking.clone().filter(|_| m == Mark::NeedsYou).unwrap_or_else(|| {
        let mut parts = vec![project_name(&t.project)];
        if let Some(b) = &t.branch {
            parts.push(b.clone());
        }
        parts.push(t.machine.clone());
        parts.join(" · ")
    });
    let title = if t.title.is_empty() { "New thread".to_string() } else { t.title.clone() };
    let hover = crate::plugins::thread_row(&t);
    // A hover card after a short wait (plugins draw it).
    let card = RwSignal::new(false);
    let timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    let t_hover = t.clone();
    view! {
        <div class=move || format!("dock-row{}{}", if on.get() { " on" } else { "" }, if matches!(m, Mark::Unread | Mark::NeedsYou) { " unread" } else { "" })
            on:mouseenter=move |_| timer.set_value(Some(gloo_timers::callback::Timeout::new(450, move || card.set(true))))
            on:mouseleave=move |_| { timer.set_value(None); card.set(false) }>
            {move || card.get().then(|| {
                let views = crate::plugins::thread_hover(&t_hover);
                (!views.is_empty()).then(|| view! { <div class="hover-card">{views}</div> })
            })}
            <button class="dock-row-hit" title=title.clone() aria-current=move || on.get().then_some("true")
                on:click=move |e| navigate("code", if e.alt_key() || e.meta_key() { Where::Right } else { Where::Replace }, &[("thread", &id)])/>
            <span class="dock-row-1"><span class="dock-title">{title}</span><span class="dock-time">{ago_ms(t.updated_at)}</span></span>
            <span class="dock-row-2"><span class="dock-meta">{meta}</span>{hover}<StatusMark m=m/></span>
        </div>
    }
}

#[component]
fn Section(id: String, name: String, count: usize, #[prop(optional)] badge: Option<&'static str>, #[prop(optional)] on_new: Option<Callback<()>>, children: ChildrenFn) -> impl IntoView {
    let key = format!("cerq.side.fold.{id}");
    let open = RwSignal::new(agentum_ui::store::get(&key).as_deref() != Some("1"));
    let bg = color(&id);
    let letter = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    view! {
        <section class="dock-section">
            <div class="dock-head">
                <span class="dock-badge" style=format!("background:{bg}")>{match badge { Some(i) => view! { <Icon name=i size=11/> }.into_any(), None => letter.into_any() }}</span>
                <span class="dock-name">{name.clone()}</span><span class="dock-count">{count}</span>
                {on_new.map(|f| view! { <button class="icon-btn dock-btn" title=format!("New in {name}") on:click=move |_| f.run(())><Icon name="plus" size=13/></button> })}
                <button class="icon-btn dock-btn" title=move || if open.get() { "Fold" } else { "Unfold" } on:click=move |_| {
                    open.update(|o| *o = !*o);
                    agentum_ui::store::set(&key, if open.get_untracked() { "" } else { "1" });
                }><Icon name="chevron" size=13 class=Signal::derive(move || if open.get() { "files-chev open".to_string() } else { "files-chev".to_string() })/></button>
            </div>
            <Show when=move || open.get()><div class="dock-rows">{children()}</div></Show>
        </section>
    }
}

/// Coding: every thread on every machine.
#[component]
pub fn Coding(collapsed: RwSignal<bool>) -> impl IntoView {
    store::start();
    let ov = overview();
    // The thread open in either pane.
    let (l_thread, r_thread) = (use_param("thread"), use_param("r_thread"));
    let current = Signal::derive(move || l_thread.get().filter(|t| !t.is_empty()).or_else(|| r_thread.get()));
    let find = RwSignal::new(None::<String>);
    let archived = RwSignal::new(false);
    let found = RwSignal::new(None::<Vec<ThreadSummary>>);
    let hits = RwSignal::new(Vec::<agentum_proto::api::SearchHit>::new());
    Effect::new(move |_| {
        let q = find.get().unwrap_or_default().trim().to_string();
        let arch = archived.get();
        if q.is_empty() && !arch {
            found.set(None);
            hits.set(vec![]);
            return;
        }
        spawn_local(async move {
            if arch {
                if let Ok(t) = api_get::<Vec<ThreadSummary>>("/code/threads?archived=1").await {
                    found.set(Some(t));
                }
            }
            if !q.is_empty() {
                let q2 = js_sys::encode_uri_component(&q);
                if let Ok(h) = api_get::<Vec<agentum_proto::api::SearchHit>>(&format!("/code/search?q={q2}")).await {
                    hits.set(h);
                }
            }
        });
    });

    // The visible order, for ⌘1–9 and ⌘⇧[ / ].
    let visible = Memo::new(move |_| ov.with(|o| o.as_ref().map(|o| store::sidebar_order(&o.threads)).unwrap_or_default()));
    let keys = window_event_listener(leptos::ev::keydown, move |e| {
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
        } else if e.shift_key() && (k == "[" || k == "]" || k == "{" || k == "}") {
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
    // The sidebar is drawn again now and then: the old listener must go with it.
    on_cleanup(move || keys.remove());

    let is_on = move |id: String| Signal::derive(move || l_thread.get().as_deref() == Some(id.as_str()) || r_thread.get().as_deref() == Some(id.as_str()));
    let groups = move || {
        ov.with(|o| {
            let Some(o) = o else { return view! { <p class="dock-empty">"Connecting to your machines…"</p> }.into_any() };
            let open: Vec<&ThreadSummary> = o.threads.iter().filter(|t| !t.archived && t.parent.is_none()).collect();
            let pinned: Vec<ThreadSummary> = open.iter().filter(|t| t.pinned).map(|t| (*t).clone()).collect();
            let by = |a: Attention| -> Vec<ThreadSummary> {
                let mut v: Vec<ThreadSummary> = open.iter().filter(|t| !t.pinned && t.attention == a).map(|t| (*t).clone()).collect();
                v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                v
            };
            let (needs, ready, working, settled) = (by(Attention::NeedsYou), by(Attention::Ready), by(Attention::Working), by(Attention::Settled));
            let week = agentum_ui::fmt::now_ms() - 7 * 86_400_000;
            let (recent, older): (Vec<ThreadSummary>, Vec<ThreadSummary>) = settled.into_iter().partition(|t| t.updated_at > week);
            let mut projects: Vec<String> = recent.iter().map(|t| t.project.clone()).collect();
            projects.dedup();
            projects.sort_by_key(|p| std::cmp::Reverse(recent.iter().filter(|t| &t.project == p).map(|t| t.updated_at).max().unwrap_or(0)));
            projects.dedup();
            let rows = move |v: Vec<ThreadSummary>| v.into_iter().map(|t| { let on = is_on(t.id.clone()); view! { <Row t=t on=on/> } }).collect_view();
            let sec = move |id: &str, name: &str, badge: Option<&'static str>, v: Vec<ThreadSummary>| {
                if v.is_empty() {
                    return ().into_any();
                }
                let n = v.len();
                let v2 = v.clone();
                view! { <Section id=id.to_string() name=name.to_string() count=n badge=badge.unwrap_or("dot")>{ let v = v2.clone(); move || rows(v.clone()) }</Section> }.into_any()
            };
            let empty = open.is_empty();
            view! {
                {sec("pinned", "Pinned", Some("pin"), pinned)}
                {sec("needs-you", "Needs you", Some("bell"), needs)}
                {sec("ready", "Ready", Some("check"), ready)}
                {sec("working", "Working", Some("bot"), working)}
                {projects.into_iter().map(|p| {
                    let v: Vec<ThreadSummary> = recent.iter().filter(|t| t.project == p).cloned().collect();
                    let first = v.first().cloned();
                    let n = v.len();
                    let name = project_name(&p);
                    let on_new = first.map(|t| Callback::new(move |_| navigate("code", Where::Replace, &[("thread", ""), ("machine", &t.machine), ("project", &t.project)])));
                    let v2 = v.clone();
                    match on_new {
                        Some(f) => view! { <Section id=p.clone() name=name count=n on_new=f>{ let v = v2.clone(); move || rows(v.clone()) }</Section> }.into_any(),
                        None => ().into_any(),
                    }
                }).collect_view()}
                {(!older.is_empty()).then(|| { let n = older.len(); let o2 = older.clone(); view! { <Section id="older".to_string() name="Older".to_string() count=n badge="archive">{ let v = o2.clone(); move || rows(v.clone()) }</Section> } })}
                {empty.then(|| view! { <p class="dock-empty">"No open threads. Start one with +."</p> })}
            }.into_any()
        })
    };

    view! {
        <Show when=move || !collapsed.get() fallback=move || view! {
            <button class="nav" title="Code" on:click=move |_| navigate("code", Where::Replace, &[])><Icon name="bot"/></button>
        }>
            <div class="side-bot">
                <button class="side-bot-main" title="Code: threads on your machines" on:click=move |_| navigate("code", Where::Replace, &[])>
                    <span class="side-bot-text"><b>"Code"</b><span class="truncate">{move || store::machines_line()}</span></span>
                </button>
                <button class="icon-btn side-bot-plus" title="New thread (⌘N)" on:click=move |_| navigate("code", Where::Replace, &[("thread", "")])><Icon name="plus" size=16/></button>
            </div>
            <div class="dock-chats-head"><span>"Threads"</span><span class="dock-count">{move || ov.with(|o| o.as_ref().map(|o| o.threads.iter().filter(|t| !t.archived).count()).unwrap_or(0))}</span><span class="grow"/>
                <button class=move || if find.get().is_some() { "icon-btn dock-btn on" } else { "icon-btn dock-btn" } title="Find a thread (what was said, too)"
                    on:click=move |_| find.update(|f| *f = if f.is_some() { None } else { Some(String::new()) })><Icon name="search" size=13/></button>
                <button class=move || if archived.get() { "icon-btn dock-btn on" } else { "icon-btn dock-btn" } title=move || if archived.get() { "Back to open threads" } else { "Archived threads" }
                    on:click=move |_| archived.update(|a| *a = !*a)><Icon name="archive" size=13/></button>
            </div>
            <Show when=move || find.get().is_some()>
                <input class="dock-find" autofocus placeholder="Find a thread or what was said" prop:value=move || find.get().unwrap_or_default()
                    on:input=move |e| find.set(Some(event_target_value(&e))) on:keydown=move |e| if e.key() == "Escape" { find.set(None) }/>
            </Show>
            <div class="dock-scroll">
                {move || {
                    let h = hits.get();
                    let f = found.get();
                    if f.is_none() && h.is_empty() { return groups().into_any(); }
                    view! {
                        {(!h.is_empty()).then(|| view! {
                            <div class="dock-found">"Said in threads "<span class="dock-count">{h.len()}</span></div>
                            {h.into_iter().map(|hit| { let id = hit.thread.clone(); let seq = hit.seq.to_string(); view! {
                                <div class="dock-row">
                                    <button class="dock-row-hit" on:click=move |_| navigate("code", Where::Replace, &[("thread", &id), ("at", &seq)])/>
                                    <span class="dock-row-1"><span class="dock-title">{hit.title.clone()}</span></span>
                                    <span class="dock-row-2"><span class="dock-meta" inner_html=hit.excerpt.clone()/></span>
                                </div>
                            }}).collect_view()}
                        })}
                        {f.map(|v| view! {
                            <div class="dock-found">"Archived "<span class="dock-count">{v.len()}</span></div>
                            {v.into_iter().map(|t| { let on = is_on(t.id.clone()); view! { <Row t=t on=on/> } }).collect_view()}
                        })}
                    }.into_any()
                }}
            </div>
        </Show>
    }
}

/// Settings, the look, each agent's limits, sign out.
#[component]
pub fn SideFoot(collapsed: RwSignal<bool>, settings_on: Signal<bool>) -> impl IntoView {
    let ov = overview();
    let meters = move || {
        ov.with(|o| {
            o.as_ref()
                .map(|o| {
                    o.limits.iter().filter_map(|l| l.limits.first().map(|w| (l.agent.label(), w.window.clone(), w.used_percent))).take(2).collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    };
    let tone = |pct: f64| if pct >= 90.0 { "var(--negative)" } else if pct >= 70.0 { "var(--warm)" } else { "var(--subdue)" };
    view! {
        <div class=move || if collapsed.get() { "dock-foot collapsed" } else { "dock-foot" }>
            <button class=move || if settings_on.get() { "icon-btn on" } else { "icon-btn" } title="Settings" on:click=move |_| navigate("settings", Where::Replace, &[])><Icon name="settings" size=16/></button>
            <ModeSwitch compact=true/>
            <Show when=move || !collapsed.get()><span class="grow"/></Show>
            <Show when=move || !collapsed.get()>
                {move || meters().into_iter().map(|(name, win, pct)| view! {
                    <button class="dock-meter" title=format!("{name}: {win} {pct:.0}% used") on:click=move |_| navigate("p/usage", Where::Replace, &[])>
                        <Icon name="bot" size=12/>
                        <span class="dock-meter-bar"><i style=format!("width:{}%;background:{}", pct.min(100.0), tone(pct))/></span>
                        <span>{format!("{pct:.0}%")}</span>
                    </button>
                }).collect_view()}
            </Show>
            <button class="icon-btn" title="Sign out" on:click=move |_| spawn_local(async {
                let _ = api_post::<Value>("/auth/logout", &json!({})).await;
                let _ = web_sys::window().unwrap().location().reload();
            })><Icon name="logout" size=15/></button>
        </div>
    }
}
