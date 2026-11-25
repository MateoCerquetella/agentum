//! Drafts's web part: what you type in a thread's composer is saved on the hub (800 ms after you
//! stop) and comes back on any device. The composer keeps its own local copy in localStorage
//! (`agentum.draft.<thread>`); this keeps when it was typed (`agentum.draft.at.<thread>`) so the
//! newer of the two wins when the composer opens.
use crate::logic::{take_hub, Draft};
use agentum_plugin::web::{ComposerCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_ui::prelude::*;
use agentum_ui::store;
use gloo_timers::callback::Timeout;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const ID: &str = "drafts";

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    fn composer_actions(&self, cx: ComposerCx) -> Option<AnyView> {
        let thread = cx.thread.clone()?;
        let text = cx.text;
        let tid = StoredValue::new(thread.clone());
        let at_key = move || format!("agentum.draft.at.{}", tid.get_value());
        let note = RwSignal::new("");
        // The next change of the text is ours (a restored draft): don't save it back.
        let skip = StoredValue::new(false);
        let timer = StoredValue::new_local(None::<Timeout>);
        let fade = StoredValue::new_local(None::<Timeout>);
        let show = move |s: &'static str| {
            if note.try_set(s).is_none() {
                fade.try_update_value(|f| *f = Some(Timeout::new(2500, move || { let _ = note.try_set(""); })));
            }
        };
        let save = move |v: String, at: i64| {
            let thread = tid.get_value();
            spawn_local(async move {
                let empty = v.trim().is_empty();
                match rpc::<Value>(ID, "set", &json!({ "thread": thread, "text": v, "at": at })).await {
                    Ok(r) if r["ok"] == true && !empty => show("Draft saved"),
                    Ok(_) => {}
                    Err(e) => {
                        if note.try_set("Draft not saved").is_none() {
                            leptos::logging::warn!("draft not saved: {}", e.message);
                        }
                    }
                }
            });
        };
        // Opening: the hub's draft if it is newer; else ours goes up if the hub has none.
        spawn_local(async move {
            let Ok(hub) = rpc::<Option<Draft>>(ID, "get", &json!({ "thread": tid.get_value() })).await else { return };
            let Some(local) = text.try_get_untracked() else { return };
            let local_at = store::get(&at_key()).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            if let Some(t) = take_hub(hub.as_ref(), &local, local_at) {
                skip.set_value(true);
                store::set(&format!("agentum.draft.{}", tid.get_value()), &t);
                store::set(&at_key(), &hub.map(|h| h.at).unwrap_or(0).to_string());
                text.set(t);
                show("Draft restored");
            } else if hub.is_none() && !local.trim().is_empty() {
                save(local, local_at.max(1));
            }
        });
        Effect::new(move |prev: Option<()>| {
            let v = text.get();
            if prev.is_none() {
                return;
            }
            if skip.get_value() {
                skip.set_value(false);
                return;
            }
            let at = now_ms();
            store::set(&at_key(), &at.to_string());
            timer.set_value(Some(Timeout::new(800, move || save(v, at))));
        });
        Some(view! { <span class=move || if note.get().is_empty() { "draft-note" } else { "draft-note on" } aria-live="polite">{move || note.get()}</span> }.into_any())
    }
}
