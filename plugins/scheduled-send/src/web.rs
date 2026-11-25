//! Send later's web part: a clock button next to Send. It opens presets (in 10 minutes, in an
//! hour, tonight at 21:00, tomorrow at 9:00, or a date and time) and the thread's scheduled
//! messages, each with a cancel. The hub part keeps the queue and sends them.
use crate::logic::Scheduled;
use agentum_plugin::web::{ComposerCx, WebPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::WsOut;
use agentum_ui::prelude::*;
use wasm_bindgen::JsValue;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const ID: &str = "scheduled-send";
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn date(ms: i64) -> js_sys::Date {
    js_sys::Date::new(&JsValue::from_f64(ms as f64))
}

/// Today (or `plus` days on) at h:00, local time.
fn at_hour(plus: u32, h: u32) -> i64 {
    let d = js_sys::Date::new_0();
    d.set_date(d.get_date() + plus);
    d.set_hours(h);
    d.set_minutes(0);
    d.set_seconds(0);
    d.set_milliseconds(0);
    d.get_time() as i64
}

/// "21:00", "tomorrow 09:00", "Fri 09:00", "Nov 30 09:00".
fn when(ms: i64) -> String {
    let (d, now) = (date(ms), js_sys::Date::new_0());
    let hm = format!("{:02}:{:02}", d.get_hours(), d.get_minutes());
    let days = ((at_day(&d) - at_day(&now)) / 86_400_000.0).round() as i64;
    match days {
        0 => hm,
        1 => format!("tomorrow {hm}"),
        2..=6 => format!("{} {hm}", DAYS[d.get_day() as usize % 7]),
        _ => format!("{}/{} {hm}", d.get_month() + 1, d.get_date()),
    }
}
fn at_day(d: &js_sys::Date) -> f64 {
    let c = date(d.get_time() as i64);
    c.set_hours(0);
    c.set_minutes(0);
    c.set_seconds(0);
    c.set_milliseconds(0);
    c.get_time()
}

fn presets() -> Vec<(&'static str, i64)> {
    let now = now_ms();
    let mut v = vec![("In 10 minutes", now + 10 * 60_000), ("In 1 hour", now + 3_600_000)];
    let tonight = at_hour(0, 21);
    if tonight > now + 60_000 {
        v.push(("Tonight", tonight));
    }
    v.push(("Tomorrow morning", at_hour(1, 9)));
    v
}

/// `<input type=datetime-local>`'s value ("2026-11-24T21:30") as local milliseconds.
fn parse_local(v: &str) -> Option<i64> {
    let t = js_sys::Date::new(&JsValue::from_str(v)).get_time();
    (!t.is_nan()).then_some(t as i64)
}
fn local_value(ms: i64) -> String {
    let d = date(ms);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}", d.get_full_year(), d.get_month() + 1, d.get_date(), d.get_hours(), d.get_minutes())
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    fn composer_actions(&self, cx: ComposerCx) -> Option<AnyView> {
        let thread = cx.thread.clone()?;
        let text = cx.text;
        let open = RwSignal::new(false);
        let items = RwSignal::new(Vec::<Scheduled>::new());
        let custom = RwSignal::new(String::new());
        let tid = StoredValue::new(thread.clone());
        let load = move || {
            let thread = tid.get_value();
            spawn_local(async move {
                if let Ok(v) = rpc::<Vec<Scheduled>>(ID, "list", &json!({ "thread": thread })).await {
                    items.try_set(v);
                }
            });
        };
        load();
        subscribe(&["plugin:scheduled-send"]);
        on_ws(move |m| {
            if let WsOut::Plugin { plugin, payload } = m {
                if plugin == ID && payload["thread"] == tid.get_value().as_str() {
                    load();
                }
            }
        });
        let schedule = move |at: i64| {
            let body = text.get_untracked();
            if body.trim().is_empty() {
                toast_bad("Type a message first.");
                return;
            }
            let thread = tid.get_value();
            spawn_local(async move {
                match rpc::<Scheduled>(ID, "schedule", &json!({ "thread": thread, "text": body, "at": at })).await {
                    Ok(s) => {
                        // Out of the composer, like a send (and out of its local draft).
                        text.set(String::new());
                        agentum_ui::store::remove(&format!("agentum.draft.{thread}"));
                        open.set(false);
                        custom.set(String::new());
                        let w = when(s.at);
                        toast(if w.contains(' ') { format!("It will send {w}.") } else { format!("It will send at {w}.") });
                        load();
                    }
                    Err(e) => toast_bad(e.message),
                }
            });
        };
        let cancel = move |id: String| {
            spawn_local(async move {
                match rpc::<Value>(ID, "cancel", &json!({ "id": id })).await {
                    Ok(_) => toast("Cancelled."),
                    Err(e) => toast_bad(e.message),
                }
                load();
            });
        };
        let empty = move || text.with(|t| t.trim().is_empty());
        let custom_at = move || parse_local(&custom.get()).filter(|t| *t > now_ms() + 5_000);
        Some(
            view! {
                <div class="sched">
                    <button class=move || if open.get() { "btn sched-btn on" } else { "btn sched-btn" } aria-expanded=move || open.get().to_string()
                        title=move || { let n = items.with(|i| i.len()); if n > 0 { format!("Send later · {n} scheduled here") } else { "Send later".to_string() } }
                        on:click=move |_| open.update(|o| *o = !*o)>
                        <Icon name="clock" size=14/>
                        <Show when=move || items.with(|i| !i.is_empty())><span class="sched-n">{move || items.with(|i| i.len())}</span></Show>
                    </button>
                    <Show when=move || open.get()>
                        <div class="sched-back" on:click=move |_| open.set(false)/>
                        <div class="sched-pop pick-pop" role="dialog" aria-label="Send later">
                            <div class="pick-list pick-plain">
                                <div class="pick-title">"Send later"</div>
                                {presets().into_iter().map(|(label, at)| view! {
                                    <button class="pick-row" disabled=empty on:click=move |_| schedule(at)>
                                        <Icon name="clock" size=14/><span class="pick-label">{label}</span><span class="pick-sub">{when(at)}</span>
                                    </button>
                                }).collect_view()}
                                <div class="bits-row bits-custom">
                                    <input type="datetime-local" aria-label="Pick a date and time" min=local_value(now_ms()) prop:value=custom on:input=move |e| custom.set(event_target_value(&e))/>
                                    <button class="btn small" disabled=move || empty() || custom_at().is_none() on:click=move |_| { if let Some(at) = custom_at() { schedule(at) } }>"Schedule"</button>
                                </div>
                                <Show when=empty><p class="bits-note">"Type the message first, then pick when."</p></Show>
                                <Show when=move || items.with(|i| !i.is_empty())>
                                    <div class="pick-title">"Scheduled here"</div>
                                    <For each=move || items.get() key=|s| s.id.clone() let:s>
                                        {
                                            let id = s.id.clone();
                                            view! {
                                                <div class="sched-row" title=s.text.clone()>
                                                    <span class="sched-when">{when(s.at)}</span>
                                                    <span class="sched-text">{s.text.clone()}</span>
                                                    {s.error.clone().map(|e| view! { <span class="sched-err" title=format!("Last try failed: {e}. It tries again.")><Icon name="alert" size=12/></span> })}
                                                    <button class="icon-btn" title="Cancel it" on:click=move |_| cancel(id.clone())><Icon name="x" size=13/></button>
                                                </div>
                                            }
                                        }
                                    </For>
                                </Show>
                            </div>
                        </div>
                    </Show>
                </div>
            }
            .into_any(),
        )
    }
}
