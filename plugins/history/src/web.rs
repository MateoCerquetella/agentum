//! History's web part: the branch's commits (the unpushed ones marked) and, for the one picked,
//! its message and patch in the diff view.
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::machine::{GitReq, Req};
use agentum_ui::diff::{parse, totals, DiffView, FileDiff};
use agentum_ui::prelude::*;
use std::collections::HashMap;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

static TABS: [RailTab; 1] = [RailTab { id: "history", title: "History", icon: "clock", view: tab }];

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

thread_local! {
    /// The commit open in each thread's History tab.
    static PICKED: RwSignal<HashMap<String, String>> = global(HashMap::new());
}
fn picked() -> RwSignal<HashMap<String, String>> {
    PICKED.with(|p| *p)
}

async fn git(machine: &str, req: GitReq) -> Result<Value, ApiError> {
    api_post::<Value>("/code/call", &json!({"machine": machine, "req": Req::Git(req)})).await
}

fn tab(cx: ThreadCx) -> AnyView {
    view! { <HistoryTab cx=cx/> }.into_any()
}

#[derive(Clone)]
struct Commit {
    sha: String,
    subject: String,
    author: String,
    ts: i64,
}

#[component]
fn HistoryTab(cx: ThreadCx) -> impl IntoView {
    let summary = cx.summary;
    let place = Memo::new(move |_| summary.with(|s| s.as_ref().map(|s| (s.machine.clone(), s.cwd.clone()))));
    let commits = RwSignal::new(Vec::<Commit>::new());
    let ahead = RwSignal::new(0usize);
    let err = RwSignal::new(None::<String>);
    let loaded = RwSignal::new(false);
    let load = Callback::new(move |_: ()| {
        let Some((m, cwd)) = place.get_untracked() else { return };
        spawn_local(async move {
            let log = git(&m, GitReq::Log { cwd: cwd.clone(), limit: Some(150), path: None }).await;
            let st = git(&m, GitReq::Status { cwd }).await;
            if loaded.is_disposed() {
                return;
            }
            loaded.set(true);
            ahead.set(st.ok().and_then(|s| s["ahead"].as_u64()).unwrap_or(0) as usize);
            match log {
                Ok(v) => {
                    err.set(None);
                    commits.set(
                        v.as_array()
                            .into_iter()
                            .flatten()
                            .map(|c| Commit {
                                sha: c["sha"].as_str().unwrap_or("").to_string(),
                                subject: c["subject"].as_str().unwrap_or("").to_string(),
                                author: c["author"].as_str().unwrap_or("").to_string(),
                                ts: c["ts"].as_i64().unwrap_or(0),
                            })
                            .collect(),
                    )
                }
                Err(e) => err.set(Some(e.message)),
            }
        });
    });
    Effect::new(move |_| {
        place.track();
        load.run(());
    });
    let id = cx.id.clone();
    let current = {
        let id = id.clone();
        Memo::new(move |_| picked().with(|p| p.get(&id).cloned()))
    };
    let pick = {
        let id = id.clone();
        Callback::new(move |sha: Option<String>| {
            picked().update(|p| match sha {
                Some(s) => {
                    p.insert(id.clone(), s);
                }
                None => {
                    p.remove(&id);
                }
            })
        })
    };
    view! {
        <div class="hx">
            {move || match current.get() {
                Some(sha) => view! { <CommitView cx=cx.clone() sha=sha place=place back=pick/> }.into_any(),
                None => view! {
                    <div class="hx-bar">
                        <b>"Commits"</b>
                        <span class="small subdue">{move || if ahead.get() > 0 { format!("{} not pushed", ahead.get()) } else { String::new() }}</span>
                        <span class="grow"></span>
                        <button class="icon-btn" title="Refresh" on:click=move |_| load.run(())><Icon name="refresh" size=14/></button>
                    </div>
                    <div class="hx-list">
                        {move || err.get().map(|e| view! { <div class="err small" style="padding:10px">{e}</div> })}
                        {move || (loaded.get() && commits.with(|c| c.is_empty()) && err.with(|e| e.is_none())).then(|| view! { <div class="empty small">"No commits yet."</div> })}
                        {move || {
                            let n = ahead.get();
                            commits.get().into_iter().enumerate().map(|(i, c)| {
                                let sha = c.sha.clone();
                                view! {
                                    <div class="hx-row" on:click=move |_| pick.run(Some(sha.clone()))>
                                        <span class="hx-dot" class:unpushed=i < n title=if i < n { "Not pushed yet" } else { "" }></span>
                                        <div class="col grow" style="gap:0">
                                            <span class="truncate hx-subject">{c.subject.clone()}</span>
                                            <span class="small subdue truncate">
                                                <span class="hx-sha">{c.sha.chars().take(7).collect::<String>()}</span>
                                                {format!(" · {} · {}", c.author, ago_ms(c.ts))}
                                            </span>
                                        </div>
                                    </div>
                                }
                            }).collect_view()
                        }}
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn CommitView(cx: ThreadCx, sha: String, place: Memo<Option<(String, String)>>, back: Callback<Option<String>>) -> impl IntoView {
    let info = RwSignal::new(None::<Value>);
    let files = RwSignal::new(Vec::<FileDiff>::new());
    let err = RwSignal::new(None::<String>);
    let split = RwSignal::new(agentum_ui::store::flag("agentum.git.split"));
    {
        let sha = sha.clone();
        Effect::new(move |_| {
            let Some((m, cwd)) = place.get() else { return };
            let sha = sha.clone();
            spawn_local(async move {
                let r = git(&m, GitReq::Show { cwd, sha }).await;
                if info.is_disposed() {
                    return;
                }
                match r {
                    Ok(v) => {
                        files.set(parse(v["diff"].as_str().unwrap_or("")));
                        info.set(Some(v));
                    }
                    Err(e) => err.set(Some(e.message)),
                }
            });
        });
    }
    let open_file = Callback::new(move |path: String| {
        let Some((_, cwd)) = place.get_untracked() else { return };
        agentum_plugin_editor::open(&cx, &format!("{}/{}", cwd.trim_end_matches('/'), path));
    });
    let field = move |k: &'static str| info.with(|i| i.as_ref().and_then(|i| i[k].as_str().map(str::to_string)).unwrap_or_default());
    let short: String = sha.chars().take(7).collect();
    view! {
        <div class="hx-bar">
            <button class="icon-btn" title="Back to the commits" on:click=move |_| back.run(None)><Icon name="chevron-left" size=15/></button>
            <span class="hx-sha" title=sha.clone()>{short}</span>
            <span class="small subdue truncate">{move || { let ts = info.with(|i| i.as_ref().and_then(|i| i["ts"].as_i64()).unwrap_or(0)); format!("{} · {}", field("author"), ago_ms(ts)) }}</span>
            <span class="grow"></span>
            <span class="small subdue">{move || { let f = files.get(); let (a, d) = totals(&f); if f.is_empty() { String::new() } else { format!("{} files · +{a} −{d}", f.len()) } }}</span>
            <button class="icon-btn" class:on=move || split.get() title="Side by side" on:click=move |_| { let s = !split.get_untracked(); agentum_ui::store::set("agentum.git.split", if s { "1" } else { "" }); split.set(s) }><Icon name="sidebar" size=14/></button>
        </div>
        <div class="hx-body">
            {move || err.get().map(|e| view! { <div class="err small">{e}</div> })}
            {move || info.get().is_none().then(|| err.with(|e| e.is_none()).then(|| view! { <div class="empty small">"Loading…"</div> }))}
            {move || info.get().map(|_| view! {
                <div class="hx-msg">
                    <b>{field("subject")}</b>
                    {(!field("body").is_empty()).then(|| view! { <pre class="hx-body-text">{field("body")}</pre> })}
                </div>
            })}
            <DiffView files=files split=split on_open=open_file/>
        </div>
    }
}
