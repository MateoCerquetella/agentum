//! Side chat's web part: the "Side chats" tab of a thread's side panel. It lists the thread's
//! side chats (live, from the "threads" channel), starts a new one, and opens each on the right.
use agentum_plugin::web::{RailTab, ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::{NewThread, ThreadSummary, WsOut};
use agentum_proto::ThreadState;
use agentum_ui::prelude::*;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const ID: &str = "side-chat";
static TABS: &[RailTab] = &[RailTab { id: "side-chats", title: "Side chats", icon: "chat", view: side_chats }];

fn open(id: &str) {
    navigate("code", Where::Right, &[("thread", id)]);
}

fn row(t: ThreadSummary) -> impl IntoView {
    let (cls, state) = match t.state {
        ThreadState::Starting | ThreadState::Running => ("working", "Working"),
        ThreadState::Waiting => ("asks", "Waiting on you"),
        ThreadState::Error => ("error", "Failed"),
        _ if t.unread => ("asks", "New answer"),
        _ => ("idle", "Idle"),
    };
    let said = if t.preview.is_empty() { "Nothing asked yet".to_string() } else { t.preview.clone() };
    let id = t.id.clone();
    view! {
        <button class="sc-row" title="Open on the right" on:click=move |_| open(&id)>
            <i class=format!("code-dot {cls}")/>
            <span class="sc-main">
                <span class="sc-said">{said}</span>
                <span class="sc-sub">{state}" · "{ago_ms(t.updated_at)}{(t.cost_usd > 0.0).then(|| format!(" · ${:.3}", t.cost_usd))}</span>
            </span>
        </button>
    }
}

fn side_chats(cx: ThreadCx) -> AnyView {
    let parent = cx.id.clone();
    let list = RwSignal::new(Vec::<ThreadSummary>::new());
    let loaded = RwSignal::new(false);
    let busy = RwSignal::new(false);
    {
        let parent = parent.clone();
        spawn_local(async move {
            match rpc::<Vec<ThreadSummary>>(ID, "list", &json!({ "thread": parent })).await {
                Ok(v) => {
                    list.try_set(v);
                }
                Err(e) => toast_bad(e.message),
            }
            loaded.try_set(true);
        });
    }
    subscribe(&["threads"]);
    {
        let parent = parent.clone();
        on_ws(move |m| match m {
            WsOut::Thread { summary } if summary.parent.as_deref() == Some(parent.as_str()) => list.update(|l| {
                l.retain(|t| t.id != summary.id);
                l.insert(0, summary.clone());
                l.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            }),
            WsOut::ThreadGone { id } => list.update(|l| l.retain(|t| &t.id != id)),
            _ => {}
        });
    }
    let start = move |_| {
        let Some(s) = cx.summary.get_untracked() else { return };
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let body = NewThread { machine: s.machine, project: s.project, agent: Some(s.agent), model: s.model, effort: s.effort, permission: s.permission, worktree: None, text: String::new(), images: vec![], parent: Some(s.id), ..Default::default() };
        spawn_local(async move {
            match api_post::<ThreadSummary>("/code/threads", &body).await {
                Ok(t) => open(&t.id),
                Err(e) => toast_bad(e.message),
            }
            busy.try_set(false);
        });
    };
    let open_ones = move || list.with(|l| l.iter().filter(|t| !t.archived).cloned().collect::<Vec<_>>());
    let archived = move || list.with(|l| l.iter().filter(|t| t.archived).cloned().collect::<Vec<_>>());
    view! {
        <div class="sc">
            <div class="sc-head">
                <button class="btn primary small" disabled=move || busy.get() on:click=start><Icon name="plus" size=13/>"New side chat"</button>
                <span class="sc-hint">"Ask about this thread without disturbing it."</span>
            </div>
            <div class="sc-list">{move || open_ones().into_iter().map(row).collect_view()}</div>
            <Show when=move || loaded.get() && open_ones().is_empty()>
                <p class="sc-empty">"No side chats yet. One starts with this thread's conversation, in the same folder, and doesn't change files unless you ask."</p>
            </Show>
            <Show when=move || !archived().is_empty()>
                <details class="sc-archived">
                    <summary>{move || format!("Archived ({})", archived().len())}</summary>
                    <div class="sc-list">{move || archived().into_iter().map(row).collect_view()}</div>
                </details>
            </Show>
            <p class="sc-note">"Side chats with nothing new for a day are archived."</p>
        </div>
    }
    .into_any()
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn rail_tabs(&self) -> &'static [RailTab] {
        TABS
    }
}
