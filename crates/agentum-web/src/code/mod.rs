//! The Code page: a new thread (machine, project, agent, model) or one thread's timeline with
//! its composer, questions and side panel (plugins' tabs: files, git, history, terminal…).
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

/// BB's model picker: agents as groups, their models, then the reasoning effort. One chip
/// ("Claude Code · Opus · high").
#[component]
fn ModelChip(machine: Signal<String>, agent: RwSignal<AgentKind>, model: RwSignal<Option<String>>, effort: RwSignal<Option<String>>, #[prop(optional)] lock_agent: bool, #[prop(optional)] up: bool) -> impl IntoView {
    let ov = store::overview();
    let models = models_of(machine, agent.into());
    let installed = move |a: AgentKind| ov.with(|o| o.as_ref().and_then(|o| o.machines.iter().find(|m| m.id == machine.get())).is_none_or(|m| m.agents.iter().any(|x| x.agent == a && x.installed)));
    let model_label = move || {
        let m = model.get();
        match m {
            None => "Default".to_string(),
            Some(id) => models.get().and_then(|v| v.iter().find(|x| x.id == id).map(|x| x.label.clone())).unwrap_or(id),
        }
    };
    let efforts = move || {
        let m = model.get();
        models.get().map(|v| v.iter().find(|x| Some(&x.id) == m.as_ref()).or_else(|| v.iter().find(|x| x.default)).map(|x| x.efforts.clone()).unwrap_or_default()).unwrap_or_default()
    };
    let items = Signal::derive(move || {
        let mut v: Vec<PickItem> = vec![];
        if !lock_agent {
            for a in AgentKind::ALL {
                v.push(PickItem { id: format!("agent:{}", a.id()), label: a.label().into(), icon: agent_icon(a), group: "Agent".into(), disabled: !installed(a), on: agent.get() == a, sub: if installed(a) { String::new() } else { "not installed".into() } });
            }
        }
        let g = format!("{} models", agent.get().label());
        v.push(PickItem { id: "model:".into(), label: "Default".into(), group: g.clone(), on: model.get().is_none(), ..Default::default() });
        for m in models.get().unwrap_or_default() {
            let on = model.get().as_deref() == Some(m.id.as_str());
            v.push(PickItem { id: format!("model:{}", m.id), label: m.label, group: g.clone(), on, sub: m.context_window.map(|c| format!("{}k", c / 1000)).unwrap_or_default(), ..Default::default() });
        }
        for e in efforts() {
            let on = effort.get().as_deref() == Some(e.as_str());
            v.push(PickItem { id: format!("effort:{e}"), label: e.clone(), group: "Reasoning".into(), on, ..Default::default() });
        }
        v
    });
    let pick = Callback::new(move |id: String| {
        if let Some(a) = id.strip_prefix("agent:").and_then(AgentKind::parse) {
            agent.set(a);
            model.set(None);
            effort.set(None);
        } else if let Some(m) = id.strip_prefix("model:") {
            model.set((!m.is_empty()).then(|| m.to_string()));
            effort.set(None);
        } else if let Some(e) = id.strip_prefix("effort:") {
            effort.set(Some(e.to_string()));
        }
    });
    view! {
        <Picker items=items on_pick=pick search=true up=up class="pk-model" title="Agent, model and reasoning" placeholder="Search models">
            <Icon name=Signal::derive(move || agent_icon(agent.get()).to_string()) size=13/>
            <span>{move || agent.get().label()}</span>
            <span class="pk-dim">{move || format!("· {}", model_label())}</span>
            {move || effort.get().map(|e| view! { <span class="pk-dim">{format!("· {e}")}</span> })}
        </Picker>
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
    let permission = RwSignal::new(agentum_ui::store::get("agentum.perm").and_then(|p| PERMS.into_iter().find(|x| perm_id(*x) == p)).unwrap_or(Permission::Ask));
    let worktree = RwSignal::new(agentum_ui::store::flag("agentum.worktree"));
    let text = RwSignal::new(agentum_ui::store::get("agentum.draft.new").unwrap_or_default());
    let busy = RwSignal::new(false);
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
        agentum_ui::store::set("agentum.perm", &perm_id(permission.get()));
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
    let project_items = Signal::derive(move || {
        let cur = project.get();
        let mut v: Vec<PickItem> = projects_of(&machine.get()).into_iter().map(|p| PickItem { on: p.path == cur, id: p.path.clone(), label: p.name, sub: p.branch.unwrap_or_default(), icon: "folder", ..Default::default() }).collect();
        v.sort_by(|a, b| b.on.cmp(&a.on).then(a.label.to_lowercase().cmp(&b.label.to_lowercase())));
        v
    });
    let machine_items = Signal::derive(move || {
        let cur = machine.get();
        let mut v: Vec<PickItem> = ov.with(|o| o.as_ref().map(|o| o.machines.clone()).unwrap_or_default()).into_iter().map(|m| PickItem {
            on: m.id == cur,
            id: format!("m:{}", m.id),
            label: m.name.clone(),
            sub: if m.online { format!("{} projects", m.projects.len()) } else { "offline".into() },
            icon: "server",
            group: "Machine".into(),
            disabled: !m.online,
        }).collect();
        v.push(PickItem { id: "w:local".into(), label: "In the project".into(), sub: "its checkout, as it is".into(), icon: "folder", group: "Where".into(), on: !worktree.get(), ..Default::default() });
        v.push(PickItem { id: "w:tree".into(), label: "New worktree".into(), sub: "its own branch".into(), icon: "branch", group: "Where".into(), on: worktree.get(), ..Default::default() });
        v
    });
    let perm_items = Signal::derive(move || PERMS.into_iter().map(|p| PickItem { id: perm_id(p), label: perm_label(p).into(), sub: perm_sub(p).into(), icon: "shield", on: permission.get() == p, ..Default::default() }).collect::<Vec<_>>());
    let start = move || {
        let t = text.get_untracked();
        if t.trim().is_empty() || project.get_untracked().is_empty() || busy.get_untracked() {
            if project.get_untracked().is_empty() {
                toast_bad("Pick a project first.");
            }
            return;
        }
        busy.set(true);
        // A worktree's branch, from the first words of the task.
        let branch = worktree.get_untracked().then(|| {
            let slug: String = t.split_whitespace().take(5).collect::<Vec<_>>().join("-").to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
            format!("agentum/{}-{}", slug.trim_matches('-'), (js_sys::Math::random() * 1296.0) as u32)
        });
        let body = NewThread { machine: machine.get_untracked(), project: project.get_untracked(), agent: Some(agent.get_untracked()), model: model.get_untracked(), effort: effort.get_untracked(), permission: permission.get_untracked(), worktree: branch, text: t, images: vec![], parent: None };
        agentum_ui::store::set(&format!("agentum.project.{}", body.machine), &body.project);
        agentum_ui::store::set("agentum.machine", &body.machine);
        spawn_local(async move {
            match api_post::<ThreadSummary>("/code/threads", &body).await {
                Ok(t) => {
                    agentum_ui::store::remove("agentum.draft.new");
                    text.set(String::new());
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
    let project_name = move || {
        let p = project.get();
        if p.is_empty() { "Pick a project".to_string() } else { p.trim_end_matches('/').rsplit('/').next().unwrap_or(&p).to_string() }
    };
    view! {
        <div class="compose-home">
            <div class="compose-col">
                <div class="cbox">
                    <textarea node_ref=area class="cbox-text" rows="3" placeholder="Ask anything. ⏎ to start, ⇧⏎ for a new line" prop:value=text
                        on:input=move |e| { let v = event_target_value(&e); agentum_ui::store::set("agentum.draft.new", &v); text.set(v) }
                        on:keydown=move |e| if e.key() == "Enter" && !e.shift_key() && !e.is_composing() { e.prevent_default(); start() }/>
                    <div class="cbox-bar">
                        <span class="grow"/>
                        <ModelChip machine=machine.into() agent=agent model=model effort=effort/>
                        <button class="send-btn" title="Start (⏎)" disabled=move || busy.get() || text.get().trim().is_empty() || project.get().is_empty() on:click=move |_| start()>
                            <Icon name="arrow-up" size=16/>
                        </button>
                    </div>
                </div>
                <div class="cbox-foot">
                    <Picker items=project_items on_pick=Callback::new(move |p: String| project.set(p)) search=true class="pk-quiet" title="Project" placeholder="Search projects"
                        footer=ViewFn::from(move || view! {
                            <input class="pk-search" placeholder="Add a project: its full path, ⏎" prop:value=adding on:input=move |e| adding.set(event_target_value(&e))
                                on:keydown=move |e| if e.key() == "Enter" { e.prevent_default(); e.stop_propagation(); add_project() }/>
                        })>
                        <Icon name="folder" size=13/><span>{project_name}</span>
                    </Picker>
                    <Picker items=machine_items class="pk-quiet" title="Machine, and where on it"
                        on_pick=Callback::new(move |id: String| {
                            if let Some(m) = id.strip_prefix("m:") { machine.set(m.to_string()) } else { worktree.set(id == "w:tree") }
                        })>
                        <Icon name="server" size=13/><span>{move || machine.get()}</span>
                        <span class="pk-dim">{move || if worktree.get() { "· new worktree" } else { "· local" }}</span>
                    </Picker>
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
    let send = {
        let id = id.clone();
        let key = key.clone();
        Callback::new(move |mut b: SendBody| {
            if b.text.trim().is_empty() {
                b.text = text.get_untracked();
            }
            if b.text.trim().is_empty() {
                return;
            }
            text.set(String::new());
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
            <div class="cbox">
                <textarea class="cbox-text" rows="2" placeholder=move || if busy() { "Queue a message (⏎) · steer the turn (⌥⏎) · stop and send (⌘⇧⏎)" } else { "Reply… ⏎ to send, ⇧⏎ for a new line" }
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
                    {crate::plugins::composer_actions(cx.clone())}
                    <span class="grow pk-dim">{move || summary.get().map(|s| if s.queued > 0 { format!("{} queued", s.queued) } else { String::new() })}</span>
                    <ModelChip machine=machine agent=agent model=model effort=effort lock_agent=true up=true/>
                    <button class="send-btn" title=move || if busy() { "Queue (⏎)" } else { "Send (⏎)" } disabled=move || text.get().trim().is_empty() on:click=move |_| send.run(SendBody::default())>
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
