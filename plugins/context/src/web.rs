//! Context's web part: a small gauge under the thread's header (tokens in the context window), and
//! at 80 % or more a Compact button (Claude Code's `/compact`, sent like any message).
use agentum_plugin::web::{ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::SendBody;
use agentum_proto::AgentKind;
use agentum_ui::fmt::tokens;
use agentum_ui::prelude::*;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

/// From here on the gauge turns warm and offers to compact.
const FULL: f64 = 80.0;

#[derive(Clone, PartialEq)]
struct Ctx {
    used: u64,
    window: u64,
    can_compact: bool,
    busy: bool,
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }
    fn thread_header(&self, cx: ThreadCx) -> Option<AnyView> {
        Some(view! { <Gauge cx=cx/> }.into_any())
    }
}

#[component]
fn Gauge(cx: ThreadCx) -> impl IntoView {
    let info = Memo::new(move |_| {
        cx.summary.with(|s| {
            let s = s.as_ref()?;
            let u = s.usage.as_ref()?;
            let window = u.context_window.filter(|w| *w > 0)?;
            Some(Ctx { used: u.context_used?, window, can_compact: s.agent == AgentKind::ClaudeCode, busy: s.state.busy() })
        })
    });
    let sending = RwSignal::new(false);
    let id = cx.id.clone();
    let compact = move |_| {
        let id = id.clone();
        let queued = info.get_untracked().is_some_and(|i| i.busy);
        sending.set(true);
        spawn_local(async move {
            let body = SendBody { text: "/compact".into(), ..Default::default() };
            match api_post::<Value>(&format!("/code/threads/{id}/send"), &body).await {
                Ok(_) => toast(if queued { "Compact queued after this turn." } else { "Compacting the context…" }),
                Err(e) => toast_bad(e.message),
            }
            sending.set(false);
        });
    };
    move || {
        info.get().map(|i| {
            let pct = (i.used as f64 / i.window as f64 * 100.0).clamp(0.0, 100.0);
            let tone = if pct >= 90.0 { "bad" } else if pct >= FULL { "warn" } else { "ok" };
            let title = format!("Context: {} of {} tokens ({pct:.0}%)", i.used, i.window);
            view! {
                <div class=format!("ctx-gauge {tone}") title=title>
                    <span class="ctx-bar" role="meter" aria-label="Context used" aria-valuemin="0" aria-valuemax="100" aria-valuenow=format!("{pct:.0}")>
                        <i style=format!("width:{pct:.1}%")/>
                    </span>
                    <span class="ctx-num">{format!("{} / {} · {pct:.0}%", tokens(i.used), tokens(i.window))}</span>
                    {(pct >= FULL && i.can_compact).then(|| {
                        let compact = compact.clone();
                        view! {
                            <button class="btn small ctx-compact" disabled=move || sending.get() on:click=compact
                                title="Summarise the conversation so far to free the context (/compact)">
                                <Icon name="broom" size=12/>"Compact"
                            </button>
                        }
                    })}
                </div>
            }
        })
    }
}
