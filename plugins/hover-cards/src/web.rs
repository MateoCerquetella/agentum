//! Hover cards's web part: the card shown when hovering a thread in the sidebar. The summary
//! draws it at once; the hub's `card` adds since when and the agent's last message.
use crate::card::{state_line, Card};
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::AgentKind;
use agentum_ui::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

thread_local! {
    /// thread → (fetched at, its last_seq then, the card).
    static CACHE: RefCell<HashMap<String, (i64, u64, Card)>> = RefCell::new(HashMap::new());
}

fn agent_icon(a: AgentKind) -> &'static str {
    match a {
        AgentKind::ClaudeCode => "sparkle",
        AgentKind::Codex => "terminal",
        AgentKind::Opencode => "bot",
        AgentKind::Cursor => "cube",
    }
}

fn project_name(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    fn thread_hover(&self, t: &ThreadSummary) -> Option<AnyView> {
        let t = t.clone();
        let cached = CACHE.with(|c| c.borrow().get(&t.id).cloned());
        let fresh = cached.as_ref().is_some_and(|(at, seq, _)| *seq == t.last_seq && now_ms() - at < 20_000);
        let card = RwSignal::new(cached.map(|c| c.2));
        if !fresh {
            let (id, seq) = (t.id.clone(), t.last_seq);
            spawn_local(async move {
                if let Ok(c) = rpc::<Card>("hover-cards", "card", &json!({ "thread": id })).await {
                    CACHE.with(|m| m.borrow_mut().insert(id, (now_ms(), seq, c.clone())));
                    card.try_set(Some(c));
                }
            });
        }

        let title = if t.title.is_empty() { "New thread".to_string() } else { t.title.clone() };
        let mut place = vec![t.machine.clone(), project_name(&t.project)];
        place.extend(t.branch.clone());
        let mut who = vec![t.agent.label().to_string()];
        who.extend(t.model.clone());
        who.extend(t.effort.clone());
        let ctx = t.usage.as_ref().and_then(|u| Some((u.context_used?, u.context_window?))).filter(|(_, w)| *w > 0).map(|(u, w)| ((u as f64 / w as f64) * 100.0).round().min(100.0));
        let cost = (t.cost_usd > 0.0).then(|| if t.cost_usd < 0.1 { format!("${:.3}", t.cost_usd) } else { format!("${:.2}", t.cost_usd) });
        let (state, updated, preview) = (t.state, t.updated_at, t.preview.clone());
        let said = move || card.with(|c| c.as_ref().and_then(|c| c.said.clone())).or_else(|| (!preview.is_empty()).then(|| preview.clone()));
        let line = move || card.with(|c| state_line(state, c.as_ref().unwrap_or(&Card::default()), updated, now_ms()));
        let queued = t.queued;
        Some(
            view! {
                <div class="hc" role="tooltip">
                    <b class="hc-title">{title}</b>
                    <div class="hc-state">
                        {move || { let (cls, _) = line(); view! { <i class=format!("code-dot {cls}")/> } }}
                        <span>{move || line().1}</span>
                        {(queued > 0).then(|| view! { <span class="subdue">{format!("· {queued} queued")}</span> })}
                    </div>
                    {move || said().map(|s| view! { <p class="hover-said">{s}</p> })}
                    {t.asking.clone().map(|a| view! { <div class="hc-ask"><Icon name="alert" size=12/><span>{a}</span></div> })}
                    <div class="hover-facts">
                        <span><Icon name="folder" size=12/>{place.join(" · ")}</span>
                        <span><Icon name=agent_icon(t.agent) size=12/>{who.join(" · ")}</span>
                        {(cost.is_some() || ctx.is_some()).then(|| view! {
                            <span class="hc-meter">
                                {cost.clone().map(|c| view! { <Icon name="money" size=12/><span>{c}</span> })}
                                {ctx.map(|p| view! {
                                    <span class="ctx-bar"><i style=format!("width:{p}%;background:{}", if p >= 85.0 { "var(--negative)" } else if p >= 65.0 { "var(--warm)" } else { "var(--primary)" })/></span>
                                    <span>{format!("context {p:.0}%")}</span>
                                })}
                            </span>
                        })}
                    </div>
                </div>
            }
            .into_any(),
        )
    }
}
