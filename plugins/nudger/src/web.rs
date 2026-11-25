//! Nudger's web part: a chip at the top right of the thread. Click it to turn Nudger on or off
//! for the thread; it shows "nudged 2/5", the countdown (with "Don't"), and why it last did or
//! didn't nudge. The gear holds the settings (message, how many, how long to wait).
use agentum_plugin::web::{ThreadCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_ui::prelude::*;
use serde::Deserialize;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const ID: &str = "nudger";

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
struct Status {
    #[serde(default)]
    on: bool,
    #[serde(default)]
    count: u32,
    #[serde(default)]
    max: u32,
    #[serde(default)]
    delay: u64,
    #[serde(default)]
    pending_at: Option<i64>,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    nudged: bool,
}

fn call(method: &'static str, id: String, extra: Value, status: RwSignal<Option<Status>>) {
    spawn_local(async move {
        let mut body = json!({ "thread": id });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        match rpc::<Status>(ID, method, &body).await {
            Ok(s) => {
                status.try_set(Some(s));
            }
            Err(e) => toast_bad(e.message),
        }
    });
}

/// The settings, editable in place.
#[component]
fn SettingsPop(on_close: Callback<()>) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let max = RwSignal::new(String::new());
    let delay = RwSignal::new(String::new());
    spawn_local(async move {
        if let Ok(v) = api_get::<Value>("/plugins").await {
            if let Some(p) = v.as_array().into_iter().flatten().find(|p| p["id"] == ID) {
                let s = &p["settings"];
                text.try_set(s["text"].as_str().unwrap_or("Continue.").to_string());
                max.try_set(s["max"].as_f64().map(|n| n.to_string()).unwrap_or_else(|| "5".into()));
                delay.try_set(s["delay"].as_f64().map(|n| n.to_string()).unwrap_or_else(|| "30".into()));
            }
        }
    });
    let save = move |_| {
        let t = text.get_untracked().trim().to_string();
        let m = max.get_untracked().trim().parse::<f64>().ok().filter(|n| (1.0..=50.0).contains(n));
        let d = delay.get_untracked().trim().parse::<f64>().ok().filter(|n| (3.0..=600.0).contains(n));
        let (Some(m), Some(d)) = (m, d) else {
            toast_bad("Nudges: 1 to 50; wait: 3 to 600 seconds.");
            return;
        };
        let t = if t.is_empty() { "Continue.".to_string() } else { t };
        spawn_local(async move {
            match api_put::<Value>(&format!("/plugins/{ID}"), &json!({ "settings": { "text": t, "max": m.round(), "delay": d.round() } })).await {
                Ok(_) => {
                    toast("Nudger's settings saved.");
                    if text.try_get_untracked().is_some() {
                        on_close.run(());
                    }
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        <div class="nudger-back" on:click=move |_| on_close.run(())/>
        <div class="nudger-pop pick-pop" role="dialog" aria-label="Nudger's settings">
            <div class="pick-title">"Nudger, in every thread"</div>
            <label class="nudger-field"><span>"It sends"</span>
                <input prop:value=text on:input=move |e| text.set(event_target_value(&e)) placeholder="Continue."/></label>
            <label class="nudger-field"><span>"At most, per thread"</span>
                <input type="number" min="1" max="50" prop:value=max on:input=move |e| max.set(event_target_value(&e))/></label>
            <label class="nudger-field"><span>"After waiting (seconds)"</span>
                <input type="number" min="3" max="600" prop:value=delay on:input=move |e| delay.set(event_target_value(&e))/></label>
            <p class="bits-note">"It nudges when a turn ends halfway: open plan steps, “next I'll…”, a sentence cut off. Never when the agent asks you something."</p>
            <div class="pick-foot nudger-foot"><button class="btn primary small" on:click=save>"Save"</button></div>
        </div>
    }
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    fn thread_header(&self, cx: ThreadCx) -> Option<AnyView> {
        let id = cx.id.clone();
        let status = RwSignal::new(None::<Status>);
        let gear = RwSignal::new(false);
        let now = RwSignal::new(now_ms());
        call("get", id.clone(), json!({}), status);
        subscribe(&["plugin:nudger"]);
        {
            let id = id.clone();
            on_ws(move |m| {
                if let WsOut::Plugin { plugin, payload } = m {
                    if plugin != ID {
                        return;
                    }
                    if payload["thread"] == id.as_str() {
                        if let Ok(s) = serde_json::from_value::<Status>(payload.clone()) {
                            status.set(Some(s));
                        }
                    } else if payload["settings"] == true {
                        call("get", id.clone(), json!({}), status);
                    }
                }
            });
        }
        // A clock for the countdown, only while one runs.
        let tick = StoredValue::new_local(None::<gloo_timers::callback::Interval>);
        Effect::new(move |_| {
            let pending = status.with(|s| s.as_ref().and_then(|s| s.pending_at).is_some());
            now.set(now_ms());
            tick.set_value(pending.then(|| gloo_timers::callback::Interval::new(500, move || now.set(now_ms()))));
        });
        let left = move || status.with(|s| s.as_ref().and_then(|s| s.pending_at)).map(|at| ((at - now.get()) as f64 / 1000.0).ceil().max(0.0) as i64);
        let label = move || {
            let Some(s) = status.get() else { return String::new() };
            if !s.on {
                "Nudger".to_string()
            } else if let Some(l) = left() {
                format!("Nudging in {l}s")
            } else if s.count > 0 {
                format!("Nudged {}/{}", s.count, s.max)
            } else {
                "Nudger on".to_string()
            }
        };
        let class = move || {
            let s = status.get().unwrap_or_default();
            let state = if !s.on { "off" } else if s.pending_at.is_some() { "pending" } else if s.count >= s.max && s.max > 0 { "full" } else { "on" };
            format!("chip nudger-chip {state}")
        };
        let title = move || {
            let s = status.get().unwrap_or_default();
            let why = s.why.map(|w| format!(" Last time: {w}.")).unwrap_or_default();
            if s.on {
                format!("Nudger is on here: when a turn stops halfway it waits {}s, then sends “continue”. Nudged {} of {} times.{why} Click to turn it off.", s.delay, s.count, s.max)
            } else {
                format!("Nudger is off here. Click to keep this thread going: when a turn stops halfway, it sends “continue” (up to {} times).", s.max.max(1))
            }
        };
        let sid = StoredValue::new(id.clone());
        Some(
            view! {
                <Show when=move || status.with(|s| s.is_some())>
                    <div class="nudger-anchor">
                        <div class=move || if left().is_some() { "nudger-bar pending" } else { "nudger-bar" }>
                            <button class=class title=title on:click=move |_| {
                                let on = status.get_untracked().is_some_and(|s| s.on);
                                call("set", sid.get_value(), json!({ "on": !on }), status)
                            }>
                                <Icon name="refresh" size=12/><span>{label}</span>
                            </button>
                            <Show when=move || left().is_some()>
                                <button class="chip nudger-dont" title="Don't nudge this time" on:click=move |_| call("cancel", sid.get_value(), json!({}), status)>"Don't"</button>
                            </Show>
                            <button class="icon-btn nudger-gear" title="Nudger's settings" on:click=move |_| gear.update(|g| *g = !*g)><Icon name="settings" size=13/></button>
                            <Show when=move || gear.get()>
                                <SettingsPop on_close=Callback::new(move |_| gear.set(false))/>
                            </Show>
                        </div>
                    </div>
                </Show>
            }
            .into_any(),
        )
    }
}
