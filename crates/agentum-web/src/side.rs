//! The sidebar: the Personal / Coding switch, each side's list, and the footer.
//!
//! Coding is Dockside (dockside.rs).
use crate::code::store::overview;
use crate::pages::{self, theme::ModeSwitch, Side};
use agentum_proto::Attention;
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

/// Settings, the look, each agent's limits, sign out.
#[component]
pub fn SideFoot(collapsed: RwSignal<bool>, settings_on: Signal<bool>) -> impl IntoView {
    let ov = overview();
    let meters = move || {
        ov.with(|o| {
            o.as_ref()
                .map(|o| {
                    // One meter per agent: its fullest window on any machine.
                    let mut by: Vec<(&'static str, String, f64)> = vec![];
                    for l in &o.limits {
                        for w in &l.limits {
                            match by.iter_mut().find(|x| x.0 == l.agent.label()) {
                                Some(x) if w.used_percent > x.2 => *x = (l.agent.label(), w.window.clone(), w.used_percent),
                                Some(_) => {}
                                None => by.push((l.agent.label(), w.window.clone(), w.used_percent)),
                            }
                        }
                    }
                    by.truncate(2);
                    by
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
