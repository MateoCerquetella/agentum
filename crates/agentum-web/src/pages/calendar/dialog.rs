//! New / edit event. Save never writes straight away: it shows what, when and which calendar, and
//! only "Create event" / "Save changes" on that review calls the API.
use super::ask::review_list;
use super::util::*;
use super::{enc, Cx, Dlg};
use agentum_ui::prelude::*;

const REPEAT: [(&str, &str); 5] = [("", "Does not repeat"), ("DAILY", "Every day"), ("WEEKLY", "Every week"), ("MONTHLY", "Every month"), ("YEARLY", "Every year")];

#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub occ: Option<Occ>,
    pub title: String,
    pub all_day: bool,
    pub sd: String,
    pub st: String,
    pub ed: String,
    pub et: String,
    pub location: String,
    pub notes: String,
    pub calendar_id: String,
    /// "", DAILY…, or "keep" (a series stays as it is).
    pub repeat: String,
    /// "this" or "all".
    pub scope: String,
}

pub fn default_cal(cals: &[Cal]) -> Option<Cal> {
    cals.iter().find(|c| c.default && !c.read_only).or_else(|| cals.iter().find(|c| !c.read_only)).cloned()
}
pub fn new_draft(day: &str, from: i32, to: i32, cals: &[Cal]) -> Draft {
    Draft {
        occ: None,
        title: String::new(),
        all_day: false,
        sd: day.into(),
        st: hm(from),
        ed: if to >= 1440 { add_days(day, 1) } else { day.into() },
        et: hm(if to >= 1440 { 0 } else { to }),
        location: String::new(),
        notes: String::new(),
        calendar_id: default_cal(cals).map(|c| c.id).unwrap_or_default(),
        repeat: String::new(),
        scope: "this".into(),
    }
}
pub fn draft_from(o: &Occ) -> Draft {
    let sd = day_of(&o.start);
    let e = day_of(if o.end.is_empty() { &o.start } else { &o.end });
    Draft {
        occ: Some(o.clone()),
        title: o.title.clone(),
        all_day: o.all_day,
        st: hm(min_of(&o.start)),
        ed: if o.all_day { if e > sd { add_days(&e, -1) } else { sd.clone() } } else { e },
        et: hm(min_of(if o.end.is_empty() { &o.start } else { &o.end })),
        sd,
        location: o.location.clone(),
        notes: o.description.clone(),
        calendar_id: o.calendar_id.clone(),
        repeat: if o.recurring { "keep".into() } else { String::new() },
        scope: "this".into(),
    }
}
/// The draft as API fields. All-day: local midnights with an exclusive end, like GET returns them.
fn fields(d: &Draft) -> Value {
    json!({
        "summary": d.title.trim(),
        "dtstart": if d.all_day { format!("{}T00:00:00", d.sd) } else { local_iso(&d.sd, mins(&d.st)) },
        "dtend": if d.all_day { format!("{}T00:00:00", add_days(&d.ed, 1)) } else { local_iso(&d.ed, mins(&d.et)) },
        "all_day": d.all_day,
        "location": d.location.trim(),
        "description": d.notes.trim(),
        "calendar_id": d.calendar_id,
    })
}
fn when(d: &Draft) -> String {
    let f = fields(d);
    when_text(f["dtstart"].as_str().unwrap_or(""), f["dtend"].as_str().unwrap_or(""), d.all_day)
}
/// Editing sends only what changed (a series keeps its own first date unless you moved it).
fn patch(d: &Draft) -> Value {
    let o = d.occ.clone().unwrap_or_default();
    let now = fields(d);
    let was = fields(&draft_from(&o));
    let mut p = serde_json::Map::new();
    for (k, v) in now.as_object().unwrap() {
        if was.get(k) != Some(v) {
            p.insert(k.clone(), v.clone());
        }
    }
    if p.contains_key("dtstart") || p.contains_key("dtend") || p.contains_key("all_day") {
        for k in ["dtstart", "dtend", "all_day"] {
            p.insert(k.into(), now[k].clone());
        }
    }
    let series_edit = o.recurring && d.scope == "all";
    if d.repeat != "keep" && (!o.recurring || series_edit) && (!d.repeat.is_empty() || o.recurring) {
        p.insert("rrule".into(), json!(if d.repeat.is_empty() { String::new() } else { format!("FREQ={}", d.repeat) }));
    }
    if o.recurring {
        p.insert("scope".into(), json!(d.scope));
    }
    Value::Object(p)
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Edit,
    Save,
    Delete,
}

#[component]
pub fn EventDialog(cx: Cx, dlg: Dlg) -> impl IntoView {
    let d = RwSignal::new(dlg.draft.clone());
    let step = RwSignal::new(if dlg.del { Step::Delete } else { Step::Edit });
    let err = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let o = dlg.draft.occ.clone();
    let ro = o.as_ref().is_some_and(|o| o.read_only);
    let recurring = o.as_ref().is_some_and(|o| o.recurring);
    let cals = cx.cals;
    let close = Callback::new(move |_| cx.dlg.set(None));
    let set = move |f: &dyn Fn(&mut Draft)| {
        d.update(|x| f(x));
        err.set(String::new());
    };
    let review = move || {
        let x = d.get_untracked();
        let f = fields(&x);
        if f["summary"].as_str().unwrap_or("").is_empty() {
            return err.set("Give it a title.".into());
        }
        if x.calendar_id.is_empty() {
            return err.set("Pick a calendar.".into());
        }
        if f["dtend"].as_str() <= f["dtstart"].as_str() {
            return err.set("It has to end after it starts.".into());
        }
        step.set(Step::Save);
    };
    let o_run = o.clone();
    let run = move || {
        busy.set(true);
        let x = d.get_untracked();
        let st = step.get_untracked();
        let o = o_run.clone();
        let cal_name = cals.with_untracked(|c| c.iter().find(|c| c.id == x.calendar_id).map(|c| c.name.clone()));
        spawn_local(async move {
            let res = match (st, &o) {
                (Step::Delete, Some(o)) => {
                    let q = if o.recurring { format!("?scope={}", x.scope) } else { String::new() };
                    api_delete::<Value>(&format!("/calendar/events/{}{q}", enc(&o.id))).await.map(|_| format!("Deleted “{}”", o.title))
                }
                (_, Some(o)) => api_put::<Value>(&format!("/calendar/events/{}", enc(&o.id)), &patch(&x)).await.map(|_| "Saved".to_string()),
                (_, None) => {
                    let mut body = fields(&x);
                    if !x.repeat.is_empty() && x.repeat != "keep" {
                        body["rrule"] = json!(format!("FREQ={}", x.repeat));
                    }
                    api_post::<Value>("/calendar/events", &body).await.map(|_| format!("Added to {}", cal_name.unwrap_or_else(|| "your calendar".into())))
                }
            };
            match res {
                Ok(msg) => {
                    if st == Step::Delete {
                        cx.sel_id.set(None);
                    }
                    cx.saved(msg);
                }
                Err(e) => {
                    toast_bad(e.message);
                    busy.set(false);
                }
            }
        });
    };

    let o_rev = o.clone();
    let reviewing = move || {
        let del = step.get() == Step::Delete;
        let x = d.get();
        let o = o_rev.clone();
        let cal = cals.with(|c| c.iter().find(|c| c.id == x.calendar_id).cloned());
        let series_edit = recurring && x.scope == "all";
        let mut rows: Vec<(String, String)> = vec![
            ("What".into(), if x.title.trim().is_empty() { o.as_ref().map(|o| o.title.clone()).unwrap_or_default() } else { x.title.trim().to_string() }),
            ("When".into(), if del { o.as_ref().map(when_of).unwrap_or_default() } else { when(&x) }),
            ("Calendar".into(), cal.as_ref().map(|c| c.name.clone()).or_else(|| o.as_ref().map(|o| o.calendar.clone())).unwrap_or_default()),
        ];
        if !del && !x.location.trim().is_empty() {
            rows.push(("Where".into(), x.location.trim().to_string()));
        }
        if !del && !x.repeat.is_empty() && x.repeat != "keep" && (!recurring || series_edit) {
            rows.push(("Repeats".into(), REPEAT.iter().find(|r| r.0 == x.repeat).map(|r| r.1.to_lowercase()).unwrap_or_default()));
        }
        if recurring {
            rows.push(("Applies to".into(), if x.scope == "all" { "every event in the series".into() } else { "only this one".into() }));
        }
        let title = if del { "Delete this event?" } else if o.is_some() { "Save these changes?" } else { "Add this event?" };
        let ok = if del { "Delete" } else if o.is_some() { "Save changes" } else { "Create event" };
        let run = run.clone();
        view! {
            <Modal title=title on_close=close>
                {review_list(rows)}
                {(del && recurring).then(|| view! { <Scope d=d verb="Delete"/> })}
                <div class="row" style="justify-content:flex-end">
                    <button class="btn" on:click=move |_| step.set(Step::Edit) disabled=move || busy.get()>"Back"</button>
                    <button class=if del { "btn danger" } else { "btn primary" } autofocus on:click=move |_| run() disabled=move || busy.get()>
                        {move || if busy.get() { "Working…" } else { ok }}
                    </button>
                </div>
            </Modal>
        }
    };

    let o_ed = o.clone();
    let editing = move || {
        let o = o_ed.clone();
        let o_cal = o.clone();
        let is_new = o.is_none();
        let title = if is_new { "New event" } else if ro { "Event" } else { "Edit event" };
        let writable = move || cals.with(|c| c.iter().filter(|c| !c.read_only).cloned().collect::<Vec<_>>());
        let repeat_opts = move || {
            let mut v: Vec<(&'static str, &'static str)> = vec![];
            if recurring {
                v.push(("keep", "Keeps repeating as it is"));
            }
            v.extend(REPEAT);
            v
        };
        view! {
            <Modal title=title on_close=close>
                <form class="cal-form" on:submit=move |e| { e.prevent_default(); if !ro { review() } }>
                    <input class="cal-title-in" autofocus=is_new placeholder="What's happening?" prop:value=move || d.with(|x| x.title.clone()) disabled=ro
                        on:input=move |e| { let v = event_target_value(&e); set(&|x| x.title = v.clone()) }/>
                    <label class="row small muted">
                        <input type="checkbox" prop:checked=move || d.with(|x| x.all_day) disabled=ro on:change=move |e| { let v = event_target_checked(&e); set(&|x| x.all_day = v) }/>" All day"
                    </label>
                    <div class="cal-when cal-times">
                        <label class="field"><span>"Starts"</span><span class="row">
                            <input type="date" prop:value=move || d.with(|x| x.sd.clone()) disabled=ro on:change=move |e| {
                                let n = event_target_value(&e);
                                if !n.is_empty() { set(&|x| { if x.ed < n || x.ed == x.sd { x.ed = n.clone() } x.sd = n.clone() }) }
                            }/>
                            <Show when=move || !d.with(|x| x.all_day)>
                                <input type="time" prop:value=move || d.with(|x| x.st.clone()) disabled=ro on:change=move |e| {
                                    let s = event_target_value(&e);
                                    if s.is_empty() { return }
                                    // Moving the start keeps the length.
                                    set(&|x| {
                                        let len = mins(&x.et) - mins(&x.st);
                                        if x.ed == x.sd && len > 0 && mins(&s) + len < 1440 { x.et = hm(mins(&s) + len) }
                                        x.st = s.clone();
                                    })
                                }/>
                            </Show>
                        </span></label>
                        <label class="field"><span>"Ends"</span><span class="row">
                            <input type="date" prop:value=move || d.with(|x| x.ed.clone()) min=move || d.with(|x| x.sd.clone()) disabled=ro on:change=move |e| {
                                let v = event_target_value(&e);
                                if !v.is_empty() { set(&|x| x.ed = v.clone()) }
                            }/>
                            <Show when=move || !d.with(|x| x.all_day)>
                                <input type="time" prop:value=move || d.with(|x| x.et.clone()) disabled=ro on:change=move |e| {
                                    let v = event_target_value(&e);
                                    if !v.is_empty() { set(&|x| x.et = v.clone()) }
                                }/>
                            </Show>
                        </span></label>
                    </div>
                    <label class="field"><span>"Location"</span>
                        <input prop:value=move || d.with(|x| x.location.clone()) disabled=ro on:input=move |e| { let v = event_target_value(&e); set(&|x| x.location = v.clone()) }/></label>
                    <label class="field"><span>"Notes"</span>
                        <textarea rows="3" prop:value=move || d.with(|x| x.notes.clone()) disabled=ro on:input=move |e| { let v = event_target_value(&e); set(&|x| x.notes = v.clone()) }/></label>
                    <div class="cal-when">
                        <label class="field"><span>"Calendar"</span>
                            {match o_cal.clone().filter(|_| ro) {
                                Some(o) => view! { <span class="row muted"><i class="cal-dot" style=format!("background:{}", safe_color(&o.color))/>{o.calendar.clone()}</span> }.into_any(),
                                None => view! {
                                    <select class="pick" aria-label="Calendar" on:change=move |e| { let v = event_target_value(&e); set(&|x| x.calendar_id = v.clone()) }>
                                        <option value="" disabled selected=move || d.with(|x| x.calendar_id.is_empty())>"Pick a calendar…"</option>
                                        {move || writable().into_iter().map(|c| {
                                            let id = c.id.clone();
                                            let label = format!("{}{}{}", c.name, if c.account.is_empty() { String::new() } else { format!(" · {}", c.account) }, if c.default { " (default)" } else { "" });
                                            view! { <option value=id.clone() selected=move || d.with(|x| x.calendar_id == id)>{label}</option> }
                                        }).collect_view()}
                                    </select>
                                }.into_any(),
                            }}
                        </label>
                        <Show when=move || (!recurring || d.with(|x| x.scope == "all")) && !ro>
                            <label class="field"><span>"Repeat"</span>
                                <select class="pick" aria-label="Repeat" on:change=move |e| { let v = event_target_value(&e); set(&|x| x.repeat = v.clone()) }>
                                    {move || repeat_opts().into_iter().map(|(v, l)| view! { <option value=v selected=move || d.with(|x| x.repeat == v)>{l}</option> }).collect_view()}
                                </select>
                            </label>
                        </Show>
                    </div>
                    {(recurring && !ro).then(|| view! { <Scope d=d verb="Change"/> })}
                    {ro.then(|| view! { <div class="small subdue">"This calendar is read-only."</div> })}
                    <Show when=move || !err.get().is_empty()><div class="small err">{move || err.get()}</div></Show>
                    <div class="row">
                        {(!is_new && !ro).then(|| view! { <button type="button" class="btn ghost danger" on:click=move |_| step.set(Step::Delete)><Icon name="trash" size=14/>" Delete…"</button> })}
                        <span class="grow"/>
                        <button type="button" class="btn" on:click=move |_| close.run(())>{if ro { "Close" } else { "Cancel" }}</button>
                        {(!ro).then(|| view! { <button type="submit" class="btn primary">"Save…"</button> })}
                    </div>
                </form>
            </Modal>
        }
    };

    move || if step.get() == Step::Edit { editing().into_any() } else { reviewing().into_any() }
}

#[component]
fn Scope(d: RwSignal<Draft>, verb: &'static str) -> impl IntoView {
    view! {
        <div class="row small" role="radiogroup" aria-label="Repeating event">
            <span class="muted">{verb}</span>
            <button type="button" class=move || if d.with(|x| x.scope == "this") { "chip on" } else { "chip" } on:click=move |_| d.update(|x| x.scope = "this".into())>"This event"</button>
            <button type="button" class=move || if d.with(|x| x.scope == "all") { "chip on" } else { "chip" } on:click=move |_| d.update(|x| { x.scope = "all".into(); x.repeat = "keep".into() })>"All events"</button>
        </div>
    }
}
