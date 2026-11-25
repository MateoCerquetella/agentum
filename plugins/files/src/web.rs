//! Files' web part: the thread's directory as a tree (folders listed when opened) and quick
//! open (fuzzy, by agentd). A click opens the file in the Editor tab.
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::machine::{FsReq, Req};
use agentum_ui::prelude::*;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

static TABS: [RailTab; 1] = [RailTab { id: "files", title: "Files", icon: "folder", view: tab }];

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        &TABS
    }
    fn css(&self) -> Option<&'static str> {
        Some(include_str!("../style.css"))
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Entry {
    name: String,
    dir: bool,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    hidden: bool,
}

type Listing = Result<Vec<Entry>, String>;

thread_local! {
    /// Open folders and the listings seen, by "machine\0path" (kept across tab switches).
    static EXPANDED: RwSignal<HashSet<String>> = global(HashSet::new());
    static LISTS: RwSignal<HashMap<String, Listing>> = global(HashMap::new());
}
fn expanded() -> RwSignal<HashSet<String>> {
    EXPANDED.with(|s| *s)
}
fn lists() -> RwSignal<HashMap<String, Listing>> {
    LISTS.with(|s| *s)
}
fn key(machine: &str, path: &str) -> String {
    format!("{machine}\u{0}{path}")
}
fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// Never shown in the tree.
const SKIP: [&str; 2] = [".git", ".DS_Store"];

async fn fs(machine: &str, req: FsReq) -> Result<Value, ApiError> {
    api_post::<Value>("/code/call", &json!({"machine": machine, "req": Req::Fs(req)})).await
}

fn list(machine: String, path: String) {
    spawn_local(async move {
        let r = fs(&machine, FsReq::List { path: path.clone() }).await;
        let l: Listing = match r {
            Ok(v) => serde_json::from_value::<Vec<Entry>>(v["entries"].clone()).map_err(|e| e.to_string()),
            Err(e) => Err(e.message),
        };
        lists().update(|m| {
            m.insert(key(&machine, &path), l);
        });
    });
}

fn tab(cx: ThreadCx) -> AnyView {
    view! { <FilesTab cx=cx/> }.into_any()
}

#[component]
fn FilesTab(cx: ThreadCx) -> impl IntoView {
    let summary = cx.summary;
    let place = Memo::new(move |_| summary.with(|s| s.as_ref().map(|s| (s.machine.clone(), s.cwd.clone()))));
    let query = RwSignal::new(String::new());
    let hits = RwSignal::new(Vec::<String>::new());
    let sel = RwSignal::new(0usize);
    let finding = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some((m, c)) = place.get() {
            if lists().with_untracked(|l| !l.contains_key(&key(&m, &c))) {
                list(m, c);
            }
        }
    });
    // Quick open: ask agentd a moment after typing stops.
    let timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    let generation = StoredValue::new(0u32);
    Effect::new(move |_| {
        let q = query.get();
        generation.update_value(|g| *g += 1);
        let g = generation.get_value();
        if q.trim().is_empty() {
            timer.set_value(None);
            hits.set(vec![]);
            finding.set(false);
            return;
        }
        let Some((m, c)) = place.get_untracked() else { return };
        finding.set(true);
        timer.set_value(Some(gloo_timers::callback::Timeout::new(120, move || {
            spawn_local(async move {
                let r = fs(&m, FsReq::Find { root: c, query: q, limit: Some(50) }).await;
                if generation.get_value() != g {
                    return;
                }
                finding.set(false);
                sel.set(0);
                match r {
                    Ok(v) => hits.set(v["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect()),
                    Err(e) => {
                        hits.set(vec![]);
                        toast_bad(e.message);
                    }
                }
            });
        })));
    });
    let open_rel = {
        let cx = cx.clone();
        Callback::new(move |rel: String| {
            let Some((_, c)) = place.get_untracked() else { return };
            agentum_plugin_editor::open(&cx, &join(&c, &rel));
        })
    };
    let refresh = move |_| {
        let Some((m, _)) = place.get_untracked() else { return };
        let open: Vec<String> = lists().with_untracked(|l| l.keys().filter(|k| k.starts_with(&format!("{m}\u{0}"))).cloned().collect());
        for k in open {
            if let Some((_, p)) = k.split_once('\u{0}') {
                list(m.clone(), p.to_string());
            }
        }
    };
    let on_key = move |e: web_sys::KeyboardEvent| match e.key().as_str() {
        "ArrowDown" => {
            e.prevent_default();
            sel.update(|s| *s = (*s + 1).min(hits.with_untracked(|h| h.len().saturating_sub(1))));
        }
        "ArrowUp" => {
            e.prevent_default();
            sel.update(|s| *s = s.saturating_sub(1));
        }
        "Enter" => {
            if let Some(p) = hits.with_untracked(|h| h.get(sel.get_untracked()).cloned()) {
                open_rel.run(p);
            }
        }
        "Escape" => query.set(String::new()),
        _ => {}
    };
    let thread = cx.id.clone();
    view! {
        <div class="fx">
            <div class="fx-top">
                <Icon name="search" size=13 class="subdue"/>
                <input class="fx-find" placeholder="Go to file…" prop:value=query on:input=move |e| query.set(event_target_value(&e)) on:keydown=on_key/>
                <button class="icon-btn" title="Refresh" on:click=refresh><Icon name="refresh" size=14/></button>
            </div>
            <div class="fx-body">
                {move || if !query.with(|q| q.trim().is_empty()) {
                    let list = hits.get();
                    if list.is_empty() {
                        return view! { <div class="empty small">{move || if finding.get() { "Looking…" } else { "No file matches." }}</div> }.into_any();
                    }
                    list.into_iter().enumerate().map(|(i, p)| {
                        let (dir, name) = match p.rfind('/') { Some(i) => (p[..=i].to_string(), p[i + 1..].to_string()), None => (String::new(), p.clone()) };
                        let p2 = p.clone();
                        view! {
                            <div class="fx-row fx-hit" class:on=move || sel.get() == i on:mouseenter=move |_| sel.set(i) on:click=move |_| open_rel.run(p2.clone())>
                                <Icon name="file" size=13 class="subdue"/>
                                <span class="fx-name">{name}</span>
                                <span class="fx-dir truncate">{dir}</span>
                            </div>
                        }
                    }).collect_view().into_any()
                } else {
                    match place.get() {
                        Some((m, c)) => dir_view(cx.clone(), thread.clone(), m, c, 0),
                        None => view! { <div class="empty small">"Loading…"</div> }.into_any(),
                    }
                }}
            </div>
        </div>
    }
}

fn indent(depth: u32) -> String {
    format!("padding-left:{}px", 8 + depth * 14)
}

/// A folder's entries (and, inside, the open folders'), listed when first shown.
fn dir_view(cx: ThreadCx, thread: String, machine: String, path: String, depth: u32) -> AnyView {
    let k = key(&machine, &path);
    view! {
        {move || match lists().with(|l| l.get(&k).cloned()) {
            None => view! { <div class="fx-row subdue small" style=indent(depth + 1)>"Loading…"</div> }.into_any(),
            Some(Err(e)) => view! { <div class="fx-row err small" style=indent(depth + 1)>{e}</div> }.into_any(),
            Some(Ok(entries)) if entries.is_empty() && depth > 0 => view! { <div class="fx-row subdue small" style=indent(depth + 1)>"Empty"</div> }.into_any(),
            Some(Ok(entries)) => entries
                .into_iter()
                .filter(|e| !SKIP.contains(&e.name.as_str()))
                .map(|e| entry_view(cx.clone(), thread.clone(), machine.clone(), join(&path, &e.name), e, depth))
                .collect_view()
                .into_any(),
        }}
    }
    .into_any()
}

fn entry_view(cx: ThreadCx, thread: String, machine: String, abs: String, e: Entry, depth: u32) -> AnyView {
    if e.dir {
        let k = key(&machine, &abs);
        let k2 = k.clone();
        let is_open = Memo::new(move |_| expanded().with(|x| x.contains(&k2)));
        let (m, a) = (machine.clone(), abs.clone());
        let toggle = move |_| {
            let now_open = expanded().with_untracked(|x| x.contains(&k));
            expanded().update(|x| {
                if now_open {
                    x.remove(&k);
                } else {
                    x.insert(k.clone());
                }
            });
            if !now_open && lists().with_untracked(|l| !l.contains_key(&k)) {
                list(m.clone(), a.clone());
            }
        };
        view! {
            <div class="fx-row" class:fx-hidden=e.hidden style=indent(depth) title=abs.clone() on:click=toggle>
                <span class="fx-chev" class:open=move || is_open.get()><Icon name="chevron-right" size=12/></span>
                <Icon name="folder" size=13 class="fx-folder"/>
                <span class="fx-name truncate">{e.name.clone()}</span>
            </div>
            {move || is_open.get().then(|| dir_view(cx.clone(), thread.clone(), machine.clone(), abs.clone(), depth + 1))}
        }
        .into_any()
    } else {
        let a = abs.clone();
        let t = thread.clone();
        let on = Memo::new(move |_| agentum_plugin_editor::active(&t).as_deref() == Some(a.as_str()));
        view! {
            <div class="fx-row" class:on=move || on.get() class:fx-hidden=e.hidden style=indent(depth) title=abs.clone() on:click=move |_| agentum_plugin_editor::open(&cx, &abs)>
                <span class="fx-chev"></span>
                <Icon name="file" size=13 class="subdue"/>
                <span class="fx-name truncate">{e.name.clone()}</span>
                <span class="fx-size">{agentum_ui::fmt::bytes(e.size)}</span>
            </div>
        }
        .into_any()
    }
}
