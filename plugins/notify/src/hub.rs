//! Notifications' hub part: a push (and a toast in open pages) when a thread asks something or a
//! turn ends, outside quiet hours. `test` sends one now.
use agentum_plugin::hub::{Ctx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::ThreadSummary;
use agentum_proto::{AskKind, Event, EventKind, StopReason};
use chrono::Timelike;
use serde_json::{json, Value};

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

fn name(t: &ThreadSummary) -> String {
    if t.title.is_empty() { "A thread".into() } else { t.title.chars().take(80).collect() }
}

async fn quiet_now(cx: &Ctx) -> bool {
    let (from, to) = (cx.setting("quiet_from").await, cx.setting("quiet_to").await);
    let now = chrono::Local::now();
    super::quiet(now.hour() * 60 + now.minute(), from.as_str().and_then(super::hhmm), to.as_str().and_then(super::hhmm))
}

#[async_trait::async_trait]
impl HubPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    async fn on_event(&self, cx: &Ctx, t: &ThreadSummary, e: &Event) {
        let (title, body) = match &e.kind {
            EventKind::Ask { kind, title, .. } if cx.setting("on_ask").await != false => {
                let what = if *kind == AskKind::Permission { "asks permission" } else { "has a question" };
                (format!("{} {what}", name(t)), title.clone())
            }
            EventKind::TurnDone { stop, duration_ms, cost_usd, .. } if *stop != StopReason::Interrupted && cx.setting("on_done").await != false => {
                let head = if *stop == StopReason::End { "Done" } else { "Failed" };
                let mut facts = vec![];
                if *duration_ms >= 1000 {
                    facts.push(format!("{}s", duration_ms / 1000));
                }
                if let Some(c) = cost_usd.filter(|c| *c > 0.0) {
                    facts.push(if c < 0.1 { format!("${c:.3}") } else { format!("${c:.2}") });
                }
                let mut body: String = t.preview.chars().take(160).collect();
                if !facts.is_empty() {
                    body = if body.is_empty() { facts.join(" · ") } else { format!("{body} ({})", facts.join(" · ")) };
                }
                (format!("{head}: {}", name(t)), body)
            }
            _ => return,
        };
        if quiet_now(cx).await {
            return;
        }
        cx.notify(&title, &body, &format!("/code?thread={}", t.id)).await;
    }

    async fn rpc(&self, cx: &Ctx, method: &str, _input: Value) -> Result<Value, String> {
        match method {
            "test" => {
                cx.notify("Agentum", "Notifications work: this is how a thread will reach you.", "/code").await;
                Ok(json!({ "ok": true, "quiet": quiet_now(cx).await }))
            }
            _ => Err(format!("no method {method}")),
        }
    }
}
