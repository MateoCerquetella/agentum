//! Git's web part: the Git tab (branch, the diff by target, commit, push) and the header badge.
use super::Stat;
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_proto::machine::{GitReq, Req};
use agentum_ui::diff::{parse, totals, DiffView, FileDiff};
use agentum_ui::prelude::*;
use std::collections::HashMap;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

static TABS: [RailTab; 1] = [RailTab { id: "git", title: "Git", icon: "branch", view: tab }];

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        &TABS
    }
    fn thread_header(&self, cx: ThreadCx) -> Option<AnyView> {
        Some(view! { <Badge cx=cx/> }.into_any())
    }
    fn css(&self) -> Option<&'static str> {
        Some(include_str!("../style.css"))
    }
}

thread_local! {
    /// Each thread's stat, for its header badge (from `stat`, the tab, and `plugin:git`).
    static STATS: RwSignal<HashMap<String, Stat>> = agentum_ui::global(HashMap::new());
    /// Bumps for a thread when the hub says it changed (after a turn).
    static CHANGED: RwSignal<(String, u32)> = agentum_ui::global((String::new(), 0));
    static LISTENING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn stats() -> RwSignal<HashMap<String, Stat>> {
    STATS.with(|s| *s)
}
fn changed() -> RwSignal<(String, u32)> {
    CHANGED.with(|s| *s)
}

/// One listener for `plugin:git`, for the page's whole life.
fn listen() {
    if LISTENING.with(|l| l.replace(true)) {
        return;
    }
    let owner = Owner::new_root(None);
    owner.with(|| {
        subscribe(&["plugin:git"]);
        on_ws(|m| {
            let WsOut::Plugin { plugin, payload } = m else { return };
            if plugin != "git" {
                return;
            }
            let Some(t) = payload["thread"].as_str() else { return };
            if let Ok(s) = serde_json::from_value::<Stat>(payload["stat"].clone()) {
                stats().update(|m| {
                    m.insert(t.to_string(), s);
                });
            }
            changed().update(|c| *c = (t.to_string(), c.1 + 1));
        });
    });
    std::mem::forget(owner);
}

/// A git request on a machine.
pub(crate) async fn git(machine: &str, req: GitReq) -> Result<Value, ApiError> {
    api_post::<Value>("/code/call", &json!({"machine": machine, "req": Req::Git(req)})).await
}

/// Show another tab of the thread's side panel.
fn open_tab(rail: RwSignal<Option<String>>, id: &str) {
    agentum_ui::store::set("agentum.rail", id);
    rail.set(Some(id.to_string()));
}

fn plural(n: u32, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

#[component]
fn Badge(cx: ThreadCx) -> impl IntoView {
    listen();
    let id = cx.id.clone();
    {
        let id = id.clone();
        spawn_local(async move {
            if let Ok(s) = rpc::<Stat>("git", "stat", &json!({"thread": id})).await {
                stats().update(|m| {
                    m.insert(id, s);
                });
            }
        });
    }
    let rail = cx.rail;
    view! {
        {move || stats().with(|m| m.get(&id).cloned()).filter(|s| s.files > 0 || s.ahead > 0).map(|s| view! {
            <div class="git-strip">
                <button class="git-badge" title="Open the Git tab" on:click=move |_| open_tab(rail, "git")>
                    <Icon name="branch" size=12/>
                    {(s.files > 0).then(|| view! {
                        <span class="git-plus">{format!("+{}", s.add)}</span>
                        <span class="git-minus">{format!("−{}", s.del)}</span>
                        <span class="subdue">{format!("· {}", plural(s.files, "file"))}</span>
                    })}
                    {(s.ahead > 0).then(|| view! { <span class="subdue">{format!("· ↑{} to push", s.ahead)}</span> })}
                </button>
            </div>
        })}
    }
}

fn tab(cx: ThreadCx) -> AnyView {
    view! { <GitTab cx=cx/> }.into_any()
}

const TARGETS: [(&str, &str, &str); 3] = [
    ("uncommitted", "Uncommitted", "Everything not committed yet, new files included"),
    ("staged", "Staged", "What the next commit takes (git add)"),
    ("branch", "Branch", "Everything on this branch since it left the base branch"),
];

#[component]
fn GitTab(cx: ThreadCx) -> impl IntoView {
    listen();
    let summary = cx.summary;
    let place = Memo::new(move |_| summary.with(|s| s.as_ref().map(|s| (s.machine.clone(), s.cwd.clone()))));
    let target = RwSignal::new(agentum_ui::store::get("agentum.git.target").filter(|t| TARGETS.iter().any(|x| x.0 == t)).unwrap_or_else(|| "uncommitted".into()));
    let split = RwSignal::new(agentum_ui::store::flag("agentum.git.split"));
    let status = RwSignal::new(None::<Value>);
    let files = RwSignal::new(Vec::<FileDiff>::new());
    let err = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let busy = RwSignal::new(None::<&'static str>);
    let generation = StoredValue::new(0u32);
    let msg_key = format!("agentum.git.msg.{}", cx.id);
    let msg = RwSignal::new(agentum_ui::store::get(&msg_key).unwrap_or_default());
    let staged_only = RwSignal::new(false);
    let id = cx.id.clone();

    let load = {
        let id = id.clone();
        Callback::new(move |_: ()| {
            let Some((m, cwd)) = place.get_untracked() else { return };
            let t = target.get_untracked();
            generation.update_value(|g| *g += 1);
            let g = generation.get_value();
            loading.set(true);
            let id = id.clone();
            spawn_local(async move {
                let st = git(&m, GitReq::Status { cwd: cwd.clone() }).await;
                let d = git(&m, GitReq::Diff { cwd, target: Some(t.clone()), path: None }).await;
                if generation.get_value() != g {
                    return;
                }
                loading.set(false);
                loaded.set(true);
                let mut problem = None;
                match st {
                    Ok(v) => status.set(Some(v)),
                    Err(e) => problem = Some(e.message),
                }
                match d {
                    Ok(v) => {
                        let f = parse(v["diff"].as_str().unwrap_or(""));
                        if t == "uncommitted" {
                            let (add, del) = totals(&f);
                            let n = f.len() as u32;
                            stats().update(|m| {
                                let s = m.entry(id).or_default();
                                (s.add, s.del, s.files) = (add, del, n);
                            });
                        }
                        files.set(f);
                    }
                    Err(e) => {
                        files.set(vec![]);
                        problem.get_or_insert(e.message);
                    }
                }
                err.set(problem);
            });
        })
    };
    // Load when the thread's place or the target changes, after a turn, and after a reconnect.
    Effect::new(move |_| {
        place.track();
        target.track();
        load.run(());
    });
    {
        let id = id.clone();
        Effect::new(move |prev: Option<u32>| {
            let (t, n) = changed().get();
            if prev.is_some() && t == id {
                load.run(());
            }
            n
        });
    }
    let conn = agentum_ui::ws::connected();
    Effect::new(move |prev: Option<u32>| {
        let n = conn.get();
        if prev.is_some() {
            load.run(());
        }
        n
    });
    // A branch's status: ahead/behind, staged and untracked counts.
    let st = move |k: &str| status.with(|s| s.as_ref().map(|s| s[k].clone()).unwrap_or(Value::Null));
    let count = move |f: fn(&str) -> bool| status.with(|s| s.as_ref().and_then(|s| s["files"].as_array().map(|a| a.iter().filter(|x| f(x["status"].as_str().unwrap_or(""))).count())).unwrap_or(0));
    let staged = move || count(|s| !s.is_empty() && !matches!(s.as_bytes()[0], b'.' | b'?' | b'!'));
    let untracked = move || count(|s| s == "??");
    let tracked_changes = move || count(|s| s.len() == 2 && s != "??" && s != "!!");
    let ahead = move || st("ahead").as_u64().unwrap_or(0);
    let behind = move || st("behind").as_u64().unwrap_or(0);

    let commit = {
        let msg_key = msg_key.clone();
        move || {
            let text = msg.get_untracked();
            if text.trim().is_empty() {
                toast_bad("Write a commit message first.");
                return;
            }
            let Some((m, cwd)) = place.get_untracked() else { return };
            let all = !staged_only.get_untracked() || staged() == 0;
            busy.set(Some("commit"));
            let msg_key = msg_key.clone();
            spawn_local(async move {
                match git(&m, GitReq::Commit { cwd, message: text, all }).await {
                    Ok(v) => {
                        toast(format!("Committed {}.", v["sha"].as_str().map(|s| &s[..s.len().min(7)]).unwrap_or("")));
                        msg.set(String::new());
                        agentum_ui::store::remove(&msg_key);
                    }
                    Err(e) => toast_bad(e.message),
                }
                busy.set(None);
                load.run(());
            });
        }
    };
    let commit2 = commit.clone();
    let push = move |_| {
        let Some((m, cwd)) = place.get_untracked() else { return };
        busy.set(Some("push"));
        spawn_local(async move {
            match git(&m, GitReq::Push { cwd }).await {
                Ok(_) => toast("Pushed."),
                Err(e) => toast_bad(e.message),
            }
            busy.set(None);
            load.run(());
        });
    };
    let open_file = Callback::new(move |path: String| {
        let Some((_, cwd)) = place.get_untracked() else { return };
        agentum_plugin_editor::open(&cx, &format!("{}/{}", cwd.trim_end_matches('/'), path));
    });
    let summary_line = move || {
        let f = files.get();
        if f.is_empty() {
            return String::new();
        }
        let (a, d) = totals(&f);
        format!("{} · +{a} −{d}", plural(f.len() as u32, "file"))
    };
    let show_commit = move || target.get() != "branch" && (tracked_changes() > 0 || staged() > 0);
    let empty_text = move || match target.get().as_str() {
        "staged" => "Nothing staged.",
        "branch" => "No commits on this branch beyond its base.",
        _ => "No uncommitted changes.",
    };

    view! {
        <div class="git-tab">
            <div class="git-bar">
                <Icon name="branch" size=14/>
                <b class="truncate">{move || st("branch").as_str().map(str::to_string).unwrap_or_else(|| if loaded.get() { "(detached)".into() } else { String::new() })}</b>
                {move || st("upstream").as_str().map(|u| view! { <span class="small subdue truncate" title="Upstream">{format!("→ {u}")}</span> })}
                {move || (ahead() > 0).then(|| view! { <span class="git-ab" title="Commits to push">{format!("↑{}", ahead())}</span> })}
                {move || (behind() > 0).then(|| view! { <span class="git-ab" title="Commits to pull">{format!("↓{}", behind())}</span> })}
                <span class="grow"></span>
                <button class="icon-btn" title="Refresh" on:click=move |_| load.run(())>
                    <Icon name="refresh" size=14 class=Signal::derive(move || if loading.get() { "spin".to_string() } else { String::new() })/>
                </button>
                <button class="btn git-sm" disabled=move || busy.get().is_some() title=move || if st("upstream").is_null() { "Push and set the upstream (origin)" } else { "git push" } on:click=push>
                    {move || if busy.get() == Some("push") { "Pushing…".to_string() } else if ahead() > 0 { format!("Push ↑{}", ahead()) } else { "Push".to_string() }}
                </button>
            </div>
            <div class="git-tools">
                <div class="git-seg">
                    {TARGETS.into_iter().map(|(t, label, help)| view! {
                        <button class:on=move || target.get() == t title=help on:click=move |_| { agentum_ui::store::set("agentum.git.target", t); target.set(t.to_string()) }>{label}</button>
                    }).collect_view()}
                </div>
                <span class="grow small subdue truncate">{summary_line}</span>
                <div class="git-seg">
                    <button class:on=move || !split.get() title="One column" on:click=move |_| { agentum_ui::store::set("agentum.git.split", ""); split.set(false) }>"Unified"</button>
                    <button class:on=move || split.get() title="Old and new side by side" on:click=move |_| { agentum_ui::store::set("agentum.git.split", "1"); split.set(true) }>"Split"</button>
                </div>
            </div>
            <div class="git-body">
                {move || err.get().map(|e| view! { <div class="git-err small">{e}</div> })}
                {move || (loaded.get() && files.with(|f| f.is_empty()) && err.with(|e| e.is_none())).then(|| view! { <div class="empty small">{empty_text}</div> })}
                <DiffView files=files split=split on_open=open_file/>
            </div>
            <Show when=show_commit>
                <div class="git-commit">
                    <textarea rows="2" placeholder="Commit message (⌘⏎ to commit)" prop:value=msg
                        on:input={ let k = msg_key.clone(); move |e| { let v = event_target_value(&e); agentum_ui::store::set(&k, &v); msg.set(v) } }
                        on:keydown={ let c = commit2.clone(); move |e| if e.key() == "Enter" && (e.meta_key() || e.ctrl_key()) { e.prevent_default(); c() } }/>
                    <div class="row" style="gap:8px">
                        <label class="row small" style="gap:4px" title="Only what is staged (else every tracked change)">
                            <input type="checkbox" prop:checked=staged_only disabled=move || staged() == 0 on:change=move |e| staged_only.set(event_target_checked(&e))/>
                            {move || format!("Staged only ({})", staged())}
                        </label>
                        <span class="grow small subdue truncate">
                            {move || (untracked() > 0 && !(staged_only.get() && staged() > 0)).then(|| format!("{} not included (stage new files to commit them)", plural(untracked() as u32, "new file")))}
                        </span>
                        <button class="btn primary git-sm" disabled=move || busy.get().is_some() || msg.with(|m| m.trim().is_empty()) on:click={ let c = commit.clone(); move |_| c() }>
                            {move || if busy.get() == Some("commit") { "Committing…" } else { "Commit" }}
                        </button>
                    </div>
                </div>
            </Show>
        </div>
    }
}
