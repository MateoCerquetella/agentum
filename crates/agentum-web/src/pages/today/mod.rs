//! Today, a planner: the ASCII sky, a strip of days, a sentence of what's left with the headline
//! and what matters, the assistant's cards, your day as one list of events and to-dos, the Inbox,
//! and tomorrow / sky / mail. Pick another day in the strip to plan it. Words in the brief's
//! language (`lang`).
mod cards;
mod hero;
mod i18n;
mod plan;
mod planner;
mod sky;

use super::Pane;
use agentum_proto::api::WsOut;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use plan::*;
use planner::{Ev, Filter};
use send_wrapper::SendWrapper;
use std::collections::BTreeMap;

/// What Today's parts share: the language and the calendar new events go to.
#[derive(Clone, Copy)]
pub struct Tx {
    lang: RwSignal<String>,
    cal: RwSignal<String>,
}
impl Tx {
    pub fn t(&self, s: &str) -> String {
        i18n::tr(&self.lang.get(), s)
    }
    pub fn lang(&self) -> String {
        self.lang.get()
    }
    /// Ask before a write: what / when / which calendar.
    pub async fn ask(&self, title: String, rows: Vec<(String, String)>, ok: String) -> bool {
        let body = rows.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(" · ");
        confirm(title, body, ok, false).await
    }
}

/// The agenda shows every event (now, next, busy, free): "what matters" keeps the rest.
const EVENT_KINDS: [&str; 4] = ["now", "next", "busy", "free"];

pub fn page(_pane: Pane) -> AnyView {
    view! { <Today/> }.into_any()
}

fn hidden() -> bool {
    web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden())
}

#[component]
fn Today() -> impl IntoView {
    let b = RwSignal::new(None::<Value>);
    let err = RwSignal::new(String::new());
    let assist = RwSignal::new(None::<Value>);
    let deliveries = RwSignal::new(None::<Value>);
    let busy = RwSignal::new(false);
    let occ = RwSignal::new(Vec::<Value>::new());
    let todos = RwSignal::new(Vec::<Todo>::new());
    let replies = RwSignal::new(Vec::<Value>::new());
    let filter = RwSignal::new(None::<Filter>);
    let tick = RwSignal::new(0u32);
    let lang = RwSignal::new("en".to_string());
    let c = Tx { lang, cal: RwSignal::new(String::new()) };
    Effect::new(move |_| {
        let l = b.with(|b| if b.as_ref().is_some_and(|b| b["lang"] == "es") { "es" } else { "en" });
        if lang.get_untracked() != l {
            lang.set(l.into());
        }
    });
    let today = Memo::new(move |_| {
        tick.track();
        today_str()
    });
    let want = use_param("day");
    let sel = Memo::new(move |_| want.get().filter(|d| is_ymd(d)).unwrap_or_else(|| today.get()));
    let start = RwSignal::new(add_days(&sel.get_untracked(), -1));
    let pick = Callback::new(move |d: String| {
        filter.set(None);
        set_param("day", if d == today.get_untracked() { None } else { Some(&d) });
        let s = start.get_untracked();
        if d < s || d > add_days(&s, 13) {
            start.set(add_days(&d, -1));
        }
    });
    let acts = TodoActs { todos, lang: lang.into() };

    let poll = StoredValue::new(None::<SendWrapper<gloo_timers::callback::Timeout>>);
    let load_assist = move |fresh: bool| {
        spawn_local(async move {
            if let Ok(a) = api_get::<Value>(if fresh { "/assist?fresh=1" } else { "/assist" }).await {
                let _ = assist.try_set(Some(a));
            }
            if let Ok(d) = api_get::<Value>("/deliveries").await {
                let _ = deliveries.try_set(Some(d));
            }
        });
    };
    // A pending brief (being written) is asked for again every 4 s.
    fn load(b: RwSignal<Option<Value>>, err: RwSignal<String>, busy: RwSignal<bool>, poll: StoredValue<Option<SendWrapper<gloo_timers::callback::Timeout>>>, refresh: bool, after: std::rc::Rc<dyn Fn(bool)>) {
        poll.set_value(None);
        busy.set(true);
        spawn_local(async move {
            let mut nb = if refresh { api_post::<Value>("/brief/refresh", &json!({})).await.ok() } else { None };
            if nb.as_ref().is_none_or(|x| x["day"].is_null() && x["pending"] != true) {
                nb = match api_get::<Value>("/brief").await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        let _ = err.try_set(e.message);
                        None
                    }
                };
            }
            if let Some(nb) = nb {
                let pending = nb["pending"] == true;
                let _ = b.try_set(Some(nb));
                let _ = err.try_set(String::new());
                if pending {
                    let _ = poll.try_set_value(Some(SendWrapper::new(gloo_timers::callback::Timeout::new(4000, move || load(b, err, busy, poll, false, std::rc::Rc::new(|_| {}))))));
                }
            }
            let _ = busy.try_set(false);
            after(refresh);
        });
    }
    let load_brief = move |refresh: bool| load(b, err, busy, poll, refresh, std::rc::Rc::new(load_assist));
    // The strip's two weeks of events, your to-dos and the mail waiting on you.
    let load_events = move || {
        let s = start.get_untracked();
        spawn_local(async move {
            if let Ok(r) = api_get::<Value>(&format!("/calendar/events?start={s}&end={}", add_days(&s, 15))).await {
                let _ = occ.try_set(r["events"].as_array().cloned().unwrap_or_default());
            }
        });
    };
    let load_todos = move || {
        spawn_local(async move {
            if let Ok(ns) = api_get::<Value>("/notes").await {
                let _ = todos.try_set(ns.as_array().into_iter().flatten().filter(|n| is_todo(n) && keep(n)).map(Todo::of).collect());
            }
        });
    };
    let load_replies = move || {
        spawn_local(async move {
            if let Ok(r) = api_get::<Value>("/mail/reply-zero").await {
                let _ = replies.try_set(r["to_reply"].as_array().cloned().unwrap_or_default());
            }
        });
    };
    Effect::new(move |_| {
        start.track();
        load_events();
    });
    // Calendar edits made elsewhere (phone, Apple Calendar) arrive with a pull.
    let pull = move |force: bool| {
        spawn_local(async move {
            if let Ok(r) = api_post::<Value>(if force { "/calendar/sync?force=1" } else { "/calendar/sync" }, &json!({})).await {
                if r["in_progress"] == true {
                    let _ = poll.try_set_value(Some(SendWrapper::new(gloo_timers::callback::Timeout::new(30_000, move || load_brief(false)))));
                } else {
                    load_brief(false);
                }
            }
        });
    };
    load_brief(false);
    pull(false);
    load_todos();
    load_replies();
    spawn_local(async move {
        if let Ok(cs) = api_get::<Value>("/calendar/calendars").await {
            let cs = cs.as_array().cloned().unwrap_or_default();
            let pick = cs.iter().find(|x| x["default"] == true && x["read_only"] != true).or_else(|| cs.iter().find(|x| x["read_only"] != true));
            if let Some(n) = pick.and_then(|x| x["name"].as_str()) {
                let _ = c.cal.try_set(n.to_string());
            }
        }
    });

    subscribe(&["app"]);
    let ev_timer = StoredValue::new(None::<SendWrapper<gloo_timers::callback::Timeout>>);
    on_ws(move |m| {
        let WsOut::App { payload } = m else { return };
        let k = payload["kind"].as_str().unwrap_or("");
        if ["today", "calendar", "mail", "todos"].contains(&k) {
            ev_timer.set_value(Some(SendWrapper::new(gloo_timers::callback::Timeout::new(800, move || load_brief(false)))));
        }
        match k {
            "todos" => load_todos(),
            "mail" => load_replies(),
            "calendar" => load_events(),
            _ => {}
        }
    });
    // Back to the app (a phone wakes it, the page sat open overnight): load again when you were
    // away a while, or the day or the part of the day changed.
    let seen = StoredValue::new(format!("{}{}", today_str(), evening()));
    let away = StoredValue::new(0.0f64);
    let fresh = move || {
        seen.set_value(format!("{}{}", today_str(), evening()));
        load_brief(false);
        load_todos();
        load_replies();
    };
    let stale = move || seen.get_value() != format!("{}{}", today_str(), evening());
    let vis = move || {
        if hidden() {
            away.set_value(js_sys::Date::now());
        } else if stale() || (away.get_value() > 0.0 && js_sys::Date::now() - away.get_value() > 120_000.0) {
            fresh();
        }
    };
    let h1 = window_event_listener(leptos::ev::visibilitychange, move |_| vis());
    let h2 = window_event_listener(leptos::ev::focus, move |_| vis());
    let h3 = window_event_listener(leptos::ev::pageshow, move |e| {
        if e.persisted() || stale() {
            fresh()
        }
    });
    let minute = SendWrapper::new(gloo_timers::callback::Interval::new(60_000, move || {
        tick.update(|n| *n += 1);
        if stale() {
            fresh();
        }
    }));
    on_cleanup(move || {
        h1.remove();
        h2.remove();
        h3.remove();
        drop(minute);
    });

    let refresh = move |_| {
        load_brief(true);
        pull(true);
        load_todos();
        load_replies();
    };
    let marks = Memo::new(move |_| {
        let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        occ.with(|o| {
            for x in o {
                for d in days_of(x) {
                    m.entry(d).or_default().0 += 1;
                }
            }
        });
        todos.with(|ts| {
            for t in ts.iter().filter(|t| !t.done) {
                if let Some(d) = due_of(&t.due) {
                    m.entry(d.day).or_default().1 += 1;
                }
            }
        });
        m
    });
    let hero = move || {
        view! {
            <hero::Hero b=b.into() c=c
                before=move || view! { <button on:click=move |_| navigate("mail", Where::Right, &[("tab", "reply")]) title=move || c.t("Open Mail") aria-label=move || c.t("Open Mail")><Icon name="mail" size=15/></button> }
                after=move || view! { <button on:click=refresh disabled=move || busy.get() title=move || c.t("Write it again") aria-label=move || c.t("Write it again")><Icon name="refresh" size=15 class=Signal::derive(move || if busy.get() { "spin".to_string() } else { String::new() })/></button> }/>
        }
    };
    let set_events = Callback::new(move |ev: Vec<Ev>| {
        let briefed = b.with_untracked(|b| b.as_ref().is_some_and(|b| b["day"].as_str() == Some(today.get_untracked().as_str())));
        if sel.get_untracked() == today.get_untracked() && briefed {
            b.update(|b| {
                if let Some(b) = b {
                    b["timed"] = Value::Array(
                        ev.iter()
                            .map(|e| {
                                let mut v = json!({"title": e.title, "calendar": e.calendar, "kind": e.kind, "location": e.location, "time": e.time, "end": e.end, "uid": e.uid, "series": e.series, "done": e.done});
                                if let Some((t, p)) = e.wx {
                                    v["wx"] = json!({"t": t, "p": p});
                                }
                                v
                            })
                            .collect(),
                    );
                }
            });
        } else {
            load_events();
        }
    });
    let on_calendar = Callback::new(move |_| {
        load_events();
        load_brief(false);
    });

    // Built once the brief is in; each part below redraws on its own (a whole-page redraw would
    // replay the rise animations and type the headline again).
    let loaded = Memo::new(move |_| b.with(Option::is_some));
    let is_t = Memo::new(move |_| sel.get() == today.get());
    let brief_today = Memo::new(move |_| b.with(|b| b.as_ref().and_then(|b| b["day"].as_str().map(str::to_string))) == Some(today.get()));
    // Today's events come from the brief (with the weather at each); other days from the calendar.
    let events = Memo::new(move |_| {
        let sl = sel.get();
        if is_t.get() && brief_today.get() {
            b.with(|b| b.as_ref().and_then(|b| b["timed"].as_array().map(|a| a.iter().map(Ev::of).collect())).unwrap_or_default())
        } else {
            occ.with(|o| o.iter().filter(|x| x["all_day"] != true && days_of(x).contains(&sl)).map(|x| Ev::of_occ(x, &sl)).collect::<Vec<_>>())
        }
    });
    let all_day = Memo::new(move |_| {
        let sl = sel.get();
        let title = |x: &Value| x["title"].as_str().unwrap_or("").to_string();
        if is_t.get() && brief_today.get() {
            b.with(|b| b.as_ref().and_then(|b| b["all_day"].as_array().map(|a| a.iter().map(title).collect())).unwrap_or_default())
        } else {
            occ.with(|o| o.iter().filter(|x| x["all_day"] == true && days_of(x).contains(&sl)).map(title).collect::<Vec<_>>())
        }
    });
    let text = Memo::new(move |_| b.with(|b| b.as_ref().and_then(|b| b["text"].as_str()).unwrap_or("").to_string()));
    let greet = move || {
        tick.track();
        let (td, sl, t) = (today.get(), sel.get(), is_t.get());
        let nm = now_min();
        let counts = b.with(|bv| {
            let bv = bv.as_ref().unwrap_or(&Value::Null);
            planner::Counts {
                ev: events.with(|e| e.iter().filter(|e| !e.done && (!t || mins(&e.end) > nm)).count()),
                todo: todos.with(|ts| ts.iter().filter(|x| !x.done && due_of(&x.due).is_some_and(|d| d.day == sl || (t && d.day < td))).count()),
                rep: if t { bv["mail"]["people_waiting"].as_u64().or_else(|| bv["mail"]["to_reply"].as_u64()).map(|n| n as usize).unwrap_or_else(|| replies.with(Vec::len)) } else { 0 },
                pay: if t { bv["matters"].as_array().map_or(0, |m| m.iter().filter(|x| x["k"] == "pay").count()) } else { 0 },
            }
        });
        planner::greeting(&sl, &td, counts, filter, c)
    };
    let matters = move || {
        tick.track();
        if !is_t.get() {
            return None;
        }
        let bv = b.get()?;
        let matters: Vec<Value> = bv["matters"].as_array().cloned().unwrap_or_default().into_iter().filter(|x| !EVENT_KINDS.contains(&x["k"].as_str().unwrap_or(""))).take(7).collect();
        let by_model = bv["by_model"] == true;
        let model_note = bv["model_note"].as_str().filter(|x| !x.is_empty()).unwrap_or("your model didn't answer").to_string();
        Some(view! {
            {(!matters.is_empty()).then(|| view! {
                <div class="td-mth">{c.t(if evening() { "Tonight and tomorrow" } else { "What matters today" })}</div>
                <ul class="td-mts">{matters.iter().enumerate().map(|(i, x)| {
                    let k = x["k"].as_str().unwrap_or("").to_string();
                    let next_in = (k == "next" && x["time"].is_string() && mins(x["time"].as_str().unwrap_or("")) > now_min()).then(|| format!(" · {} {} min", c.t("in"), mins(x["time"].as_str().unwrap_or("")) - now_min()));
                    let who = x["who"].as_array().cloned().unwrap_or_default();
                    view! {
                        <li class=format!("mt-{k} rise") style=format!("--d:{}s", 0.2 + i as f64 * 0.07)>
                            <span class="ic">{cards::matter_icon(&k)}</span>
                            <span class="tx"><span>{x["text"].as_str().unwrap_or("").to_string()}</span><small>{x["detail"].as_str().unwrap_or("").to_string()}{next_in}</small>
                                {(!who.is_empty()).then(|| view! { <span class="who">{who.iter().map(|p| {
                                    let id = p["id"].as_str().unwrap_or("").to_string();
                                    view! { <a title=p["subject"].as_str().unwrap_or("").to_string() on:click=move |_| navigate("mail", Where::Right, &[("msg", &id), ("folder", "INBOX")])>{format!("{} · {}d", p["from"].as_str().unwrap_or(""), p["days"])}</a> }
                                }).collect_view()}</span> })}
                            </span>
                        </li>
                    }
                }).collect_view()}</ul>
            })}
            {(bv["pending"] != true).then(|| view! {
                <div class=if by_model { "td-by" } else { "td-by off" }>
                    <b>"●"</b>" "
                    {if by_model { c.t("Headline by your model") } else { format!("{} {} · {}", c.t("No headline:"), c.t(&model_note), c.t("the list comes from your calendar, mail and the forecast")) }}
                </div>
            })}
        })
    };
    let plan = move || {
        tick.track();
        let (td, sl) = (today.get(), sel.get());
        let shape = if is_t.get() { b.with(|bv| bv.as_ref().map(|bv| cards::shape_chips(bv, c)).unwrap_or_default()) } else { vec![] };
        planner::day_plan(sl, td, events.get(), all_day.get(), todos.get(), shape, filter.get(), c, acts, set_events, on_calendar)
    };
    let side = move || {
        tick.track();
        if !is_t.get() {
            return None;
        }
        b.with(|bv| bv.as_ref().map(|bv| cards::side_card(bv, c)))
    };
    let main = move || {
        view! {
            <main class="td-main">
                <planner::Strip start=start sel=sel.into() today=today.into() marks=marks c=c on_pick=pick
                    on_drop=Callback::new(move |(id, d): (String, String)| { if let Some(t) = todos.with_untracked(|ts| ts.iter().find(|x| x.id == id).cloned()) { acts.due(&t, &d, None) } })/>
                <section class="td-card td-sum rise" style="--d:.15s">
                    {greet}
                    {move || (is_t.get() && !text.get().is_empty()).then(|| view! { <div class="td-say"><hero::Typed text=text.get()/></div> })}
                    {matters}
                    {move || (!is_t.get()).then(|| view! { <button type="button" class="td-chip plain" style="margin-top:6px" on:click=move |_| pick.run(today.get_untracked())>{format!("← {}", c.t("Back to today"))}</button> })}
                    {move || (!err.get().is_empty()).then(|| view! { <div class="small err" style="margin-top:8px">{format!("{} ({})", c.t("Could not load the brief"), err.get())}</div> })}
                </section>
                {move || is_t.get().then(|| view! { <NotifyAsk/> })}
                {move || is_t.get().then(|| { let day = b.with_untracked(|b| b.as_ref().and_then(|b| b["day"].as_str().map(str::to_string))).unwrap_or_else(|| today.get_untracked()); view! { <cards::AssistCards a=assist.into() x=deliveries.into() day=day c=c/> } })}
                <div class="td-cols">
                    {plan}
                    <div class="td-side">
                        <planner::Inbox todos=todos replies=replies today=today.into() c=c acts=acts/>
                        {side}
                    </div>
                </div>
            </main>
        }
    };
    let body = move || {
        if loaded.get() {
            return main().into_any();
        }
        view! {
            <main class="td-main"><section class="td-card"><div class="td-say">{move || { let e = err.get(); if e.is_empty() { c.t("Writing your brief…") } else { format!("{} ({e}).", c.t("Could not load the brief")) } }}</div></section></main>
        }
        .into_any()
    };
    // Once the page has risen, later redraws appear in place (no rise again).
    let settled = RwSignal::new(false);
    Effect::new(move |_| {
        if loaded.get() {
            gloo_timers::callback::Timeout::new(1800, move || {
                let _ = settled.try_set(true);
            })
            .forget();
        }
    });
    view! {
        <div class=move || if settled.get() { "td settled" } else { "td" }>
            {hero}
            {body}
        </div>
    }
}

const ASKED: &str = "cerq.notify.asked";
/// Today asks once per device while it gets nothing (and the browser can show them).
#[component]
fn NotifyAsk() -> impl IntoView {
    use crate::pages::mail::push;
    let on = RwSignal::new(None::<bool>);
    let devices = RwSignal::new(0u64);
    let gone = RwSignal::new(agentum_ui::store::get(ASKED).is_some());
    spawn_local(async move {
        let s = push::push_state().await;
        let _ = on.try_set(Some(s));
        if let Ok(k) = api_get::<Value>("/push/key").await {
            let _ = devices.try_set(k["devices"].as_u64().unwrap_or(0));
        }
    });
    let done = move || {
        agentum_ui::store::set(ASKED, "1");
        gone.set(true);
    };
    let denied = || web_sys::Notification::permission() == web_sys::NotificationPermission::Denied;
    move || {
        (!gone.get() && on.get() == Some(false) && push::pushable() && !denied()).then(|| view! {
            <section class="td-card td-notify">
                <span class="grow">{move || format!("Get new mail, meeting prep and your brief as notifications on this device{}.", if devices.get() == 0 { " (no device has them on yet)" } else { "" })}</span>
                <button class="btn primary" on:click=move |_| spawn_local(async move {
                    match push::enable_push().await {
                        Ok(()) => { on.set(Some(true)); toast("Notifications on for this device") }
                        Err(e) => toast_bad(e),
                    }
                    done();
                })>"Turn on"</button>
                <button class="btn ghost" on:click=move |_| done()>"Not now"</button>
            </section>
        })
    }
}
