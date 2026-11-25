//! Message times's web part: the time under each message (12- or 24-hour, as set), and under a
//! turn's last message what the turn cost (from the hub's `costs`, again after each turn).
use crate::logic::{hm, money};
use agentum_plugin::web::WebPlugin;
use agentum_plugin::Manifest;
use agentum_proto::api::{EventView, WsOut};
use agentum_proto::EventKind;
use agentum_ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use wasm_bindgen::JsValue;

pub struct Plugin;
pub static PLUGIN: Plugin = Plugin;

const ID: &str = "timestamps";

thread_local! {
    static TWELVE: RwSignal<bool> = agentum_ui::global(false);
    /// thread → seq of a turn's last message → the turn's cost.
    static COSTS: RwSignal<HashMap<String, HashMap<u64, f64>>> = agentum_ui::global(HashMap::new());
    static ASKED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

fn load_settings() {
    spawn_local(async {
        if let Ok(v) = api_get::<Value>("/plugins").await {
            let mine = v.as_array().into_iter().flatten().find(|p| p["id"] == ID);
            let twelve = mine.is_some_and(|p| p["settings"]["clock"] == "12h");
            TWELVE.with(|s| s.set(twelve));
        }
    });
}

fn load_costs(thread: String) {
    ASKED.with(|a| a.borrow_mut().insert(thread.clone()));
    spawn_local(async move {
        if let Ok(m) = rpc::<HashMap<String, f64>>(ID, "costs", &json!({ "thread": thread })).await {
            let m: HashMap<u64, f64> = m.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v))).collect();
            COSTS.with(|c| c.update(|c| {
                c.insert(thread, m);
            }));
        }
    });
}

/// Once per page: the setting, and fresh costs whenever a watched thread finishes a turn.
fn start() {
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    load_settings();
    let owner = Owner::new_root(None);
    owner.with(|| {
        subscribe(&["plugin:timestamps"]);
        on_ws(|m| match m {
            WsOut::Plugin { plugin, .. } if plugin == ID => load_settings(),
            WsOut::Event { thread, event } if matches!(event.event.kind, EventKind::TurnDone { .. }) => {
                if ASKED.with(|a| a.borrow().contains(thread)) {
                    load_costs(thread.clone());
                }
            }
            _ => {}
        });
    });
    std::mem::forget(owner);
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

fn date(ts: i64) -> js_sys::Date {
    js_sys::Date::new(&JsValue::from_f64(ts as f64))
}

/// "14:05" today, "Nov 20 · 14:05" this year, "Nov 20, 2025 · 14:05" before.
fn stamp(ts: i64, twelve: bool) -> String {
    let (d, now) = (date(ts), js_sys::Date::new_0());
    let t = hm(d.get_hours(), d.get_minutes(), twelve);
    let mon = MONTHS[d.get_month() as usize % 12];
    if d.to_date_string() == now.to_date_string() {
        t
    } else if d.get_full_year() == now.get_full_year() {
        format!("{mon} {} · {t}", d.get_date())
    } else {
        format!("{mon} {}, {} · {t}", d.get_date(), d.get_full_year())
    }
}

/// "Tuesday, Nov 24 2026, 14:05:09".
fn full(ts: i64, twelve: bool) -> String {
    let d = date(ts);
    let t = hm(d.get_hours(), d.get_minutes(), twelve);
    let (clock, ampm) = t.split_once(' ').map(|(a, b)| (a.to_string(), format!(" {b}"))).unwrap_or((t.clone(), String::new()));
    format!("{}, {} {} {}, {clock}:{:02}{ampm}", DAYS[d.get_day() as usize % 7], MONTHS[d.get_month() as usize % 12], d.get_date(), d.get_full_year(), d.get_seconds())
}

impl WebPlugin for Plugin {
    fn manifest(&self) -> &'static Manifest {
        &super::MANIFEST
    }

    fn message_meta(&self, e: &EventView) -> Option<AnyView> {
        let agent = match &e.event.kind {
            EventKind::Message { .. } => true,
            EventKind::User { .. } => false,
            _ => return None,
        };
        start();
        let (seq, ts) = (e.event.seq, e.event.ts);
        let twelve = TWELVE.with(|s| *s);
        let costs = COSTS.with(|s| *s);
        // One thread page at a time: the URL's thread is the one this message is in.
        let thread = use_param("thread").get_untracked().unwrap_or_default();
        if agent && !thread.is_empty() && !ASKED.with(|a| a.borrow().contains(&thread)) {
            load_costs(thread.clone());
        }
        let cost = move || costs.with(|c| c.get(&thread).and_then(|m| m.get(&seq)).copied());
        Some(
            view! {
                <span class="ts-time" title=move || full(ts, twelve.get())>{move || stamp(ts, twelve.get())}</span>
                {move || cost().map(|c| view! { <span class="ts-cost" title="What this turn cost">{money(c)}</span> })}
            }
            .into_any(),
        )
    }
}
