//! Sticky prompt's web part: your last prompt in a strip at the top of the thread, one line or
//! unfolded; a click scrolls the timeline to it (`#ev-<seq>`).
use crate::logic::Prompt;
use agentum_plugin::web::{ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_proto::EventKind;
use agentum_ui::prelude::*;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const ID: &str = "sticky-prompt";
const OPEN: &str = "agentum.sticky.open";

fn scroll_to(seq: u64) {
    let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(&format!("ev-{seq}"))) else {
        toast("It's further up: load the earlier messages to reach it.");
        return;
    };
    let o = web_sys::ScrollIntoViewOptions::new();
    o.set_behavior(web_sys::ScrollBehavior::Smooth);
    o.set_block(web_sys::ScrollLogicalPosition::Start);
    el.scroll_into_view_with_scroll_into_view_options(&o);
    // Flash it again even if it flashed before.
    let cl = el.class_list();
    let _ = cl.remove_1("flash");
    let _ = el.get_bounding_client_rect();
    let _ = cl.add_1("flash");
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    fn thread_header(&self, cx: ThreadCx) -> Option<AnyView> {
        let id = cx.id.clone();
        let prompt = RwSignal::new(None::<Prompt>);
        let open = RwSignal::new(agentum_ui::store::flag(OPEN));
        {
            let id = id.clone();
            spawn_local(async move {
                if let Ok(p) = rpc::<Option<Prompt>>(ID, "last", &json!({ "thread": id })).await {
                    // One that arrived on the socket meanwhile is newer.
                    prompt.try_update(|cur| {
                        if cur.as_ref().is_none_or(|c| p.as_ref().is_some_and(|p| p.seq > c.seq)) {
                            *cur = p;
                        }
                    });
                }
            });
        }
        subscribe(&[&format!("thread:{id}")]);
        on_ws(move |m| {
            if let WsOut::Event { thread, event } = m {
                if let (true, EventKind::User { text, .. }) = (*thread == id, &event.event.kind) {
                    if !text.trim().is_empty() {
                        prompt.set(Some(Prompt { seq: event.event.seq, text: text.trim().to_string(), ts: event.event.ts }));
                    }
                }
            }
        });
        let long = move || prompt.with(|p| p.as_ref().is_some_and(|p| p.text.contains('\n') || p.text.chars().count() > 110));
        let toggle = move |_| {
            open.update(|o| *o = !*o);
            agentum_ui::store::set(OPEN, if open.get_untracked() { "1" } else { "" });
        };
        Some(
            view! {
                <Show when=move || prompt.with(|p| p.is_some())>
                    <div class=move || if open.get() && long() { "sticky-strip open" } else { "sticky-strip" }>
                        <button class="sticky-go" title="Go to your message" on:click=move |_| { if let Some(p) = prompt.get_untracked() { scroll_to(p.seq) } }>
                            <Icon name="reply" size=13/>
                            <span class="sticky-text">{move || prompt.with(|p| p.as_ref().map(|p| p.text.clone()).unwrap_or_default())}</span>
                        </button>
                        <Show when=long>
                            <button class="icon-btn sticky-fold" title=move || if open.get() { "Fold" } else { "Show it all" } on:click=toggle>
                                <Icon name="chevron" size=13 class=Signal::derive(move || if open.get() { "files-chev open".to_string() } else { "files-chev".to_string() })/>
                            </button>
                        </Show>
                    </div>
                </Show>
            }
            .into_any(),
        )
    }
}
