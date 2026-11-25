//! The Code page: a new thread (machine, project, agent, model) or one thread's timeline with
//! its composer, questions and side panel (plugins' tabs: files, git, history, terminal…).
mod attach;
pub mod marks;
pub mod store;
mod timeline;

use crate::pages::Pane;
use agentum_plugin::web::{ComposerCx, ThreadCx};
use agentum_proto::api::{AnswerBody, EventView, Models, NewThread, Partial, PatchThread, SendBody, ThreadDetail, ThreadSummary, WsOut};
use agentum_proto::{AgentKind, EventKind, Permission, SendMode};
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

pub fn page(_pane: Pane) -> AnyView {
    let thread = use_param("thread");
    view! {
        {move || match thread.get().filter(|t| !t.is_empty()) {
            Some(id) => view! { <ThreadPage id=id/> }.into_any(),
            None => view! { <NewThreadPage/> }.into_any(),
        }}
    }
    .into_any()
}

pub(crate) fn agent_icon(a: AgentKind) -> &'static str {
    match a {
        AgentKind::ClaudeCode => "sparkle",
        AgentKind::Codex => "terminal",
        AgentKind::Opencode => "code",
        AgentKind::Cursor => "cube",
    }
}

fn perm_label(p: Permission) -> &'static str {
    match p {
        Permission::Ask => "Ask first",
        Permission::Edits => "Edit freely",
        Permission::Plan => "Plan only",
        Permission::Full => "Full access",
    }
}
fn perm_sub(p: Permission) -> &'static str {
    match p {
        Permission::Ask => "Asks before edits and commands",
        Permission::Edits => "Edits files, asks before commands",
        Permission::Plan => "Reads and plans, changes nothing",
        Permission::Full => "Does anything without asking",
    }
}
fn perm_id(p: Permission) -> String {
    serde_json::to_value(p).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}
const PERMS: [Permission; 4] = [Permission::Ask, Permission::Edits, Permission::Plan, Permission::Full];

/// The models of an agent on a machine.
fn models_of(machine: Signal<String>, agent: Signal<AgentKind>) -> LocalResource<Vec<agentum_proto::machine::ModelInfo>> {
    LocalResource::new(move || {
        let (m, a) = (machine.get(), agent.get());
        async move {
            if m.is_empty() {
                return vec![];
            }
            api_get::<Models>(&format!("/code/models?machine={m}&agent={}", a.id())).await.map(|r| r.models).unwrap_or_default()
        }
    })
}

/// A model's provider, when its id says it (`deepseek/deepseek-v4-pro` → `deepseek`).
fn provider_of(id: &str) -> Option<&str> {
    id.split_once('/').map(|(p, _)| p).filter(|p| !p.is_empty())
}

/// BB's model picker: the agents as tabs (their marks), the agent's models (the first few, then
/// "More models"), searchable, and a Reasoning row. One chip: mark · model · effort.
#[component]
fn ModelChip(machine: Signal<String>, agent: RwSignal<AgentKind>, model: RwSignal<Option<String>>, effort: RwSignal<Option<String>>, #[prop(optional)] lock_agent: bool, #[prop(optional)] up: bool) -> impl IntoView {
    let ov = store::overview();
    let open = RwSignal::new(false);
    // The tab being looked at (committed only when a model is picked).
    let tab = RwSignal::new(agent.get_untracked());
    let q = RwSignal::new(String::new());
    let more = RwSignal::new(false);
    let models = models_of(machine, tab.into());
    let current_models = models_of(machine, agent.into());
    let root = NodeRef::<leptos::html::Div>::new();
    on_window(leptos::ev::mousedown, move |e| {
        if !open.get_untracked() {
            return;
        }
        let inside = e.target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Node>(t).ok()).is_some_and(|n| root.get_untracked().is_some_and(|r| r.contains(Some(&n))));
        if !inside {
            open.set(false);
        }
    });
    Effect::new(move |_| {
        if open.get() {
            tab.set(agent.get_untracked());
            q.set(String::new());
            more.set(false);
        }
    });
    let installed = move |a: AgentKind| ov.with(|o| o.as_ref().and_then(|o| o.machines.iter().find(|m| m.id == machine.get())).is_none_or(|m| m.agents.iter().any(|x| x.agent == a && x.installed)));
    // A window used up on the account this machine is signed in to (the agent's on another
    // machine may be another account's).
    let full = move |a: AgentKind| {
        let m = machine.get();
        ov.with(|o| o.as_ref().and_then(|o| agentum_proto::api::limits_for(&o.limits, a, &m)).and_then(|l| l.limits.iter().filter(|w| w.used_percent >= 99.5 && w.resets_at.is_none_or(|r| r > now_ms())).map(|w| w.window.clone()).next()))
    };
    let model_label = move || match model.get() {
        None => "Default".to_string(),
        Some(id) => current_models.get().and_then(|v| v.iter().find(|x| x.id == id).map(|x| x.label.clone())).unwrap_or(id),
    };
    let efforts = move || {
        let a = tab.get();
        let m = if a == agent.get() { model.get() } else { None };
        models.get().map(|v| v.iter().find(|x| Some(&x.id) == m.as_ref()).or_else(|| v.iter().find(|x| x.default)).map(|x| x.efforts.clone()).unwrap_or_default()).unwrap_or_default()
    };
    const FIRST: usize = 7;
    let rows = Memo::new(move |_| {
        let ql = q.get().to_lowercase();
        let v = models.get().unwrap_or_default();
        let hit: Vec<agentum_proto::machine::ModelInfo> = v.into_iter().filter(|m| ql.is_empty() || m.label.to_lowercase().contains(&ql) || m.id.to_lowercase().contains(&ql)).collect();
        hit
    });
    let pick = move |id: Option<String>| {
        let a = tab.get_untracked();
        if a != agent.get_untracked() {
            agent.set(a);
            effort.set(None);
        }
        model.set(id.filter(|m| m != "default"));
        open.set(false);
    };
    view! {
        <div class="pk mp" node_ref=root>
            <button type="button" class="pk-chip pk-model" title=move || model.get().map(|m| format!("{m} · agent, model and reasoning")).unwrap_or_else(|| "Agent, model and reasoning".into()) aria-expanded=move || open.get().to_string() on:click=move |_| open.update(|o| *o = !*o)>
                {move || view! { <crate::code::marks::AgentMark agent=agent.get() size=14/> }}
                <span>{move || model_label()}</span>
                {move || effort.get().map(|e| view! { <span class="pk-dim">{e}</span> })}
                <Icon name="chevron" size=12 class="pk-chev"/>
            </button>
            <Show when=move || open.get()>
                <div class=if up { "pk-pop mp-pop up right" } else { "pk-pop mp-pop right" } role="dialog" aria-label="Model"
                    on:keydown=move |e| if e.key() == "Escape" { open.set(false) }>
                    {(!lock_agent).then(|| view! {
                        <div class="mp-tabs">
                            {AgentKind::ALL.into_iter().map(|a| view! {
                                <button type="button" class=move || format!("mp-tab{}", if tab.get() == a { " on" } else { "" })
                                    disabled=move || !installed(a)
                                    title=move || match (installed(a), full(a)) { (false, _) => format!("{} (not installed on {})", a.label(), machine.get()), (_, Some(w)) => format!("{}: {w} limit used up", a.label()), _ => a.label().to_string() }
                                    on:mousedown=move |e| e.prevent_default()
                                    on:click=move |_| { tab.set(a); more.set(false) }>
                                    <crate::code::marks::AgentMark agent=a size=16/>
                                    {move || full(a).map(|_| view! { <span class="mp-full"/> })}
                                </button>
                            }).collect_view()}
                            <span class="mp-tabname">{move || tab.get().label()}</span>
                        </div>
                    })}
                    <Show when=move || models.get().map(|v| v.len() > FIRST).unwrap_or(false)>
                        <div class="mp-search"><Icon name="search" size=13/>
                            <input placeholder="Search models" prop:value=q on:input=move |e| { q.set(event_target_value(&e)); more.set(true) }/>
                        </div>
                    </Show>
                    <div class="mp-list">
                        <div class="mp-label">"Model"</div>
                        {move || {
                            let a_on = tab.get() == agent.get();
                            let cur = if a_on { model.get() } else { None };
                            if models.get().is_none() {
                                return view! { <div class="mp-empty">"Loading models…"</div> }.into_any();
                            }
                            let all = rows.get();
                            let searching = !q.get().is_empty();
                            let shown: Vec<_> = if searching || more.get() { all.clone() } else { all.iter().take(FIRST).cloned().collect() };
                            let hidden = all.len().saturating_sub(shown.len());
                            view! {
                                {(!searching && !all.iter().any(|m| m.id == "default")).then(|| view! {
                                    <button type="button" class="pk-item" on:click=move |_| pick(None)>
                                        <span class="pk-label">"Default"</span>
                                        <span class="pk-sub">"the agent's own choice"</span>
                                        {(a_on && cur.is_none()).then(|| view! { <Icon name="check" size=13 class="pk-check"/> })}
                                    </button>
                                })}
                                {shown.into_iter().map(|m| {
                                    let on = cur.as_deref() == Some(m.id.as_str());
                                    let id = m.id.clone();
                                    view! {
                                        <button type="button" class="pk-item" title=format!("{}{}", m.id, m.context_window.map(|c| format!(" · {}k context", c / 1000)).unwrap_or_default()) on:click=move |_| pick(Some(id.clone()))>
                                            // As BB: the provider after the name (two "DeepSeek V4 Pro" differ by it).
                                            <span class="pk-label">{m.label.clone()}{provider_of(&m.id).map(|p| view! { <span class="mp-prov">{p.to_string()}</span> })}</span>
                                            {on.then(|| view! { <Icon name="check" size=13 class="pk-check"/> })}
                                        </button>
                                    }
                                }).collect_view()}
                                {(hidden > 0).then(|| view! {
                                    <button type="button" class="pk-item mp-more" on:click=move |_| more.set(true)>
                                        <span class="pk-label">{format!("More models ({hidden})")}</span><Icon name="chevron" size=12/>
                                    </button>
                                })}
                                {(searching && all.is_empty()).then(|| view! { <div class="mp-empty">"No models match your search."</div> })}
                            }.into_any()
                        }}
                    </div>
                    <Show when=move || !efforts().is_empty()>
                        <div class="mp-reason">
                            <div class="mp-label">"Reasoning"</div>
                            <div class="mp-seg">
                                {move || {
                                    let on_tab = tab.get() == agent.get();
                                    let mut opts: Vec<Option<String>> = vec![None];
                                    opts.extend(efforts().into_iter().map(Some));
                                    opts.into_iter().map(|e| {
                                        let sel = on_tab && effort.get() == e;
                                        let label = e.clone().unwrap_or_else(|| "Auto".into());
                                        view! {
                                            <button type="button" class=if sel { "on" } else { "" }
                                                on:click=move |_| { if tab.get_untracked() != agent.get_untracked() { agent.set(tab.get_untracked()); model.set(None); } effort.set(e.clone()) }>{label}</button>
                                        }
                                    }).collect_view()
                                }}
                            </div>
                        </div>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

/// BB's new thread: one composer box (the model chip inside it), and under it the project, the
/// machine (here or in a new worktree) and the permission.
#[component]
fn NewThreadPage() -> impl IntoView {
    let ov = store::overview();
    let p_machine = use_param("machine");
    let p_project = use_param("project");
    let at = here();
    let machine = RwSignal::new(String::new());
    let project = RwSignal::new(String::new());
    let agent = RwSignal::new(agentum_ui::store::get("agentum.agent").and_then(|a| AgentKind::parse(&a)).unwrap_or(AgentKind::ClaudeCode));
    let model = RwSignal::new(agentum_ui::store::get("agentum.model").filter(|m| !m.is_empty()));
    let effort = RwSignal::new(agentum_ui::store::get("agentum.effort").filter(|m| !m.is_empty()));
    // Full access by default, as BB's; a pick is remembered.
    let permission = RwSignal::new(agentum_ui::store::get("agentum.permission").and_then(|p| PERMS.into_iter().find(|x| perm_id(*x) == p)).unwrap_or(Permission::Full));
    let worktree = RwSignal::new(agentum_ui::store::flag("agentum.worktree"));
    // The worktree chip (as BB's): a new worktree from `base` (None: the default branch), or an
    // existing worktree of the project (`reuse`, at `existing`).
    let base = RwSignal::new(None::<String>);
    let reuse = RwSignal::new(false);
    let existing = RwSignal::new(None::<String>);
    let starts = RwSignal::new(None::<(String, Value)>);
    let text = RwSignal::new(agentum_ui::store::get("agentum.draft.new").unwrap_or_default());
    let busy = RwSignal::new(false);
    let images = RwSignal::new(Vec::<String>::new());
    let adding = RwSignal::new(String::new());
    let area = NodeRef::<leptos::html::Textarea>::new();
    if let Some(p) = pane() {
        p.title.set(Some("New thread".into()));
    }
    Effect::new(move |_| {
        if let Some(t) = area.get() {
            let _ = t.focus();
        }
    });
    // Remember the last choices.
    Effect::new(move |_| {
        agentum_ui::store::set("agentum.agent", agent.get().id());
        agentum_ui::store::set("agentum.model", model.get().as_deref().unwrap_or(""));
        agentum_ui::store::set("agentum.effort", effort.get().as_deref().unwrap_or(""));
        agentum_ui::store::set("agentum.permission", &perm_id(permission.get()));
        agentum_ui::store::set("agentum.worktree", if worktree.get() { "1" } else { "" });
    });
    // Defaults: the URL's machine, else the last used, else the main one, else the first online.
    Effect::new(move |_| {
        ov.with(|o| {
            let Some(o) = o else { return };
            if machine.get_untracked().is_empty() || !o.machines.iter().any(|m| m.id == machine.get_untracked()) {
                let want = p_machine.get().or_else(|| agentum_ui::store::get("agentum.machine"));
                let m = o.machines.iter().find(|m| Some(&m.id) == want.as_ref() && m.online).or_else(|| o.machines.iter().find(|m| m.main && m.online)).or_else(|| o.machines.iter().find(|m| m.online)).or(o.machines.first());
                if let Some(m) = m {
                    machine.set(m.id.clone());
                }
            }
        })
    });
    let projects_of = move |m: &str| ov.with(|o| o.as_ref().and_then(|o| o.machines.iter().find(|x| x.id == m).map(|x| x.projects.clone())).unwrap_or_default());
    Effect::new(move |_| {
        let m = machine.get();
        let want = p_project.get();
        let projects = projects_of(&m);
        if let Some(w) = want.filter(|w| projects.iter().any(|p| &p.path == w)) {
            project.set(w);
        } else if !projects.iter().any(|p| p.path == project.get_untracked()) {
            let last = agentum_ui::store::get(&format!("agentum.project.{m}"));
            project.set(last.filter(|l| projects.iter().any(|p| &p.path == l)).or_else(|| projects.first().map(|p| p.path.clone())).unwrap_or_default());
        }
    });
    // Projects as BB lists them: once each by name, whichever machines have them.
    let has = move |m: &str, name: &str| projects_of(m).into_iter().find(|p| p.name == name).map(|p| p.path);
    let project_label = move || {
        let p = project.get();
        if p.is_empty() { String::new() } else { projects_of(&machine.get()).into_iter().find(|x| x.path == p).map(|x| x.name).unwrap_or_else(|| p.trim_end_matches('/').rsplit('/').next().unwrap_or(&p).to_string()) }
    };
    let project_items = Signal::derive(move || {
        let cur = project_label();
        let mut names: Vec<String> = ov.with(|o| o.as_ref().map(|o| o.machines.iter().flat_map(|m| m.projects.iter().map(|p| p.name.clone())).collect()).unwrap_or_default());
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names.into_iter().map(|n| PickItem { on: n == cur, id: n.clone(), label: n, icon: "folder", group: "Project".into(), ..Default::default() }).collect::<Vec<_>>()
    });
    let pick_project = move |name: String| {
        let machines = ov.with_untracked(|o| o.as_ref().map(|o| o.machines.clone()).unwrap_or_default());
        let here_ = machine.get_untracked();
        let remembered = agentum_ui::store::get(&format!("agentum.machine.{name}"));
        let m = [Some(here_), remembered].into_iter().flatten().find(|m| machines.iter().any(|x| &x.id == m && x.online) && has(m, &name).is_some())
            .or_else(|| machines.iter().find(|x| x.main && x.online && has(&x.id, &name).is_some()).map(|x| x.id.clone()))
            .or_else(|| machines.iter().find(|x| x.online && has(&x.id, &name).is_some()).map(|x| x.id.clone()))
            .or_else(|| machines.iter().find(|x| has(&x.id, &name).is_some()).map(|x| x.id.clone()));
        if let Some(m) = m {
            if let Some(path) = has(&m, &name) {
                machine.set(m);
                project.set(path);
            }
        }
    };
    // The environment: a machine (that has the project) × the project's checkout or a worktree.
    let env_items = Signal::derive(move || {
        let (cur_m, wt, name) = (machine.get(), worktree.get(), project_label());
        let mut ms = ov.with(|o| o.as_ref().map(|o| o.machines.clone()).unwrap_or_default());
        ms.sort_by_key(|m| (!m.online, !m.main, m.name.to_lowercase()));
        let mut v = vec![];
        for m in ms {
            let group = if m.online { m.name.clone() } else { format!("{} · offline", m.name) };
            if name.is_empty() || has(&m.id, &name).is_some() {
                for (kind, label, icon) in [("local", "Project checkout", "folder"), ("wt", "Worktree", "branch")] {
                    v.push(PickItem { id: format!("{kind}:{}", m.id), label: label.into(), icon, group: group.clone(), disabled: !m.online, on: m.id == cur_m && wt == (kind == "wt"), ..Default::default() });
                }
            } else {
                v.push(PickItem { id: format!("none:{}", m.id), label: "Not set up for this project".into(), icon: "folder", group, disabled: true, ..Default::default() });
            }
        }
        v
    });
    let env_label = move || {
        let m = machine.get();
        let online = ov.with(|o| o.as_ref().and_then(|o| o.machines.iter().find(|x| x.id == m).map(|x| x.online)));
        match online {
            None if m.is_empty() => ("server", "No host connected".to_string()),
            Some(false) => ("alert", "Host is offline".to_string()),
            _ => (if worktree.get() { "branch" } else { "folder" }, format!("{m} · {}", if worktree.get() { "Worktree" } else { "Project checkout" })),
        }
    };
    // What a worktree can start from, for this machine's project.
    Effect::new(move |_| {
        let (m, p, wt) = (machine.get(), project.get(), worktree.get());
        let key = format!("{m}\u{1f}{p}");
        if !wt || m.is_empty() || p.is_empty() || starts.with_untracked(|s| s.as_ref().is_some_and(|(k, _)| *k == key)) {
            return;
        }
        base.set(None);
        existing.set(None);
        starts.set(None);
        spawn_local(async move {
            let req = agentum_proto::machine::Req::Git(agentum_proto::machine::GitReq::StartPoints { repo: p });
            let v = api_post::<Value>("/code/call", &json!({"machine": m, "req": req})).await.unwrap_or(Value::Null);
            let _ = starts.try_set(Some((key, v)));
        });
    });
    let sp = move || starts.with(|s| s.as_ref().map(|(_, v)| v.clone()));
    let default_base = move || sp().and_then(|v| v["default"].as_str().map(str::to_string));
    let wt_items = Signal::derive(move || {
        let r = reuse.get();
        let mut v = vec![
            PickItem { id: "mode:new".into(), label: "New worktree".into(), icon: "plus", group: "Work in:".into(), on: !r, keep_open: true, ..Default::default() },
            PickItem { id: "mode:reuse".into(), label: "Existing worktree".into(), icon: "folder", group: "Work in:".into(), on: r, keep_open: true, ..Default::default() },
        ];
        let Some(sp) = sp() else {
            v.push(PickItem { id: "-".into(), label: if r { "Loading worktrees…" } else { "Loading branches…" }.into(), disabled: true, group: if r { "Existing worktree:" } else { "Branch from:" }.into(), ..Default::default() });
            return v;
        };
        if sp.is_null() {
            v.push(PickItem { id: "-".into(), label: "Couldn't read this project's git.".into(), disabled: true, group: "Branch from:".into(), ..Default::default() });
        } else if r {
            let cur = existing.get();
            let ws = sp["worktrees"].as_array().cloned().unwrap_or_default();
            if ws.is_empty() {
                v.push(PickItem { id: "-".into(), label: "No existing worktrees found.".into(), disabled: true, group: "Existing worktree:".into(), ..Default::default() });
            }
            for w in ws {
                let path = w["path"].as_str().unwrap_or("").to_string();
                let short = path.trim_end_matches('/').rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
                let mut sub = w["branch"].as_str().unwrap_or("Detached HEAD").to_string();
                if w["locked"].as_bool() == Some(true) { sub.push_str(" · locked"); }
                if w["prunable"].as_bool() == Some(true) { sub.push_str(" · prunable"); }
                v.push(PickItem { on: cur.as_deref() == Some(path.as_str()), id: format!("wt:{path}"), label: short, sub, icon: "branch", group: "Existing worktree:".into(), disabled: w["prunable"].as_bool() == Some(true), ..Default::default() });
            }
        } else {
            let cur = base.get();
            v.push(PickItem { id: "base:".into(), label: "Default branch".into(), sub: sp["default"].as_str().unwrap_or("").into(), icon: "branch", group: "Branch from:".into(), on: cur.is_none(), ..Default::default() });
            for b in sp["branches"].as_array().into_iter().flatten().chain(sp["remote"].as_array().into_iter().flatten()).filter_map(|b| b.as_str()) {
                v.push(PickItem { on: cur.as_deref() == Some(b), id: format!("base:{b}"), label: b.into(), icon: "branch", group: "Branch from:".into(), ..Default::default() });
            }
        }
        v
    });
    let pick_wt = move |id: String| match id.split_once(':') {
        Some(("mode", m)) => reuse.set(m == "reuse"),
        Some(("base", b)) => base.set(Some(b.to_string()).filter(|b| !b.is_empty())),
        Some(("wt", p)) => existing.set(Some(p.to_string())),
        _ => {}
    };
    let checkout_label = move || {
        let p = project.get();
        projects_of(&machine.get()).into_iter().find(|x| x.path == p).and_then(|x| x.branch).map(|b| format!("Current ({b})")).unwrap_or_else(|| "Current branch".into())
    };
    let perm_items = Signal::derive(move || PERMS.into_iter().map(|p| PickItem { id: perm_id(p), label: perm_label(p).into(), sub: perm_sub(p).into(), icon: "shield", on: permission.get() == p, ..Default::default() }).collect::<Vec<_>>());
    let start = move || {
        let t = text.get_untracked();
        if (t.trim().is_empty() && images.with_untracked(|v| v.is_empty())) || project.get_untracked().is_empty() || busy.get_untracked() {
            if project.get_untracked().is_empty() {
                toast_bad("Pick a project first.");
            }
            return;
        }
        let reusing = worktree.get_untracked() && reuse.get_untracked();
        if reusing && existing.get_untracked().is_none() {
            toast_bad("Pick a worktree first.");
            return;
        }
        busy.set(true);
        // A worktree's branch, from the first words of the task.
        let branch = (worktree.get_untracked() && !reusing).then(|| {
            let slug: String = t.split_whitespace().take(5).collect::<Vec<_>>().join("-").to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
            format!("agentum/{}-{}", slug.trim_matches('-'), (js_sys::Math::random() * 1296.0) as u32)
        });
        let body = NewThread { machine: machine.get_untracked(), project: project.get_untracked(), agent: Some(agent.get_untracked()), model: model.get_untracked(), effort: effort.get_untracked(), permission: permission.get_untracked(), worktree: branch.clone(), base: branch.as_ref().and_then(|_| base.get_untracked()), cwd: if reusing { existing.get_untracked() } else { None }, text: t, images: images.get_untracked(), parent: None };
        agentum_ui::store::set(&format!("agentum.project.{}", body.machine), &body.project);
        agentum_ui::store::set(&format!("agentum.machine.{}", project_label()), &body.machine);
        agentum_ui::store::set("agentum.machine", &body.machine);
        spawn_local(async move {
            match api_post::<ThreadSummary>("/code/threads", &body).await {
                Ok(t) => {
                    agentum_ui::store::remove("agentum.draft.new");
                    text.set(String::new());
                    images.set(vec![]);
                    navigate("code", at, &[("thread", &t.id)]);
                }
                Err(e) => toast_bad(e.message),
            }
            busy.set(false);
        });
    };
    let add_project = move || {
        let path = adding.get_untracked().trim().to_string();
        if path.is_empty() {
            return;
        }
        let m = machine.get_untracked();
        spawn_local(async move {
            match api_post::<Value>(&format!("/code/machines/{m}/projects"), &json!({"path": path})).await {
                Ok(_) => {
                    adding.set(String::new());
                    project.set(path);
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let recent = move || {
        let seen = store::visited();
        ov.with(|o| {
            let o = o.as_ref()?;
            let mut v: Vec<ThreadSummary> = seen.iter().rev().filter_map(|id| o.threads.iter().find(|t| &t.id == id && !t.archived).cloned()).take(5).collect();
            if v.len() < 5 {
                let mut more: Vec<ThreadSummary> = o.threads.iter().filter(|t| !t.archived && t.parent.is_none() && !v.iter().any(|x| x.id == t.id)).cloned().collect();
                more.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                v.extend(more.into_iter().take(5 - v.len()));
            }
            Some(v)
        })
        .unwrap_or_default()
    };
    view! {
        <div class="compose-home">
            <div class="compose-col">
                <div class="cbox" on:dragover=attach::on_dragover on:drop=move |e| attach::on_drop(e, images)>
                    <attach::Thumbs images=images/>
                    <textarea node_ref=area class="cbox-text" rows="3" placeholder="Ask anything. ⏎ to start, ⇧⏎ for a new line" prop:value=text
                        on:paste=move |e| attach::on_paste(e, images)
                        on:input=move |e| { let v = event_target_value(&e); agentum_ui::store::set("agentum.draft.new", &v); text.set(v) }
                        on:keydown=move |e| if e.key() == "Enter" && !e.shift_key() && !e.is_composing() { e.prevent_default(); start() }/>
                    <div class="cbox-bar">
                        <attach::AttachButton images=images/>
                        <span class="grow"/>
                        <ModelChip machine=machine.into() agent=agent model=model effort=effort/>
                        <button class="send-btn" title="Start (⏎)" disabled=move || busy.get() || (text.get().trim().is_empty() && images.with(|v| v.is_empty())) || project.get().is_empty() on:click=move |_| start()>
                            <Icon name="arrow-up" size=16/>
                        </button>
                    </div>
                </div>
                <div class="cbox-foot">
                    <Picker items=project_items on_pick=Callback::new(pick_project) search=true class="pk-quiet" title="Project" placeholder="Search projects"
                        footer=ViewFn::from(move || view! {
                            <input class="pk-search" placeholder="New project: its full path on this machine, ⏎" prop:value=adding on:input=move |e| adding.set(event_target_value(&e))
                                on:keydown=move |e| if e.key() == "Enter" { e.prevent_default(); e.stop_propagation(); add_project() }/>
                        })>
                        <Icon name="folder" size=13/><span>{move || { let n = project_label(); if n.is_empty() { "Pick a project".to_string() } else { n } }}</span>
                    </Picker>
                    <Picker items=env_items class="pk-quiet" title="Where it works: the machine, and the project's checkout or a worktree"
                        on_pick=Callback::new(move |id: String| if let Some((kind, m)) = id.split_once(':') {
                            if kind == "none" { return; }
                            let name = project_label();
                            if let Some(path) = has(m, &name) { project.set(path); }
                            machine.set(m.to_string());
                            worktree.set(kind == "wt");
                        })>
                        {move || { let (icon, label) = env_label(); view! { <Icon name=icon size=13/><span>{label}</span> } }}
                    </Picker>
                    {move || if worktree.get() {
                        view! {
                            <Picker items=wt_items on_pick=Callback::new(pick_wt) search=true class="pk-quiet" placeholder="Search branches and worktrees"
                                title="A new worktree for this thread, from a branch; or one you already have">
                                <Icon name="branch" size=13/>
                                {move || if reuse.get() {
                                    match existing.get() {
                                        Some(p) => view! { <span class="pk-dim">"Reuse:"</span><b>{p.trim_end_matches('/').rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/")}</b> }.into_any(),
                                        None => view! { <span>"Pick a worktree"</span> }.into_any(),
                                    }
                                } else {
                                    view! { <span class="pk-dim">"Branch from:"</span><b>{base.get().or_else(default_base).unwrap_or_else(|| "default".into())}</b> }.into_any()
                                }}
                            </Picker>
                        }.into_any()
                    } else {
                        view! { <span class="pk-chip pk-quiet pk-static" title="Works in the project's checkout, on its current branch"><Icon name="branch" size=13/><span>{checkout_label}</span></span> }.into_any()
                    }}
                    <span class="grow"/>
                    <Picker items=perm_items class="pk-quiet" title="What the agent may do without asking"
                        on_pick=Callback::new(move |id: String| if let Some(p) = PERMS.into_iter().find(|p| perm_id(*p) == id) { permission.set(p) })>
                        <Icon name="shield" size=13/><span>{move || perm_label(permission.get())}</span>
                    </Picker>
                </div>
                <Show when=move || !recent().is_empty()>
                    <div class="home-recent">
                        <div class="home-recent-h">"Recent"</div>
                        {move || recent().into_iter().map(|t| {
                            let id = t.id.clone();
                            let id2 = t.id.clone();
                            view! {
                                <button class="home-recent-row" draggable="true"
                                    on:dragstart=move |e| agentum_ui::dnd::start(&e, "code", &[("thread", &id2)])
                                    on:click=move |e| navigate("code", if e.meta_key() || e.ctrl_key() { Where::Right } else { at }, &[("thread", &id)])>
                                    <Icon name=agent_icon(t.agent) size=13/>
                                    <span class="grow truncate">{if t.title.is_empty() { "New thread".to_string() } else { t.title.clone() }}</span>
                                    <span class="pk-dim">{format!("{} · {}", t.project.rsplit('/').next().unwrap_or(""), t.machine)}</span>
                                    <span class="pk-dim">{ago_ms(t.updated_at)}</span>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn ThreadPage(id: String) -> impl IntoView {
    store::note_visit(&id);
    let tid = id.clone();
    let summary = store::thread(move || tid.clone());
    let events = RwSignal::new(Vec::<EventView>::new());
    let partial = RwSignal::new(BTreeMap::<String, Partial>::new());
    let more_before = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let rail = RwSignal::new(agentum_ui::store::get("agentum.rail").filter(|r| !r.is_empty()));
    let at_seq = use_param("at");

    let load = {
        let id = id.clone();
        move || {
            let id = id.clone();
            spawn_local(async move {
                match api_get::<ThreadDetail>(&format!("/code/threads/{id}")).await {
                    Ok(d) => {
                        more_before.set(d.more_before);
                        partial.set(d.partial.into_iter().map(|p| (p.item.clone(), p)).collect());
                        events.set(d.events);
                        loaded.set(true);
                    }
                    Err(e) => {
                        toast_bad(e.message);
                        loaded.set(true);
                    }
                }
            });
        }
    };
    load();
    let ch = format!("thread:{id}");
    subscribe(&[&ch]);
    {
        let id = id.clone();
        on_ws(move |m| {
            if let WsOut::Event { thread, event } = m {
                if *thread != id {
                    return;
                }
                apply(events, partial, event.clone());
            }
        });
    }
    let c = agentum_ui::ws::connected();
    {
        let load = load.clone();
        Effect::new(move |prev: Option<u32>| {
            let n = c.get();
            if prev.is_some() {
                load();
            }
            n
        });
    }
    // Opened (or new events while open): it's read.
    {
        let id = id.clone();
        Effect::new(move |_| {
            let unread = summary.with(|s| s.as_ref().is_some_and(|s| s.unread));
            if unread && !document_hidden() {
                let id = id.clone();
                spawn_local(async move {
                    let _ = api(&format!("/code/threads/{id}"), "PATCH", Some(&serde_json::to_value(PatchThread { read: Some(true), ..Default::default() }).unwrap())).await.map(|_: Value| ());
                });
            }
        });
    }
    let load_older = {
        let id = id.clone();
        move || {
            let first = events.with_untracked(|e| e.first().map(|e| e.event.seq).unwrap_or(0));
            let id = id.clone();
            spawn_local(async move {
                if let Ok(d) = api_get::<ThreadDetail>(&format!("/code/threads/{id}?before={first}")).await {
                    more_before.set(d.more_before);
                    events.update(|e| {
                        let mut old = d.events;
                        old.append(e);
                        *e = old;
                    });
                }
            });
        }
    };

    let cx = ThreadCx { id: id.clone(), summary, rail };
    let tabs = crate::plugins::rail_tabs();
    let at = here();

    // The pane's header: the thread's title, where it runs, and its few actions.
    if let Some(p) = pane() {
        Effect::new(move |_| {
            let s = summary.get();
            let _ = p.title.try_set(Some(s.as_ref().map(|s| if s.title.is_empty() { "New thread".to_string() } else { s.title.clone() }).unwrap_or_default()));
            let _ = p.sub.try_set(s.map(|s| format!("{} · {}{}", s.machine, s.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(""), s.branch.map(|b| format!(" · {b}")).unwrap_or_default())));
        });
        let (id_s, id_p, id_a, id_t, id_r) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
        let busy = Signal::derive(move || summary.with(|s| s.as_ref().is_some_and(|s| s.state.busy())));
        let panel_on = Signal::derive(move || rail.get().is_some());
        let first_tab = tabs.first().map(|t| t.id);
        Effect::new(move |_| {
            let mut a = vec![];
            if busy.get() {
                let id = id_s.clone();
                a.push(PaneAction::new("stop", "Stop the turn (Esc)", move || stop_thread(&id)).danger());
            }
            if let Some(first) = first_tab {
                a.push(PaneAction::new("panel-right", "Side panel: files, git, terminal…", move || {
                    let next = if rail.get_untracked().is_some() { None } else { Some(agentum_ui::store::get("agentum.rail.last").filter(|r| !r.is_empty()).unwrap_or_else(|| first.to_string())) };
                    agentum_ui::store::set("agentum.rail", next.as_deref().unwrap_or(""));
                    rail.set(next);
                }).toggled(panel_on));
            }
            let _ = p.actions.try_set(a);
        });
        // The header's + launcher: each side-panel tab (a click opens it in the panel, a drag beside).
        {
            let id = id.clone();
            let _ = p.tools.try_set(tabs.iter().map(|t| {
                let tid = t.id;
                PaneTool {
                    label: t.title.to_string(),
                    icon: t.icon,
                    page: "panel",
                    params: vec![("tab".into(), tid.into()), ("thread".into(), id.clone())],
                    open: Some(Callback::new(move |_| {
                        agentum_ui::store::set("agentum.rail", tid);
                        agentum_ui::store::set("agentum.rail.last", tid);
                        rail.set(Some(tid.to_string()));
                    })),
                }
            }).collect());
        }
        let pinned = Signal::derive(move || summary.with(|s| s.as_ref().is_some_and(|s| s.pinned)));
        Effect::new(move |_| {
            let pin = pinned.get();
            let (i1, i2, i3, i4, i5) = (id_p.clone(), id_a.clone(), id_t.clone(), id_r.clone(), id_p.clone());
            let _ = p.menu.try_set(vec![
                PaneAction::new("terminal", "Terminal beside", move || navigate("panel", if p.right { Where::Left } else { Where::Right }, &[("tab", "terminal"), ("thread", &i1)])),
                PaneAction::new("edit", "Rename", move || {
                    let cur = summary.get_untracked().map(|s| s.title).unwrap_or_default();
                    if let Some(t) = web_sys::window().and_then(|w| w.prompt_with_message_and_default("Rename the thread", &cur).ok().flatten()) {
                        patch_thread(&i4, PatchThread { title: Some(t), ..Default::default() });
                    }
                }),
                PaneAction::new("pin", if pin { "Unpin" } else { "Pin" }, move || patch_thread(&i2, PatchThread { pinned: Some(!pin), ..Default::default() })),
                PaneAction::new("alarm", "Snooze until tomorrow", move || patch_thread(&i5, PatchThread { snoozed_until: Some(Some(tomorrow_9())), ..Default::default() })),
                PaneAction::new("check-circle", "Settle (archive)", move || { patch_thread(&i3, PatchThread { archived: Some(true), ..Default::default() }); navigate("code", at, &[("thread", "")]) }),
            ]);
        });
        on_cleanup(move || p.clear());
    }
    let cx2 = cx.clone();
    view! {
        <div class="code-thread">
            <div class="code-main">
                <div class="thread-strip">{crate::plugins::thread_header(cx.clone())}</div>
                <timeline::Timeline events=events partial=partial more_before=more_before loaded=loaded summary=summary at=at_seq on_older=Callback::new(move |_| load_older())/>
                <Composer id=id.clone() summary=summary/>
            </div>
            {move || rail.get().map(|r| {
                let t = tabs.iter().find(|t| t.id == r).copied().or_else(|| tabs.first().copied());
                let cx = cx2.clone();
                let thread = cx.id.clone();
                view! {
                    <aside class="code-rail">
                        <div class="rail-tabs">
                            {tabs.iter().map(|t| {
                                let tid = t.id;
                                let th = thread.clone();
                                view! {
                                    <button class=move || if rail.get().as_deref() == Some(tid) { "rail-tab on" } else { "rail-tab" } title=format!("{} · drag it to open beside", t.title) draggable="true"
                                        on:dragstart=move |e| agentum_ui::dnd::start(&e, "panel", &[("tab", tid), ("thread", &th)])
                                        on:click=move |_| { agentum_ui::store::set("agentum.rail", tid); agentum_ui::store::set("agentum.rail.last", tid); rail.set(Some(tid.to_string())) }>
                                        <Icon name=t.icon size=14/>{let title = t.title; move || (rail.get().as_deref() == Some(tid)).then_some(view! { <span>{title}</span> })}
                                    </button>
                                }
                            }).collect_view()}
                            <span class="grow"/>
                            {t.map(|t| { let th = thread.clone(); view! {
                                <button class="icon-btn" title="Open beside" on:click=move |_| { rail.set(None); agentum_ui::store::set("agentum.rail", ""); navigate("panel", if at == Where::Right { Where::Left } else { Where::Right }, &[("tab", t.id), ("thread", &th)]) }><Icon name="split" size=14/></button>
                            }})}
                            <button class="icon-btn" title="Close" on:click=move |_| { rail.set(None); agentum_ui::store::set("agentum.rail", "") }><Icon name="x" size=14/></button>
                        </div>
                        <div class="rail-body">{t.map(|t| (t.view)(cx.clone()))}</div>
                    </aside>
                }
            })}
        </div>
    }
}

/// A side-panel tab (Terminal, Files, Git…) of a thread as a pane of its own:
/// `/panel?tab=terminal&thread=<id>`, beside the thread or anything else.
pub fn panel_page(_pane: Pane) -> AnyView {
    let tab = use_param("tab");
    let thread = use_param("thread");
    let p = pane();
    view! {
        {move || {
            let (Some(tab), Some(id)) = (tab.get(), thread.get().filter(|t| !t.is_empty())) else {
                return view! { <div class="empty">"Drag a tab from a thread's side panel here."</div> }.into_any();
            };
            let Some(t) = crate::plugins::rail_tabs().into_iter().find(|t| t.id == tab) else {
                return view! { <div class="empty">"That tool is turned off."</div> }.into_any();
            };
            let id2 = id.clone();
            let summary = store::thread(move || id2.clone());
            if let Some(p) = p {
                let title = t.title;
                Effect::new(move |_| {
                    let _ = p.title.try_set(Some(title.to_string()));
                    let _ = p.sub.try_set(summary.get().map(|s| format!("{} · {} · {}", s.title, s.machine, s.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(""))));
                });
            }
            let rail = RwSignal::new(Some(tab.clone()));
            let cx = ThreadCx { id, summary, rail };
            view! { <div class="panel-page">{(t.view)(cx)}</div> }.into_any()
        }}
    }
    .into_any()
}

/// 09:00 tomorrow, local time.
pub(crate) fn tomorrow_9() -> i64 {
    let d = js_sys::Date::new_0();
    d.set_date(d.get_date() + 1);
    d.set_hours(9);
    d.set_minutes(0);
    d.set_seconds(0);
    d.set_milliseconds(0);
    d.get_time() as i64
}

pub(crate) fn patch_thread(id: &str, p: PatchThread) {
    let id = id.to_string();
    spawn_local(async move {
        if let Err(e) = api::<Value>(&format!("/code/threads/{id}"), "PATCH", Some(&serde_json::to_value(p).unwrap())).await {
            toast_bad(e.message);
        }
    });
}

fn stop_thread(id: &str) {
    let id = id.to_string();
    spawn_local(async move {
        if let Err(e) = api_post::<Value>(&format!("/code/threads/{id}/stop"), &json!({})).await {
            toast_bad(e.message);
        }
    });
}

fn document_hidden() -> bool {
    web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden())
}

/// Fold an event into the timeline: deltas into `partial`, everything else appended (once).
fn apply(events: RwSignal<Vec<EventView>>, partial: RwSignal<BTreeMap<String, Partial>>, ev: EventView) {
    match &ev.event.kind {
        EventKind::Delta { item, text } | EventKind::ThinkingDelta { item, text } => {
            let thinking = matches!(ev.event.kind, EventKind::ThinkingDelta { .. });
            partial.update(|p| p.entry(item.clone()).or_insert_with(|| Partial { item: item.clone(), thinking, text: String::new() }).text.push_str(text));
        }
        EventKind::ToolOutput { item, text } => {
            partial.update(|p| p.entry(format!("tool:{item}")).or_insert_with(|| Partial { item: item.clone(), thinking: false, text: String::new() }).text.push_str(text));
        }
        _ => {
            match &ev.event.kind {
                EventKind::Message { item, .. } | EventKind::Thinking { item, .. } => partial.update(|p| {
                    p.remove(item);
                }),
                EventKind::ToolDone { item, .. } => partial.update(|p| {
                    p.remove(&format!("tool:{item}"));
                }),
                EventKind::TurnDone { .. } => partial.update(|p| p.retain(|k, _| k.starts_with("tool:"))),
                _ => {}
            }
            events.update(|e| {
                if e.last().is_none_or(|l| l.event.seq < ev.event.seq) {
                    e.push(ev);
                } else if !e.iter().any(|x| x.event.seq == ev.event.seq) {
                    e.push(ev);
                    e.sort_by_key(|x| x.event.seq);
                }
            });
        }
    }
}

/// BB's composer: one box, the text, and a bar with the plugins' buttons, the model chip and send.
#[component]
fn Composer(id: String, summary: Signal<Option<ThreadSummary>>) -> impl IntoView {
    let key = format!("agentum.draft.{id}");
    let text = RwSignal::new(agentum_ui::store::get(&key).unwrap_or_default());
    let busy = move || summary.with(|s| s.as_ref().is_some_and(|s| s.state.busy()));
    let images = RwSignal::new(Vec::<String>::new());
    let send = {
        let id = id.clone();
        let key = key.clone();
        Callback::new(move |mut b: SendBody| {
            if b.text.trim().is_empty() {
                b.text = text.get_untracked();
            }
            if b.images.is_empty() {
                b.images = images.get_untracked();
            }
            if b.text.trim().is_empty() && b.images.is_empty() {
                return;
            }
            text.set(String::new());
            images.set(vec![]);
            agentum_ui::store::remove(&key);
            let id = id.clone();
            spawn_local(async move {
                if let Err(e) = api_post::<Value>(&format!("/code/threads/{id}/send"), &b).await {
                    toast_bad(e.message);
                }
            });
        })
    };
    let cx = ComposerCx { thread: Some(id.clone()), text, send };
    // The model chip changes the thread's model (and effort) for the next turns.
    let agent = RwSignal::new(summary.get_untracked().map(|s| s.agent).unwrap_or(AgentKind::ClaudeCode));
    let model = RwSignal::new(summary.get_untracked().and_then(|s| s.model));
    let effort = RwSignal::new(summary.get_untracked().and_then(|s| s.effort));
    let machine = Signal::derive(move || summary.with(|s| s.as_ref().map(|s| s.machine.clone()).unwrap_or_default()));
    Effect::new(move |_| {
        if let Some(s) = summary.get() {
            agent.set(s.agent);
            if model.get_untracked() != s.model {
                model.set(s.model.clone());
            }
            if effort.get_untracked() != s.effort {
                effort.set(s.effort.clone());
            }
        }
    });
    {
        let id = id.clone();
        Effect::new(move |prev: Option<(Option<String>, Option<String>)>| {
            let now = (model.get(), effort.get());
            let saved = summary.with_untracked(|s| s.as_ref().map(|s| (s.model.clone(), s.effort.clone())));
            if prev.is_some() && saved.is_some_and(|x| x != now) {
                patch_thread(&id, PatchThread { model: Some(now.0.clone().unwrap_or_default()), effort: Some(now.1.clone().unwrap_or_default()), ..Default::default() });
            }
            now
        });
    }
    let stop = {
        let id = id.clone();
        move || stop_thread(&id)
    };
    let s1 = send;
    view! {
        <div class="code-composer">
            <div class="cbox" on:dragover=attach::on_dragover on:drop=move |e| attach::on_drop(e, images)>
                <attach::Thumbs images=images/>
                <textarea class="cbox-text" rows="2" on:paste=move |e| attach::on_paste(e, images) placeholder=move || if busy() { "Queue a message (⏎) · steer the turn (⌥⏎) · stop and send (⌘⇧⏎)" } else { "Reply… ⏎ to send, ⇧⏎ for a new line" }
                    prop:value=text
                    on:input=move |e| { let v = event_target_value(&e); agentum_ui::store::set(&key, &v); text.set(v) }
                    on:keydown=move |e| {
                        if e.key() == "Escape" && busy() { stop(); return; }
                        if e.key() != "Enter" || e.shift_key() || e.is_composing() { return; }
                        e.prevent_default();
                        let mode = if (e.meta_key() || e.ctrl_key()) && e.shift_key() { SendMode::Interrupt } else if e.alt_key() { SendMode::Steer } else { SendMode::Queue };
                        s1.run(SendBody { text: String::new(), images: vec![], mode, send_at: None });
                    }/>
                <div class="cbox-bar">
                    <attach::AttachButton images=images/>
                    {crate::plugins::composer_actions(cx.clone())}
                    <span class="grow pk-dim">{move || summary.get().map(|s| if s.queued > 0 { format!("{} queued", s.queued) } else { String::new() })}</span>
                    <ModelChip machine=machine agent=agent model=model effort=effort lock_agent=true up=true/>
                    <button class="send-btn" title=move || if busy() { "Queue (⏎)" } else { "Send (⏎)" } disabled=move || text.get().trim().is_empty() && images.with(|v| v.is_empty()) on:click=move |_| send.run(SendBody::default())>
                        <Icon name="arrow-up" size=16/>
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Answer a question or a permission prompt.
pub(crate) fn answer(thread: String, ask: String, option: String, text: Option<String>) {
    spawn_local(async move {
        if let Err(e) = api_post::<Value>(&format!("/code/threads/{thread}/answer"), &AnswerBody { ask, option, text }).await {
            toast_bad(e.message);
        }
    });
}
