//! Week and day views: the hours as a grid, an all-day row on top, the now line, overlapping
//! events side by side. Drag an event to move it, its bottom edge to resize it, or drag on empty
//! time to make a new one; every change goes through the page, which asks before writing.
//! Today's column also shows the assistant's plan (to-dos for free time, "leave by" lines).
//! To-dos sit on the grid too: a timed one as a dashed 30-minute block, a dated one in the
//! all-day row; drag one (from here or the to-do panel) to a time or a day.
use super::lists::{okey, tkey};
use super::util::*;
use super::{drag_todo, has_todo, left_for_good, target_in, todo_data, Cx, Gap};
use agentum_ui::prelude::*;
use wasm_bindgen::JsCast;

const HOUR: f64 = 48.0;
fn px(m: i32) -> f64 {
    m as f64 * HOUR / 60.0
}
fn snap(m: f64) -> i32 {
    ((m / 15.0).round() * 15.0) as i32
}

#[derive(Clone, Debug, PartialEq)]
struct Drag {
    id: String,
    read_only: bool,
    resize: bool,
    x0: i32,
    y0: i32,
    col_w: f64,
    d_min: i32,
    d_day: i32,
    moved: bool,
}
#[derive(Clone, Debug, PartialEq)]
struct Mk {
    day: String,
    a: i32,
    b: i32,
    y0: i32,
    mouse: bool,
}

/// Shift a wall-clock ISO time by days and minutes, staying in wall-clock terms.
fn shift(iso: &str, d_day: i32, d_min: i32) -> String {
    let t = min_of(iso) + d_min;
    let carry = t.div_euclid(1440);
    format!("{}T{}:00", add_days(&day_of(iso), (d_day + carry) as i64), hm(t.rem_euclid(1440)))
}

fn rect_of(e: &web_sys::Event) -> Option<web_sys::DomRect> {
    e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).map(|el| el.get_bounding_client_rect())
}

#[derive(Clone, PartialEq)]
enum Item {
    Ev(Occ),
    Td(Todo),
}

#[component]
pub fn TimeGrid(cx: Cx, days: Vec<String>) -> impl IntoView {
    let boxr = NodeRef::<leptos::html::Div>::new();
    let drag = RwSignal::new(None::<Drag>);
    let mk = RwSignal::new(None::<Mk>);
    let open_gap = RwSignal::new(None::<String>);
    let ghost = RwSignal::new(None::<(String, i32)>);
    let grab = StoredValue::new(0.0f64);
    let tick = RwSignal::new(0u32);
    if let Ok(h) = set_interval_with_handle(move || tick.update(|n| *n += 1), std::time::Duration::from_secs(60)) {
        on_cleanup(move || h.clear());
    }
    let today = today_str();
    let week = days.len() > 1;
    let n = days.len();

    // Open at the hour that matters: just before now on this week, else 07:00; narrow panes
    // scroll sideways to today.
    {
        let has_today = days.contains(&today);
        let today = today.clone();
        Effect::new(move |_| {
            let Some(el) = boxr.get() else { return };
            el.set_scroll_top(px(if has_today { (now_min() - 120).max(0) } else { 7 * 60 }) as i32);
            let col = el.query_selector(&format!(".tg-col[data-date=\"{today}\"]")).ok().flatten().and_then(|c| c.dyn_into::<web_sys::HtmlElement>().ok());
            el.set_scroll_left(match col {
                Some(c) if el.scroll_width() > el.client_width() + 4 => (c.offset_left() - 52).max(0),
                _ => 0,
            });
        });
    }

    let all_day = move |d: &str| {
        let mut v: Vec<Occ> = cx.events.with(|l| l.iter().flatten().filter(|o| (o.all_day || days_of(o).len() > 2) && covers(o, d)).cloned().collect());
        v.sort_by(by_time);
        v
    };
    let due_on = move |d: &str, timed: bool| cx.todos.with(|ts| ts.iter().filter(|t| t.due().is_some_and(|x| x.day == d && x.min.is_some() == timed)).cloned().collect::<Vec<_>>());
    let timed = move |d: &str| {
        let mut items: Vec<(i32, i32, Item)> = cx.events.with(|l| {
            l.iter()
                .flatten()
                .filter(|o| !o.all_day && days_of(o).len() <= 2 && covers(o, d))
                .map(|o| {
                    let (s, e) = segment(o, d);
                    (s, e, Item::Ev(o.clone()))
                })
                .collect()
        });
        for t in due_on(d, true) {
            let s = t.due().and_then(|x| x.min).unwrap_or(0);
            items.push((s, (s + 30).min(1440), Item::Td(t)));
        }
        layout_lanes(items)
    };

    // A to-do dragged over a column: where it would land (15-min steps, from the top of the
    // block you grabbed).
    let drop_min = move |e: &web_sys::DragEvent| {
        let top = rect_of(e).map(|r| r.top()).unwrap_or(0.0);
        snap((e.client_y() as f64 - top - grab.get_value()) / HOUR * 60.0).clamp(0, 1425)
    };
    let td_drop = move |e: &web_sys::DragEvent, day: &str, timed: bool| {
        let id = todo_data(e);
        ghost.set(None);
        if id.is_empty() {
            return;
        }
        e.prevent_default();
        cx.drop_todo(&id, day, if timed { Some(drop_min(e)) } else { None });
    };

    let ev_down = move |e: &web_sys::PointerEvent, o: &Occ, resize: bool| {
        if e.button() != 0 {
            return;
        }
        e.stop_propagation();
        let mut d = Drag { id: o.id.clone(), read_only: o.read_only, resize, x0: e.client_x(), y0: e.client_y(), col_w: 1.0, d_min: 0, d_day: 0, moved: false };
        if e.pointer_type() == "mouse" && !o.read_only {
            let el = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
            if let Some(el) = el {
                d.col_w = el.closest(".tg-col").ok().flatten().map(|c| c.get_bounding_client_rect().width()).unwrap_or(1.0);
                let _ = el.set_pointer_capture(e.pointer_id());
            }
        }
        drag.set(Some(d));
    };
    let ev_move = move |e: &web_sys::PointerEvent| {
        let Some(mut d) = drag.get_untracked() else { return };
        if d.read_only || e.pointer_type() != "mouse" {
            return;
        }
        let (dx, dy) = (e.client_x() - d.x0, e.client_y() - d.y0);
        let moved = d.moved || dx.abs() > 4 || dy.abs() > 4;
        let d_min = snap(dy as f64 / HOUR * 60.0);
        let d_day = if !d.resize && week { (dx as f64 / d.col_w).round() as i32 } else { 0 };
        if moved != d.moved || d_min != d.d_min || d_day != d.d_day {
            d.moved = moved;
            d.d_min = d_min;
            d.d_day = d_day;
            drag.set(Some(d));
        }
    };
    let ev_up = move |o: &Occ| {
        let d = drag.get_untracked();
        drag.set(None);
        let Some(d) = d.filter(|d| d.id == o.id) else { return };
        if !d.moved {
            cx.select(o);
            return;
        }
        if d.d_min == 0 && d.d_day == 0 {
            return;
        }
        if !d.resize {
            cx.move_to(o.clone(), shift(&o.start, d.d_day, d.d_min), shift(&o.end, d.d_day, d.d_min));
        } else {
            let end = shift(&o.end, 0, d.d_min);
            let min_end = shift(&o.start, 0, 15);
            cx.move_to(o.clone(), o.start.clone(), if end > min_end { end } else { min_end });
        }
    };

    // Empty time: a click makes a 1-hour event, a drag picks the length.
    let col_y = |e: &web_sys::PointerEvent| {
        let top = rect_of(e).map(|r| r.top()).unwrap_or(0.0);
        snap((e.client_y() as f64 - top) / HOUR * 60.0).clamp(0, 1440)
    };
    let col_down = move |e: web_sys::PointerEvent, day: String| {
        if e.button() != 0 || target_in(&e, ".tg-ev, .tg-plan") {
            return;
        }
        let mouse = e.pointer_type() == "mouse";
        if mouse {
            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                let _ = el.set_pointer_capture(e.pointer_id());
            }
        }
        let top = rect_of(&e).map(|r| r.top()).unwrap_or(0.0);
        let a = (((e.client_y() as f64 - top) / HOUR * 4.0).floor() as i32 * 15).min(1425);
        mk.set(Some(Mk { day, a, b: a, y0: e.client_y(), mouse }));
    };
    let col_move = move |e: web_sys::PointerEvent| {
        if let Some(mut m) = mk.get_untracked().filter(|m| m.mouse) {
            let b = col_y(&e);
            if b != m.b {
                m.b = b;
                mk.set(Some(m));
            }
        }
    };
    let col_up = move |e: web_sys::PointerEvent| {
        let Some(m) = mk.get_untracked() else { return };
        mk.set(None);
        if !m.mouse && (e.client_y() - m.y0).abs() > 8 {
            return;
        }
        let (lo, hi) = (m.a.min(m.b), m.a.max(m.b));
        if hi - lo < 15 {
            cx.new_at(&m.day, Some(m.a), Some((m.a + 60).min(1440)));
        } else {
            cx.new_at(&m.day, Some(lo), Some(hi));
        }
    };

    let chip = move |o: Occ| {
        let (o1, o2, o3, o4) = (o.clone(), o.clone(), o.clone(), o.clone());
        let id = o.id.clone();
        let done = Memo::new(move |_| cx.is_done(&o4));
        view! {
            <button class=move || format!("tg-chip{}{}", if cx.sel_id.get().as_deref() == Some(id.as_str()) { " sel" } else { "" }, if done.get() { " done" } else { "" })
                style=format!("--kc:{}", kind_color(&o.kind)) title=format!("{}\n{}", o.title, o.calendar)
                on:click=move |_| cx.select(&o1) on:dblclick=move |_| cx.open(&o2)
                on:contextmenu=move |e| { e.prevent_default(); cx.on_menu(e.client_x(), e.client_y(), &o3) }>
                {move || done.get().then_some("✓ ")}{o.title.clone()}
            </button>
        }
    };
    let todo_chip = move |t: Todo| {
        let (t1, t2, t3) = (t.clone(), t.clone(), t.clone());
        let id = t.id.clone();
        let label = format!("{}: {}", if t.done { "Not done" } else { "Mark done" }, t.title);
        view! {
            <button class=move || format!("tg-chip tg-tdc{}{}", if t.done { " done" } else { "" }, if cx.sel_todo.get().as_deref() == Some(id.as_str()) { " sel" } else { "" })
                data-todo=t.id.clone() title=t.title.clone() draggable=if t.done { "false" } else { "true" }
                on:dragstart=move |e| drag_todo(&e, &t1) on:click=move |_| cx.pick_todo(&t2)>
                <span role="checkbox" aria-checked=t.done.to_string() aria-label=label class="td-ring" on:click=move |e| { e.stop_propagation(); cx.todo_done(&t3) }/>
                {t.title.clone()}
            </button>
        }
    };

    let event_block = move |o: Occ, s: i32, e: i32, pos: String| {
        let id = o.id.clone();
        let (o1, o2, o3, o4, o5, o6) = (o.clone(), o.clone(), o.clone(), o.clone(), o.clone(), o.clone());
        let mine = Memo::new({
            let id = id.clone();
            move |_| drag.with(|d| d.as_ref().filter(|d| d.id == id && d.moved).cloned())
        });
        let h = Memo::new(move |_| (px(e - s) + mine.get().filter(|d| d.resize).map(|d| px(d.d_min)).unwrap_or(0.0)).max(18.0));
        let done = Memo::new(move |_| cx.is_done(&o4));
        let sel_id = id.clone();
        let tip = format!("{}–{} {}{}\n{}", hm(s), hm(e), o.title, if o.location.is_empty() { String::new() } else { format!(" · {}", o.location) }, o.calendar);
        let kc = kind_color(&o.kind);
        let loc = o.location.clone();
        view! {
            <div class=move || format!("tg-ev{}{}{}{}", if cx.sel_id.get().as_deref() == Some(sel_id.as_str()) { " sel" } else { "" }, if done.get() { " done" } else { "" }, if mine.with(|m| m.is_some()) { " dragging" } else { "" }, if h.get() < 34.0 { " short" } else { "" })
                style=move || {
                    let dr = mine.get();
                    let tr = dr.filter(|d| !d.resize).map(|d| format!(";transform:translate({}px, {}px)", d.d_day as f64 * d.col_w, px(d.d_min))).unwrap_or_default();
                    format!("{pos};height:{}px;--kc:{kc}{tr}", h.get())
                }
                title=tip
                on:pointerdown=move |ev| ev_down(&ev, &o1, false) on:pointermove=move |ev| ev_move(&ev) on:pointerup=move |_| ev_up(&o2) on:pointercancel=move |_| drag.set(None)
                on:dblclick=move |_| cx.open(&o3) on:contextmenu=move |ev| { ev.prevent_default(); cx.on_menu(ev.client_x(), ev.client_y(), &o5) }>
                <div class="tg-evt"><i class="cal-dot" style=format!("background:{}", safe_color(&o.color))/>{move || done.get().then_some("✓ ")}{o.title.clone()}</div>
                {
                    let loc = loc.clone();
                    move || (h.get() >= 34.0).then(|| {
                        let dr = mine.get();
                        let t = match dr {
                            Some(d) => format!("{}–{}", hm(s + if d.resize { 0 } else { d.d_min }), hm(e + d.d_min)),
                            None => format!("{}–{}", hm(s), hm(e)),
                        };
                        view! { <div class="tg-evs">{t}{if loc.is_empty() { String::new() } else { format!(" · {loc}") }}</div> }
                    })
                }
                {(!o.read_only).then(|| view! { <span class="tg-rs" on:pointerdown=move |ev| ev_down(&ev, &o6, true)/> })}
            </div>
        }
    };
    let todo_block = move |t: Todo, s: i32, e: i32, pos: String| {
        let (t1, t2, t3) = (t.clone(), t.clone(), t.clone());
        let id = t.id.clone();
        let label = format!("{}: {}", if t.done { "Not done" } else { "Mark done" }, t.title);
        view! {
            <div class=move || format!("tg-ev tg-td short{}{}", if t.done { " done" } else { "" }, if cx.sel_todo.get().as_deref() == Some(id.as_str()) { " sel" } else { "" })
                data-todo=t.id.clone() style=format!("{pos};height:{}px", px(e - s)) title=format!("{} {} (to-do)", hm(s), t.title)
                draggable=if t.done { "false" } else { "true" }
                on:dragstart=move |ev| {
                    let top = rect_of(&ev).map(|r| r.top()).unwrap_or(0.0);
                    grab.set_value(ev.client_y() as f64 - top);
                    drag_todo(&ev, &t1)
                }
                on:dragend=move |_| { grab.set_value(0.0); ghost.set(None) }
                on:click=move |_| cx.pick_todo(&t2)>
                <button type="button" class="td-ring" aria-pressed=t.done.to_string() aria-label=label on:click=move |ev| { ev.stop_propagation(); cx.todo_done(&t3) }/>
                <span class="tg-evt">{t.title.clone()}</span>
            </div>
        }
    };

    let plan = move |d: String| {
        let gaps: Vec<Gap> = cx.gaps.with(|g| cx.skipped.with(|sk| g.iter().filter(|g| g.date == d && g.todo.as_ref().is_some_and(|t| !sk.contains(&t.id))).cloned().collect()));
        gaps.into_iter()
            .map(|g| {
                let s = mins(&g.from);
                let e = mins(&g.to).min(s + 60);
                let t = g.todo.clone().unwrap_or_default();
                let k = t.id.clone();
                let k2 = k.clone();
                let why = g.why.clone().filter(|w| !w.is_empty()).map(|w| format!(" · {w}")).unwrap_or_default();
                let tip = format!("Free {}–{}{why} (suggested by {})", g.from, g.to, if g.by.as_deref() == Some("model") { "your model" } else { "due date" });
                let (g1, g2) = (g.clone(), g.clone());
                view! {
                    <div class=move || if open_gap.get().as_deref() == Some(k.as_str()) { "tg-plan open" } else { "tg-plan" }
                        style=format!("top:{}px;min-height:{}px", px(s) + 1.0, (px(e - s) - 2.0).max(22.0)) title=tip
                        on:click=move |_| open_gap.update(|o| *o = if o.as_deref() == Some(k2.as_str()) { None } else { Some(k2.clone()) })>
                        <b>"✦ "{t.title.clone()}</b><small>{format!("free {}–{}{why}", g.from, g.to)}</small>
                        <Show when={ let k = t.id.clone(); move || open_gap.get().as_deref() == Some(k.as_str()) }>
                            <span class="row" style="gap:4px;margin-top:4px">
                                <button class="btn primary" on:click={ let g = g1.clone(); move |ev| { ev.stop_propagation(); cx.put_in(g.clone()) } }>"Put it in"</button>
                                <button class="btn" on:click={ let g = g2.clone(); move |ev| { ev.stop_propagation(); cx.skip(&g) } }>"Not now"</button>
                            </span>
                        </Show>
                    </div>
                }
            })
            .collect_view()
    };

    let today2 = today.clone();
    let heads = days.clone();
    let ads = days.clone();
    view! {
        <div class=if week { "tg week" } else { "tg one" } node_ref=boxr style=format!("--n:{n}")>
            <div class="tg-top">
                <div class="tg-gut"/>
                {heads.into_iter().map(|d| {
                    let d2 = d.clone();
                    view! {
                        <button class=if d == today2 { "tg-dh today" } else { "tg-dh" } on:click=move |_| cx.go(&d2, CalView::Day) title="Open this day">
                            <span class="subdue">{fmt_day(&d, &[("weekday", "short")])}</span>" "<b>{fmt_day(&d, &[("day", "numeric")])}</b>
                        </button>
                    }
                }).collect_view()}
                <div class="tg-gut small subdue">"all-day"</div>
                {ads.into_iter().map(|d| {
                    let (d1, d2, d3, d4) = (d.clone(), d.clone(), d.clone(), d.clone());
                    let list = Memo::new(move |_| all_day(&d1));
                    let tds = Memo::new(move |_| due_on(&d2, false));
                    let more = move || list.with(|l| l.len().saturating_sub(3)) + tds.with(|t| t.len().saturating_sub(3));
                    view! {
                        <div class="tg-ad" data-date=d.clone() on:dragover=move |e| { if has_todo(&e) { e.prevent_default() } } on:drop=move |e| td_drop(&e, &d3, false)>
                            <For each=move || list.get().into_iter().take(3) key=okey let:o>{chip(o)}</For>
                            <For each=move || tds.get().into_iter().take(3) key=tkey let:t>{todo_chip(t)}</For>
                            {move || (more() > 0).then(|| { let d = d4.clone(); view! { <button class="tg-more" on:click=move |_| cx.go(&d, CalView::Day)>"+"{more()}" more"</button> } })}
                        </div>
                    }
                }).collect_view()}
            </div>
            <div class="tg-grid" style=format!("height:{}px", px(1440))>
                <div class="tg-gut tg-hours">{(1..24).map(|i| view! { <span style=format!("top:{}px", px(i * 60))>{hm(i * 60)}</span> }).collect_view()}</div>
                {days.into_iter().map(|d| {
                    let is_today = d == today;
                    let (d1, d2, d3, d4, d5, d6) = (d.clone(), d.clone(), d.clone(), d.clone(), d.clone(), d.clone());
                    let items = Memo::new(move |_| timed(&d1));
                    view! {
                        <div class=if is_today { "tg-col today" } else { "tg-col" } data-date=d.clone()
                            on:pointerdown=move |e| col_down(e, d2.clone()) on:pointermove=col_move on:pointerup=col_up on:pointercancel=move |_| mk.set(None)
                            on:contextmenu=move |e| { if !target_in(&e, ".tg-ev") { e.prevent_default() } }
                            on:dragover=move |e| {
                                if !has_todo(&e) { return }
                                e.prevent_default();
                                let m = drop_min(&e);
                                if ghost.with_untracked(|g| g.as_ref() != Some(&(d3.clone(), m))) { ghost.set(Some((d3.clone(), m))) }
                            }
                            on:dragleave=move |e| { if left_for_good(&e) { ghost.set(None) } }
                            on:drop=move |e| td_drop(&e, &d4, true)>
                            <For each=move || items.get() key=|(s, e, lane, lanes, it)| match it {
                                Item::Ev(o) => format!("e|{}|{s}|{e}|{lane}|{lanes}", okey(o)),
                                Item::Td(t) => format!("t|{}|{s}|{e}|{lane}|{lanes}", tkey(t)),
                            } let:item>
                                {
                                    let (s, e, lane, lanes, it) = item;
                                    let pos = format!("top:{}px;left:calc({}% + 1px);width:calc({}% - 3px)", px(s), lane as f64 / lanes as f64 * 100.0, 100.0 / lanes as f64);
                                    match it {
                                        Item::Ev(o) => event_block(o, s, e, pos).into_any(),
                                        Item::Td(t) => todo_block(t, s, e, pos).into_any(),
                                    }
                                }
                            </For>
                            {is_today.then(|| { let d = d5.clone(); move || plan(d.clone()) })}
                            {is_today.then(|| move || cx.leave.get().into_iter().map(|x| view! {
                                <div class="tg-leave" style=format!("top:{}px", px(mins(&x.leave))) title=format!("{} at {} · ~{} min{}", x.title, x.location, x.travel, if x.rain { " · rain" } else { "" })>
                                    <span>"⇢ leave "{x.leave.clone()}</span>
                                </div>
                            }).collect_view())}
                            {move || mk.get().filter(|m| m.mouse && m.day == d6).map(|m| {
                                let (lo, hi) = (m.a.min(m.b), m.a.max(m.b));
                                view! { <div class="tg-new" style=format!("top:{}px;height:{}px", px(lo), px(15).max(px(hi - lo)))>{format!("{}–{}", hm(lo), hm(hi.max(lo + 15)))}</div> }
                            })}
                            {
                                let d = d.clone();
                                move || ghost.get().filter(|g| g.0 == d).map(|g| view! { <div class="tg-new" style=format!("top:{}px;height:{}px", px(g.1), px(30))>{hm(g.1)}</div> })
                            }
                            {is_today.then(|| view! { <div class="tg-now" style=move || { tick.track(); format!("top:{}px", px(now_min())) }/> })}
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}
