//! The Code page: a new thread (machine, project, agent, model) or one thread's timeline with
//! its composer, questions and side panel (plugins' tabs: files, git, history, terminal…).
pub mod store;
mod timeline;

use crate::pages::Pane;
use agentum_plugin::web::{ComposerCx, ThreadCx};
use agentum_proto::api::{AnswerBody, EventView, Machine, Models, NewThread, Partial, PatchThread, SendBody, ThreadDetail, ThreadSummary, WsOut};
use agentum_proto::{AgentKind, EventKind, Permission, SendMode};
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

/// Which pane this Code page is in: the right one reads and writes "r_"-prefixed params.
#[derive(Clone, Copy)]
pub(crate) struct Here(pub bool);
pub(crate) fn key(k: &'static str) -> &'static str {
    let right = use_context::<Here>().is_some_and(|h| h.0);
    if !right {
        return k;
    }
    match k {
        "thread" => "r_thread",
        "machine" => "r_machine",
        "project" => "r_project",
        "at" => "r_at",
        _ => k,
    }
}
/// Open something in this Code pane.
pub fn page(pane: Pane) -> AnyView {
    provide_context(Here(pane == Pane::Right));
    let thread = use_param(key("thread"));
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
const PERMS: [Permission; 4] = [Permission::Ask, Permission::Edits, Permission::Plan, Permission::Full];

/// The models of an agent on a machine (cached per page).
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

/// Machine, agent, model, effort and permission pickers (new threads, and the thread header).
#[component]
fn Pickers(machine: RwSignal<String>, agent: RwSignal<AgentKind>, model: RwSignal<Option<String>>, effort: RwSignal<Option<String>>, permission: RwSignal<Permission>, #[prop(optional)] locked_machine: bool) -> impl IntoView {
    let ov = store::overview();
    let machines = move || ov.with(|o| o.as_ref().map(|o| o.machines.clone()).unwrap_or_default());
    let models = models_of(machine.into(), agent.into());
    let efforts = move || {
        let m = model.get();
        models.get().map(|v| v.iter().find(|x| Some(&x.id) == m.as_ref()).or_else(|| v.iter().find(|x| x.default)).map(|x| x.efforts.clone()).unwrap_or_default()).unwrap_or_default()
    };
    let installed = move |a: AgentKind| machines().iter().find(|m| m.id == machine.get()).is_none_or(|m| m.agents.iter().any(|x| x.agent == a && x.installed));
    view! {
        <div class="row wrap pickers" style="gap:6px">
            <select class="pick" disabled=locked_machine title="Machine" on:change=move |e| machine.set(event_target_value(&e))>
                {move || machines().into_iter().map(|m: Machine| {
                    let id = m.id.clone();
                    let sel = machine.get() == id;
                    view! { <option value=id.clone() selected=sel disabled=!m.online>{format!("{} {}", if m.online { "●" } else { "○" }, m.name)}{(!m.online).then_some(" (offline)")}</option> }
                }).collect_view()}
            </select>
            <select class="pick" title="Agent" on:change=move |e| { if let Some(a) = AgentKind::parse(&event_target_value(&e)) { agent.set(a); model.set(None); effort.set(None) } }>
                {move || AgentKind::ALL.into_iter().map(|a| view! { <option value=a.id() selected=agent.get() == a disabled=!installed(a)>{a.label()}</option> }).collect_view()}
            </select>
            <select class="pick" title="Model" on:change=move |e| { let v = event_target_value(&e); model.set((!v.is_empty()).then_some(v)); effort.set(None) }>
                <option value="" selected=move || model.get().is_none()>"Default model"</option>
                {move || models.get().unwrap_or_default().into_iter().map(|m| { let id = m.id.clone(); let sel = model.get().as_deref() == Some(id.as_str()); view! { <option value=id selected=sel>{m.label}</option> } }).collect_view()}
            </select>
            <Show when=move || !efforts().is_empty()>
                <select class="pick" title="Reasoning effort" on:change=move |e| { let v = event_target_value(&e); effort.set((!v.is_empty()).then_some(v)) }>
                    <option value="" selected=move || effort.get().is_none()>"Default effort"</option>
                    {move || efforts().into_iter().map(|x| { let sel = effort.get().as_deref() == Some(x.as_str()); view! { <option value=x.clone() selected=sel>{x.clone()}</option> } }).collect_view()}
                </select>
            </Show>
            <select class="pick" title="What the agent may do without asking" on:change=move |e| { let v = event_target_value(&e); if let Some(p) = PERMS.iter().find(|p| serde_json::to_value(p).ok().and_then(|j| j.as_str().map(str::to_string)).as_deref() == Some(v.as_str())) { permission.set(*p) } }>
                {PERMS.into_iter().map(|p| { let v = serde_json::to_value(p).unwrap().as_str().unwrap().to_string(); view! { <option value=v selected=move || permission.get() == p>{perm_label(p)}</option> } }).collect_view()}
            </select>
        </div>
    }
}

#[component]
fn NewThreadPage() -> impl IntoView {
    let ov = store::overview();
    let p_machine = use_param(key("machine"));
    let p_project = use_param(key("project"));
    let machine = RwSignal::new(String::new());
    let project = RwSignal::new(String::new());
    let agent = RwSignal::new(AgentKind::ClaudeCode);
    let model = RwSignal::new(None::<String>);
    let effort = RwSignal::new(None::<String>);
    let permission = RwSignal::new(Permission::Ask);
    let worktree = RwSignal::new(false);
    let branch = RwSignal::new(String::new());
    let text = RwSignal::new(agentum_ui::store::get("agentum.draft.new").unwrap_or_default());
    let busy = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let here = use_context::<Here>().is_some_and(|h| h.0);
    // Defaults: the URL's machine, else the main one, else the first online; its first project.
    Effect::new(move |_| {
        ov.with(|o| {
            let Some(o) = o else { return };
            if machine.get_untracked().is_empty() || !o.machines.iter().any(|m| m.id == machine.get_untracked()) {
                let want = p_machine.get();
                let m = o.machines.iter().find(|m| Some(&m.id) == want.as_ref()).or_else(|| o.machines.iter().find(|m| m.main && m.online)).or_else(|| o.machines.iter().find(|m| m.online)).or(o.machines.first());
                if let Some(m) = m {
                    machine.set(m.id.clone());
                }
            }
        })
    });
    Effect::new(move |_| {
        let m = machine.get();
        let want = p_project.get();
        let projects = ov.with(|o| o.as_ref().and_then(|o| o.machines.iter().find(|x| x.id == m).map(|x| x.projects.clone())).unwrap_or_default());
        if let Some(w) = want.filter(|w| projects.iter().any(|p| &p.path == w)) {
            project.set(w);
        } else if !projects.iter().any(|p| p.path == project.get_untracked()) {
            // The project last used on this machine, else the first.
            let last = agentum_ui::store::get(&format!("agentum.project.{m}"));
            project.set(last.filter(|l| projects.iter().any(|p| &p.path == l)).or_else(|| projects.first().map(|p| p.path.clone())).unwrap_or_default());
        }
    });
    let projects = move || {
        let m = machine.get();
        let f = filter.get().to_lowercase();
        ov.with(|o| o.as_ref().and_then(|o| o.machines.iter().find(|x| x.id == m).map(|x| x.projects.clone())).unwrap_or_default())
            .into_iter()
            .filter(|p| f.is_empty() || p.name.to_lowercase().contains(&f) || p.path.to_lowercase().contains(&f))
            .collect::<Vec<_>>()
    };
    let start = move || {
        let t = text.get_untracked();
        if t.trim().is_empty() || project.get_untracked().is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        let body = NewThread {
            machine: machine.get_untracked(),
            project: project.get_untracked(),
            agent: Some(agent.get_untracked()),
            model: model.get_untracked(),
            effort: effort.get_untracked(),
            permission: permission.get_untracked(),
            worktree: worktree.get_untracked().then(|| branch.get_untracked()).filter(|b| !b.trim().is_empty()),
            text: t,
            images: vec![],
            parent: None,
        };
        agentum_ui::store::set(&format!("agentum.project.{}", body.machine), &body.project);
        spawn_local(async move {
            match api_post::<ThreadSummary>("/code/threads", &body).await {
                Ok(t) => {
                    agentum_ui::store::remove("agentum.draft.new");
                    text.set(String::new());
                    navigate("code", if here { Where::Right } else { Where::Left }, &[("thread", &t.id)]);
                }
                Err(e) => toast_bad(e.message),
            }
            busy.set(false);
        });
    };
    let add_project = move |_| {
        let path = filter.get_untracked();
        if !path.starts_with('/') && !path.starts_with('~') {
            toast_bad("Type the project's full path in the filter, then Add.");
            return;
        }
        let m = machine.get_untracked();
        spawn_local(async move {
            match api_post::<Value>(&format!("/code/machines/{m}/projects"), &json!({"path": path})).await {
                Ok(_) => {
                    toast("Added.");
                    filter.set(String::new());
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        <div class="code-new col" style="max-width:860px;margin:0 auto;padding:16px;gap:12px;width:100%">
            <h2 style="margin:0">"New thread"</h2>
            <Pickers machine=machine agent=agent model=model effort=effort permission=permission/>
            <div class="card col" style="gap:6px;padding:10px">
                <div class="row" style="gap:6px">
                    <Icon name="folder"/>
                    <b class="grow truncate">{move || if project.get().is_empty() { "Pick a project".to_string() } else { project.get() }}</b>
                    <input class="dock-find" style="max-width:260px" placeholder="Filter, or a path to add" prop:value=filter on:input=move |e| filter.set(event_target_value(&e))/>
                    <Show when=move || filter.get().starts_with('/') || filter.get().starts_with('~')><button class="btn" on:click=add_project>"Add"</button></Show>
                </div>
                <div class="projects" style="max-height:180px;overflow:auto;display:flex;flex-wrap:wrap;gap:4px">
                    {move || projects().into_iter().take(80).map(|p| { let path = p.path.clone(); let on = project.get() == path; view! {
                        <button class=if on { "chip on" } else { "chip" } title=p.path.clone() on:click=move |_| project.set(path.clone())>
                            {p.name.clone()}{p.branch.map(|b| view! { <span class="subdue">" · "{b}</span> })}
                        </button>
                    }}).collect_view()}
                </div>
                <label class="row small" style="gap:6px">
                    <input type="checkbox" prop:checked=worktree on:change=move |e| worktree.set(event_target_checked(&e))/>
                    "In a new worktree, on branch"
                    <input class="dock-find" style="max-width:220px" placeholder="agentum/my-change" prop:value=branch disabled=move || !worktree.get() on:input=move |e| branch.set(event_target_value(&e))/>
                </label>
            </div>
            <textarea class="composer-text" rows="6" placeholder="What should it do? (⏎ to start, ⇧⏎ for a new line)" prop:value=text autofocus
                on:input=move |e| { let v = event_target_value(&e); agentum_ui::store::set("agentum.draft.new", &v); text.set(v) }
                on:keydown=move |e| if e.key() == "Enter" && !e.shift_key() { e.prevent_default(); start() }/>
            <div class="row" style="justify-content:flex-end;gap:8px">
                <button class="btn primary" disabled=move || busy.get() || text.get().trim().is_empty() || project.get().is_empty() on:click=move |_| start()>
                    {move || if busy.get() { "Starting…" } else { "Start" }}
                </button>
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
    let at = use_param(key("at"));

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

    let cx = ThreadCx { id: id.clone(), summary };
    let tabs = crate::plugins::rail_tabs();
    let has_tabs = !tabs.is_empty();
    view! {
        <div class="code-thread" style="display:flex;height:100%;min-height:0">
            <div class="col" style="flex:1;min-width:0;min-height:0">
                <Header id=id.clone() summary=summary rail=rail has_tabs=has_tabs/>
                {crate::plugins::thread_header(cx.clone())}
                <timeline::Timeline events=events partial=partial more_before=more_before loaded=loaded summary=summary at=at on_older=Callback::new(move |_| load_older())/>
                <Composer id=id.clone() summary=summary/>
            </div>
            {move || rail.get().and_then(|r| tabs.iter().find(|t| t.id == r).copied()).map(|t| view! {
                <aside class="code-rail" style="width:min(46%,560px);border-left:1px solid var(--border);display:flex;flex-direction:column;min-height:0">
                    <div class="row" style="padding:6px 8px;gap:6px;border-bottom:1px solid var(--border)">
                        <Icon name=t.icon/><b class="grow">{t.title}</b>
                        <button class="icon-btn" title="Close" on:click=move |_| { rail.set(None); agentum_ui::store::set("agentum.rail", "") }><Icon name="x"/></button>
                    </div>
                    <div style="flex:1;min-height:0;overflow:auto">{(t.view)(cx.clone())}</div>
                </aside>
            })}
        </div>
    }
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

#[component]
fn Header(id: String, summary: Signal<Option<ThreadSummary>>, rail: RwSignal<Option<String>>, has_tabs: bool) -> impl IntoView {
    let here = use_context::<Here>().is_some_and(|h| h.0);
    let editing = RwSignal::new(false);
    let title = RwSignal::new(String::new());
    let patch = {
        let id = id.clone();
        move |p: PatchThread| {
            let id = id.clone();
            spawn_local(async move {
                if let Err(e) = api::<Value>(&format!("/code/threads/{id}"), "PATCH", Some(&serde_json::to_value(p).unwrap())).await {
                    toast_bad(e.message);
                }
            });
        }
    };
    let stop = {
        let id = id.clone();
        move |_| {
            let id = id.clone();
            spawn_local(async move {
                if let Err(e) = api_post::<Value>(&format!("/code/threads/{id}/stop"), &json!({})).await {
                    toast_bad(e.message);
                }
            });
        }
    };
    let tabs = crate::plugins::rail_tabs();
    let p2 = patch.clone();
    let p3 = patch.clone();
    let p4 = patch.clone();
    view! {
        <div class="code-head row" style="gap:8px;padding:8px 12px;border-bottom:1px solid var(--border)">
            {move || summary.get().map(|s| view! { <Icon name=agent_icon(s.agent) title=s.agent.label().to_string()/> })}
            {move || if editing.get() {
                let p = p2.clone();
                view! {
                    <input class="grow" autofocus prop:value=title on:input=move |e| title.set(event_target_value(&e))
                        on:keydown=move |e| match e.key().as_str() {
                            "Enter" => { p(PatchThread { title: Some(title.get_untracked()), ..Default::default() }); editing.set(false) }
                            "Escape" => editing.set(false),
                            _ => {}
                        }/>
                }.into_any()
            } else {
                view! {
                    <b class="grow truncate" title="Click to rename" on:click=move |_| { title.set(summary.get_untracked().map(|s| s.title).unwrap_or_default()); editing.set(true) }>
                        {move || summary.get().map(|s| if s.title.is_empty() { "New thread".to_string() } else { s.title }).unwrap_or_default()}
                    </b>
                }.into_any()
            }}
            {move || summary.get().map(|s| view! {
                <span class="small subdue truncate" style="max-width:40%" title=s.cwd.clone()>
                    {format!("{} · {}{}", s.machine, s.cwd.rsplit('/').next().unwrap_or(""), s.branch.as_ref().map(|b| format!(" · {b}")).unwrap_or_default())}
                    {s.model.as_ref().map(|m| format!(" · {m}"))}
                </span>
            })}
            <Show when=move || summary.get().is_some_and(|s| s.state.busy())>
                <button class="btn danger small" title="Stop the turn (Esc)" on:click=stop.clone()><Icon name="stop" size=13/>" Stop"</button>
            </Show>
            {tabs.into_iter().map(|t| view! {
                <button class=move || if rail.get().as_deref() == Some(t.id) { "icon-btn on" } else { "icon-btn" } title=t.title
                    on:click=move |_| { let next = if rail.get_untracked().as_deref() == Some(t.id) { None } else { Some(t.id.to_string()) }; agentum_ui::store::set("agentum.rail", next.as_deref().unwrap_or("")); rail.set(next) }>
                    <Icon name=t.icon/>
                </button>
            }).collect_view()}
            {(!has_tabs).then_some(())}
            <button class="icon-btn" title=move || if summary.get().is_some_and(|s| s.pinned) { "Unpin" } else { "Pin" }
                on:click=move |_| { let pinned = summary.get_untracked().is_some_and(|s| s.pinned); p3(PatchThread { pinned: Some(!pinned), ..Default::default() }) }><Icon name="pin"/></button>
            <button class="icon-btn" title="Archive" on:click=move |_| { p4(PatchThread { archived: Some(true), ..Default::default() }); navigate("code", if here { Where::Right } else { Where::Left }, &[("thread", "")]) }><Icon name="archive"/></button>
        </div>
    }
}

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
    let stop = {
        let id = id.clone();
        move || {
            let id = id.clone();
            spawn_local(async move {
                let _ = api_post::<Value>(&format!("/code/threads/{id}/stop"), &json!({})).await;
            });
        }
    };
    let s1 = send;
    view! {
        <div class="code-composer col" style="gap:6px;padding:8px 12px;border-top:1px solid var(--border)">
            <textarea class="composer-text" rows="3" placeholder=move || if busy() { "Queue a message (⏎), steer the turn (⌥⏎), or stop and send (⌘⇧⏎)" } else { "Reply (⏎ to send, ⇧⏎ for a new line)" }
                prop:value=text
                on:input=move |e| { let v = event_target_value(&e); agentum_ui::store::set(&key, &v); text.set(v) }
                on:keydown=move |e| {
                    if e.key() == "Escape" && busy() { stop(); return; }
                    if e.key() != "Enter" || e.shift_key() { return; }
                    e.prevent_default();
                    let mode = if (e.meta_key() || e.ctrl_key()) && e.shift_key() { SendMode::Interrupt } else if e.alt_key() { SendMode::Steer } else { SendMode::Queue };
                    s1.run(SendBody { text: String::new(), images: vec![], mode, send_at: None });
                }/>
            <div class="row" style="gap:6px">
                {crate::plugins::composer_actions(cx.clone())}
                <span class="grow small subdue">{move || summary.get().map(|s| if s.queued > 0 { format!("{} queued", s.queued) } else { String::new() })}</span>
                <Show when=busy>
                    <button class="btn" title="Into the running turn (⌥⏎)" on:click=move |_| send.run(SendBody { mode: SendMode::Steer, ..Default::default() })>"Steer"</button>
                </Show>
                <button class="btn primary" disabled=move || text.get().trim().is_empty() on:click=move |_| send.run(SendBody::default())>
                    {move || if busy() { "Queue" } else { "Send" }}
                </button>
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
