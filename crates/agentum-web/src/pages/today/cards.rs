//! Today's cards: the assist cards (evening wrap-up, leave by, arriving, plan for today, from your
//! mail, waiting on you, week ahead; calendar writes ask first), the shape of the day and the
//! Tomorrow / Sky / Mail card.
use super::plan::*;
use super::Tx;
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

/// The sign of each thing that matters (the brief's matters[].k).
pub fn matter_icon(k: &str) -> &'static str {
    match k {
        "pay" => "$",
        "now" => "●",
        "next" => "→",
        "busy" => "▮",
        "free" => "○",
        "rain" => "☂",
        "heat" => "☀",
        "cold" => "❄",
        "mail" => "✉",
        "weekend" | "holiday" => "★",
        "birthday" => "✦",
        _ => "·",
    }
}

pub fn card(title: AnyView, right: AnyView, d: f64, class: &str, body: AnyView) -> AnyView {
    view! { <section class=format!("td-card rise {class}") style=format!("--d:{d}s")><h2><span>{title}</span><span>{right}</span></h2>{body}</section> }.into_any()
}
/// A row of an assist list: icon, text with a detail line, actions.
pub fn li(ic: &'static str, tone: &'static str, text: AnyView, small: Option<AnyView>, acts: Option<AnyView>) -> AnyView {
    view! { <li style=format!("--tc:{tone}")><span class="ic">{ic}</span><span class="tx">{text}{small.map(|s| view! { <small>{s}</small> })}</span><span class="acts">{acts}</span></li> }.into_any()
}

pub type Job = Rc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Option<String>, String>>>>>;
pub fn job<F: Future<Output = Result<Option<String>, String>> + 'static>(f: impl Fn() -> F + 'static) -> Job {
    Rc::new(move || Box::pin(f()))
}
/// A button that runs once: "…" while it works, then its result text (None: cancelled).
#[component]
pub fn Act(label: String, run: Job, #[prop(optional)] go: bool, c: Tx) -> impl IntoView {
    let busy = RwSignal::new(false);
    let out = RwSignal::new(None::<String>);
    let fail = RwSignal::new(false);
    let run = send_wrapper::SendWrapper::new(run);
    move || match out.get() {
        Some(o) => view! { <small class="muted">{o}</small> }.into_any(),
        None => {
            let run = run.clone();
            let label = label.clone();
            view! {
                <button type="button" class=if go { "go" } else { "" } disabled=move || busy.get() on:click=move |_| {
                    let f = (*run)();
                    busy.set(true);
                    spawn_local(async move {
                        match f.await {
                            Ok(Some(t)) => { let _ = out.try_set(Some(t)); }
                            Ok(None) => {}
                            Err(e) => { toast_bad(e); let _ = fail.try_set(true); }
                        }
                        let _ = busy.try_set(false);
                    });
                }>{move || if busy.get() { "…".to_string() } else if fail.get() { c.t("Failed: retry") } else { label.clone() }}</button>
            }.into_any()
        }
    }
}

async fn hide(id: String) -> Result<Value, String> {
    api_post::<Value>("/assist/hide", &json!({"id": id})).await.map_err(|e| e.message)
}
async fn todo_done(id: String) -> Result<Value, String> {
    api_put::<Value>(&format!("/todos/{}", js_sys::encode_uri_component(&id)), &json!({"done": true})).await.map_err(|e| e.message)
}
fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

#[component]
pub fn AssistCards(a: Signal<Option<Value>>, x: Signal<Option<Value>>, day: String, c: Tx) -> impl IntoView {
    let gone = RwSignal::new(BTreeSet::<String>::new());
    let planned = RwSignal::new(BTreeSet::<String>::new());
    let tomorrow = add_days(&day, 1);
    let ev_rows = move |title: &str, start: &str, end: &str, all_day: bool| vec![(c.t("What"), title.to_string()), (c.t("When"), when_text(start, end, all_day)), (c.t("Calendar"), c.cal.get_untracked())];
    move || {
        let a = a.get().unwrap_or(Value::Null);
        let lang = c.lang();
        let mut out: Vec<AnyView> = vec![];
        let t = |s: &str| c.t(s);

        // The wrap-up is for the evening (by this device's clock).
        let w = if evening() { a["wrapup"].clone() } else { Value::Null };
        if w.is_object() {
            let tw = &w["weather"];
            let sky = if tw.is_object() {
                let rain = tw["rain"].as_f64().unwrap_or(0.0);
                format!(" · {}°–{}° {}{}", s(tw, "low"), s(tw, "high"), s(tw, "sky"), if rain >= 30.0 { format!(" · {} {rain}%{}", t("rain"), if tw["rain_from"].is_string() { format!(" {} {}", t("from"), s(tw, "rain_from")) } else { String::new() }) } else { String::new() })
            } else {
                String::new()
            };
            let mut rows: Vec<AnyView> = vec![];
            if w["tomorrow"].is_object() {
                rows.push(li("→", "var(--positive)", format!("{} {} · {}", t("Tomorrow"), s(&w["tomorrow"], "time"), s(&w["tomorrow"], "title")).into_any(), Some(format!("{} {}{sky}", w["tomorrow_count"].as_u64().unwrap_or(1), t("timed")).into_any()), None));
            } else {
                rows.push(li("○", "var(--positive)", t("Nothing timed tomorrow").into_any(), Some(sky.get(3..).unwrap_or("").to_string().into_any()), None));
            }
            let done: Vec<String> = w["done"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
            if !done.is_empty() {
                rows.push(li("✓", "var(--positive)", format!("{} {}", done.len(), t("done today")).into_any(), Some(done.join(" · ").into_any()), None));
            }
            for xv in w["check"].as_array().cloned().unwrap_or_default() {
                let uid = s(&xv, "uid");
                if gone.with(|g| g.contains(&format!("ev:{uid}"))) {
                    continue;
                }
                let (x1, x2) = (xv.clone(), xv.clone());
                let (tm, d2) = (tomorrow.clone(), day.clone());
                let mv = job(move || {
                    let x = x1.clone();
                    let tm = tm.clone();
                    async move {
                        let st = format!("{tm}T{}:00", s(&x, "time"));
                        let en = format!("{tm}T{}:00", if s(&x, "end") == "24:00" { "23:59".into() } else { s(&x, "end") });
                        if !c.ask(c.t("Move it to tomorrow?"), ev_rows(&s(&x, "title"), &st, &en, false), c.t("Move")).await {
                            return Ok(None);
                        }
                        api_put::<Value>(&format!("/calendar/events/{}", js_sys::encode_uri_component(&s(&x, "uid"))), &json!({"dtstart": st, "dtend": en})).await.map_err(|e| e.message)?;
                        Ok(Some(c.t("moved to tomorrow")))
                    }
                });
                let dn = job(move || {
                    let x = x2.clone();
                    let d2 = d2.clone();
                    async move {
                        if !c.ask(c.t("Mark it done?"), vec![(c.t("What"), s(&x, "title")), (c.t("When"), format!("{}–{}", s(&x, "time"), s(&x, "end")))], c.t("Mark done")).await {
                            return Ok(None);
                        }
                        hide(format!("ev:{}", s(&x, "uid"))).await?;
                        let series = if x["series"].is_string() { s(&x, "series") } else { s(&x, "uid") };
                        let _ = api_post::<Value>("/calendar/done", &json!({"key": format!("{}|{d2}", series.split("::").next().unwrap_or("")), "done": true})).await;
                        Ok(Some(c.t("done")))
                    }
                });
                rows.push(li("?", "var(--money)", s(&xv, "title").into_any(), Some(format!("{}–{} · {}", s(&xv, "time"), s(&xv, "end"), t("did you get to it?")).into_any()), Some(view! { <Act c=c label=t("Tomorrow") run=mv/><Act c=c go=true label=t("Done") run=dn/> }.into_any())));
            }
            for xv in w["slipped"].as_array().cloned().unwrap_or_default() {
                let id = s(&xv, "id");
                let dn = job(move || {
                    let id = id.clone();
                    async move {
                        todo_done(id).await?;
                        Ok(Some(c.t("done")))
                    }
                });
                rows.push(li("↷", "var(--money)", s(&xv, "title").into_any(), Some(format!("{} {}", t("slipped · was due"), s(&xv, "due")).into_any()), Some(view! { <Act c=c go=true label=t("Done") run=dn/> }.into_any())));
            }
            let body = view! {
                {w["prepare"].as_str().filter(|p| !p.is_empty()).map(|p| view! { <p class="td-wrap">{p.to_string()}</p> })}
                <ul class="td-as">{rows}</ul>
                <div class="td-by2">{format!("● {}", if w["by_model"] == true { t("Tip by your model") } else { t("From your calendar and to-dos") })}</div>
            }
            .into_any();
            out.push(card(t("Evening wrap-up").into_any(), t("tonight").into_any(), 0.2, "", body));
        }

        let leave = a["leave"].as_array().cloned().unwrap_or_default();
        if !leave.is_empty() {
            let rows: Vec<AnyView> = leave
                .iter()
                .map(|l| li("⇢", "var(--money)", format!("{} {} · {}", t("Leave"), s(l, "leave"), s(l, "title")).into_any(), Some(format!("{} {} {} · ~{} min{}", s(l, "time"), t("at"), s(l, "location"), s(l, "travel"), if l["rain"] == true { format!(" · {}, +10 min", t("rain")) } else { String::new() }).into_any()), None))
                .collect();
            out.push(card(t("Leave by").into_any(), ().into_any(), 0.25, "", view! { <ul class="td-as">{rows}</ul> }.into_any()));
        }

        // Arriving: what's on its way, what comes today, what came today.
        let dl = x.get().map(|x| x["items"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
        if !dl.is_empty() {
            let heard = |when: &str| -> String {
                if when.is_empty() {
                    return String::new();
                }
                let d = ((js_sys::Date::parse(&format!("{day}T12:00")) - js_sys::Date::parse(&format!("{}T12:00", day_of(when)))) / 864e5).round() as i64;
                if d <= 0 { format!("{} {}", t("today"), when.get(11..16).unwrap_or("")) } else if d == 1 { t("yesterday") } else if lang == "es" { format!("hace {d} días") } else { format!("{d} days ago") }
            };
            let coming = dl.iter().filter(|p| p["status"] != "delivered").count();
            let rows: Vec<AnyView> = dl
                .iter()
                .map(|p| {
                    let st = s(p, "status");
                    let (ic, tone) = match st.as_str() {
                        "delivered" => ("✓", "var(--positive)"),
                        "today" => ("⇣", "var(--money)"),
                        "ordered" => ("□", "var(--primary)"),
                        _ => ("▸", "var(--primary)"),
                    };
                    let small = [s(p, "subject"), if p["order"].is_string() && !s(p, "order").is_empty() { format!("#{}", s(p, "order")) } else { String::new() }, heard(&s(p, "when"))].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
                    li(ic, tone, view! { {s(p, "from")}" "<span class=format!("td-pill s-{st}")>{s(p, "label")}</span> }.into_any(), Some(small.into_any()), None)
                })
                .collect();
            let right = if coming > 0 { format!("{coming} {}", t(if coming == 1 { "parcel" } else { "parcels" })) } else { t("delivered today") };
            out.push(card(t("Arriving").into_any(), right.into_any(), 0.27, "", view! { <ul class="td-as">{rows}</ul> }.into_any()));
        }

        // Plan for today: to-dos for the free time (from the model, or by due date).
        let gaps: Vec<Value> = a["gaps"].as_array().cloned().unwrap_or_default().into_iter().filter(|g| g["todo"].is_object() && !gone.with(|x| x.contains(&format!("todo:{}", s(&g["todo"], "id"))))).collect();
        if !gaps.is_empty() {
            let slot = |g: &Value| {
                let st = mins(&s(g, "from"));
                (local_iso(&s(g, "date"), st), local_iso(&s(g, "date"), (st + 60).min(mins(&s(g, "to")))))
            };
            let put_in = move |g: Value| async move {
                let (st, en) = slot(&g);
                api_post::<Value>("/calendar/events", &json!({"summary": s(&g["todo"], "title"), "dtstart": st, "dtend": en, "description": "Planned by Agentum in your free time"})).await.map_err(|e| e.message)
            };
            let open: Vec<Value> = gaps.iter().filter(|g| !planned.with(|p| p.contains(&s(&g["todo"], "id")))).cloned().collect();
            let open_n = open.len();
            let all = move |_| {
                let open = open.clone();
                spawn_local(async move {
                    let mut rows: Vec<(String, String)> = open.iter().map(|g| (format!("{}–{}", s(g, "from"), hm((mins(&s(g, "from")) + 60).min(mins(&s(g, "to"))))), s(&g["todo"], "title"))).collect();
                    rows.push((c.t("Calendar"), c.cal.get_untracked()));
                    if !c.ask(c.t("Add these to your calendar?"), rows, c.t("Create events")).await {
                        return;
                    }
                    for g in open {
                        match put_in(g.clone()).await {
                            Ok(_) => planned.update(|p| {
                                p.insert(s(&g["todo"], "id"));
                            }),
                            Err(e) => return toast_bad(e),
                        }
                        gloo_timers::future::TimeoutFuture::new(250).await;
                    }
                });
            };
            let right = if gaps.len() > 1 { view! { <button type="button" class="td-planall" disabled=open_n == 0 on:click=all>{if open_n > 0 { t("Accept all") } else { t("Planned") }}</button> }.into_any() } else { t("your to-dos").into_any() };
            let by_model = gaps.iter().any(|g| g["by"] == "model");
            let rows: Vec<AnyView> = gaps
                .into_iter()
                .map(|g| {
                    let id = s(&g["todo"], "id");
                    let small = format!("{} {}{}{}", t("free"), dur(g["min"].as_i64().unwrap_or(0) as i32), if g["why"].is_string() && !s(&g, "why").is_empty() { format!(" · {}", s(&g, "why")) } else { String::new() }, if g["todo"]["due"].is_string() && !s(&g["todo"], "due").is_empty() { format!(" · {} {}", t("due"), s(&g["todo"], "due")) } else { String::new() });
                    let acts = if planned.with(|p| p.contains(&id)) {
                        view! { <small class="muted">{t("In your calendar")}</small> }.into_any()
                    } else {
                        let id2 = id.clone();
                        let dn = job(move || {
                            let id = id2.clone();
                            async move {
                                todo_done(id.clone()).await?;
                                gloo_timers::callback::Timeout::new(1200, move || gone.update(|x| { x.insert(format!("todo:{id}")); })).forget();
                                Ok(Some(c.t("done")))
                            }
                        });
                        let (g2, id3) = (g.clone(), id.clone());
                        let ok = job(move || {
                            let g = g2.clone();
                            let id = id3.clone();
                            async move {
                                let (st, en) = slot(&g);
                                if !c.ask(c.t("Add this to your calendar?"), ev_rows(&s(&g["todo"], "title"), &st, &en, false), c.t("Create event")).await {
                                    return Ok(None);
                                }
                                put_in(g).await?;
                                planned.update(|p| {
                                    p.insert(id);
                                });
                                Ok(Some(c.t("In your calendar")))
                            }
                        });
                        view! { <Act c=c label=t("Done") run=dn/><Act c=c go=true label=t("Accept") run=ok/> }.into_any()
                    };
                    li("✦", "var(--positive)", format!("{}–{} · {}", s(&g, "from"), s(&g, "to"), s(&g["todo"], "title")).into_any(), Some(small.into_any()), Some(acts))
                })
                .collect();
            out.push(card(
                t("Plan for today").into_any(),
                right,
                0.3,
                "",
                view! { <ul class="td-as">{rows}</ul><div class="td-by2">{format!("● {}", if by_model { t("Picked by your model") } else { t("By due date") })}</div> }.into_any(),
            ));
        }

        // Dates found in your mail: added only when you press Add.
        let rem: Vec<Value> = a["reminders"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| !gone.with(|g| g.contains(&format!("rem:{}", s(r, "id"))))).collect();
        if !rem.is_empty() {
            let rows: Vec<AnyView> = rem
                .into_iter()
                .map(|r| {
                    let pay = r["kind"] == "pay";
                    let (date, time) = (s(&r, "date"), s(&r, "time"));
                    let st = if time.is_empty() { format!("{date}T00:00:00") } else { format!("{date}T{time}:00") };
                    let en = if time.is_empty() { format!("{}T00:00:00", add_days(&date, 1)) } else { local_iso(&date, (mins(&time) + 60).min(1439)) };
                    let id = s(&r, "id");
                    let msg = s(&r, "msg");
                    let link = if s(&r, "subject").is_empty() { s(&r, "from") } else { s(&r, "subject") };
                    let small = view! {
                        {if pay { format!("{} ", t("due")) } else { String::new() }}{day_name(&date, &lang)}{if time.is_empty() { String::new() } else { format!(" {time}") }}" · "
                        <a on:click=move |_| navigate("mail", Where::Right, &[("msg", &msg), ("folder", "INBOX")])>{link}</a>
                    }
                    .into_any();
                    let (r2, id2, id3) = (r.clone(), id.clone(), id.clone());
                    let add = job(move || {
                        let (r, id, st, en) = (r2.clone(), id2.clone(), st.clone(), en.clone());
                        async move {
                            let no_time = s(&r, "time").is_empty();
                            if !c.ask(c.t("Add this to your calendar?"), ev_rows(&s(&r, "title"), &st, &en, no_time), c.t("Create event")).await {
                                return Ok(None);
                            }
                            api_post::<Value>("/calendar/events", &json!({"summary": s(&r, "title"), "dtstart": st, "dtend": en, "all_day": no_time, "description": format!("From the email \"{}\" ({})", s(&r, "subject"), s(&r, "from"))})).await.map_err(|e| e.message)?;
                            let _ = hide(id).await;
                            Ok(Some(c.t("Added")))
                        }
                    });
                    let acts = view! {
                        <button type="button" on:click=move |_| { let id = id3.clone(); gone.update(|g| { g.insert(format!("rem:{id}")); }); spawn_local(async move { let _ = hide(id).await; }) }>{t("Not now")}</button>
                        <Act c=c go=true label=t("Add") run=add/>
                    }
                    .into_any();
                    li(if pay { "$" } else { "◷" }, if pay { "var(--money)" } else { "var(--primary)" }, s(&r, "title").into_any(), Some(small), Some(acts))
                })
                .collect();
            out.push(card(t("From your mail").into_any(), t("add to calendar?").into_any(), 0.35, "", view! { <ul class="td-as">{rows}</ul> }.into_any()));
        }

        let rep = a["replies"].as_array().cloned().unwrap_or_default();
        if !rep.is_empty() {
            let rows: Vec<AnyView> = rep
                .iter()
                .map(|p| {
                    let id = s(p, "id");
                    let acts = view! { <button type="button" class="go" title=t("Opens the email with a reply drafted by your model, to review and send") on:click=move |_| navigate("mail", Where::Right, &[("msg", &id), ("folder", "INBOX"), ("reply", "1")])>{format!("✨ {}", t("Reply"))}</button> }.into_any();
                    li("✉", "var(--primary)", s(p, "from").into_any(), Some(format!("{} · {}d", s(p, "subject"), s(p, "days")).into_any()), Some(acts))
                })
                .collect();
            out.push(card(t("Waiting on you").into_any(), t("replies").into_any(), 0.4, "", view! { <ul class="td-as">{rows}</ul> }.into_any()));
        }

        let wk = &a["week"];
        if wk.is_object() {
            let busy: Vec<String> = wk["busy"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let kicks: Vec<Value> = wk["coming_up"].as_array().cloned().unwrap_or_default().into_iter().filter(|u| u["kind"] != "money").collect();
            let days = wk["days"].as_array().cloned().unwrap_or_default();
            let body = view! {
                {wk["text"].as_str().filter(|x| !x.is_empty()).map(|x| view! { <p class="td-wrap">{x.to_string()}</p> })}
                <div class="td-wk">{days.iter().map(|d| {
                    let date = s(d, "date");
                    let cls = if busy.contains(&date) { "busy" } else if d["n"].as_i64() == Some(0) { "free" } else { "" };
                    let title = format!("{} {} · {} h{}", s(d, "n"), t("timed"), s(d, "hours"), if d["first"].is_string() && !s(d, "first").is_empty() { format!(" · {} {}", t("from"), s(d, "first")) } else { String::new() });
                    view! { <div class=cls title=title>{format!("{} {}", s(d, "dow"), s(d, "day"))}<b>{s(d, "n")}</b></div> }
                }).collect_view()}</div>
                {(!kicks.is_empty()).then(|| view! { <div class="td-kicks">{kicks.iter().map(|u| view! { <span>{format!("{} · {}", s(u, "title"), s(u, "when"))}</span> }).collect_view()}</div> })}
                <div class="td-by2">{format!("● {}", if wk["by_model"] == true { t("Written by your model") } else { t("From your calendar") })}</div>
            }
            .into_any();
            out.push(card(t("Week ahead").into_any(), ().into_any(), 0.45, "", body));
        }
        (!out.is_empty()).then(|| view! { <div class="td-assist">{out}</div> })
    }
}

/// The shape of today (chips over "Your day").
pub fn shape_chips(b: &Value, c: Tx) -> Vec<(&'static str, String)> {
    let sh = &b["shape"];
    let nm = now_min();
    let cn = &sh["counts"];
    let mut out = vec![];
    let n = |k: &str| cn[k].as_i64().unwrap_or(0);
    let kinds: Vec<String> = [(n("work"), "work"), (n("personal"), "personal"), (n("money"), "to pay")].into_iter().filter(|(k, _)| *k > 0).map(|(k, l)| format!("{k} {}", c.t(l))).collect();
    if !kinds.is_empty() {
        out.push(("", kinds.join(" · ")));
    }
    let at = nm.max(480);
    if let Some(lead) = sh["free"].as_array().into_iter().flatten().find(|f| mins(&s(f, "from")) <= at && at < mins(&s(f, "to"))) {
        if mins(&s(lead, "to")) - nm.max(mins(&s(lead, "from"))) >= 45 {
            out.push(("free", format!("{} {}", c.t("free until"), s(lead, "to"))));
        }
    }
    if sh["busy"].is_object() {
        out.push(("busy", format!("{} {}–{} · {} {}", c.t("busy"), s(&sh["busy"], "from"), s(&sh["busy"], "to"), s(&sh["busy"], "n"), c.t("in a row"))));
    }
    if sh["last_end"].is_string() {
        out.push(("", format!("{} {}", c.t("done by"), s(sh, "last_end"))));
    }
    out
}

/// Tomorrow, the sky and the mail.
pub fn side_card(b: &Value, c: Tx) -> AnyView {
    let t = |x: &str| c.t(x);
    let (w, m, tm, tw) = (&b["weather"], &b["mail"], &b["tomorrow_first"], &b["tomorrow"]);
    let all_day = tw["all_day"].as_array().into_iter().flatten().map(|x| s(x, "title")).collect::<Vec<_>>().join(" · ");
    let tmr = if tm.is_object() {
        view! { <div class="td-tmr">{format!("{} · {}", s(tm, "time"), s(tm, "title"))}<small>{format!("{} {}{}", tw["count"].as_u64().unwrap_or(1), t("timed"), if all_day.is_empty() { String::new() } else { format!(" · {all_day}") })}</small></div> }.into_any()
    } else {
        view! { <div class="td-tmr">{if all_day.is_empty() { t("Nothing yet") } else { all_day.clone() }}<small>{t("no timed events")}</small></div> }.into_any()
    };
    let sky = if w.is_object() {
        let sun = (w["sunrise"].is_string() && w["sunset"].is_string()).then(|| {
            let (sr, ss) = (mins(&s(w, "sunrise")), mins(&s(w, "sunset")));
            let p = ((now_min() - sr) as f64 / ((ss - sr).max(60)) as f64).clamp(0.0, 1.0);
            let (x, y) = (4.0 + p * 92.0, 30.0 - (std::f64::consts::PI * p).sin() * 26.0);
            view! {
                <div class="td-sunbar">
                    <svg viewBox="0 0 100 34" preserveAspectRatio="none"><path d="M4 30 Q50 -22 96 30" fill="none" stroke="var(--border)" stroke-width="1" stroke-dasharray="2 2" vector-effect="non-scaling-stroke"/><line x1="0" y1="30" x2="100" y2="30" stroke="var(--border)" vector-effect="non-scaling-stroke"/></svg>
                    <span class="td-sundot" style=format!("left:{x}%;top:{}%", y / 34.0 * 100.0)/>
                </div>
                <dl class="td-kv"><dt>{format!("{} {}", t("Sunrise"), s(w, "sunrise"))}</dt><dd>{format!("{} {}", t("Sunset"), s(w, "sunset"))}</dd></dl>
            }
        });
        view! {
            <dl class="td-kv">
                <dt>{t("Today")}</dt><dd>{format!("{}° – {}°", s(w, "low"), s(w, "high"))}</dd>
                <dt>{t("Rain")}</dt><dd>{format!("{}%{}", s(w, "rain"), if w["rain_from"].is_string() && !s(w, "rain_from").is_empty() { format!(" {} {}", t("from"), s(w, "rain_from")) } else { String::new() })}</dd>
                <dt>"UV"</dt><dd>{s(w, "uv")}</dd>
            </dl>
            {sun}
        }
        .into_any()
    } else {
        view! { <div class="td-empty">{t("No forecast right now.")}</div> }.into_any()
    };
    let num = |v: &Value| v.as_i64().unwrap_or(0);
    let waiting = if m["people_waiting"].is_number() { num(&m["people_waiting"]) } else { num(&m["to_reply"]) };
    let body = view! {
        {tmr}
        <h2 style="margin-top:18px"><span>{t("Sky")}</span><span>{s(w, "day_sky")}</span></h2>
        {sky}
        <h2 style="margin-top:18px"><span>{t("Mail")}</span></h2>
        <dl class="td-kv"><dt>{t("People waiting")}</dt><dd>{waiting}</dd><dt>{t("You're waiting on")}</dt><dd>{num(&m["awaiting"])}</dd><dt>{t("New overnight")}</dt><dd>{num(&m["new_overnight"])}</dd></dl>
    }
    .into_any();
    card(t("Tomorrow").into_any(), ().into_any(), 0.65, "", body)
}
