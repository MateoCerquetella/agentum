//! The month (with the chosen day's list under it) and the agenda list. The month also shows the
//! to-dos due each day (after the events); drop one on a day.
use super::util::*;
use super::{drag_todo, has_todo, left_for_good, todo_data, Cx};
use agentum_ui::prelude::*;

/// What identifies an occurrence's look (a changed one is drawn anew).
pub fn okey(o: &Occ) -> String {
    format!("{}|{}|{}|{}|{}|{}|{}|{}", o.id, o.start, o.end, o.title, o.color, o.kind, o.location, o.read_only)
}
pub fn tkey(t: &Todo) -> String {
    format!("{}|{}|{}|{}|{}", t.id, t.title, t.due, t.label, t.done)
}

/// One event in a list: time, title, calendar · place, and its ✓.
#[component]
pub fn EventRow(cx: Cx, o: Occ, day: String) -> impl IntoView {
    let time = if o.all_day {
        "all day".to_string()
    } else if day_of(&o.start) < day {
        format!("until {}", hm(min_of(&o.end)))
    } else {
        format!("{}–{}", hm(min_of(&o.start)), if day_of(&o.end) > day { "…".to_string() } else { hm(min_of(&o.end)) })
    };
    let place = [o.calendar.clone(), o.location.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let (o1, o2, o3, o4, o5) = (o.clone(), o.clone(), o.clone(), o.clone(), o.clone());
    let id = o.id.clone();
    let done = Memo::new(move |_| cx.is_done(&o5));
    view! {
        <div class=move || format!("cal-row{}{}", if cx.sel_id.get().as_deref() == Some(id.as_str()) { " sel" } else { "" }, if done.get() { " done" } else { "" })
            style=format!("--kc:{}", kind_color(&o.kind))
            on:click=move |_| cx.select(&o1) on:dblclick=move |_| cx.open(&o2)
            on:contextmenu=move |e| { e.prevent_default(); cx.on_menu(e.client_x(), e.client_y(), &o3) }>
            <span class="t small">{time}</span>
            <span class="grow" style="min-width:0">
                <span class="ti truncate">{o.title.clone()}</span>
                <span class="small subdue truncate" style="display:block"><i class="cal-dot" style=format!("background:{}", safe_color(&o.color))/>{place}</span>
            </span>
            <button class=move || if done.get() { "icon-btn cal-tick on" } else { "icon-btn cal-tick" } title=move || if done.get() { "Not done" } else { "Mark done" }
                aria-pressed=move || done.get().to_string() on:click=move |e| { e.stop_propagation(); cx.toggle_done(o4.clone()) }>"✓"</button>
        </div>
    }
}

#[component]
pub fn MonthView(cx: Cx, days: Vec<String>) -> impl IntoView {
    let today = today_str();
    let over = RwSignal::new(String::new());
    let date = cx.date;
    let m = date.get_untracked().get(..7).unwrap_or("").to_string();
    // Six weeks unless the last one is all next month.
    let days: Vec<String> = if days.len() > 35 && days[35..].iter().all(|d| d.get(..7) != Some(m.as_str())) { days[..35].to_vec() } else { days };
    let rows = days.len() / 7;
    let pick = move |d: &str| set_param("date", Some(d));
    let cell = move |d: String| {
        let m = m.clone();
        let today = today.clone();
        let evs = Memo::new({
            let d = d.clone();
            move |_| cx.events.with(|l| on_day(l.as_deref().unwrap_or(&[]), &d))
        });
        let tds = Memo::new({
            let d = d.clone();
            move |_| {
                let mut v: Vec<Todo> = cx.todos.with(|ts| ts.iter().filter(|t| t.due().is_some_and(|x| x.day == d)).cloned().collect());
                v.sort_by_key(|t| t.done);
                v
            }
        });
        let holiday = move || evs.with(|e| e.iter().any(|o| o.kind == "holiday"));
        let (d1, d2, d3, d4, d5, d6, d7) = (d.clone(), d.clone(), d.clone(), d.clone(), d.clone(), d.clone(), d.clone());
        let num = day_num(&d);
        let base = format!("mv-day{}{}{}", if d.get(..7) != Some(m.as_str()) { " spill" } else { "" }, if d == today { " today" } else { "" }, if d < today { " past" } else { "" });
        view! {
            <div class=move || format!("{base}{}{}", if date.get() == d1 { " picked" } else { "" }, if over.get() == d1 { " over" } else { "" })
                data-date=d on:click=move |_| pick(&d2) on:dblclick=move |_| cx.new_at(&d3, None, None)
                on:dragover=move |e| { if has_todo(&e) { e.prevent_default(); over.set(d4.clone()) } }
                on:dragleave=move |e| { if left_for_good(&e) { over.set(String::new()) } }
                on:drop=move |e| { over.set(String::new()); let id = todo_data(&e); if !id.is_empty() { e.prevent_default(); cx.drop_todo(&id, &d5, None) } }>
                <span class=move || if holiday() { "mv-n neg" } else { "mv-n" }>{num}</span>
                <div class="mv-evs">
                    <For each=move || evs.get().into_iter().take(3) key=okey let:o>
                        {
                            let (o1, o2, o3, o4) = (o.clone(), o.clone(), o.clone(), o.clone());
                            let id = o.id.clone();
                            let dd = d6.clone();
                            let tip = format!("{}{}\n{}", if o.all_day { String::new() } else { format!("{} ", hm(min_of(&o.start))) }, o.title, o.calendar);
                            view! {
                                <div class=move || format!("mv-ev{}{}{}", if o.all_day { " ad" } else { "" }, if cx.is_done(&o4) { " done" } else { "" }, if cx.sel_id.get().as_deref() == Some(id.as_str()) { " sel" } else { "" })
                                    style=format!("--kc:{}", kind_color(&o.kind)) title=tip
                                    on:click=move |e| { e.stop_propagation(); pick(&dd); cx.select(&o1) }
                                    on:dblclick=move |e| { e.stop_propagation(); cx.open(&o2) }
                                    on:contextmenu=move |e| { e.prevent_default(); e.stop_propagation(); cx.on_menu(e.client_x(), e.client_y(), &o3) }>
                                    {(!o.all_day).then(|| view! { <span class="subdue">{hm(min_of(&o.start))}" "</span> })}{o.title.clone()}
                                </div>
                            }
                        }
                    </For>
                    <For each=move || { let n = evs.with(|e| e.len()); tds.get().into_iter().take(3usize.saturating_sub(n)) } key=tkey let:t>
                        {
                            let min = t.due().and_then(|x| x.min);
                            let (t1, t2, t3) = (t.clone(), t.clone(), t.clone());
                            let id = t.id.clone();
                            let dd = d7.clone();
                            let label = format!("{}: {}", if t.done { "Not done" } else { "Mark done" }, t.title);
                            view! {
                                <div class=move || format!("mv-ev mv-td{}{}", if t.done { " done" } else { "" }, if cx.sel_todo.get().as_deref() == Some(id.as_str()) { " sel" } else { "" })
                                    data-todo=t.id.clone() title=format!("{}{} (to-do)", min.map(|m| format!("{} ", hm(m))).unwrap_or_default(), t.title)
                                    draggable=if t.done { "false" } else { "true" } on:dragstart=move |e| drag_todo(&e, &t1)
                                    on:click=move |e| { e.stop_propagation(); pick(&dd); cx.pick_todo(&t2) } on:dblclick=|e| e.stop_propagation()>
                                    <span role="checkbox" aria-checked=t.done.to_string() aria-label=label class="td-ring" on:click=move |e| { e.stop_propagation(); cx.todo_done(&t3) }/>
                                    {min.map(|m| view! { <span class="subdue">{hm(m)}" "</span> })}{t.title.clone()}
                                </div>
                            }
                        }
                    </For>
                    {move || { let n = evs.with(|e| e.len()) + tds.with(|t| t.len()); (n > 3).then(|| view! { <div class="mv-more">"+"{n - 3}" more"</div> }) }}
                </div>
                <div class="mv-dots">{move || evs.get().into_iter().take(4).map(|o| view! { <i style=format!("background:{}", kind_color(&o.kind))/> }).collect_view()}</div>
            </div>
        }
    };
    let dow: Vec<String> = days.iter().take(7).map(|d| fmt_day(d, &[("weekday", "short")])).collect();
    let list = Memo::new(move |_| cx.events.with(|l| on_day(l.as_deref().unwrap_or(&[]), &date.get())));
    view! {
        <div class="mv">
            <div class="mv-dow">{dow.into_iter().map(|d| view! { <span>{d}</span> }).collect_view()}</div>
            <div class="mv-grid" style=format!("grid-template-rows:repeat({rows}, minmax(88px, 1fr))")>
                {days.into_iter().map(cell).collect_view()}
            </div>
            <div class="cal-daylist">
                <div class="row">
                    <span class="hi grow">{move || fmt_day(&date.get(), &[("weekday", "long"), ("day", "numeric"), ("month", "long")])}</span>
                    <button class="btn ghost" on:click=move |_| cx.new_at(&date.get_untracked(), None, None)>"+ New event"</button>
                </div>
                <Show when=move || list.with(|l| !l.is_empty()) fallback=|| view! { <div class="small subdue">"Nothing on this day."</div> }>
                    <For each=move || list.get() key=move |o| format!("{}|{}", okey(o), date.get_untracked()) let:o>
                        <EventRow cx=cx o=o day=date.get_untracked()/>
                    </For>
                </Show>
            </div>
        </div>
    }
}

#[component]
pub fn AgendaView(cx: Cx, days: Vec<String>) -> impl IntoView {
    let today = today_str();
    let n = days.len();
    let groups = Memo::new(move |_| cx.events.with(|l| days.iter().map(|d| (d.clone(), on_day(l.as_deref().unwrap_or(&[]), d))).filter(|(_, l)| !l.is_empty()).collect::<Vec<_>>()));
    move || {
        let g = groups.get();
        if g.is_empty() {
            return view! { <div class="empty">{format!("Nothing in the next {n} days.")}</div> }.into_any();
        }
        let today = today.clone();
        view! {
            <div class="ag-list">
                {g.into_iter().map(|(d, l)| {
                    let is_today = d == today;
                    view! {
                        <section>
                            <h4 class=is_today.then_some("today")>{is_today.then_some("Today · ")}{fmt_day(&d, &[("weekday", "long"), ("day", "numeric"), ("month", "long")])}</h4>
                            {l.into_iter().map(|o| view! { <EventRow cx=cx o=o day=d.clone()/> }).collect_view()}
                        </section>
                    }
                }).collect_view()}
            </div>
        }.into_any()
    }
}
