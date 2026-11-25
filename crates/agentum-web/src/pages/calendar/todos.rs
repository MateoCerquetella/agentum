//! To-dos beside the calendar: Overdue, Today, the next 7 days and No date (the Inbox), with
//! quick add. Drag a to-do onto the grid to give it a time, onto a day for just the date, or onto
//! No date to take the date off. To-dos change at once, like on Today; events ask first.
use super::ask::ask;
use super::lists::tkey;
use super::parse::parse;
use super::util::*;
use super::{drag_todo, has_todo, left_for_good, todo_data, Cx};
use agentum_ui::prelude::*;

/// "10:00", "Today", "Fri 10:00" (within a week), "17 Nov" (overdue or later).
fn due_text(d: &Due, today: &str) -> String {
    let day = if d.day == today {
        if d.min.is_none() { "Today".to_string() } else { String::new() }
    } else if d.day.as_str() > today && d.day <= add_days(today, 6) {
        fmt_day(&d.day, &[("weekday", "short")])
    } else {
        fmt_day(&d.day, &[("day", "numeric"), ("month", "short")])
    };
    [day, d.min.map(hm).unwrap_or_default()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ")
}
fn order(a: &Todo, b: &Todo) -> std::cmp::Ordering {
    let k = |t: &Todo| format!("{}{}", t.done as u8, t.due().map(|d| format!("{}{}", d.day, d.min.map(hm).unwrap_or_default())).unwrap_or_default());
    k(a).cmp(&k(b)).then_with(|| a.title.cmp(&b.title))
}

#[component]
pub fn TodoPanel(cx: Cx, on_close: Callback<()>, sheet: Signal<bool>) -> impl IntoView {
    let over = RwSignal::new(false);
    let dragging = RwSignal::new(false);
    let groups = Memo::new(move |_| {
        let today = today_str();
        let week = add_days(&today, 7);
        let mut g: [Vec<Todo>; 4] = Default::default();
        for t in cx.todos.get() {
            let k = match t.due() {
                None => if t.done { None } else { Some(3) },
                Some(d) if d.day < today => if t.done { None } else { Some(0) },
                Some(d) if d.day == today => Some(1),
                Some(d) if d.day <= week => Some(2),
                _ => None,
            };
            if let Some(k) = k {
                g[k].push(t);
            }
        }
        for v in &mut g {
            v.sort_by(order);
        }
        g
    });
    let row = move |t: Todo| {
        let today = today_str();
        let d = t.due();
        let (t1, t2, t3) = (t.clone(), t.clone(), t.clone());
        let id = t.id.clone();
        let label = format!("{}: {}", if t.done { "Not done" } else { "Mark done" }, t.title);
        view! {
            <div class=move || format!("ctd-row{}{}", if t.done { " done" } else { "" }, if cx.sel_todo.get().as_deref() == Some(id.as_str()) { " sel" } else { "" })
                data-id=t.id.clone() draggable=if t.done { "false" } else { "true" } on:dragstart=move |e| drag_todo(&e, &t1) on:click=move |_| cx.pick_todo(&t2)>
                <button type="button" class="td-ring" aria-pressed=t.done.to_string() title=if t.done { "Not done" } else { "Mark done" } aria-label=label
                    on:click=move |e| { e.stop_propagation(); cx.todo_done(&t3) }/>
                <span class="ti">{t.title.clone()}{(!t.label.is_empty()).then(|| view! { <small>" #"{t.label.clone()}</small> })}</span>
                {d.map(|d| view! { <span class="ctd-due">{due_text(&d, &today)}</span> })}
            </div>
        }
    };
    let head = |k: &'static str, title: &'static str, n: Signal<usize>| view! { <div class=format!("ctd-h {k}")>{title}<span>{move || { let n = n.get(); if n > 0 { n.to_string() } else { String::new() } }}</span></div> };
    let count = move |i: usize| Signal::derive(move || groups.with(|g| g[i].len()));
    view! {
        <aside class=move || format!("ctd{}{}", if sheet.get() { " sheet" } else { "" }, if dragging.get() { " dragging" } else { "" }) aria-label="To-dos"
            on:dragstart=move |_| { if sheet.get_untracked() { set_timeout(move || dragging.set(true), std::time::Duration::ZERO) } }
            on:dragend=move |_| dragging.set(false)>
            <div class="row ctd-top">
                <span class="hi grow">"To-dos"</span>
                <button class="icon-btn" on:click=move |_| on_close.run(()) aria-label="Hide to-dos" title="Hide to-dos"><Icon name="x" size=14/></button>
            </div>
            <QuickAdd cx=cx/>
            <Show when=move || groups.with(|g| !g[0].is_empty())>
                <section data-sec="late">{head("late", "Overdue", count(0))}<For each=move || groups.get()[0].clone() key=tkey let:t>{row(t)}</For></section>
            </Show>
            <section data-sec="today">
                {head("", "Today", Signal::derive(move || groups.with(|g| g[1].iter().filter(|t| !t.done).count())))}
                <Show when=move || groups.with(|g| !g[1].is_empty()) fallback=|| view! { <div class="ctd-empty">"Nothing due today."</div> }>
                    <For each=move || groups.get()[1].clone() key=tkey let:t>{row(t)}</For>
                </Show>
            </section>
            <Show when=move || groups.with(|g| !g[2].is_empty())>
                <section data-sec="next">{head("", "Upcoming", count(2))}<For each=move || groups.get()[2].clone() key=tkey let:t>{row(t)}</For></section>
            </Show>
            <section data-sec="none" class=move || if over.get() { "ctd-none over" } else { "ctd-none" }
                on:dragover=move |e| { if has_todo(&e) { e.prevent_default(); over.set(true) } }
                on:dragleave=move |e| { if left_for_good(&e) { over.set(false) } }
                on:drop=move |e| {
                    over.set(false);
                    let id = todo_data(&e);
                    if let Some(t) = cx.todos.with_untracked(|ts| ts.iter().find(|x| x.id == id).cloned()) {
                        e.prevent_default();
                        if !t.due.is_empty() { cx.todo_due(&t, "", None) }
                    }
                }>
                {head("", "No date", count(3))}
                <For each=move || groups.get()[3].clone() key=tkey let:t>{row(t)}</For>
                <p class="ctd-tip">"Drag a to-do onto the calendar to give it a time; drop it here to take the date off."</p>
            </section>
        </aside>
    }
}

/// One line to add a to-do (or an event, which asks first) to the Inbox or a day.
#[component]
pub fn QuickAdd(cx: Cx) -> impl IntoView {
    let q = RwSignal::new(String::new());
    let p = Memo::new(move |_| q.with(|q| (!q.trim().is_empty()).then(|| parse(q))));
    let submit = move || {
        let Some(p) = p.get_untracked() else { return };
        let title = if p.title.is_empty() { q.get_untracked().trim().to_string() } else { p.title.clone() };
        let when = p.date.clone();
        spawn_local(async move {
            if let (true, Some(day)) = (p.event, when.clone()) {
                let when_s = match (p.start, p.end) {
                    (Some(s), Some(e)) => format!("{} {}–{}", day_name(&day), hm(s), hm(e)),
                    _ => format!("{} · all day", day_name(&day)),
                };
                let rows = vec![("What".into(), title.clone()), ("When".into(), when_s), ("Calendar".into(), cx.default_cal_name())];
                if !ask("Add this to your calendar?", rows, "Create event").await {
                    return;
                }
                let body = match (p.start, p.end) {
                    (Some(s), Some(e)) => json!({"summary": title, "dtstart": local_iso(&day, s), "dtend": local_iso(&day, e)}),
                    _ => json!({"summary": title, "dtstart": day, "all_day": true}),
                };
                match api_post::<Value>("/calendar/events", &body).await {
                    Ok(_) => {
                        toast(format!("Added: {title}"));
                        q.set(String::new());
                        cx.load();
                    }
                    Err(e) => toast_bad(e.message),
                }
                return;
            }
            let due = when.map(|d| due_str(&d, p.start)).unwrap_or_default();
            if cx.todo_add(title, due, p.label.clone()).await {
                q.set(String::new());
            }
        });
    };
    let hint = move || {
        let p = p.get()?;
        let ev = p.event && p.date.is_some();
        let mut h: Vec<String> = vec![if ev { "Event".into() } else { "To-do".into() }, p.date.as_deref().map(day_name).unwrap_or_else(|| "no date".into())];
        if let Some(s) = p.start {
            h.push(format!("{}{}", hm(s), if ev { format!("–{}", hm(p.end.unwrap_or(s))) } else { String::new() }));
        }
        if let Some(l) = &p.label {
            h.push(format!("#{l}"));
        }
        if ev {
            h.push(cx.default_cal_name());
        }
        Some(view! {
            <div class="td-addhint">"↵ "{h.into_iter().map(|x| view! { <span>{x}</span> }).collect_view()}{p.guessed.then(|| view! { <span class="subdue">"looks like an event"</span> })}</div>
        })
    };
    view! {
        <div class="td-add">
            <label class="td-addbox">
                <Icon name="plus" size=14/>
                <input prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))
                    on:keydown=move |e| { if e.key() == "Enter" { e.prevent_default(); submit() } }
                    placeholder="New to-do… “fix the light #home”" aria-label="Add a to-do"/>
            </label>
            {hint}
        </div>
    }
}

/// The selected to-do, docked under the calendar like a selected event.
#[component]
pub fn TodoBar(cx: Cx, t: Todo) -> impl IntoView {
    let ed = RwSignal::new(None::<(String, String, String)>);
    let today = today_str();
    let d = t.due();
    let tt = t.clone();
    let save = move || {
        let Some((title, day, time)) = ed.get_untracked() else { return };
        if title.trim().is_empty() {
            return;
        }
        let due = if day.is_empty() { String::new() } else { due_str(&day, (!time.is_empty()).then(|| mins(&time))) };
        cx.todo_save(&tt, json!({"title": title.trim(), "due": due}));
        ed.set(None);
    };
    let t_view = t.clone();
    move || {
        if ed.with(|e| e.is_some()) {
            let save = save.clone();
            return view! {
                <form class="cal-sel ctd-sel" on:submit=move |e| { e.prevent_default(); save() }>
                    <input class="grow" prop:value=move || ed.with(|e| e.as_ref().map(|x| x.0.clone()).unwrap_or_default()) on:input=move |e| { let v = event_target_value(&e); ed.update(|x| if let Some(x) = x { x.0 = v }) } aria-label="Title" autofocus/>
                    <input type="date" prop:value=move || ed.with(|e| e.as_ref().map(|x| x.1.clone()).unwrap_or_default()) on:input=move |e| { let v = event_target_value(&e); ed.update(|x| if let Some(x) = x { x.1 = v }) } aria-label="Date"/>
                    <input type="time" step="900" prop:value=move || ed.with(|e| e.as_ref().map(|x| x.2.clone()).unwrap_or_default()) disabled=move || ed.with(|e| e.as_ref().is_none_or(|x| x.1.is_empty()))
                        on:input=move |e| { let v = event_target_value(&e); ed.update(|x| if let Some(x) = x { x.2 = v }) } aria-label="Time"/>
                    <button class="btn primary">"Save"</button>
                    <button type="button" class="btn" on:click=move |_| ed.set(None)>"Cancel"</button>
                </form>
            }
            .into_any();
        }
        let t = t_view.clone();
        let d = d.clone();
        let (t1, t2, t3, t4, t5, t6) = (t.clone(), t.clone(), t.clone(), t.clone(), t.clone(), t.clone());
        let later = d.as_ref().is_some_and(|d| d.day > today);
        let next_day = add_days(if later { &d.as_ref().unwrap().day } else { &today }, 1);
        let (d1, d2) = (d.clone(), d.clone());
        let info = format!(
            "To-do · {}{}",
            d.as_ref().map(|d| format!("{} · {}", fmt_short(&d.day), d.min.map(hm).unwrap_or_else(|| "no time".into()))).unwrap_or_else(|| "no date".into()),
            if t.label.is_empty() { String::new() } else { format!(" · #{}", t.label) }
        );
        view! {
            <div class="cal-sel ctd-sel">
                <div class="grow" style="min-width:0">
                    <div class=if t.done { "hi cal-done-t" } else { "hi" }>{t.done.then_some("✓ ")}{t.title.clone()}</div>
                    <div class="small muted">{info}</div>
                </div>
                <div class="cal-sel-acts">
                    <button class="btn" on:click=move |_| cx.todo_done(&t1)><Icon name="check" size=14/>{if t.done { " Not done" } else { " Done" }}</button>
                    {(!t.done).then(|| view! { <button class="btn" on:click=move |_| cx.todo_due(&t2, &next_day, None)>{if later { "Move to the next day" } else { "Move to tomorrow" }}</button> })}
                    <button class="btn" on:click=move |_| ed.set(Some((t3.title.clone(), d1.as_ref().map(|d| d.day.clone()).unwrap_or_default(), d1.as_ref().and_then(|d| d.min).map(hm).unwrap_or_default())))>
                        <Icon name="edit" size=14/>" Edit"
                    </button>
                    {d2.as_ref().filter(|d| d.min.is_some()).map(|d| { let day = d.day.clone(); view! { <button class="btn" on:click=move |_| cx.todo_due(&t4, &day, Some(None))>"Remove time"</button> } })}
                    {d2.is_some().then(|| view! { <button class="btn" on:click=move |_| cx.todo_due(&t5, "", None)>"No date"</button> })}
                    <button class="icon-btn" on:click=move |_| { let _ = &t6; cx.sel_todo.set(None) } aria-label="Close"><Icon name="x"/></button>
                </div>
            </div>
        }
        .into_any()
    }
}
