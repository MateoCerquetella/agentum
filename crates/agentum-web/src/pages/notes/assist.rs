//! Your model on a note (crates/agentum/src/floatnote.rs): "To-dos & events" finds what's still
//! to do in it and you add each one (to-dos at once, events after a question, like every calendar
//! write); "Tidy" proposes a cleaner version, shown as a diff you accept or discard. Your model is
//! slow: both say so.
use agentum_ui::prelude::*;

fn plus_hour(t: &str) -> String {
    let (h, m) = t.split_once(':').unwrap_or(("0", "0"));
    format!("{:02}:{:02}", (h.parse::<u32>().unwrap_or(0) + 1).min(23), m.parse::<u32>().unwrap_or(0))
}

fn day_of(date: &str) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(&format!("{date}T12:00:00")));
    const D: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if d.get_time().is_nan() {
        return date.to_string();
    }
    format!("{} {} {}", D[d.get_day() as usize % 7], d.get_date(), M[d.get_month() as usize % 12])
}

fn when_of(f: &Value) -> String {
    match f["date"].as_str() {
        None => "no date".into(),
        Some(d) => {
            let mut s = day_of(d);
            if let Some(t) = f["time"].as_str() {
                s += &format!(" {t}");
                if let Some(e) = f["end"].as_str() {
                    s += &format!("–{e}");
                }
            }
            s
        }
    }
}

/// "2026-11-20T10:00:00-03:00" for a day and time here (a date alone for no time).
fn due_of(date: &str, time: Option<&str>) -> String {
    let Some(t) = time else { return date.to_string() };
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(&format!("{date}T{t}:00")));
    let o = -(d.get_timezone_offset() as i64);
    format!("{date}T{t}:00{}{:02}:{:02}", if o >= 0 { '+' } else { '-' }, o.abs() / 60, o.abs() % 60)
}

#[component]
pub fn Suggest(url: String, close: Callback<()>) -> impl IntoView {
    let res = RwSignal::new(None::<Value>);
    let err = RwSignal::new(String::new());
    let added = RwSignal::new(std::collections::BTreeMap::<usize, &'static str>::new());
    spawn_local(async move {
        match api_post::<Value>(&format!("{url}/suggest"), &json!({})).await {
            Ok(r) => res.set(Some(r)),
            Err(e) => err.set(e.message),
        }
    });
    let add = move |f: Value, i: usize, event: bool| {
        spawn_local(async move {
            let title = f["title"].as_str().unwrap_or("").to_string();
            let r = if !event {
                let due = f["date"].as_str().map(|d| due_of(d, f["time"].as_str())).unwrap_or_default();
                api_post::<Value>("/todos", &json!({"title": title, "due": due})).await
            } else {
                if !confirm("Add this to your calendar?", format!("{title} · {} · your default calendar", when_of(&f)), "Create event", false).await {
                    return;
                }
                let date = f["date"].as_str().unwrap_or("");
                let body = match f["time"].as_str() {
                    Some(t) => json!({"summary": title, "dtstart": format!("{date}T{t}:00"), "dtend": format!("{date}T{}:00", f["end"].as_str().map(str::to_string).unwrap_or_else(|| plus_hour(t)))}),
                    None => json!({"summary": title, "dtstart": date, "all_day": true}),
                };
                api_post::<Value>("/calendar/events", &body).await
            };
            match r {
                Ok(_) => added.update(|a| {
                    a.insert(i, if event { "In your calendar" } else { "To-do added" });
                }),
                Err(e) => toast_bad(e.message),
            }
        });
    };
    view! {
        <div class="nf-ai">
            <div class="nf-ai-head"><Icon name="sparkle" size=14/><span class="grow">"To-dos & events in this note"</span>
                <button class="icon-btn" aria-label="Close" on:click=move |_| close.run(())><Icon name="x" size=14/></button></div>
            {move || {
                if !err.get().is_empty() {
                    return view! { <div class="err small">{err.get()}</div> }.into_any();
                }
                let Some(r) = res.get() else { return view! { <div class="muted small">"Your model is reading the note… (it can take a minute)"</div> }.into_any() };
                let items = r["items"].as_array().cloned().unwrap_or_default();
                let by_model = r["by_model"] == true;
                view! {
                    {if items.is_empty() {
                        view! { <div class="muted small">"Nothing left to do in this note."</div> }.into_any()
                    } else {
                        view! { <ul class="nf-found">{items.into_iter().enumerate().map(|(i, f)| {
                            let event = f["kind"] == "event";
                            let line = f["line"].as_str().filter(|l| !l.is_empty() && Some(*l) != f["title"].as_str()).map(|l| format!(" · “{l}”")).unwrap_or_default();
                            let (f1, f2) = (f.clone(), f.clone());
                            view! {
                                <li>
                                    <span class="ic">{if event { view! { <Icon name="calendar" size=13/> }.into_any() } else { "○".into_any() }}</span>
                                    <span class="grow"><span class="hi">{f["title"].as_str().unwrap_or("").to_string()}</span><small>{when_of(&f)}{line}</small></span>
                                    {move || match added.with(|a| a.get(&i).copied()) {
                                        Some(done) => view! { <small class="pos">"✓ "{done}</small> }.into_any(),
                                        None => {
                                            let (f1, f2) = (f1.clone(), f2.clone());
                                            view! {
                                                {event.then(|| view! { <button class="btn primary" on:click=move |_| add(f1.clone(), i, true)>"Add event"</button> })}
                                                <button class=if event { "btn" } else { "btn primary" } on:click=move |_| add(f2.clone(), i, false)>"Add to-do"</button>
                                            }.into_any()
                                        }
                                    }}
                                </li>
                            }
                        }).collect_view()}</ul> }.into_any()
                    }}
                    {(!by_model).then(|| view! { <div class="subdue small">"Your model didn't answer: these are the note's unticked checklist items."</div> })}
                }.into_any()
            }}
        </div>
    }
}

/// Line diff (longest common subsequence): the lines kept, removed and added, in order.
fn diff_lines<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, &'a str)> {
    let (n, m) = (a.len(), b.len());
    let mut l = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if a[i] == b[j] { l[i + 1][j + 1] + 1 } else { l[i + 1][j].max(l[i][j + 1]) };
        }
    }
    let (mut out, mut i, mut j) = (vec![], 0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if l[i + 1][j] >= l[i][j + 1] {
            out.push(('-', a[i]));
            i += 1;
        } else {
            out.push(('+', b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|s| ('-', *s)));
    out.extend(b[j..].iter().map(|s| ('+', *s)));
    out
}

#[component]
pub fn Tidy(url: String, md: String, close: Callback<()>, accept: Callback<String>) -> impl IntoView {
    let res = RwSignal::new(None::<String>);
    let err = RwSignal::new(String::new());
    spawn_local(async move {
        match api_post::<Value>(&format!("{url}/tidy"), &json!({})).await {
            Ok(r) => res.set(Some(r["md"].as_str().unwrap_or("").to_string())),
            Err(e) => err.set(e.message),
        }
    });
    let from = md;
    let f2 = from.clone();
    let same = move || res.with(|r| r.as_ref().is_some_and(|r| r.trim() == f2.trim()));
    let same2 = same.clone();
    view! {
        <div class="nf-ai">
            <div class="nf-ai-head"><Icon name="sparkle" size=14/><span class="grow">"Tidy this note"</span>
                <Show when=move || res.with(|r| r.is_some()) && !same2()>
                    <button class="btn ghost" on:click=move |_| close.run(())>"Discard"</button>
                    <button class="btn primary" on:click=move |_| { if let Some(t) = res.get_untracked() { accept.run(t) } }>"Use this version"</button>
                </Show>
                <button class="icon-btn" aria-label="Close" on:click=move |_| close.run(())><Icon name="x" size=14/></button></div>
            {move || {
                if !err.get().is_empty() {
                    return view! { <div class="err small">{err.get()}</div> }.into_any();
                }
                match res.get() {
                    None => view! { <div class="muted small">"Your model is tidying the note… (it can take a minute)"</div> }.into_any(),
                    Some(_) if same() => view! { <div class="muted small">"It's already tidy: your model changed nothing."</div> }.into_any(),
                    Some(r) => {
                        let (a, b): (Vec<&str>, Vec<&str>) = (from.split('\n').collect(), r.split('\n').collect());
                        view! { <pre class="nf-diff">{diff_lines(&a, &b).into_iter().map(|(t, s)| view! {
                            <div class=match t { '+' => "add", '-' => "del", _ => "" }>{if t == ' ' { "  ".to_string() } else { format!("{t} ") }}{if s.is_empty() { " ".to_string() } else { s.to_string() }}</div>
                        }).collect_view()}</pre> }.into_any()
                    }
                }
            }}
        </div>
    }
}
