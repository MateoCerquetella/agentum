//! Each widget type's body: its data from GET /api/widgets/{id}/data (feeds, markets, weather,
//! monitors… fetched and cached by the hub), or from the personal space itself (calendar, mail
//! numbers, the brief, to-dos).
use crate::pages::calendar::util::{is_todo, keep, mins, now_min, today_str, Todo};
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;
use wasm_bindgen::JsValue;

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn n(v: &Value, k: &str) -> i64 {
    v[k].as_i64().or_else(|| v[k].as_f64().map(|f| f as i64)).unwrap_or(0)
}
fn arr(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}

/// GET a path into a signal (None while loading, Err on failure).
fn fetch(path: String) -> RwSignal<Option<Result<Value, String>>> {
    let out = RwSignal::new(None);
    spawn_local(async move {
        let r = api_get::<Value>(&path).await.map_err(|e| e.message);
        out.try_set(Some(r));
    });
    out
}
fn loading(r: &Option<Result<Value, String>>) -> Option<AnyView> {
    match r {
        None => Some(view! { <div class="dw-empty">"Loading…"</div> }.into_any()),
        Some(Err(e)) => Some(view! { <div class="dw-empty err">{e.clone()}</div> }.into_any()),
        Some(Ok(_)) => None,
    }
}
fn link(url: String, text: impl IntoView + 'static) -> impl IntoView {
    view! { <a href=url target="_blank" rel="noopener noreferrer">{text}</a> }
}

/// The body of a widget.
pub fn body(w: &Value) -> AnyView {
    let id = s(w, "id");
    let o = w["options"].clone();
    match s(w, "type").as_str() {
        "calendar" => view! { <CalendarW/> }.into_any(),
        "mail" => view! { <MailW/> }.into_any(),
        "brief" => view! { <BriefW/> }.into_any(),
        "todo" => view! { <TodoW/> }.into_any(),
        "clock" => view! { <ClockW o=o/> }.into_any(),
        "search" => view! { <SearchW o=o/> }.into_any(),
        "bookmarks" => bookmarks(&o).into_any(),
        t => {
            let t = t.to_string();
            let d = fetch(format!("/widgets/{}/data", js_sys::encode_uri_component(&id)));
            let collapse = o["collapse-after"].as_u64().map(|x| x as usize);
            (move || {
                let r = d.get();
                if let Some(v) = loading(&r) {
                    return v;
                }
                let v = r.and_then(|r| r.ok()).unwrap_or_default();
                match t.as_str() {
                    "weather" => weather(&v).into_any(),
                    "monitor" => monitor(&v).into_any(),
                    "markets" => markets(&arr(&v, "markets")).into_any(),
                    "watchlist" => view! {
                        <div class="dw-watch">
                            {arr(&v, "categories").into_iter().map(|c| view! {
                                <div class="dw-sub">{s(&c, "title")}</div>{markets(&arr(&c, "markets"))}
                            }).collect_view()}
                        </div>
                    }.into_any(),
                    "repository" => repository(&v).into_any(),
                    "releases" => view! { <Collapse n=collapse items=arr(&v, "items") row=release/> }.into_any(),
                    "videos" => videos(&arr(&v, "items")).into_any(),
                    "rss" if o["style"].as_str() == Some("horizontal-cards") => cards(&arr(&v, "items")).into_any(),
                    _ => view! { <Collapse n=collapse items=arr(&v, "items") row=story/> }.into_any(),
                }
            })
            .into_any()
        }
    }
}

/// A list that shows `n` rows and a "Show more".
#[component]
fn Collapse(n: Option<usize>, items: Vec<Value>, row: fn(Value) -> AnyView) -> impl IntoView {
    let open = RwSignal::new(false);
    let total = items.len();
    let n = n.filter(|n| *n > 0 && *n < total);
    if items.is_empty() {
        return view! { <div class="dw-empty">"Nothing here."</div> }.into_any();
    }
    view! {
        <ul class="dw-list">
            {items.into_iter().enumerate().map(|(i, it)| {
                let hidden = move || n.is_some_and(|n| i >= n) && !open.get();
                view! { <li class:hide=hidden>{row(it)}</li> }
            }).collect_view()}
        </ul>
        {n.map(|n| view! {
            <button class="dw-more" on:click=move |_| open.update(|o| *o = !*o)>{move || if open.get() { "Show less".to_string() } else { format!("Show {} more", total - n) }}</button>
        })}
    }
    .into_any()
}

fn story(it: Value) -> AnyView {
    let mut meta: Vec<String> = vec![];
    let t = n(&it, "time");
    if t > 0 {
        meta.push(ago(t));
    }
    for k in ["source", "domain", "flair"] {
        let x = s(&it, k);
        if !x.is_empty() {
            meta.push(x);
        }
    }
    if it["score"].is_number() {
        meta.push(format!("{} points", n(&it, "score")));
    }
    let comments = s(&it, "comments_url");
    let nc = n(&it, "comments");
    view! {
        <div class="dw-title">{link(s(&it, "url"), s(&it, "title"))}</div>
        <div class="dw-meta">
            {meta.join(" · ")}
            {(!comments.is_empty()).then(|| view! { " · " {link(comments.clone(), format!("{nc} comments"))} })}
        </div>
    }
    .into_any()
}
fn release(it: Value) -> AnyView {
    view! {
        <div class="dw-title">{link(s(&it, "url"), s(&it, "repo"))}</div>
        <div class="dw-meta">{s(&it, "name")}{(n(&it, "time") > 0).then(|| format!(" · {}", ago(n(&it, "time"))))}</div>
    }
    .into_any()
}
fn cards(items: &[Value]) -> impl IntoView + use<> {
    view! {
        <div class="dw-cards">
            {items.iter().take(15).map(|it| {
                let img = s(it, "image");
                view! {
                    <a class="dw-card" href=s(it, "url") target="_blank" rel="noopener noreferrer">
                        {(!img.is_empty()).then(|| view! { <img src=img alt="" loading="lazy"/> })}
                        <span class="dw-title">{s(it, "title")}</span>
                        <span class="dw-meta">{s(it, "source")}{(n(it, "time") > 0).then(|| format!(" · {}", ago(n(it, "time"))))}</span>
                    </a>
                }
            }).collect_view()}
        </div>
    }
}
fn videos(items: &[Value]) -> impl IntoView + use<> {
    view! {
        <div class="dw-cards">
            {items.iter().map(|it| {
                let img = s(it, "image");
                view! {
                    <a class="dw-card" href=s(it, "url") target="_blank" rel="noopener noreferrer">
                        {(!img.is_empty()).then(|| view! { <img src=img alt="" loading="lazy"/> })}
                        <span class="dw-title">{s(it, "title")}</span>
                        <span class="dw-meta">{s(it, "source")}{(n(it, "time") > 0).then(|| format!(" · {}", ago(n(it, "time"))))}</span>
                    </a>
                }
            }).collect_view()}
        </div>
    }
}

fn spark(points: &[f64], up: bool) -> impl IntoView + use<> {
    let (lo, hi) = points.iter().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
    let k = if hi > lo { hi - lo } else { 1.0 };
    let m = points.len().max(2) - 1;
    let pts = points.iter().enumerate().map(|(i, v)| format!("{:.1},{:.1}", i as f64 * 100.0 / m as f64, 28.0 - (v - lo) / k * 26.0)).collect::<Vec<_>>().join(" ");
    view! {
        <svg class="dw-spark" viewBox="0 0 100 30" preserveAspectRatio="none" aria-hidden="true">
            <polyline points=pts fill="none" stroke=if up { "var(--positive)" } else { "var(--negative)" } stroke-width="1.5" vector-effect="non-scaling-stroke"/>
        </svg>
    }
}
fn markets(list: &[Value]) -> impl IntoView + use<> {
    view! {
        <ul class="dw-markets">
            {list.iter().map(|m| {
                let ch = m["change"].as_f64().unwrap_or(0.0);
                let price = m["price"].as_f64().unwrap_or(0.0);
                let pts: Vec<f64> = m["spark"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
                let sym = s(m, "symbol");
                let url = format!("https://finance.yahoo.com/quote/{}", js_sys::encode_uri_component(&sym));
                view! {
                    <li>
                        <span class="dw-mk-name">{link(url, sym.clone())}<small>{s(m, "name")}</small></span>
                        {(pts.len() > 1).then(|| spark(&pts, ch >= 0.0))}
                        <span class="dw-mk-num">
                            {if m["error"].as_bool() == Some(true) { "–".to_string() } else { format!("{price:.2}") }}
                            <small class={ if ch >= 0.0 { "pos" } else { "neg" } }>{format!("{}{ch:.2}%", if ch >= 0.0 { "+" } else { "" })}</small>
                        </span>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}
fn monitor(v: &Value) -> impl IntoView + use<> {
    view! {
        <ul class="dw-monitor">
            {arr(v, "sites").into_iter().map(|x| {
                let up = x["up"].as_bool().unwrap_or(false);
                let status = if up { format!("{} ms", n(&x, "ms")) } else { x["status"].as_i64().map(|c| format!("{c}")).unwrap_or_else(|| "down".into()) };
                view! {
                    <li>
                        <i class=if up { "dw-dot up" } else { "dw-dot down" }/>
                        <span class="grow truncate">{link(s(&x, "url"), s(&x, "title"))}</span>
                        <small class=if up { "subdue" } else { "neg" }>{status}</small>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}
fn repository(v: &Value) -> impl IntoView + use<> {
    let section = |title: &'static str, items: Vec<Value>| {
        (!items.is_empty()).then(|| view! {
            <div class="dw-sub">{title}</div>
            <ul class="dw-list">{items.into_iter().map(|i| view! { <li><div class="dw-title">{link(s(&i, "url"), s(&i, "title"))}</div></li> }).collect_view()}</ul>
        })
    };
    view! {
        <div class="dw-title">{link(s(v, "url"), s(v, "name"))}</div>
        <div class="dw-meta">{format!("{} stars · {} forks · {} open issues", n(v, "stars"), n(v, "forks"), n(v, "open_issues"))}</div>
        {section("Pull requests", arr(v, "pulls"))}
        {section("Issues", arr(v, "issues"))}
        {section("Commits", arr(v, "commits"))}
    }
}
fn bookmarks(o: &Value) -> impl IntoView + use<> {
    view! {
        <div class="dw-bookmarks">
            {arr(o, "groups").into_iter().map(|g| view! {
                <div class="dw-sub">{s(&g, "title")}</div>
                <ul class="dw-list">
                    {arr(&g, "links").into_iter().map(|l| view! { <li><div class="dw-title">{link(s(&l, "url"), s(&l, "title"))}</div></li> }).collect_view()}
                </ul>
            }).collect_view()}
        </div>
    }
}

/// WMO weather codes in words.
fn sky(code: i64) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95..=99 => "Thunderstorm",
        _ => "",
    }
}
fn weather(v: &Value) -> impl IntoView + use<> {
    let c = &v["current"];
    let d = &v["daily"];
    let t = c["temperature_2m"].as_f64().unwrap_or(0.0).round();
    let feels = c["apparent_temperature"].as_f64().unwrap_or(0.0).round();
    let hi = d["temperature_2m_max"][0].as_f64().unwrap_or(0.0).round();
    let lo = d["temperature_2m_min"][0].as_f64().unwrap_or(0.0).round();
    let now = c["time"].as_str().unwrap_or("").get(..13).unwrap_or("").to_string();
    let h = &v["hourly"];
    let times: Vec<String> = h["time"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let start = times.iter().position(|x| x.get(..13) == Some(now.as_str())).unwrap_or(0);
    let hours: Vec<(String, f64, i64)> = (start..(start + 12).min(times.len()))
        .step_by(2)
        .map(|i| (times[i].get(11..13).unwrap_or("").to_string(), h["temperature_2m"][i].as_f64().unwrap_or(0.0).round(), h["precipitation_probability"][i].as_i64().unwrap_or(0)))
        .collect();
    view! {
        <div class="dw-weather">
            <div class="row">
                <span class="dw-temp">{format!("{t}°")}</span>
                <span class="grow">
                    <span class="hi">{sky(c["weather_code"].as_i64().unwrap_or(-1))}</span>
                    <span class="dw-meta" style="display:block">{format!("Feels {feels}° · {lo}°–{hi}°")}</span>
                </span>
            </div>
            <div class="dw-meta">{s(v, "place")}</div>
            <div class="dw-hours">
                {hours.into_iter().map(|(hh, t, p)| view! { <span><small>{hh}</small><b>{format!("{t}°")}</b>{(p >= 20).then(|| view! { <small class="pos">{format!("{p}%")}</small> })}</span> }).collect_view()}
            </div>
        </div>
    }
}

#[component]
fn ClockW(o: Value) -> impl IntoView {
    let tick = RwSignal::new(0u32);
    if let Ok(h) = set_interval_with_handle(move || tick.update(|n| *n += 1), std::time::Duration::from_secs(10)) {
        on_cleanup(move || h.clear());
    }
    let h12 = o["hour-format"].as_str() == Some("12h");
    let fmt = move |tz: Option<&str>| {
        let opts = js_sys::Object::new();
        let set = |k: &str, v: JsValue| {
            let _ = js_sys::Reflect::set(&opts, &k.into(), &v);
        };
        set("hour", "2-digit".into());
        set("minute", "2-digit".into());
        set("hour12", JsValue::from_bool(h12));
        if let Some(tz) = tz {
            set("timeZone", tz.into());
        }
        let d = js_sys::Date::new_0();
        String::from(d.to_locale_string("en-GB", &opts))
    };
    let zones = arr(&o, "timezones");
    view! {
        <div class="dw-clock">
            <div class="dw-time">{move || { tick.track(); fmt(None) }}</div>
            <div class="dw-meta">{move || { tick.track(); crate::pages::calendar::util::fmt_day(&today_str(), &[("weekday", "long"), ("day", "numeric"), ("month", "long")]) }}</div>
            <ul class="dw-zones">
                {zones.into_iter().map(|z| {
                    let tz = s(&z, "timezone");
                    let label = s(&z, "label");
                    view! { <li><span class="grow">{if label.is_empty() { tz.clone() } else { label }}</span><b>{move || { tick.track(); fmt(Some(&tz)) }}</b></li> }
                }).collect_view()}
            </ul>
        </div>
    }
}

#[component]
fn SearchW(o: Value) -> impl IntoView {
    let q = RwSignal::new(String::new());
    let engine = s(&o, "search-engine");
    let go = move || {
        let text = q.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        let e = js_sys::encode_uri_component(text.trim());
        let url = match engine.as_str() {
            "google" => format!("https://www.google.com/search?q={e}"),
            "bing" => format!("https://www.bing.com/search?q={e}"),
            "kagi" => format!("https://kagi.com/search?q={e}"),
            "startpage" => format!("https://www.startpage.com/search?q={e}"),
            _ => format!("https://duckduckgo.com/?q={e}"),
        };
        if let Some(w) = web_sys::window() {
            let _ = w.open_with_url_and_target(&url, "_blank");
        }
        q.set(String::new());
    };
    view! {
        <form class="dw-search" on:submit=move |e| { e.prevent_default(); go() }>
            <Icon name="search"/>
            <input placeholder="Search the web…" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e)) aria-label="Search the web"/>
        </form>
    }
}

/// The mail numbers; each opens Mail.
#[component]
fn MailW() -> impl IntoView {
    let d = fetch("/summary".into());
    move || {
        let r = d.get();
        if let Some(v) = loading(&r) {
            return v;
        }
        let v = r.and_then(|r| r.ok()).unwrap_or_default();
        let cell = |k: &str, label: &'static str, tab: &'static str| {
            let x = n(&v, k);
            view! { <button class="dw-num" on:click=move |_| navigate("mail", Where::Replace, &[("tab", tab)])><b>{x}</b><small>{label}</small></button> }
        };
        view! {
            <div class="dw-nums">
                {cell("to_reply", "To reply", "reply")}
                {cell("awaiting", "Awaiting", "reply")}
                {cell("unread", "Unread", "inbox")}
                {cell("actions_today", "Auto-handled", "stats")}
            </div>
        }
        .into_any()
    }
}

/// This month with your events (dots per day), and what's next. A day opens the Calendar there.
#[component]
fn CalendarW() -> impl IntoView {
    let d = fetch("/calendar/widget".into());
    let open = |date: String| navigate("calendar", Where::Replace, &[("date", &date), ("view", "week")]);
    move || {
        let r = d.get();
        if let Some(v) = loading(&r) {
            return v;
        }
        let v = r.and_then(|r| r.ok()).unwrap_or_default();
        let left = n(&v, "today_left");
        let next = arr(&v, "next");
        view! {
            <div class="dw-cal">
                <div class="row">
                    <span class="hi grow">{s(&v, "title")}</span>
                    <small class="subdue">{format!("{} {}", n(&v, "today_n"), s(&v["labels"], "today"))}{(left > 0).then(|| format!(" · {}", s(&v["labels"], "left")))}</small>
                </div>
                <div class="dw-cal-grid">
                    {arr(&v, "dow").into_iter().map(|x| view! { <span class="dw-dow">{x.as_str().unwrap_or("").to_string()}</span> }).collect_view()}
                    {arr(&v, "days").into_iter().map(|x| {
                        let date = s(&x, "date");
                        let cls = format!("dw-day{}{}{}{}", if x["today"].as_bool() == Some(true) { " today" } else { "" }, if x["month"].as_bool() == Some(false) { " spill" } else { "" },
                            if x["past"].as_bool() == Some(true) { " past" } else { "" }, if x["holiday"].as_bool() == Some(true) { " holiday" } else { "" });
                        view! {
                            <button class=cls title=s(&x, "tip") on:click=move |_| open(date.clone())>
                                {n(&x, "d")}
                                <span class="dw-dots">{arr(&x, "dots").into_iter().map(|k| view! { <i class=format!("k-{}", k.as_str().unwrap_or("personal"))/> }).collect_view()}</span>
                            </button>
                        }
                    }).collect_view()}
                </div>
                {if next.is_empty() {
                    view! { <div class="dw-meta">{s(&v["labels"], "none")}</div> }.into_any()
                } else {
                    view! {
                        <ul class="dw-next">
                            {next.into_iter().map(|e| {
                                let date = s(&e, "date");
                                view! {
                                    <li class=format!("k-{}", s(&e, "kind")) on:click=move |_| open(date.clone())>
                                        <small class=if e["now"].as_bool() == Some(true) { "pos" } else { "subdue" }>{s(&e, "when")}</small>
                                        <span class="hi truncate">{s(&e, "title")}</span>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }}
            </div>
        }
        .into_any()
    }
}

/// Open to-dos (and the ones done today), tick them off, add one.
#[component]
fn TodoW() -> impl IntoView {
    let todos = RwSignal::new(None::<Vec<Todo>>);
    let load = move || {
        spawn_local(async move {
            if let Ok(ns) = api_get::<Vec<Value>>("/notes").await {
                todos.try_set(Some(ns.iter().filter(|n| is_todo(n) && keep(n)).map(Todo::from).collect()));
            }
        })
    };
    load();
    let text = RwSignal::new(String::new());
    let add = move || {
        let t = text.get_untracked().trim().to_string();
        if t.is_empty() {
            return;
        }
        spawn_local(async move {
            match api_post::<Value>("/todos", &json!({"title": t, "due": "", "label": ""})).await {
                Ok(_) => {
                    text.set(String::new());
                    load();
                }
                Err(e) => toast_bad(e.message),
            }
        });
    };
    let flip = move |t: Todo| {
        spawn_local(async move {
            match api_put::<Value>(&format!("/todos/{}", js_sys::encode_uri_component(&t.id)), &json!({"done": !t.done})).await {
                Ok(_) => load(),
                Err(e) => toast_bad(e.message),
            }
        })
    };
    view! {
        <form class="dw-search" on:submit=move |e| { e.prevent_default(); add() }>
            <Icon name="plus"/>
            <input placeholder="Add a to-do…" prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e)) aria-label="Add a to-do"/>
        </form>
        {move || match todos.get() {
            None => view! { <div class="dw-empty">"Loading…"</div> }.into_any(),
            Some(v) if v.is_empty() => view! { <div class="dw-empty">"Nothing to do."</div> }.into_any(),
            Some(v) => view! {
                <ul class="dw-todos">
                    {v.into_iter().map(|t| {
                        let t2 = t.clone();
                        view! {
                            <li class:done=t.done>
                                <button type="button" class="td-ring" aria-pressed=t.done.to_string() aria-label=format!("{}: {}", if t.done { "Not done" } else { "Mark done" }, t.title) on:click=move |_| flip(t2.clone())/>
                                <span class="grow">{t.title.clone()}</span>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }.into_any(),
        }}
    }
}

/// The brief: the greeting and the weather, the headline typed out, what matters, and today as
/// a ribbon with a now line. A click opens Today.
#[component]
fn BriefW() -> impl IntoView {
    let b = RwSignal::new(None::<Result<Value, String>>);
    let load = move || load_brief(b);
    load();
    let tick = RwSignal::new(0u32);
    if let Ok(h) = set_interval_with_handle(move || tick.update(|n| *n += 1), std::time::Duration::from_secs(60)) {
        on_cleanup(move || h.clear());
    }
    if let Ok(h) = set_interval_with_handle(move || {
        if !web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden()) {
            load()
        }
    }, std::time::Duration::from_secs(300)) {
        on_cleanup(move || h.clear());
    }
    subscribe(&["app"]);
    on_ws(move |m| {
        if let agentum_proto::api::WsOut::App { payload } = m {
            if payload["kind"] == "today" {
                load();
            }
        }
    });
    let open_today = |_| navigate("today", Where::Replace, &[]);
    move || {
        tick.track();
        let r = b.get();
        let v = r.clone().and_then(|r| r.ok()).unwrap_or_default();
        let w = &v["weather"];
        let hour = js_sys::Date::new_0().get_hours();
        let greet = if !(4..19).contains(&hour) { "Good evening" } else if hour < 12 { "Good morning" } else { "Good afternoon" };
        let kicker = [s(&v, "date"), s(w, "place")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        let pending = v["pending"].as_bool() == Some(true);
        let say = match &r {
            Some(Err(e)) => format!("Could not load the brief ({e})."),
            None => "Writing your brief…".into(),
            Some(Ok(_)) if pending => "Writing your brief…".into(),
            Some(Ok(_)) => s(&v, "text"),
        };
        let matters: Vec<Value> = arr(&v, "matters").into_iter().filter(|x| x["k"] != "now" && x["k"] != "next").take(3).collect();
        let ready = matches!(r, Some(Ok(_))) && !pending;
        view! {
            <div class="td bw">
                <section class="td-hero dw-hero" on:click=open_today>
                    <div class="td-head">
                        <div class="td-kicker">{kicker}</div>
                        <h1>{greet}</h1>
                        {w.is_object().then(|| view! { <div class="td-temp">{format!("{}°", n(w, "now"))}<small>{format!("{} · feels {}°", s(w, "sky"), n(w, "feels"))}</small></div> })}
                    </div>
                </section>
                <main class="bw-main">
                    {(!say.is_empty()).then(|| view! { <div class="td-say"><span>{say}</span></div> })}
                    {(!matters.is_empty()).then(|| view! {
                        <ul class="td-mts">
                            {matters.into_iter().map(|x| view! {
                                <li class=format!("mt-{}", s(&x, "k"))>
                                    <span class="ic">{matter_icon(&s(&x, "k"))}</span>
                                    <span class="tx"><span>{s(&x, "text")}</span><small>{s(&x, "detail")}</small></span>
                                </li>
                            }).collect_view()}
                        </ul>
                    })}
                    {ready.then(|| ribbon(&v))}
                    <div class=if v["by_model"].as_bool() == Some(true) { "bw-foot" } else { "bw-foot off" }>
                        <span>{ready.then(|| view! { <b>"●"</b>{if v["by_model"].as_bool() == Some(true) { " Headline by your model" } else { " From your calendar and mail" }} })}</span>
                        <a href="/today" on:click=move |e| { e.prevent_default(); navigate("today", Where::Replace, &[]) }>"Open Today →"</a>
                    </div>
                </main>
            </div>
        }
    }
}
/// A brief still being written is asked for again in 4 s.
fn load_brief(b: RwSignal<Option<Result<Value, String>>>) {
    spawn_local(async move {
        let r = api_get::<Value>("/brief").await.map_err(|e| e.message);
        let pending = r.as_ref().is_ok_and(|v| v["pending"].as_bool() == Some(true));
        if b.try_set(Some(r)).is_none() && pending {
            set_timeout(move || load_brief(b), std::time::Duration::from_secs(4));
        }
    })
}
fn matter_icon(k: &str) -> &'static str {
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
fn dur(m: i32) -> String {
    if m >= 60 { format!("{}h{}", m / 60, if m % 60 > 0 { format!("{:02}", m % 60) } else { String::new() }) } else { format!("{m} min") }
}
/// Today as a ribbon: what's on now or next, the events as blocks, the now line.
fn ribbon(b: &Value) -> impl IntoView + use<> {
    let nm = now_min();
    let ev: Vec<(String, String, String, String, i32, i32)> = arr(b, "timed")
        .iter()
        .map(|e| {
            let st = mins(&s(e, "time"));
            (s(e, "title"), s(e, "time"), s(e, "end"), s(e, "kind"), st, mins(&s(e, "end")).max(st + 10))
        })
        .collect();
    let lo = ev.iter().map(|e| e.4 / 60 * 60).min().unwrap_or(420).min(420);
    let hi = ev.iter().map(|e| (e.5 + 59) / 60 * 60).max().unwrap_or(1440).max(1440);
    let pct = move |m: i32| (m.clamp(lo, hi) - lo) as f64 / (hi - lo) as f64 * 100.0;
    let live = ev.iter().position(|e| e.4 <= nm && e.5 > nm);
    let next = ev.iter().position(|e| e.4 > nm);
    let tm = &b["tomorrow_first"];
    let head = if let Some(i) = live {
        view! { <span class="lbl">"Now"</span><span class="ttl">{ev[i].0.clone()}</span><span class="in">"until "{ev[i].2.clone()}</span> }.into_any()
    } else if let Some(i) = next {
        view! { <span class="lbl">"Next"</span><b>{ev[i].1.clone()}</b><span class="ttl">{ev[i].0.clone()}</span><span class="in">"in "{dur(ev[i].4 - nm)}</span> }.into_any()
    } else if tm.is_object() {
        view! { <span class="lbl">"Tomorrow"</span>{(!s(tm, "time").is_empty()).then(|| view! { <b>{s(tm, "time")}</b> })}<span class="ttl">{s(tm, "title")}</span> }.into_any()
    } else {
        view! { <span class="lbl">"Free"</span><span class="ttl">{if ev.is_empty() { "Nothing on the calendar today" } else { "Nothing else today" }}</span> }.into_any()
    };
    let hours: Vec<i32> = ((lo + 179) / 180 * 3..).step_by(3).take_while(|h| h * 60 <= hi).collect();
    view! {
        <div>
            <div class="bw-next">{head}</div>
            <div class="bw-rib" aria-hidden="true">
                {ev.iter().enumerate().map(|(i, e)| {
                    let state = if e.5 <= nm { "past" } else if Some(i) == live { "live" } else if Some(i) == next { "nxt" } else { "" };
                    let kind = if e.3.is_empty() { "personal".to_string() } else { e.3.clone() };
                    view! { <span class=format!("ev k-{kind} {state}") style=format!("left:{}%;width:{}%;--d:{}s", pct(e.4), pct(e.5) - pct(e.4), 0.5 + i as f64 * 0.07) title=format!("{} {}", e.1, e.0)/> }
                }).collect_view()}
                {(nm >= lo && nm <= hi).then(|| view! { <span class="now" style=format!("left:{}%", pct(nm))/> })}
            </div>
            <div class="bw-ticks" aria-hidden="true">{hours.into_iter().map(|h| view! { <span style=format!("left:{}%", pct(h * 60))>{format!("{:02}", h % 24)}</span> }).collect_view()}</div>
        </div>
    }
}
