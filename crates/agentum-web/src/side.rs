//! The sidebar: the Personal / Coding switch, each side's list, and the footer.
//!
//! Coding is Dockside (dockside.rs).
use crate::code::store::overview;
use crate::pages::{self, theme::ModeSwitch, Side};
use agentum_proto::api::Overview;
use agentum_proto::{AgentKind, Attention, RateLimit};
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use std::collections::BTreeMap;
use wasm_bindgen::JsCast;

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
                    on:dragstart=move |e| { agentum_ui::dnd::start(&e, id, &[]); dragging.set(Some(id)) }
                    on:dragend=move |_| { dragging.set(None); over.set(None); agentum_ui::dnd::end() }
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

/// One agent's subscription limits: the newest report among its machines.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentUse {
    pub agent: AgentKind,
    pub machine: String,
    pub at: i64,
    pub windows: Vec<RateLimit>,
}
/// Every agent that reported limits, the one used last first.
pub fn agents_use(o: &Overview) -> Vec<AgentUse> {
    let mut by: Vec<AgentUse> = vec![];
    for l in o.limits.iter().filter(|l| !l.limits.is_empty()) {
        let u = AgentUse { agent: l.agent, machine: l.machine.clone(), at: l.at, windows: l.limits.clone() };
        match by.iter_mut().find(|x| x.agent == l.agent) {
            Some(x) if l.at > x.at => *x = u,
            Some(_) => {}
            None => by.push(u),
        }
    }
    by.sort_by(|a, b| b.at.cmp(&a.at));
    by
}
/// Percent used now: a window whose reset has passed starts again from zero.
fn used(w: &RateLimit) -> f64 {
    if w.resets_at.is_some_and(|r| r <= now_ms()) { 0.0 } else { w.used_percent }
}
fn window_name(w: &str) -> String {
    match w {
        "5h" => "5-hour".into(),
        "weekly" => "Weekly".into(),
        "opus-weekly" => "Weekly · Opus".into(),
        "sonnet-weekly" => "Weekly · Sonnet".into(),
        x => x.replace('-', " "),
    }
}
/// "Resets Wed 15:00" (or "Reset …" once it has).
fn resets(at: Option<i64>) -> String {
    let Some(at) = at else { return String::new() };
    let o = js_sys::Object::new();
    for (k, v) in [("weekday", "short"), ("hour", "2-digit"), ("minute", "2-digit")] {
        let _ = js_sys::Reflect::set(&o, &k.into(), &v.into());
    }
    let when = String::from(js_sys::Date::new(&(at as f64).into()).to_locale_string("en-GB", &o));
    format!("{} {when}", if at <= now_ms() { "Reset" } else { "Resets" })
}
fn tone(pct: f64) -> &'static str {
    if pct >= 90.0 { "var(--negative)" } else if pct >= 70.0 { "var(--warm)" } else { "var(--subdue)" }
}
/// The agent's logo (simple-icons, tinted by the theme).
pub fn agent_logo(a: AgentKind, size: u32) -> impl IntoView {
    let name = match a {
        AgentKind::ClaudeCode => "claude",
        AgentKind::Codex => "openai",
        AgentKind::Opencode => "opencode",
    };
    view! { <span class="pick-logo si" style=format!("width:{size}px;height:{size}px;-webkit-mask-image:url(/api/icons/si/{name});mask-image:url(/api/icons/si/{name})") aria-hidden="true"/> }
}

/// BB's usage card: one agent's limits, each window with its bar and when it resets.
#[component]
pub fn UsageCard(agent: AgentKind, #[prop(optional)] on_close: Option<Callback<()>>) -> impl IntoView {
    let ov = overview();
    let busy = RwSignal::new(false);
    let u = Memo::new(move |_| ov.with(|o| o.as_ref().and_then(|o| agents_use(o).into_iter().find(|x| x.agent == agent))));
    let refresh = move |_| {
        busy.set(true);
        spawn_local(async move {
            crate::code::store::refresh().await;
            busy.try_set(false);
        })
    };
    view! {
        <div class="ucard-card">
            <div class="ucard-head">
                {agent_logo(agent, 16)}
                <div class="grow"><b>{agent.label()}</b><div class="subdue">"Subscription usage"</div></div>
                <button class="icon-btn" title="Refresh" aria-label="Refresh" disabled=move || busy.get() on:click=refresh><Icon name="refresh" size=14 class=Signal::derive(move || if busy.get() { "spin".to_string() } else { String::new() })/></button>
                {on_close.map(|c| view! { <button class="icon-btn" title="Close" aria-label="Close" on:click=move |_| c.run(())><Icon name="x" size=14/></button> })}
            </div>
            {move || u.get().map(|u| view! {
                <p class="subdue ucard-msg">{format!("From a thread on {}, {}", u.machine, match ago_ms(u.at).as_str() { "now" => "just now".to_string(), a if a.ends_with(['m', 'h', 'd']) => format!("{a} ago"), a => a.to_string() })}</p>
                <div class="ucard-grid">
                    {u.windows.iter().map(|w| {
                        let pct = used(w).round();
                        view! {
                            <div class="ucard-win">
                                <div class="ucard-row"><span>{window_name(&w.window)}</span><b style={ if pct >= 70.0 { format!("color:{}", tone(pct)) } else { String::new() } }>{format!("{pct:.0}%")}</b></div>
                                <span class="ucard-bar"><i style=format!("width:{}%;background:{}", pct.min(100.0), tone(pct))/></span>
                                <span class="subdue">{resets(w.resets_at)}</span>
                            </div>
                        }
                    }).collect_view()}
                </div>
            })}
        </div>
    }
}

/// The Dashboard's "Agent usage" widget: every agent's card.
#[component]
pub fn UsageW() -> impl IntoView {
    let ov = overview();
    let agents = Memo::new(move |_| ov.with(|o| o.as_ref().map(|o| agents_use(o).into_iter().map(|u| u.agent).collect::<Vec<_>>())));
    move || match agents.get() {
        None => view! { <div class="dw-empty">"Loading…"</div> }.into_any(),
        Some(a) if a.is_empty() => view! { <div class="dw-empty">"No agent has reported its limits yet: they come with a thread's first turn."</div> }.into_any(),
        Some(a) => view! { <div class="dw-usage">{a.into_iter().map(|agent| view! { <UsageCard agent=agent/> }).collect_view()}</div> }.into_any(),
    }
}

/// Settings, the look, each agent's limits (a meter opens its card), sign out.
#[component]
pub fn SideFoot(collapsed: RwSignal<bool>, settings_on: Signal<bool>) -> impl IntoView {
    let ov = overview();
    // As BB's footer: the agent used last first, then the next one; each meter its fullest window.
    let meters = move || {
        ov.with(|o| {
            o.as_ref()
                .map(|o| {
                    agents_use(o)
                        .into_iter()
                        .take(2)
                        .filter_map(|u| u.windows.iter().map(|w| (window_name(&w.window), used(w))).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(w, p)| (u.agent, w, p)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    };
    // The card: which agent, and where (left, bottom) above its meter.
    let shown = RwSignal::new(None::<(AgentKind, f64, f64)>);
    let card = NodeRef::<leptos::html::Div>::new();
    on_window(leptos::ev::mousedown, move |e: web_sys::MouseEvent| {
        if shown.get_untracked().is_none() {
            return;
        }
        let inside = |el: Option<web_sys::Element>| el.is_some_and(|el| e.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()).is_some_and(|n| el.contains(Some(&n))));
        let on_meter = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|t| t.closest(".dock-meter").ok().flatten()).is_some();
        if !inside(card.get_untracked().map(Into::into)) && !on_meter {
            shown.set(None);
        }
    });
    on_window(leptos::ev::keydown, move |e: web_sys::KeyboardEvent| {
        if e.key() == "Escape" && shown.get_untracked().is_some() {
            shown.set(None);
        }
    });
    let toggle = move |agent: AgentKind, e: web_sys::MouseEvent| {
        if shown.get_untracked().is_some_and(|s| s.0 == agent) {
            shown.set(None);
            return;
        }
        let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
        let r = el.get_bounding_client_rect();
        let w = web_sys::window().unwrap();
        let (vw, vh) = (w.inner_width().ok().and_then(|x| x.as_f64()).unwrap_or(1200.0), w.inner_height().ok().and_then(|x| x.as_f64()).unwrap_or(800.0));
        shown.set(Some((agent, r.left().min(vw - 348.0).max(8.0), vh - r.top() + 6.0)));
    };
    view! {
        <div class=move || if collapsed.get() { "dock-foot collapsed" } else { "dock-foot" }>
            <button class=move || if settings_on.get() { "icon-btn on" } else { "icon-btn" } title="Settings" on:click=move |_| navigate("settings", Where::Replace, &[])><Icon name="settings" size=16/></button>
            <ModeSwitch compact=true/>
            <Show when=move || !collapsed.get()><span class="grow"/></Show>
            <Show when=move || !collapsed.get()>
                {move || meters().into_iter().map(|(agent, win, pct)| view! {
                    <button class=move || if shown.get().is_some_and(|s| s.0 == agent) { "dock-meter on" } else { "dock-meter" } title=format!("{}: {win} {pct:.0}% used", agent.label())
                        aria-expanded=move || shown.get().is_some_and(|s| s.0 == agent).to_string() on:click=move |e| toggle(agent, e)>
                        {agent_logo(agent, 12)}
                        <span class="dock-meter-bar"><i style=format!("width:{}%;background:{}", pct.min(100.0), tone(pct))/></span>
                        <span style={ if pct >= 70.0 { format!("color:{}", tone(pct)) } else { String::new() } }>{format!("{pct:.0}%")}</span>
                    </button>
                }).collect_view()}
            </Show>
            {move || shown.get().map(|(agent, left, bottom)| view! {
                <leptos::portal::Portal>
                    <div class="pick-pop ucard-pop" node_ref=card role="dialog" aria-label="Usage" style=format!("left:{left}px;bottom:{bottom}px;width:340px")>
                        <UsageCard agent=agent on_close=Callback::new(move |_| shown.set(None))/>
                    </div>
                </leptos::portal::Portal>
            })}
            <button class="icon-btn" title="Sign out" on:click=move |_| spawn_local(async {
                let _ = api_post::<Value>("/auth/logout", &json!({})).await;
                let _ = web_sys::window().unwrap().location().reload();
            })><Icon name="logout" size=15/></button>
        </div>
    }
}
