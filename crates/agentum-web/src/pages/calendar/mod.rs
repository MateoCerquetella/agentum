//! Calendar: month, week, day and a list over /api/calendar (kind colours, done marks, the
//! right-click menu, ✨ Summarize, the assistant's plan in today's column). Every write asks
//! first: the event dialog reviews before it saves, and drags, menu moves and done marks confirm
//! what, when and which calendar. To-dos meet the calendar here: a panel beside it, to-dos on the
//! grid and in the month, drag one to a time to block it out.
mod ask;
mod dialog;
mod grid;
mod lists;
pub mod parse;
mod todos;
pub mod util;

use super::Pane;
use agentum_proto::api::WsOut;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use agentum_ui::store;
use ask::{ask, facts, AskLayer};
use dialog::{default_cal, draft_from, new_draft, Draft, EventDialog};
use serde::Deserialize;
use std::collections::HashSet;
use util::*;
use wasm_bindgen::JsCast;

/// The to-do panel docks beside the calendar on wide panes (remembered here); narrower ones get a sheet.
const PANEL: &str = "cerq.calTodos";

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GapTodo {
    pub id: String,
    pub title: String,
}
/// Free time today, with a to-do for it (GET /api/assist).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Gap {
    pub date: String,
    pub from: String,
    pub to: String,
    pub why: Option<String>,
    pub by: Option<String>,
    pub todo: Option<GapTodo>,
}
/// "Leave by" for an event with a place.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Leave {
    pub title: String,
    pub time: String,
    pub location: String,
    pub travel: i64,
    pub leave: String,
    pub rain: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dlg {
    pub draft: Draft,
    pub del: bool,
}

thread_local! {
    /// Plan suggestions skipped with "Not now" stay hidden for this visit.
    static SKIPPED: RwSignal<HashSet<String>> = agentum_ui::global(HashSet::new());
    /// Settings › Weeks start on (Monday unless set to Sunday), read once.
    static SUNDAY: RwSignal<Option<bool>> = agentum_ui::global(None);
}

/// The page's state, shared by its parts (all signals: Copy).
#[derive(Clone, Copy)]
pub struct Cx {
    pub date: Memo<String>,
    pub range: Memo<Range>,
    pub events: RwSignal<Option<Vec<Occ>>>,
    pub cals: RwSignal<Vec<Cal>>,
    pub err: RwSignal<String>,
    pub sel_id: RwSignal<Option<String>>,
    pub todos: RwSignal<Vec<Todo>>,
    pub sel_todo: RwSignal<Option<String>>,
    pub dlg: RwSignal<Option<Dlg>>,
    pub menu: RwSignal<Option<(i32, i32, Occ)>>,
    pub done: Memo<HashSet<String>>,
    pub gaps: RwSignal<Vec<Gap>>,
    pub leave: RwSignal<Vec<Leave>>,
    pub skipped: RwSignal<HashSet<String>>,
    seq: StoredValue<u32>,
}

impl Cx {
    pub fn is_done(&self, o: &Occ) -> bool {
        self.done.with(|d| d.contains(&done_key(o)))
    }
    /// Loads: the newest request wins (fast clicking through weeks).
    pub fn load(self) {
        let r = self.range.get_untracked();
        let n = self.seq.get_value() + 1;
        self.seq.set_value(n);
        spawn_local(async move {
            let res = api_get::<Value>(&format!("/calendar/events?start={}&end={}", r.start, r.end)).await;
            if self.seq.try_get_value() != Some(n) {
                return;
            }
            match res {
                Ok(d) => {
                    let evs: Vec<Occ> = serde_json::from_value(d["events"].clone()).unwrap_or_default();
                    self.events.set(Some(evs));
                    if let Ok(c) = serde_json::from_value::<Vec<Cal>>(d["calendars"].clone()) {
                        if !c.is_empty() {
                            self.cals.set(c);
                        }
                    }
                    self.err.set(String::new());
                }
                Err(e) => self.err.set(e.message),
            }
        });
    }
    pub fn load_plan(self) {
        spawn_local(async move {
            if let Ok(a) = api_get::<Value>("/assist").await {
                self.gaps.set(serde_json::from_value(a["gaps"].clone()).unwrap_or_default());
                self.leave.set(serde_json::from_value(a["leave"].clone()).unwrap_or_default());
            }
        });
    }
    pub fn load_todos(self) {
        spawn_local(async move {
            if let Ok(ns) = api_get::<Vec<Value>>("/notes").await {
                self.todos.set(ns.iter().filter(|n| is_todo(n) && keep(n)).map(Todo::from).collect());
            }
        });
    }
    pub fn go(self, d: &str, v: CalView) {
        set_param("view", Some(v.id()));
        set_param("date", Some(d));
    }
    pub fn select(self, o: &Occ) {
        self.sel_id.set(Some(o.id.clone()));
        self.sel_todo.set(None);
    }
    pub fn open(self, o: &Occ) {
        self.dlg.set(Some(Dlg { draft: draft_from(o), del: false }));
    }
    pub fn on_menu(self, x: i32, y: i32, o: &Occ) {
        self.sel_id.set(Some(o.id.clone()));
        self.menu.set(Some((x, y, o.clone())));
    }
    fn cal_of(self, id: &str) -> Option<Cal> {
        self.cals.with_untracked(|c| c.iter().find(|c| c.id == id).cloned())
    }
    pub fn toggle_done(self, o: Occ) {
        let now = !self.is_done(&o);
        spawn_local(async move {
            if !ask(if now { "Mark it done?" } else { "Mark it not done?" }, facts(&o.title, &when_of(&o), &o.calendar), if now { "Mark done" } else { "Not done" }).await {
                return;
            }
            let key = done_key(&o);
            match api_post::<Value>("/calendar/done", &json!({"key": key, "done": now})).await {
                Ok(_) => self.events.update(|l| {
                    if let Some(l) = l {
                        for x in l.iter_mut().filter(|x| done_key(x) == key) {
                            x.done = Some(now);
                        }
                    }
                }),
                Err(e) => toast_bad(e.message),
            }
        });
    }
    pub fn move_to(self, o: Occ, start: String, end: String) {
        spawn_local(async move {
            let mut rows = facts(&o.title, &when_text(&start, &end, o.all_day), &o.calendar);
            if o.recurring {
                rows.push(("Applies to".into(), "only this one (the series stays)".into()));
            }
            if !ask("Move this event?", rows, "Move").await {
                return;
            }
            let mut body = json!({"dtstart": start, "dtend": end, "all_day": o.all_day});
            if o.recurring {
                body["scope"] = json!("this");
            }
            match api_put::<Value>(&format!("/calendar/events/{}", enc(&o.id)), &body).await {
                Ok(_) => {
                    toast("Moved");
                    self.load();
                }
                Err(e) => toast_bad(e.message),
            }
        });
    }
    pub fn move_by(self, o: Occ, days: i64) {
        let sh = |iso: &str| format!("{}{}", add_days(&day_of(iso), days), iso.get(10..19).unwrap_or(""));
        let (s, e) = (sh(&o.start), sh(&o.end));
        self.move_to(o, s, e);
    }
    /// A new event straight from the page (asks first). True when it was made.
    pub async fn create(self, title: String, mut body: Value, cal: Option<Cal>) -> bool {
        let Some(c) = cal.or_else(|| default_cal(&self.cals.get_untracked())) else {
            toast_bad("No calendar to add it to");
            return false;
        };
        let when = when_text(body["dtstart"].as_str().unwrap_or(""), body["dtend"].as_str().unwrap_or(""), body["all_day"].as_bool().unwrap_or(false));
        if !ask("Add this event?", facts(&title, &when, &c.name), "Create event").await {
            return false;
        }
        body["summary"] = json!(title);
        body["calendar_id"] = json!(c.id);
        match api_post::<Value>("/calendar/events", &body).await {
            Ok(_) => {
                toast(format!("Added to {}", c.name));
                self.load();
                true
            }
            Err(e) => {
                toast_bad(e.message);
                false
            }
        }
    }
    pub fn duplicate(self, o: Occ) {
        let c = self.cal_of(&o.calendar_id).filter(|c| !c.read_only);
        let body = json!({"dtstart": o.start.get(..19).unwrap_or(&o.start), "dtend": o.end.get(..19).unwrap_or(&o.end), "all_day": o.all_day, "location": o.location, "description": o.description});
        spawn_local(async move {
            self.create(o.title.clone(), body, c).await;
        });
    }
    pub fn put_in(self, g: Gap) {
        let Some(t) = g.todo.clone() else { return };
        let s = mins(&g.from);
        let body = json!({"dtstart": local_iso(&g.date, s), "dtend": local_iso(&g.date, (s + 60).min(mins(&g.to))), "description": "Planned by Agentum in your free time"});
        spawn_local(async move {
            if self.create(t.title.clone(), body, None).await {
                self.skipped.update(|k| {
                    k.insert(t.id.clone());
                });
            }
        });
    }
    pub fn skip(self, g: &Gap) {
        if let Some(t) = &g.todo {
            self.skipped.update(|k| {
                k.insert(t.id.clone());
            });
        }
    }
    pub fn ask_about(self, o: &Occ) {
        let q = format!("What should I know about “{}” ({})?", o.title, when_of(o));
        navigate("chat", Where::Right, &[("q", &q), ("ctx", &format!("calendar:{}", o.id))]);
    }
    pub fn new_at(self, day: &str, from: Option<i32>, to: Option<i32>) {
        let start = from.unwrap_or(if day == today_str() { (23 * 60).min(((now_min() + 1) as f64 / 60.0).ceil() as i32 * 60) } else { 9 * 60 });
        let d = new_draft(day, start, to.unwrap_or((start + 60).min(1440)), &self.cals.get_untracked());
        self.dlg.set(Some(Dlg { draft: d, del: false }));
    }
    pub fn saved(self, msg: String) {
        self.dlg.set(None);
        toast(msg);
        self.load();
        self.load_plan();
    }

    // ---- to-dos: they change at once, like on Today (events made from quick add ask first)
    pub fn pick_todo(self, t: &Todo) {
        self.sel_todo.set(Some(t.id.clone()));
        self.sel_id.set(None);
    }
    fn todo_put(self, t: Todo, body: Value, said: Option<String>) {
        spawn_local(async move {
            match api_put::<Value>(&format!("/todos/{}", enc(&t.id)), &body).await {
                Ok(x) => {
                    self.todos.update(|ts| {
                        for y in ts.iter_mut().filter(|y| y.id == t.id) {
                            let mut merged = json!({"id": y.id, "title": y.title, "due": y.due, "label": y.label, "done": y.done});
                            if let Some(o) = x.as_object() {
                                for (k, v) in o {
                                    merged[k] = v.clone();
                                }
                            }
                            *y = Todo::from(&merged);
                        }
                    });
                    if let Some(s) = said {
                        toast(s);
                    }
                }
                Err(e) => toast_bad(e.message),
            }
        });
    }
    pub fn todo_done(self, t: &Todo) {
        self.todo_put(t.clone(), json!({"done": !t.done}), None);
    }
    /// A new day keeps the to-do's time unless one is given (Some(None): no time); "" takes the
    /// date off (back to the Inbox).
    pub fn todo_due(self, t: &Todo, day: &str, min: Option<Option<i32>>) {
        let m = min.unwrap_or_else(|| t.due().and_then(|d| d.min));
        let said = if day.is_empty() { format!("{} → Inbox", t.title) } else { format!("{} → {}{}", t.title, day_name(day), min.flatten().map(|m| format!(" {}", hm(m))).unwrap_or_default()) };
        let due = if day.is_empty() { String::new() } else { due_str(day, m) };
        self.todo_put(t.clone(), json!({"due": due}), Some(said));
    }
    pub fn todo_save(self, t: &Todo, body: Value) {
        self.todo_put(t.clone(), body, Some("Saved".into()));
    }
    pub async fn todo_add(self, title: String, due: String, label: Option<String>) -> bool {
        match api_post::<Value>("/todos", &json!({"title": title, "due": due, "label": label.unwrap_or_default()})).await {
            Ok(x) => {
                self.todos.update(|ts| ts.push(Todo::from(&x)));
                toast(match due_of(&due) {
                    Some(d) => format!("To-do for {}{}", day_name(&d.day), d.min.map(|m| format!(" {}", hm(m))).unwrap_or_default()),
                    None => "To-do in your Inbox".into(),
                });
                true
            }
            Err(e) => {
                toast_bad(e.message);
                false
            }
        }
    }
    pub fn drop_todo(self, id: &str, day: &str, min: Option<i32>) {
        let Some(t) = self.todos.with_untracked(|ts| ts.iter().find(|x| x.id == id).cloned()) else { return };
        let d = t.due();
        if d.as_ref().map(|d| d.day.as_str()) != Some(day) || d.as_ref().and_then(|d| d.min) != min {
            self.todo_due(&t, day, Some(min));
        }
    }
    pub fn default_cal_name(self) -> String {
        self.cals.with(|c| default_cal(c).map(|c| c.name)).unwrap_or_else(|| "your default calendar".into())
    }
}

pub fn enc(s: &str) -> String {
    js_sys::encode_uri_component(s).into()
}
/// Does a drag carry a to-do?
pub fn has_todo(e: &web_sys::DragEvent) -> bool {
    e.data_transfer().is_some_and(|dt| dt.types().includes(&wasm_bindgen::JsValue::from_str(DRAG), 0))
}
pub fn drag_todo(e: &web_sys::DragEvent, t: &Todo) {
    if let Some(dt) = e.data_transfer() {
        let _ = dt.set_data(DRAG, &t.id);
        dt.set_effect_allowed("move");
    }
}
pub fn todo_data(e: &web_sys::DragEvent) -> String {
    e.data_transfer().and_then(|dt| dt.get_data(DRAG).ok()).unwrap_or_default()
}
/// Leaving an element for one of its own children doesn't count.
pub fn left_for_good(e: &web_sys::DragEvent) -> bool {
    let cur = e.current_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
    let rel = e.related_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
    match (cur, rel) {
        (Some(c), Some(r)) => !c.contains(Some(&r)),
        _ => true,
    }
}
pub fn target_in(e: &web_sys::Event, sel: &str) -> bool {
    e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|el| el.closest(sel).ok().flatten()).is_some()
}

pub fn page(pane: Pane) -> AnyView {
    view! { <CalendarPage pane=pane/> }.into_any()
}

#[derive(Clone, Debug, PartialEq)]
struct Sum {
    key: String,
    label: String,
    loading: bool,
    v: Option<Value>,
    error: Option<String>,
}

#[component]
fn CalendarPage(pane: Pane) -> impl IntoView {
    let p_view = use_param("view");
    let p_date = use_param("date");
    let view = Memo::new(move |_| p_view.get().and_then(|v| CalView::parse(&v)).unwrap_or(CalView::Week));
    let date = Memo::new(move |_| p_date.get().filter(|d| is_ymd(d)).unwrap_or_else(today_str));
    let sunday = SUNDAY.with(|s| *s);
    if sunday.get_untracked().is_none() {
        spawn_local(async move {
            let v = api_get::<Value>("/settings").await.ok().and_then(|s| s["fields"].as_array().map(|f| f.iter().any(|f| f["key"] == "CERQ_WEEK_START" && f["value"] == "Sunday")));
            sunday.set(Some(v.unwrap_or(false)));
        });
    }
    let range = Memo::new(move |_| range_for(view.get(), &date.get(), sunday.get().unwrap_or(false)));
    let events = RwSignal::new(None::<Vec<Occ>>);
    let done = Memo::new(move |_| events.with(|l| l.iter().flatten().filter(|o| o.done == Some(true)).map(done_key).collect::<HashSet<_>>()));
    let cx = Cx {
        date,
        range,
        events,
        cals: RwSignal::new(vec![]),
        err: RwSignal::new(String::new()),
        sel_id: RwSignal::new(None),
        todos: RwSignal::new(vec![]),
        sel_todo: RwSignal::new(None),
        dlg: RwSignal::new(None),
        menu: RwSignal::new(None),
        done,
        gaps: RwSignal::new(vec![]),
        leave: RwSignal::new(vec![]),
        skipped: SKIPPED.with(|s| *s),
        seq: StoredValue::new(0),
    };
    let syncing = RwSignal::new(false);
    let quick = RwSignal::new(String::new());
    let parsing = RwSignal::new(false);
    let sum = RwSignal::new(None::<Sum>);
    let prep_open = RwSignal::new(false);

    // Wait for the week start before the first load (else it loads twice).
    Effect::new(move |_| {
        range.track();
        if sunday.get().is_some() {
            cx.load();
        }
    });
    spawn_local(async move {
        if let Ok(c) = api_get::<Vec<Cal>>("/calendar/calendars").await {
            if !c.is_empty() {
                cx.cals.set(c);
            }
        }
    });
    cx.load_plan();
    cx.load_todos();
    // A pull when the calendar opens (the hub reuses one under a minute old).
    spawn_local(async move {
        if let Ok(r) = api_post::<Value>("/calendar/sync", &json!({})).await {
            if !r["in_progress"].as_bool().unwrap_or(false) && sunday.get_untracked().is_some() {
                cx.load();
            }
        }
    });
    subscribe(&["app"]);
    on_ws(move |m| {
        if let WsOut::App { payload } = m {
            match payload["kind"].as_str() {
                Some("calendar") => {
                    cx.load();
                    cx.load_plan();
                }
                Some("todos") => cx.load_todos(),
                _ => {}
            }
        }
    });

    // Wide panes dock the to-do panel; narrow ones open it as a sheet.
    let root = NodeRef::<leptos::html::Div>::new();
    let wide = RwSignal::new(true);
    let dock = RwSignal::new(store::get(PANEL).as_deref() != Some("closed"));
    let sheet = RwSignal::new(false);
    let observer = StoredValue::new_local(None::<(web_sys::ResizeObserver, wasm_bindgen::closure::Closure<dyn FnMut(js_sys::Array)>)>);
    Effect::new(move |_| {
        let Some(el) = root.get() else { return };
        let f = wasm_bindgen::closure::Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            if let Ok(e) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                let w = e.content_rect().width() >= 900.0;
                if wide.get_untracked() != w {
                    wide.set(w);
                }
            }
        });
        if let Ok(ro) = web_sys::ResizeObserver::new(f.as_ref().unchecked_ref()) {
            ro.observe(&el);
            observer.set_value(Some((ro, f)));
        }
    });
    on_cleanup(move || {
        observer.try_with_value(|o| {
            if let Some((ro, _)) = o {
                ro.disconnect();
            }
        });
    });
    let panel = move || if wide.get() { dock.get() } else { sheet.get() };
    let toggle_panel = move || {
        if !wide.get_untracked() {
            sheet.update(|s| *s = !*s);
            return;
        }
        let d = dock.get_untracked();
        dock.set(!d);
        store::set(PANEL, if d { "closed" } else { "open" });
    };

    let step = move |n: i64| {
        let d = date.get_untracked();
        let v = view.get_untracked();
        let to = match v {
            CalView::Month => first_of_month(&d, n),
            CalView::Week => add_days(&d, n * 7),
            CalView::Day => add_days(&d, n),
            CalView::Agenda => add_days(&d, n * 30),
        };
        cx.go(&to, v);
    };
    let late = move || {
        let t = today_str();
        cx.todos.with(|ts| ts.iter().filter(|x| !x.done && x.due().is_some_and(|d| d.day <= t)).count())
    };

    let quick_add = move || {
        let text = quick.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }
        parsing.set(true);
        spawn_local(async move {
            match api_post::<Value>("/calendar/quick-parse", &json!({"text": text})).await {
                Ok(r) => {
                    let ds = r["dtstart"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(today_str);
                    let sd = day_of(&ds);
                    let s = min_of(&ds);
                    let de = r["dtend"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
                    let ed = de.as_deref().map(day_of).unwrap_or_else(|| sd.clone());
                    let mut d = new_draft(&sd, s, s + 60, &cx.cals.get_untracked());
                    d.title = r["summary"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or(text.clone());
                    d.all_day = r["all_day"].as_bool().unwrap_or(false);
                    if d.all_day {
                        d.ed = if ed > sd { add_days(&ed, -1) } else { sd.clone() };
                    } else if let Some(de) = &de {
                        d.ed = ed;
                        d.et = hm(min_of(de));
                    }
                    cx.dlg.set(Some(Dlg { draft: d, del: false }));
                    quick.set(String::new());
                }
                Err(e) => toast_bad(format!("Couldn't read that: {}", e.message)),
            }
            parsing.set(false);
        });
    };
    let sync = move || {
        syncing.set(true);
        spawn_local(async move {
            match api_post::<Value>("/calendar/sync?force=1", &json!({})).await {
                Ok(r) => {
                    let busy = r["in_progress"].as_bool().unwrap_or(false);
                    if !busy {
                        cx.load();
                    }
                    toast(if busy { "Syncing…" } else { "Calendars synced" });
                }
                Err(e) => toast_bad(e.message),
            }
            syncing.set(false);
        });
    };

    // ✨ Summarize what's on screen (the list: the next 7 days).
    let sum_range = move || {
        let (v, d, r) = (view.get_untracked(), date.get_untracked(), range.get_untracked());
        match v {
            CalView::Month => (first_of_month(&d, 0), first_of_month(&d, 1)),
            CalView::Agenda => (d.clone(), add_days(&d, 7)),
            _ => (r.start, r.end),
        }
    };
    let sum_label = move || {
        let (v, d, r) = (view.get(), date.get(), range.get());
        let t = today_str();
        match v {
            CalView::Day => if d == t { "today".into() } else { fmt_short(&d) },
            CalView::Week => if r.days.contains(&t) { "this week".into() } else { format!("week of {}", fmt_short(&r.start)) },
            CalView::Month => fmt_day(&d, &[("month", "long"), ("year", "numeric")]),
            CalView::Agenda => "the next 7 days".into(),
        }
    };
    let summarize = move || {
        let (a, b) = sum_range();
        let key = format!("{a}/{b}");
        let label = untrack(sum_label);
        sum.set(Some(Sum { key: key.clone(), label: label.clone(), loading: true, v: None, error: None }));
        spawn_local(async move {
            let s = match api_get::<Value>(&format!("/calendar/summary?start={a}&end={b}")).await {
                Ok(v) => Sum { key, label: v["label"].as_str().filter(|l| !l.is_empty()).map(str::to_string).unwrap_or(label), loading: false, v: Some(v), error: None },
                Err(e) => Sum { key, label, loading: false, v: None, error: Some(e.message) },
            };
            sum.set(Some(s));
        });
    };

    let title = move || {
        let (v, d, r) = (view.get(), date.get(), range.get());
        match v {
            CalView::Month => fmt_day(&d, &[("month", "long"), ("year", "numeric")]),
            CalView::Week => format!("{} – {}", fmt_day(&r.start, &[("day", "numeric"), ("month", "short")]), fmt_day(&add_days(&r.end, -1), &[("day", "numeric"), ("month", "short"), ("year", "numeric")])),
            CalView::Day => fmt_day(&d, &[("weekday", "long"), ("day", "numeric"), ("month", "long"), ("year", "numeric")]),
            CalView::Agenda => format!("From {}", fmt_day(&d, &[("day", "numeric"), ("month", "long")])),
        }
    };
    let new_btn = move |_| {
        let (v, d, r) = (view.get_untracked(), date.get_untracked(), range.get_untracked());
        let t = today_str();
        let day = match v {
            CalView::Month | CalView::Day => d,
            _ if r.days.contains(&t) => t,
            _ => r.start,
        };
        cx.new_at(&day, None, None);
    };

    // The pane header's tools: sync now.
    let tools = RwSignal::new_local(None::<web_sys::Element>);
    Effect::new(move |_| {
        let id = if pane == Pane::Left { "pane-tools-left" } else { "pane-tools-right" };
        tools.set(web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)));
    });

    let sel = Memo::new(move |_| {
        let id = cx.sel_id.get()?;
        cx.events.with(|l| l.iter().flatten().find(|o| o.id == id).cloned())
    });
    let sel_t = Memo::new(move |_| {
        let id = cx.sel_todo.get()?;
        cx.todos.with(|l| l.iter().find(|t| t.id == id).cloned())
    });
    Effect::new(move |_| {
        sel.track();
        prep_open.set(false);
    });

    view! {
        <div class="cal" node_ref=root>
            {move || tools.get().map(|el| view! {
                <leptos::portal::Portal mount=el>
                    <button class="icon-btn" title="Sync calendars now" on:click=move |_| sync() disabled=move || syncing.get()>
                        <Icon name="refresh" class=Signal::derive(move || if syncing.get() { "spin".to_string() } else { String::new() })/>
                    </button>
                </leptos::portal::Portal>
            })}
            <div class="cal-bar">
                <button class="btn" on:click=move |_| cx.go(&today_str(), view.get_untracked())>"Today"</button>
                <span class="row" style="gap:0">
                    <button class="icon-btn" on:click=move |_| step(-1) aria-label="Previous"><Icon name="chevronL"/></button>
                    <button class="icon-btn" on:click=move |_| step(1) aria-label="Next"><Icon name="chevronR"/></button>
                </span>
                <span class="cal-title hi truncate">{title}</span>
                <span class="grow"/>
                <span class="cal-seg">
                    {CalView::ALL.iter().map(|(v, _, l)| {
                        let v = *v;
                        view! { <button class=move || if view.get() == v { "on" } else { "" } on:click=move |_| cx.go(&date.get_untracked(), v)>{*l}</button> }
                    }).collect_view()}
                </span>
                <button class=move || if panel() { "btn on" } else { "btn" } aria-pressed=move || panel().to_string() on:click=move |_| toggle_panel()
                    title=move || if panel() { "Hide to-dos" } else { "Show to-dos beside the calendar" }>
                    <Icon name="check" size=14/>" To-dos"
                    <Show when=move || { late() > 0 }><span class="cal-count">{late}</span></Show>
                </button>
                <button class="btn cal-sum-btn" on:click=move |_| summarize() disabled=move || sum.with(|s| s.as_ref().is_some_and(|s| s.loading)) title=move || format!("Summarize {}", sum_label())>
                    <Icon name="sparkle" size=14/>" Summarize"
                </button>
                <button class="btn primary" on:click=new_btn><Icon name="plus" size=14/>" New"</button>
                <form class="cal-quick" on:submit=move |e| { e.prevent_default(); quick_add() }>
                    <input placeholder=move || if parsing.get() { "Reading it…" } else { "Quick add: lunch with Ana tomorrow 1pm" } prop:value=move || quick.get()
                        disabled=move || parsing.get() on:input=move |e| quick.set(event_target_value(&e)) aria-label="Quick add"/>
                </form>
            </div>
            {move || sum.get().map(|s| view! { <SummaryCard s=s again=Callback::new(move |_| summarize()) close=Callback::new(move |_| sum.set(None))/> })}
            <Show when=move || !cx.err.get().is_empty()>
                <div class="small err" style="padding:0 14px 8px">{move || cx.err.get()}</div>
            </Show>
            <div class="cal-main">
                <div class="cal-view">
                    {move || {
                        let loaded = cx.events.with(|e| e.is_some());
                        if !loaded {
                            return view! { <div class="empty">"Loading…"</div> }.into_any();
                        }
                        let r = range.get();
                        match view.get() {
                            CalView::Month => view! { <lists::MonthView cx=cx days=r.days/> }.into_any(),
                            CalView::Agenda => view! { <lists::AgendaView cx=cx days=r.days/> }.into_any(),
                            _ => view! { <grid::TimeGrid cx=cx days=r.days/> }.into_any(),
                        }
                    }}
                </div>
                <Show when=panel>
                    <todos::TodoPanel cx=cx on_close=Callback::new(move |_| toggle_panel()) sheet=Signal::derive(move || !wide.get())/>
                </Show>
            </div>
            {move || sel_t.get().map(|t| view! { <todos::TodoBar cx=cx t=t/> })}
            {move || sel.get().map(|o| {
                let done = cx.is_done(&o);
                let (o1, o2, o3) = (o.clone(), o.clone(), o.clone());
                let place = [o.calendar.clone(), o.location.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
                view! {
                    <div class="cal-sel" style=format!("--kc:{}", kind_color(&o.kind))>
                        <div class="grow" style="min-width:0">
                            <div class=if done { "hi cal-done-t" } else { "hi" }>{done.then_some("✓ ")}{o.title.clone()}</div>
                            <div class="small muted">{when_of(&o)}{o.recurring.then_some(" · repeats")}</div>
                            <div class="small subdue truncate"><i class="cal-dot" style=format!("background:{}", safe_color(&o.color))/>{place}</div>
                            {(!o.description.is_empty()).then(|| view! { <div class="small muted cal-desc">{o.description.clone()}</div> })}
                        </div>
                        <div class="cal-sel-acts">
                            <button class="btn" on:click=move |_| cx.open(&o1)><Icon name="edit" size=14/>{if o.read_only { " Details" } else { " Edit" }}</button>
                            <button class="btn" on:click=move |_| cx.toggle_done(o2.clone())><Icon name="check" size=14/>{if done { " Not done" } else { " Done" }}</button>
                            <button class="btn" on:click=move |_| cx.ask_about(&o3)><Icon name="chat" size=14/>" Ask Agentum"</button>
                            <button class="icon-btn" on:click=move |_| cx.sel_id.set(None) aria-label="Close"><Icon name="x"/></button>
                        </div>
                    </div>
                }
            })}
            {move || cx.menu.get().map(|(x, y, o)| view! { <EventMenu cx=cx x=x y=y o=o/> })}
            {move || cx.dlg.get().map(|d| view! { <EventDialog cx=cx dlg=d/> })}
            <AskLayer/>
        </div>
    }
}

#[component]
fn SummaryCard(s: Sum, again: Callback<()>, close: Callback<()>) -> impl IntoView {
    let _ = &s.key;
    let mut chips: Vec<AnyView> = vec![];
    if let Some(v) = &s.v {
        let arr = |k: &str| v[k].as_array().cloned().unwrap_or_default();
        let (busy, clashes, free) = (arr("busy"), arr("clashes"), arr("free_evenings"));
        let kind = v["kind"].as_str().unwrap_or("");
        if let (Some(b), true) = (busy.first(), kind != "day") {
            let tip = busy.iter().map(|b| format!("{}: {}", b["day"].as_str().unwrap_or(""), b["count"])).collect::<Vec<_>>().join(" · ");
            chips.push(view! { <span class="chip" title=tip>"Busiest "{b["day"].as_str().unwrap_or("").to_string()}</span> }.into_any());
        }
        if !clashes.is_empty() {
            let tip = clashes.iter().map(|c| format!("{} {} {} / {}", fmt_short(c["date"].as_str().unwrap_or("")), c["time"].as_str().unwrap_or(""), c["a"].as_str().unwrap_or(""), c["b"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n");
            let n = clashes.len();
            chips.push(view! { <span class="chip neg" title=tip>{format!("{n} clash{}", if n > 1 { "es" } else { "" })}</span> }.into_any());
        }
        if !free.is_empty() && kind != "day" {
            let tip = free.iter().map(|d| fmt_short(d.as_str().unwrap_or(""))).collect::<Vec<_>>().join(" · ");
            let n = free.len();
            chips.push(view! { <span class="chip pos" title=tip>{format!("{n} free evening{}", if n > 1 { "s" } else { "" })}</span> }.into_any());
        }
        if let Some(d) = v["done"].as_i64().filter(|d| *d > 0) {
            chips.push(view! { <span class="chip pos">{format!("{d} done")}</span> }.into_any());
        }
    }
    let text = if s.loading {
        "Your model is reading your calendar…".to_string()
    } else if let Some(e) = &s.error {
        format!("Couldn't summarize: {e}")
    } else {
        let v = s.v.clone().unwrap_or_default();
        let n = v["events"].as_i64().unwrap_or(0);
        v["text"].as_str().filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{n} event{}", if n == 1 { "" } else { "s" }))
    };
    let has_chips = !chips.is_empty();
    view! {
        <div class="cal-sum">
            <div class="row">
                <Icon name="sparkle" size=14 class="hi"/>
                <span class="small upper muted grow">"Summary · "{s.label.clone()}</span>
                {(!s.loading).then(|| view! { <button class="btn ghost" on:click=move |_| again.run(())><Icon name="refresh" size=12/>" Again"</button> })}
                <button class="icon-btn" on:click=move |_| close.run(()) aria-label="Close summary"><Icon name="x" size=14/></button>
            </div>
            <div class="hi">{text}</div>
            {has_chips.then(|| view! { <div class="row" style="flex-wrap:wrap;gap:6px">{chips}</div> })}
        </div>
    }
}

/// The right-click menu: closes on a click elsewhere, Esc, scroll or resize; ↑↓ move.
#[component]
fn EventMenu(cx: Cx, x: i32, y: i32, o: Occ) -> impl IntoView {
    let el = NodeRef::<leptos::html::Div>::new();
    let pos = RwSignal::new((x, y));
    let close = move || cx.menu.set(None);
    Effect::new(move |_| {
        let Some(m) = el.get() else { return };
        let r = m.get_bounding_client_rect();
        let w = web_sys::window().unwrap();
        let iw = w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1000.0);
        let ih = w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(800.0);
        pos.set(((x as f64).min(iw - r.width() - 6.0).max(4.0) as i32, (y as f64).min(ih - r.height() - 6.0).max(4.0) as i32));
        if let Ok(Some(b)) = m.query_selector("button:not(:disabled)") {
            if let Ok(b) = b.dyn_into::<web_sys::HtmlElement>() {
                let _ = b.focus();
            }
        }
    });
    let h1 = window_event_listener(leptos::ev::pointerdown, move |e| {
        let inside = el.get_untracked().zip(e.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok())).is_some_and(|(m, t)| m.contains(Some(&t)));
        if !inside {
            close();
        }
    });
    let h2 = window_event_listener(leptos::ev::keydown, move |e| {
        let k = e.key();
        if k == "Escape" {
            e.prevent_default();
            close();
        }
        if k == "ArrowDown" || k == "ArrowUp" {
            e.prevent_default();
            let Some(m) = el.get_untracked() else { return };
            let Ok(list) = m.query_selector_all("button:not(:disabled)") else { return };
            let n = list.length();
            if n == 0 {
                return;
            }
            let active = web_sys::window().and_then(|w| w.document()).and_then(|d| d.active_element());
            let i = (0..n).find(|i| list.item(*i).zip(active.clone()).is_some_and(|(a, b)| a.is_same_node(Some(&b)))).map(|i| i as i64).unwrap_or(-1);
            let j = (i + if k == "ArrowDown" { 1 } else { -1 } + n as i64).rem_euclid(n as i64) as u32;
            if let Some(b) = list.item(j).and_then(|b| b.dyn_into::<web_sys::HtmlElement>().ok()) {
                let _ = b.focus();
            }
        }
    });
    let h3 = window_event_listener(leptos::ev::resize, move |_| close());
    let h4 = window_event_listener(leptos::ev::blur, move |_| close());
    let h5 = StoredValue::new_local(None::<wasm_bindgen::closure::Closure<dyn FnMut()>>);
    {
        let f = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || cx.menu.set(None));
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_capture(true);
        if let Some(d) = web_sys::window().and_then(|w| w.document()) {
            let _ = d.add_event_listener_with_callback_and_add_event_listener_options("scroll", f.as_ref().unchecked_ref(), &opts);
        }
        h5.set_value(Some(f));
    }
    on_cleanup(move || {
        h1.remove();
        h2.remove();
        h3.remove();
        h4.remove();
        h5.try_with_value(|f| {
            if let (Some(f), Some(d)) = (f, web_sys::window().and_then(|w| w.document())) {
                let _ = d.remove_event_listener_with_callback_and_bool("scroll", f.as_ref().unchecked_ref(), true);
            }
        });
    });
    let done = cx.is_done(&o);
    let fixed = o.recurring || o.read_only;
    let item = move |t: &'static str, off: bool, danger: bool, run: Box<dyn Fn() + Send + Sync>| {
        view! { <button role="menuitem" disabled=off class=danger.then_some("neg") on:click=move |_| { close(); run() }>{t}</button> }
    };
    let (a, b, c, d, e, f, g) = (o.clone(), o.clone(), o.clone(), o.clone(), o.clone(), o.clone(), o.clone());
    view! {
        <div node_ref=el class="cal-menu" role="menu" style=move || format!("left:{}px;top:{}px", pos.get().0, pos.get().1)>
            {item(if o.read_only { "Details" } else { "Open" }, false, false, Box::new(move || cx.open(&a)))}
            {item(if done { "Not done" } else { "Mark done" }, false, false, Box::new(move || cx.toggle_done(b.clone())))}
            {item("Ask Agentum about it", false, false, Box::new(move || cx.ask_about(&c)))}
            <hr/>
            <div class="small subdue cal-menu-l">{if o.recurring { "Move to (one-off events only)" } else { "Move to" }}</div>
            {item("Tomorrow", fixed, false, Box::new(move || cx.move_by(d.clone(), 1)))}
            {item("Next week", fixed, false, Box::new(move || cx.move_by(e.clone(), 7)))}
            <hr/>
            {item("Duplicate", false, false, Box::new(move || cx.duplicate(f.clone())))}
            {item("Delete…", o.read_only, true, Box::new(move || cx.dlg.set(Some(Dlg { draft: draft_from(&g), del: true }))))}
        </div>
    }
}
