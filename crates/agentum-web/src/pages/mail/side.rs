//! Mail's folders in the app's sidebar, under Mail, while Mail is open: one sidebar, not two.
//! All accounts' mailboxes first, then each account's (folded until you open it), then labels.
use super::lib::*;
use super::{shared, Filter};
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use agentum_ui::store;
use std::collections::BTreeSet;

const OPEN_KEY: &str = "cerq.mail.side.open";

/// A mailbox: folder, Unread or not, its label and icon.
const BOXES: [(&str, bool, &str, &str); 5] = [("inbox", false, "Inbox", "inbox"), ("inbox", true, "Unread", "circle"), ("archive", false, "Archive", "archive"), ("sent", false, "Sent", "send"), ("junk", false, "Junk", "spam")];

#[component]
pub fn Folders() -> impl IntoView {
    let sh = shared();
    sh.in_side.set(true);
    on_cleanup(move || {
        let _ = sh.in_side.try_set(false);
    });
    // Folded sections (an account's id, or "labels"); accounts are folded until opened.
    let open = RwSignal::new(store::get(OPEN_KEY).map(|v| v.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect::<BTreeSet<_>>()).unwrap_or_default());
    let toggle = move |k: String| {
        open.update(|o| {
            if !o.remove(&k) {
                o.insert(k);
            }
        });
        store::set(OPEN_KEY, &open.with_untracked(|o| o.iter().cloned().collect::<Vec<_>>().join(",")));
    };
    // Keeps the search; Mail goes back to its Inbox tab and the drawer closes on phones.
    let pick = move |g: Box<dyn Fn(&mut Filter)>| {
        let mut f = sh.filter.get_untracked();
        g(&mut f);
        sh.filter.set(f);
        navigate("mail", Where::Replace, &[]);
    };
    let mailbox = move |acct: String, (folder, unread, label, icon): (&'static str, bool, &'static str, &'static str), count: Option<Signal<u64>>| {
        let a = acct.clone();
        let on = Memo::new(move |_| sh.filter.with(|f| f.account == a && f.folder == folder && f.unread == unread));
        let cls = if acct.is_empty() { "nav side-mbox" } else { "nav side-mbox sub" };
        let a = acct.clone();
        view! {
            <button type="button" class=move || format!("{cls}{}", if on.get() { " on" } else { "" }) aria-current=move || on.get().then_some("true")
                on:click=move |_| { let a = a.clone(); pick(Box::new(move |f| { f.account = a.clone(); f.folder = folder.into(); f.unread = unread; })) }>
                <Icon name=icon size=15 class=if icon == "circle" { "fill" } else { "" }/>
                <span class="truncate">{label}</span>
                {count.map(|n| view! { <Show when=move || { n.get() > 0 }><span class=if folder == "junk" { "count quiet" } else { "count" }>{move || n.get()}</span></Show> })}
            </button>
        }
    };
    let total = Signal::derive(move || sh.counts.with(|c| c.total));
    let junk_total = Signal::derive(move || sh.counts.with(|c| c.junk_total));
    view! {
        <div class="side-mail" role="group" aria-label="Mail folders">
            {BOXES.iter().map(|b| mailbox(String::new(), *b, match b.0 { "inbox" => Some(total), "junk" => Some(junk_total), _ => None })).collect_view()}
            {move || {
                let accounts = sh.meta.with(|m| m.as_ref().map(|m| m.accounts.clone()).unwrap_or_default());
                // One account: the mailboxes above are already its own.
                (accounts.len() > 1).then(|| accounts.into_iter().map(|a| {
                    let (id2, id3, id4, id5) = (a.id.clone(), a.id.clone(), a.id.clone(), a.id.clone());
                    let is_open = {
                        let id = a.id.clone();
                        move || open.with(|o| o.contains(&id))
                    };
                    let (is_open2, is_open3) = (is_open.clone(), is_open.clone());
                    let n = Signal::derive(move || sh.counts.with(|c| c.unread.get(&id2).copied().unwrap_or(0)));
                    let here = move || sh.filter.with(|f| f.account == id3);
                    view! {
                        <button type="button" class=move || format!("side-mail-acct{}", if here() { " here" } else { "" }) style=format!("--acct:{}", color_of_name(&a.name))
                            title=a.from.clone() aria-expanded=move || is_open3().to_string() on:click=move |_| toggle(id4.clone())>
                            <Icon name="chevron" size=12 class=Signal::derive(move || if is_open2() { "open".to_string() } else { String::new() })/>
                            <i class="ml-dot"/>
                            <span class="truncate">{a.name.clone()}</span>
                            <Show when=move || { n.get() > 0 }><span class="count">{move || n.get()}</span></Show>
                        </button>
                        <Show when=is_open>
                            {let id = id5.clone(); BOXES.iter().map(move |b| {
                                let j = { let id = id.clone(); Signal::derive(move || sh.counts.with(|c| c.junk.get(&id).copied().unwrap_or(0))) };
                                mailbox(id.clone(), *b, match b.0 { "inbox" => Some(n), "junk" => Some(j), _ => None })
                            }).collect_view()}
                        </Show>
                    }
                }).collect_view())
            }}
            <button type="button" class="side-mail-acct" aria-expanded=move || open.with(|o| o.contains("labels")).to_string() on:click=move |_| toggle("labels".into())>
                <Icon name="chevron" size=12 class=Signal::derive(move || if open.with(|o| o.contains("labels")) { "open".to_string() } else { String::new() })/>
                <span class="truncate">"Labels"</span>
                <Show when=move || sh.filter.with(|f| !f.cat.is_empty())><span class="count">{move || sh.filter.with(|f| cat(&f.cat))}</span></Show>
            </button>
            <Show when=move || open.with(|o| o.contains("labels"))>
                {CAT.iter().map(|(k, l)| {
                    let k = *k;
                    let on = move || sh.filter.with(|f| f.cat == k);
                    view! {
                        <button type="button" class=move || if on() { "nav side-mbox sub on" } else { "nav side-mbox sub" } title="Show only this kind of email; click again for all"
                            on:click=move |_| pick(Box::new(move |f| { f.cat = if f.cat == k { String::new() } else { k.into() } }))>
                            <i class="ml-dot" style=format!("--acct:{}", color_of_name(k))/>
                            <span class="truncate">{*l}</span>
                        </button>
                    }
                }).collect_view()}
            </Show>
        </div>
    }
}
