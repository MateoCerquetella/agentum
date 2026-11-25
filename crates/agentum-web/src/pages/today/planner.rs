//! Today as a planner: a strip of days to look ahead (drop a to-do on a day to plan it), a sentence
//! of what's left, your day as one list of events and to-dos, quick add and the Inbox (to-dos
//! without a date, mail waiting for your reply). Calendar writes ask first; to-dos change at once.
use super::cards::card;
use super::plan::*;
use super::Tx;
use crate::pages::mail::lib::fmt_date;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Ev {
    pub title: String,
    pub calendar: String,
    pub kind: String,
    pub location: String,
    pub time: String,
    pub end: String,
    pub uid: String,
    pub series: String,
    pub done: bool,
    pub wx: Option<(i64, i64)>,
}
impl Ev {
    /// A timed event from the brief.
    pub fn of(v: &Value) -> Ev {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Ev {
            title: s("title"),
            calendar: s("calendar"),
            kind: s("kind"),
            location: s("location"),
            time: s("time"),
            end: s("end"),
            uid: s("uid"),
            series: s("series"),
            done: v["done"] == true,
            wx: v["wx"].is_object().then(|| (v["wx"]["t"].as_i64().unwrap_or(0), v["wx"]["p"].as_i64().unwrap_or(0))),
        }
    }
    /// An occurrence from the calendar as a row of one of its days.
    pub fn of_occ(o: &Value, d: &str) -> Ev {
        let s = |k: &str| o[k].as_str().unwrap_or("").to_string();
        let (st, en) = (s("start"), s("end"));
        Ev {
            title: s("title"),
            calendar: s("calendar"),
            kind: s("kind"),
            location: s("location"),
            time: if day_of(&st) < d { "00:00".into() } else { hm(min_of(&st)) },
            end: if day_of(&en) > d { "24:00".into() } else { hm(min_of(&en)) },
            uid: if o["recurring"] == true { String::new() } else { s("id") },
            series: s("uid"),
            done: o["done"] == true,
            wx: None,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Filter {
    Events,
    Todos,
}
fn until(m: i32, c: Tx) -> String {
    let d = m - now_min();
    format!("{} {}", c.t("in"), if d < 60 { format!("{d} min") } else { format!("{}h{}", d / 60, if d % 60 > 0 { format!(" {}m", d % 60) } else { String::new() }) })
}
fn n(k: usize, one: &str, many: &str, c: Tx) -> String {
    format!("{k} {}", c.t(if k == 1 { one } else { many }))
}
fn drag_id(e: &web_sys::DragEvent) -> Option<String> {
    e.data_transfer().and_then(|d| d.get_data(DRAG).ok()).filter(|x| !x.is_empty())
}
fn drag_has(e: &web_sys::DragEvent) -> bool {
    e.data_transfer().is_some_and(|d| js_sys::Array::from(&d.types()).includes(&DRAG.into(), 0))
}
fn drag_start(id: String) -> impl Fn(web_sys::DragEvent) {
    move |e| {
        if let Some(d) = e.data_transfer() {
            let _ = d.set_data(DRAG, &id);
            d.set_effect_allowed("move");
        }
    }
}

/// Two weeks of days: dots for events and to-dos, today in red, drop a to-do to plan it.
#[component]
pub fn Strip(start: RwSignal<String>, sel: Signal<String>, today: Signal<String>, marks: Memo<BTreeMap<String, (usize, usize)>>, c: Tx, on_pick: Callback<String>, on_drop: Callback<(String, String)>) -> impl IntoView {
    let over = RwSignal::new(String::new());
    let bx = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        sel.track();
        start.track();
        let Some(el) = bx.get() else { return };
        request_animation_frame(move || {
            let Some(cell) = el.query_selector(".td-dc.sel").ok().flatten().and_then(|c| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(c).ok()) else { return };
            if el.scroll_width() > el.client_width() {
                el.set_scroll_left(cell.offset_left() - el.offset_left() - (el.client_width() - cell.offset_width()) / 2);
            }
        });
    });
    view! {
        <nav class="td-strip rise" aria-label=move || c.t("Days")>
            <button type="button" class="td-strip-nav" on:click=move |_| start.update(|s| *s = add_days(s, -7)) aria-label=move || c.t("Earlier days")><Icon name="chevron-left" size=15/></button>
            <div class="td-days" node_ref=bx>
                {move || {
                    let (s0, sl, td, lang) = (start.get(), sel.get(), today.get(), c.lang());
                    (0..14).map(|i| {
                        let d = add_days(&s0, i);
                        let (ev, todo) = marks.with(|m| m.get(&d).copied().unwrap_or((0, 0)));
                        let ms = to_date(&d).get_time();
                        let label = fmt_date(ms, Some(locale(&lang)), &[("weekday", "long"), ("day", "numeric"), ("month", "long")]);
                        let wd = fmt_date(ms, Some(locale(&lang)), &[("weekday", "short")]).replace('.', "");
                        let (d1, d2, d3, d4) = (d.clone(), d.clone(), d.clone(), d.clone());
                        let cls = format!("td-dc{}{}{}", if d == sl { " sel" } else { "" }, if d == td { " tod" } else { "" }, if d < td { " past" } else { "" });
                        view! {
                            <button type="button" class=move || format!("{cls}{}", if over.get() == d4 { " over" } else { "" }) aria-pressed=(d == sl).to_string() aria-label=label
                                on:click=move |_| on_pick.run(d1.clone())
                                on:dragover=move |e| { if drag_has(&e) { e.prevent_default(); over.set(d2.clone()) } }
                                on:dragleave=move |_| over.set(String::new())
                                on:drop=move |e| { over.set(String::new()); if let Some(id) = drag_id(&e) { e.prevent_default(); on_drop.run((id, d3.clone())) } }>
                                <small>{wd}</small>
                                <b>{to_date(&d).get_date()}</b>
                                <span class="td-dots">{(ev > 0).then(|| view! { <i class="ev"/> })}{(todo > 0).then(|| view! { <i class="todo"/> })}</span>
                            </button>
                        }
                    }).collect_view()
                }}
            </div>
            <button type="button" class="td-strip-nav" on:click=move |_| start.update(|s| *s = add_days(s, 7)) aria-label=move || c.t("Later days")><Icon name="chevron-right" size=15/></button>
        </nav>
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Counts {
    pub ev: usize,
    pub todo: usize,
    pub rep: usize,
    pub pay: usize,
}

/// "You have [3 events], [2 to-dos] and [1 reply] left today." The chips filter the list.
pub fn greeting(day: &str, today: &str, counts: Counts, filter: RwSignal<Option<Filter>>, c: Tx) -> AnyView {
    let is_t = day == today;
    let chip = |k: &'static str, label: String, icon: &'static str, tone: &'static str| -> AnyView {
        let f = match k {
            "events" => Some(Filter::Events),
            "todos" => Some(Filter::Todos),
            _ => None,
        };
        view! {
            <button type="button" class="td-chip" style=format!("--tc:{tone}") aria-pressed=move || (f.is_some() && filter.get() == f).to_string()
                on:click=move |_| match k {
                    "rep" => navigate("mail", Where::Right, &[("tab", "reply")]),
                    "pay" => filter.set(None),
                    _ => filter.update(|x| *x = if *x == f { None } else { f }),
                }>
                <Icon name=icon size=13/>{label}
            </button>
        }
        .into_any()
    };
    let mut parts: Vec<AnyView> = vec![];
    if counts.ev > 0 {
        parts.push(chip("events", n(counts.ev, "event", "events", c), "calendar", "var(--primary)"));
    }
    if counts.todo > 0 {
        parts.push(chip("todos", n(counts.todo, "to-do", "to-dos", c), "check", "var(--warm)"));
    }
    if is_t && counts.pay > 0 {
        parts.push(chip("pay", format!("{} {}", counts.pay, c.t("to pay")), "money", "var(--money)"));
    }
    if is_t && counts.rep > 0 {
        parts.push(chip("rep", n(counts.rep, "reply", "replies", c), "reply", "var(--positive)"));
    }
    let k = parts.len();
    let mut joined: Vec<AnyView> = vec![];
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            joined.push(if i == k - 1 { format!(" {} ", c.t("and")).into_any() } else { ", ".into_any() });
        }
        joined.push(p);
    }
    let name = day_name(day, &c.lang());
    let body = if is_t {
        if k > 0 { view! { {format!("{} ", c.t("You have"))}{joined}{format!(" {}.", c.t("left today"))} }.into_any() } else { c.t("Nothing left on your list today.").into_any() }
    } else if k > 0 {
        view! { {format!("{name}: ")}{joined}"." }.into_any()
    } else {
        format!("{name} {}", c.t("is open.")).into_any()
    };
    view! {
        <p class="td-greet">
            {body}
            {move || filter.get().map(|_| view! { " "<button type="button" class="td-chip plain" on:click=move |_| filter.set(None)>{c.t("Show everything")}</button> })}
        </p>
    }
    .into_any()
}

enum En {
    Ev(Ev, i32, i32),
    Td(Todo, i32, i32),
}
impl En {
    fn s(&self) -> i32 {
        match self {
            En::Ev(_, s, _) | En::Td(_, s, _) => *s,
        }
    }
    fn en(&self) -> i32 {
        match self {
            En::Ev(_, _, e) | En::Td(_, _, e) => *e,
        }
    }
}

/// Your day: events and to-dos in one list, morning / afternoon / evening, free time between, the
/// now row, overdue to-dos first, then anytime ones.
#[allow(clippy::too_many_arguments)]
pub fn day_plan(day: String, today: String, events: Vec<Ev>, all_day: Vec<String>, todos: Vec<Todo>, shape: Vec<(&'static str, String)>, filter: Option<Filter>, c: Tx, acts: TodoActs, set_events: Callback<Vec<Ev>>, on_calendar: Callback<()>) -> AnyView {
    let is_t = day == today;
    let nm = now_min();
    let next = add_days(&day, 1);
    let lang = c.lang();
    let kind_word = |k: &str| match k {
        "work" | "personal" | "birthday" | "holiday" => c.t(k),
        "money" => c.t("to pay"),
        _ => String::new(),
    };
    let ev_all = events.clone();
    let toggle_ev = {
        let (day, evs) = (day.clone(), ev_all.clone());
        move |e: Ev| {
            let (day, evs) = (day.clone(), evs.clone());
            spawn_local(async move {
                let done = !e.done;
                if !c.ask(c.t(if done { "Mark it done?" } else { "Mark it not done?" }), vec![(c.t("What"), e.title.clone()), (c.t("When"), format!("{}–{}", e.time, e.end)), (c.t("Calendar"), e.calendar.clone())], c.t(if done { "Mark done" } else { "Not done" })).await {
                    return;
                }
                let key = format!("{}|{day}", if e.series.is_empty() { &e.uid } else { &e.series }.split("::").next().unwrap_or(""));
                match api_post::<Value>("/calendar/done", &json!({"key": key, "done": done})).await {
                    Ok(_) => set_events.run(evs.into_iter().map(|x| if x == e { Ev { done, ..x } } else { x }).collect()),
                    Err(er) => toast_bad(er.message),
                }
            });
        }
    };
    let move_ev = {
        let (next, evs, lang) = (next.clone(), ev_all.clone(), lang.clone());
        move |e: Ev| {
            let (next, evs, lang) = (next.clone(), evs.clone(), lang.clone());
            spawn_local(async move {
                let end = if e.end == "24:00" { "23:59".to_string() } else { e.end.clone() };
                if !c.ask(c.t("Move it to the next day?"), vec![(c.t("What"), e.title.clone()), (c.t("When"), format!("{} {}–{end}", day_name(&next, &lang), e.time)), (c.t("Calendar"), e.calendar.clone())], c.t("Move")).await {
                    return;
                }
                match api_put::<Value>(&format!("/calendar/events/{}", js_sys::encode_uri_component(&e.uid)), &json!({"dtstart": format!("{next}T{}:00", e.time), "dtend": format!("{next}T{end}:00")})).await {
                    Ok(_) => {
                        set_events.run(evs.into_iter().filter(|x| *x != e).collect());
                        on_calendar.run(());
                        toast(format!("{} → {}", e.title, day_name(&next, &lang)));
                    }
                    Err(er) => toast_bad(er.message),
                }
            });
        }
    };
    let ev_row = |e: Ev, s: i32, past: bool, live: bool, soon: bool| -> AnyView {
        let sub = [kind_word(&e.kind), e.calendar.clone(), e.location.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        let flag = if e.done { Some(format!("✓ {}", c.t("done"))) } else if live { Some(c.t("now")) } else if soon { Some(until(s, c)) } else { None };
        let wx = e.wx.map(|(t, p)| format!("{t}° · {}", if p >= 50 { format!("{} {p}%", c.t("rain")) } else { c.t("dry") })).unwrap_or_default();
        let wet = e.wx.is_some_and(|w| w.1 >= 50);
        let (e1, e2) = (e.clone(), e.clone());
        let (tg, mv) = (toggle_ev.clone(), move_ev.clone());
        view! {
            <div class=format!("td-ev k-{}{}{}", e.kind, if past { " past" } else if live { " live" } else { "" }, if e.done { " done" } else { "" })>
                <span class="t">{format!("{}–{}", e.time, e.end)}</span>
                <span class="ti">{e.title.clone()}<small>{flag.map(|f| view! { <span class="soon">{f}</span>" · " })}{sub}</small></span>
                <span class=if wet { "w wet" } else { "w" }>{wx}</span>
                <span class="acts2">
                    <button type="button" aria-pressed=e.done.to_string() title=if e.done { c.t("Not done") } else { c.t("Mark done") } on:click=move |_| tg(e1.clone())>"✓"</button>
                    {(!e.uid.is_empty() && !e.done).then(|| view! { <button type="button" title=c.t("Move to the next day") on:click=move |_| mv(e2.clone())>"→"</button> })}
                </span>
            </div>
        }
        .into_any()
    };
    let todo_row = |t: Todo| -> AnyView {
        let d = due_of(&t.due);
        let late = !t.done && d.as_ref().is_some_and(|d| d.day < today || (d.day == today && d.min.is_some_and(|m| m < nm)));
        let small = [
            d.as_ref().filter(|d| d.day != day).map(|d| if d.day < today { format!("{} {}", c.t("since"), day_name(&d.day, &lang)) } else { day_name(&d.day, &lang) }).unwrap_or_default(),
            if t.label.is_empty() { String::new() } else { format!("#{}", t.label) },
        ]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let time = d.as_ref().filter(|d| d.day == day).and_then(|d| d.min).map(hm).unwrap_or_default();
        let next_day = add_days(d.as_ref().filter(|d| d.day > today).map(|d| d.day.as_str()).unwrap_or(&today), 1);
        let (t1, t2) = (t.clone(), t.clone());
        let lbl = if t.done { c.t("Not done") } else { c.t("Mark done") };
        view! {
            <div class=format!("td-ev td-todo{}{}", if t.done { " done" } else { "" }, if late { " late" } else { "" }) draggable=(!t.done).to_string() on:dragstart=drag_start(t.id.clone())>
                <span class="t"><button type="button" class="td-ring" aria-pressed=t.done.to_string() title=lbl.clone() aria-label=format!("{lbl}: {}", t.title) on:click=move |_| acts.done(&t1)/>{time}</span>
                <span class="ti">{t.title.clone()}{(!small.is_empty()).then(|| view! { <small>{small.clone()}</small> })}</span>
                <span class="w"/>
                <span class="acts2">{(!t.done).then(|| view! { <button type="button" title=c.t("Move to the next day") on:click=move |_| acts.due(&t2, &next_day, None)>"→"</button> })}</span>
            </div>
        }
        .into_any()
    };

    let due_on: Vec<Todo> = todos.iter().filter(|t| due_of(&t.due).is_some_and(|d| d.day == day)).cloned().collect();
    let overdue: Vec<Todo> = if is_t { todos.iter().filter(|t| !t.done && due_of(&t.due).is_some_and(|d| d.day < today)).cloned().collect() } else { vec![] };
    let (show_ev, show_td) = (filter != Some(Filter::Todos), filter != Some(Filter::Events));
    let mut timed: Vec<En> = vec![];
    if show_ev {
        timed.extend(events.iter().map(|e| En::Ev(e.clone(), mins(&e.time), mins(&e.end))));
    }
    if show_td {
        timed.extend(due_on.iter().filter_map(|t| due_of(&t.due).and_then(|d| d.min).map(|s| En::Td(t.clone(), s, s + 30))));
    }
    timed.sort_by_key(En::s);
    let mut anytime: Vec<Todo> = if show_td { due_on.iter().filter(|t| due_of(&t.due).is_some_and(|d| d.min.is_none())).cloned().collect() } else { vec![] };
    anytime.sort_by_key(|t| t.done);

    let (over, rest): (Vec<En>, Vec<En>) = if is_t && evening() { timed.into_iter().partition(|x| x.en() <= nm) } else { (vec![], timed) };
    let mut items: Vec<AnyView> = vec![];
    let (mut part, mut prev_end, mut now_done, mut soon_done) = (String::new(), None::<i32>, !is_t, false);
    let now_row = |c: Tx| view! { <div class="td-nowr">{format!("{} {}", c.t("now"), hm(nm))}</div> }.into_any();
    let rest_n = rest.len();
    for x in rest {
        let s = x.s();
        let p = c.t(if s < 720 { "Morning" } else if s < 1140 { "Afternoon" } else { "Evening" });
        if p != part {
            part = p.clone();
            items.push(view! { <div class="td-part">{p}</div> }.into_any());
        }
        if let Some(pe) = prev_end.filter(|pe| s - pe >= 45) {
            items.push(view! { <div class="td-gap">{format!("{} · {}", c.t("free"), dur(s - pe))}</div> }.into_any());
        }
        if !now_done && s > nm {
            now_done = true;
            items.push(now_row(c));
        }
        let en = x.en();
        match x {
            En::Ev(e, s, en) => {
                let past = is_t && en <= nm;
                let live = is_t && s <= nm && en > nm;
                let soon = is_t && !past && !live && !soon_done;
                if soon {
                    soon_done = true;
                }
                items.push(ev_row(e, s, past, live, soon));
            }
            En::Td(t, ..) => items.push(todo_row(t)),
        }
        prev_end = Some(prev_end.unwrap_or(0).max(en));
    }
    if !now_done && rest_n > 0 {
        items.push(now_row(c));
    }
    let over_n = over.len();
    let over_rows: Vec<AnyView> = over
        .into_iter()
        .map(|x| match x {
            En::Ev(e, s, _) => ev_row(e, s, true, false, false),
            En::Td(t, ..) => todo_row(t),
        })
        .collect();
    let nothing = rest_n == 0 && anytime.is_empty() && overdue.is_empty();
    let body = view! {
        {is_t.then(|| view! { <div class="td-shape">{shape.into_iter().map(|(k, t)| view! { <span class=k>{t}</span> }).collect_view()}</div> })}
        {(!all_day.is_empty() && show_ev).then(|| view! { <div class="td-allday">{all_day.into_iter().map(|t| view! { <span>{t}</span> }).collect_view()}</div> })}
        <div class="td-ag">
            {(!overdue.is_empty() && show_td).then(|| view! { <div class="td-part late">{format!("{} · {}", c.t("Overdue"), overdue.len())}</div>{overdue.iter().cloned().map(todo_row).collect_view()} })}
            {(over_n > 0).then(|| view! { <details class="td-earlier"><summary>{format!("{over_n} {}", c.t("earlier today"))}</summary>{over_rows}</details> })}
            {items}
            {(!anytime.is_empty()).then(|| view! { <div class="td-part">{c.t("Anytime")}</div>{anytime.into_iter().map(todo_row).collect_view()} })}
            {nothing.then(|| view! { <div class="td-gap">{if is_t { c.t("Nothing else today") } else { c.t("Nothing planned yet") }}</div> })}
        </div>
        <QuickAdd day=Some(day.clone()) c=c acts=acts on_calendar=on_calendar/>
    }
    .into_any();
    card(c.t("Your day").into_any(), day_name(&day, &lang).into_any(), 0.3, "td-plan", body)
}

/// One line to add a to-do (or an event, which asks first) to a day, or to the Inbox.
#[component]
pub fn QuickAdd(day: Option<String>, c: Tx, acts: TodoActs, #[prop(optional)] on_calendar: Option<Callback<()>>) -> impl IntoView {
    let q = RwSignal::new(String::new());
    let day2 = day.clone();
    let parsed = Memo::new(move |_| {
        let v = q.get();
        (!v.trim().is_empty()).then(|| parse(&v, &today_str()))
    });
    let when = move || parsed.get().and_then(|p| p.date.or_else(|| day2.clone()));
    let when2 = when.clone();
    let is_ev = move || parsed.get().is_some_and(|p| p.event) && when2().is_some();
    let submit = {
        let when = when.clone();
        let is_ev = is_ev.clone();
        move || {
            let Some(p) = parsed.get_untracked() else { return };
            let title = if p.title.is_empty() { q.get_untracked().trim().to_string() } else { p.title.clone() };
            let w = when();
            let ev = is_ev();
            spawn_local(async move {
                if let (true, Some(w)) = (ev, w.clone()) {
                    let lang = c.lang();
                    let wtext = match p.start {
                        Some(s) => format!("{} {}–{}", day_name(&w, &lang), hm(s), hm(p.end.unwrap_or(s + 60))),
                        None => format!("{} · {}", day_name(&w, &lang), c.t("all day")),
                    };
                    if !c.ask(c.t("Add this to your calendar?"), vec![(c.t("What"), title.clone()), (c.t("When"), wtext), (c.t("Calendar"), c.cal.get_untracked())], c.t("Create event")).await {
                        return;
                    }
                    let body = match p.start {
                        Some(s) => json!({"summary": title, "dtstart": local_iso(&w, s), "dtend": local_iso(&w, p.end.unwrap_or(s + 60))}),
                        None => json!({"summary": title, "dtstart": w, "all_day": true}),
                    };
                    match api_post::<Value>("/calendar/events", &body).await {
                        Ok(_) => {
                            toast(format!("{}: {title}", c.t("Added")));
                            q.set(String::new());
                            if let Some(f) = on_calendar {
                                f.run(())
                            }
                        }
                        Err(e) => toast_bad(e.message),
                    }
                    return;
                }
                let due = w.map(|w| due_str(&w, p.start)).unwrap_or_default();
                if acts.add(title, due, p.label.clone()).await {
                    q.set(String::new());
                }
            });
        }
    };
    let placeholder = move || match &day {
        Some(d) => format!("{} {}… “{}”", c.t("Add to"), day_name(d, &c.lang()), c.t("call the bank 10:00")),
        None => format!("{}… “{}”", c.t("New to-do"), c.t("fix the light #home")),
    };
    let hint = move || {
        let p = parsed.get()?;
        let ev = is_ev();
        let w = when();
        let parts: Vec<String> = [
            if ev { c.t("Event") } else { c.t("To-do") },
            w.as_ref().map(|w| day_name(w, &c.lang())).unwrap_or_else(|| c.t("no date")),
            p.start.map(|s| format!("{}{}", hm(s), if ev { format!("–{}", hm(p.end.unwrap_or(s))) } else { String::new() })).unwrap_or_default(),
            p.label.as_ref().map(|l| format!("#{l}")).unwrap_or_default(),
            if ev { c.cal.get() } else { String::new() },
        ]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect();
        Some(view! {
            <div class="td-addhint">"↵ "{parts.into_iter().map(|h| view! { <span>{h}</span> }).collect_view()}{p.guessed.then(|| view! { <span class="subdue">{c.t("looks like an event")}</span> })}</div>
        })
    };
    view! {
        <div class="td-add">
            <label class="td-addbox"><Icon name="plus" size=14/>
                <input prop:value=q on:input=move |e| q.set(event_target_value(&e)) on:keydown=move |e| { if e.key() == "Enter" { e.prevent_default(); submit() } }
                    placeholder=placeholder aria-label=move || c.t("Add a to-do")/>
            </label>
            {hint}
        </div>
    }
}

/// The Inbox: to-dos without a date and mail waiting for your reply. Drop a to-do here to take its date off.
#[component]
pub fn Inbox(todos: RwSignal<Vec<Todo>>, replies: RwSignal<Vec<Value>>, today: Signal<String>, c: Tx, acts: TodoActs) -> impl IntoView {
    let over = RwSignal::new(false);
    let none = move || todos.with(|ts| ts.iter().filter(|t| t.due.is_empty() && !t.done).cloned().collect::<Vec<_>>());
    let mail = move || replies.with(|r| r.iter().take(6).cloned().collect::<Vec<_>>());
    view! {
        <section class=move || format!("td-card rise td-inbox{}", if over.get() { " over" } else { "" }) style="--d:.4s"
            on:dragover=move |e| { if drag_has(&e) { e.prevent_default(); over.set(true) } }
            on:dragleave=move |_| over.set(false)
            on:drop=move |e| {
                over.set(false);
                if let Some(id) = drag_id(&e) {
                    if let Some(t) = todos.with_untracked(|ts| ts.iter().find(|x| x.id == id).cloned()).filter(|t| !t.due.is_empty()) {
                        e.prevent_default();
                        acts.due(&t, "", None);
                    }
                }
            }>
            <h2><span>{move || c.t("Inbox")}</span><span>{move || { let k = none().len() + mail().len(); if k > 0 { k.to_string() } else { String::new() } }}</span></h2>
            <QuickAdd day=None c=c acts=acts/>
            <div class="td-sub3">{move || c.t("No date")}</div>
            {move || {
                let ns = none();
                if ns.is_empty() {
                    return view! { <div class="td-empty small">{c.t("Inbox zero.")}</div> }.into_any();
                }
                ns.into_iter().map(|t| {
                    let (t1, t2) = (t.clone(), t.clone());
                    view! {
                        <div class="td-ib" draggable="true" on:dragstart=drag_start(t.id.clone())>
                            <button type="button" class="td-ring" aria-pressed="false" title=c.t("Mark done") aria-label=format!("{}: {}", c.t("Mark done"), t.title) on:click=move |_| acts.done(&t1)/>
                            <span class="ti">{t.title.clone()}{(!t.label.is_empty()).then(|| view! { <small>{format!("#{}", t.label)}</small> })}</span>
                            <button type="button" class="td-ib-go" on:click=move |_| acts.due(&t2, &today.get_untracked(), None) title=c.t("Do it today")>{c.t("Today")}</button>
                        </div>
                    }
                }).collect_view().into_any()
            }}
            {move || {
                let ms = mail();
                (!ms.is_empty()).then(|| view! {
                    <div class="td-sub3">{c.t("Waiting for your reply")}</div>
                    {ms.into_iter().map(|m| {
                        let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
                        let id = format!("{}:{}", s("account"), m["uid"]);
                        let folder = if s("folder").is_empty() { "INBOX".to_string() } else { s("folder") };
                        let who = if s("from_name").is_empty() { s("from_addr") } else { s("from_name") };
                        let subj = s("subject");
                        let todo_title = format!("{} {}: {}", c.t("Reply to"), who, subj).trim().to_string();
                        view! {
                            <div class="td-ib mail">
                                <Icon name="mail" size=14 class="muted"/>
                                <button type="button" class="ti" on:click=move |_| navigate("mail", Where::Right, &[("msg", &id), ("folder", &folder)])>
                                    {if subj.is_empty() { c.t("(no subject)") } else { subj.clone() }}<small>{format!("{who} · {}", ago(m["ts"].as_i64().unwrap_or(0)))}</small>
                                </button>
                                <button type="button" class="td-ib-go" title=c.t("A to-do for today to answer it") on:click=move |_| { let tt = todo_title.clone(); spawn_local(async move { acts.add(tt, today.get_untracked(), None).await; }) }>{c.t("To-do")}</button>
                            </div>
                        }
                    }).collect_view()}
                })
            }}
            <p class="td-tip">{move || c.t("Drag a to-do onto a day above to plan it.")}</p>
        </section>
    }
}
