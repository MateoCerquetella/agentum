//! Mail › Inbox: the filter bar (the folders are in the app's sidebar, side.rs), the select bar and the
//! list grouped into Pinned / Important / Updates / Newsletters when nothing narrows it. Also the
//! row (one conversation) and the two-click confirm button the tabs share.
use super::lib::*;
use super::{use_mail, Filter, List, Reading};
use agentum_ui::prelude::*;
use std::collections::{BTreeSet, HashSet};

/// A button that asks once more ("Delete?") before it acts; it disarms after 4 s.
#[component]
pub fn Confirm(on_confirm: Callback<()>, #[prop(into)] ask: Signal<String>, #[prop(optional, into)] class: Option<String>, #[prop(optional, into)] title: Option<String>, #[prop(optional, into)] disabled: Signal<bool>, children: ChildrenFn) -> impl IntoView {
    let armed = RwSignal::new(false);
    let class = class.unwrap_or_else(|| "btn".into());
    let title2 = title.clone();
    view! {
        <button type="button" class=move || format!("{class}{}", if armed.get() { " armed" } else { "" }) title=title aria-label=title2 disabled=disabled
            on:click=move |e| {
                e.stop_propagation();
                if armed.get_untracked() {
                    armed.set(false);
                    on_confirm.run(());
                } else {
                    armed.set(true);
                    gloo_timers::callback::Timeout::new(4000, move || { let _ = armed.try_set(false); }).forget();
                }
            }>
            {move || if armed.get() { ask.get().into_any() } else { children().into_any() }}
        </button>
    }
}

/// The account picker (a select styled like the form's fields).
#[component]
pub fn AccountSelect(value: Signal<String>, on_change: Callback<String>, #[prop(optional, into)] all: Option<String>) -> impl IntoView {
    let c = use_mail();
    let all = all.unwrap_or_else(|| "All accounts".into());
    view! {
        <span class="account-select">
            <select class="pick-field" aria-label="Account" on:change=move |e| on_change.run(event_target_value(&e))>
                <option value="" selected=move || value.get().is_empty()>{all.clone()}</option>
                {move || c.accounts().into_iter().map(|a| {
                    let id = a.id.clone();
                    let n = c.counts.with(|x| x.unread.get(&a.id).copied().unwrap_or(0));
                    let sel = value.get() == id;
                    view! { <option value=id selected=sel>{format!("{} · {}{}", a.name, a.from, if n > 0 { format!(" ({n} unread)") } else { String::new() })}</option> }
                }).collect_view()}
            </select>
        </span>
    }
}

struct Group {
    k: &'static str,
    t: &'static str,
    lim: usize,
}
const GROUPS: [Group; 4] = [
    Group { k: "pinned", t: "Pinned", lim: 5 },
    Group { k: "important", t: "Important", lim: 8 },
    Group { k: "updates", t: "Updates", lim: 5 },
    Group { k: "news", t: "Newsletters & promotions", lim: 5 },
];
fn group_of(t: &Thread, reply: &HashSet<String>) -> usize {
    if t.m.starred {
        0
    } else if ["personal", "to_reply", "receipt", "otp", "calendar"].contains(&t.m.category.as_str()) || reply.contains(&t.m.msgid) {
        1
    } else if t.m.category == "notification" {
        2
    } else {
        3
    }
}
const BULK: [(&str, &str, &str); 5] = [("archive", "archive", "Archive"), ("mark_read", "check", "Read"), ("mark_unread", "circle", "Unread"), ("star", "star", "Star"), ("spam", "spam", "Spam")];

#[component]
pub fn Inbox(msgs: List, f: RwSignal<Filter>, set_f: Callback<Filter>, with_nav: Signal<bool>, loading: RwSignal<bool>, error: RwSignal<String>) -> impl IntoView {
    let c = use_mail();
    let q = RwSignal::new(f.get_untracked().q);
    let open_groups = RwSignal::new(BTreeSet::<&'static str>::new());
    let tq = StoredValue::new(None::<send_wrapper::SendWrapper<gloo_timers::callback::Timeout>>);
    let search = move |v: String| {
        q.set(v.clone());
        tq.set_value(Some(send_wrapper::SendWrapper::new(gloo_timers::callback::Timeout::new(350, move || {
            let mut x = f.get_untracked();
            x.q = v;
            set_f.run(x);
        }))));
    };
    let all = Memo::new(move |_| {
        let sent = f.with(|x| x.folder == "sent");
        msgs.with(|ms| {
            let ms = ms.clone().unwrap_or_default();
            let ms: Vec<Msg> = if sent { ms.into_iter().map(|mut m| { m.from_name = format!("→ {}", if m.to.is_empty() { &m.from_addr } else { &m.to }); m }).collect() } else { ms };
            threads(&ms)
        })
    });
    let grouped = move || f.with(|x| x.folder == "inbox" && x.cat.is_empty() && x.q.is_empty());
    let groups = Memo::new(move |_| {
        if !grouped() {
            return None;
        }
        let reply = c.reply_ids.get();
        let mut gs: Vec<Vec<Thread>> = vec![vec![]; 4];
        for t in all.get() {
            gs[group_of(&t, &reply)].push(t);
        }
        Some(gs)
    });
    let visible = move || -> Vec<Thread> {
        match groups.get() {
            Some(gs) => {
                let og = open_groups.get();
                gs.into_iter().enumerate().flat_map(|(i, g)| if og.contains(GROUPS[i].k) { g } else { g.into_iter().take(GROUPS[i].lim).collect() }).collect()
            }
            None => all.get(),
        }
    };
    c.use_rows(move || visible().into_iter().map(|t| (t, msgs)).collect());
    // Drop selected keys that are no longer in the list.
    Effect::new(move |_| {
        let keys: HashSet<String> = all.with(|a| a.iter().map(|t| t.key.clone()).collect());
        if c.sel.with_untracked(|s| s.iter().any(|k| !keys.contains(k))) {
            c.sel.update(|s| s.retain(|k| keys.contains(k)));
        }
    });
    let picked = move || all.with(|a| c.sel.with(|s| a.iter().filter(|t| s.contains(&t.key)).cloned().collect::<Vec<_>>()));
    let bulk = move |act: &'static str| {
        let p = picked();
        spawn_local(async move {
            c.run(p, act, msgs, true).await;
            c.sel.set(BTreeSet::new());
        });
    };
    let unread_n = move || {
        let a = f.with(|x| x.account.clone());
        c.counts.with(|x| x.of_account(&a))
    };
    let set = move |g: fn(&mut Filter)| {
        let mut x = f.get_untracked();
        g(&mut x);
        set_f.run(x);
    };
    view! {
        <div class=move || format!("ml-inbox{}{}", if loading.get() { " loading" } else { "" }, if c.sel.with(|s| !s.is_empty()) { " has-sel" } else { "" })>
            <div class="ml-bar">
                <Show when=move || !with_nav.get()>
                    <div class="ml-seg">
                        {FOLDERS.iter().map(|(k, l)| { let k = *k; view! {
                            <button type="button" aria-pressed=move || f.with(|x| x.folder == k).to_string() on:click=move |_| { let mut x = f.get_untracked(); x.folder = k.into(); set_f.run(x) }>{*l}</button>
                        }}).collect_view()}
                    </div>
                    <AccountSelect value=Signal::derive(move || f.with(|x| x.account.clone())) on_change=Callback::new(move |v| { let mut x = f.get_untracked(); x.account = v; set_f.run(x) })/>
                </Show>
                <input type="search" class="ml-search grow" placeholder="Search sender or subject (/)" prop:value=q on:input=move |e| search(event_target_value(&e))/>
            </div>
            <Show when=move || !with_nav.get()>
                <div class="ml-chips">
                    <button type="button" class="ml-chip" aria-pressed=move || f.with(|x| x.unread).to_string() on:click=move |_| set(|x| x.unread = !x.unread)>
                        <Icon name="circle" size=12 class="fill"/>"Unread "<b>{unread_n}</b>
                    </button>
                    <button type="button" class="ml-chip" aria-pressed=move || f.with(|x| x.cat.is_empty()).to_string() on:click=move |_| set(|x| x.cat.clear())>"All"</button>
                    {CAT.iter().map(|(k, l)| { let k = *k; view! {
                        <button type="button" class="ml-chip" aria-pressed=move || f.with(|x| x.cat == k).to_string() on:click=move |_| { let mut x = f.get_untracked(); x.cat = k.into(); set_f.run(x) }>{*l}</button>
                    }}).collect_view()}
                </div>
            </Show>
            <Show when=move || all.with(|a| !a.is_empty())>
                <div class="ml-selbar">
                    <label class="row">
                        <input type="checkbox" prop:checked=move || { let n = all.with(Vec::len); n > 0 && picked().len() == n }
                            on:change=move |e| c.sel.set(if event_target_checked(&e) { all.with(|a| a.iter().map(|t| t.key.clone()).collect()) } else { BTreeSet::new() })/>
                        " Select all"
                    </label>
                    <span class="subdue grow">{move || { let n = picked().len(); if n > 0 { format!("{n} selected") } else { String::new() } }}</span>
                    {BULK.iter().map(|(k, ic, l)| { let k = *k; view! {
                        <button type="button" class="btn ghost" disabled=move || picked().is_empty() on:click=move |_| bulk(k)>
                            <Icon name=*ic size=14 class=if *ic == "circle" { "fill" } else { "" }/>{*l}
                        </button>
                    }}).collect_view()}
                    <Confirm class="btn ghost" ask=Signal::derive(move || format!("Delete {}?", picked().len())) disabled=Signal::derive(move || picked().is_empty()) on_confirm=Callback::new(move |_| bulk("delete"))>
                        <Icon name="trash" size=14/>"Delete"
                    </Confirm>
                </div>
            </Show>
            {move || {
                if !error.get().is_empty() && msgs.with(Option::is_none) {
                    return view! { <div class="empty err">{error.get()}</div> }.into_any();
                }
                if msgs.with(Option::is_none) {
                    return view! { <div class="empty">"Loading…"</div> }.into_any();
                }
                if all.with(Vec::is_empty) {
                    return view! { <div class="empty">"Nothing here."</div> }.into_any();
                }
                match groups.get() {
                    Some(gs) => gs.into_iter().enumerate().filter(|(_, g)| !g.is_empty()).map(|(i, items)| {
                        let g = &GROUPS[i];
                        let (k, lim, n) = (g.k, g.lim, items.len());
                        let unread: usize = items.iter().map(|t| t.all.iter().filter(|m| m.unread).count()).sum();
                        let items2 = items.clone();
                        let shown = move || if open_groups.with(|o| o.contains(k)) { items.clone() } else { items.iter().take(lim).cloned().collect() };
                        view! {
                            <section class="ml-group">
                                <header class="ml-ghead">
                                    <h3>{g.t}<span class="subdue">{n}</span></h3>
                                    {(unread > 0).then(|| { let it = items2.clone(); view! {
                                        <button type="button" class="ml-link" on:click=move |_| { let it = it.clone(); spawn_local(async move { c.run(it, "mark_read", msgs, true).await; }) }>{format!("Mark {unread} read")}</button>
                                    }})}
                                    {(n > lim).then(|| view! {
                                        <button type="button" class="ml-link" on:click=move |_| open_groups.update(|o| { if !o.remove(k) { o.insert(k); } })>
                                            {move || if open_groups.with(|o| o.contains(k)) { "Show less".to_string() } else { format!("Show all {n}") }}
                                        </button>
                                    })}
                                </header>
                                <ul class="ml-rows">
                                    <For each=shown key=|t| t.rev() let:t><Row t=t set=msgs/></For>
                                </ul>
                            </section>
                        }
                    }).collect_view().into_any(),
                    None => view! { <ul class="ml-rows"><For each=move || all.get() key=|t| t.rev() let:t><Row t=t set=msgs/></For></ul> }.into_any(),
                }
            }}
        </div>
    }
}

fn avatar(t: &Thread, color: String) -> impl IntoView + use<> {
    let base = if t.m.from_name.is_empty() { &t.m.from_addr } else { &t.m.from_name };
    let n = base.trim_start_matches(|ch: char| !ch.is_alphanumeric());
    let h = avatar_hue(if t.m.from_addr.is_empty() { n } else { &t.m.from_addr });
    let letter = n.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    view! { <span class="mr-av" style=format!("--h:{h};--acct:{color}") title=t.m.account_name.clone()>{letter}</span> }
}

/// Unread dot / checkbox, avatar, sender (+ thread count), subject and preview, what the body says
/// (an amount, a code to copy, "Expects a reply"), a label and the time; pointing at it shows its
/// actions instead of the time.
#[component]
pub fn Row(t: Thread, set: List, #[prop(optional)] extra: Option<AnyView>, #[prop(default = true)] selectable: bool, #[prop(default = true)] actions: bool) -> impl IntoView {
    let c = use_mail();
    let el = NodeRef::<leptos::html::Li>::new();
    let key = t.key.clone();
    let x = t.m.extra.clone().unwrap_or_default();
    let snip = if !x.snippet.is_empty() && !bare_subject(&x.snippet).starts_with(&bare_subject(&t.m.subject).chars().take(24).collect::<String>()) { x.snippet.clone() } else { String::new() };
    let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
    let on = Signal::derive(move || c.sel.with(|s| s.contains(&k1)));
    let focus = Signal::derive(move || c.focus.with(|f| f.as_deref() == Some(k2.as_str())));
    Effect::new(move |_| {
        if focus.get() {
            if let Some(e) = el.get() {
                let o = web_sys::ScrollIntoViewOptions::new();
                o.set_block(web_sys::ScrollLogicalPosition::Nearest);
                e.scroll_into_view_with_scroll_into_view_options(&o);
            }
        }
    });
    let tt = t.clone();
    let open = move |_| {
        c.focus.set(Some(tt.key.clone()));
        c.open(Reading { item: tt.item(), thread: Some(tt.clone()), set, then: None });
    };
    let act = {
        let t = t.clone();
        move |a: &'static str| {
            let t = t.clone();
            move |e: leptos::ev::MouseEvent| {
                e.stop_propagation();
                c.run_now(vec![t.clone()], a, set);
            }
        }
    };
    let del = {
        let t = t.clone();
        Callback::new(move |_| c.run_now(vec![t.clone()], "delete", set))
    };
    let reply = c.reply_ids.with_untracked(|r| r.contains(&t.m.msgid));
    let lab = label(&t.m.category);
    let unread = t.m.unread;
    let starred = t.m.starred;
    let code = x.code.clone();
    view! {
        <li node_ref=el class=move || format!("mr{}{}{}", if unread { " unread" } else { "" }, if on.get() { " sel" } else { "" }, if focus.get() { " focus" } else { "" }) on:click=open>
            <span class="mr-pick">
                <span class="mr-dot"/>
                {selectable.then(|| view! {
                    <input type="checkbox" aria-label="Select" prop:checked=on on:click=|e| e.stop_propagation() on:change=move |e| c.toggle_sel(&k3, Some(event_target_checked(&e)))/>
                })}
            </span>
            {avatar(&t, c.color(&t.m.account))}
            <span class="mr-who">
                <span class="mr-from" title=t.m.from_addr.clone()>{t.m.who().to_string()}</span>
                {(t.all.len() > 1).then(|| view! { <span class="mr-n" title=format!("{} emails", t.all.len())>{t.all.len()}</span> })}
            </span>
            <span class="mr-line">
                <span class="mr-subj">{if t.m.subject.is_empty() { "(no subject)".to_string() } else { t.m.subject.clone() }}</span>
                {(!snip.is_empty()).then(|| view! { <span class="mr-snip">{snip.clone()}</span> })}
            </span>
            <span class="mr-tags">
                {(!x.amount.is_empty()).then(|| view! { <span class="mr-chip">{x.amount.clone()}</span> })}
                {(!code.is_empty()).then(|| { let cd = code.clone(); view! {
                    <button type="button" class="mr-chip mr-code" title="Copy code" on:click=move |e| { e.stop_propagation(); copy_text(&cd); toast(format!("Copied {cd}")) }>{code.clone()}<Icon name="copy" size=12/></button>
                }})}
                {reply.then(|| view! { <span class="mr-chip mr-reply">"Expects a reply"</span> })}
                {lab.map(|(l, k)| view! { <span class=format!("mr-lab l-{k}")>{l}</span> })}
                {extra}
            </span>
            <span class="mr-time">{when(t.m.ts)}</span>
            {actions.then(|| view! {
                <span class="mr-acts">
                    <button type="button" class=if starred { "icon-btn star" } else { "icon-btn" } title="Pin (s)" aria-label="Pin" on:click=act(if starred { "unstar" } else { "star" })><Icon name="star" size=14/></button>
                    <button type="button" class="icon-btn" title="Archive (e)" aria-label="Archive" on:click=act("archive")><Icon name="archive" size=14/></button>
                    <button type="button" class="icon-btn" title=if unread { "Mark read (u)" } else { "Mark unread (u)" } aria-label=if unread { "Mark read" } else { "Mark unread" } on:click=act(if unread { "mark_read" } else { "mark_unread" })>
                        <Icon name=if unread { "check" } else { "circle" } size=14 class=if unread { "" } else { "fill" }/>
                    </button>
                    <button type="button" class="icon-btn" title="Spam (!)" aria-label="Spam" on:click=act("spam")><Icon name="spam" size=14/></button>
                    <Confirm class="icon-btn" ask="Delete?" title="Delete (#)" on_confirm=del><Icon name="trash" size=14/></Confirm>
                </span>
            })}
        </li>
    }
}
