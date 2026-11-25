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

/// GET a path into a signal (None while loading, Err on failure). With `every`, again on that
/// interval while the page is visible (a failed refresh keeps what was there).
fn fetch(path: String, every: Option<u32>) -> RwSignal<Option<Result<Value, String>>> {
    let out = RwSignal::new(None);
    let get = move |path: String| {
        spawn_local(async move {
            let r = api_get::<Value>(&path).await.map_err(|e| e.message);
            if r.is_ok() || out.try_with_untracked(|o| !matches!(o, Some(Ok(_)))).unwrap_or(false) {
                out.try_set(Some(r));
            }
        })
    };
    get(path.clone());
    if let Some(secs) = every {
        if let Ok(h) = set_interval_with_handle(move || {
            if !web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden()) {
                get(path.clone())
            }
        }, std::time::Duration::from_secs(secs as u64)) {
            on_cleanup(move || h.clear());
        }
    }
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

/// A logo as Glance draws `icon:` — `si:<name>` (simple-icons, served by the hub) as a mask the
/// theme tints, or an image URL as it is.
pub fn logo(icon: &str, class: &'static str) -> Option<AnyView> {
    if let Some(n) = icon.strip_prefix("si:") {
        let n: String = n.trim().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if n.is_empty() {
            return None;
        }
        let url = format!("url(/api/icons/si/{n})");
        return Some(view! { <span class=format!("si {class}") style=format!("-webkit-mask-image:{url};mask-image:{url}") aria-hidden="true"/> }.into_any());
    }
    (icon.starts_with("https://") || icon.starts_with("http://") || icon.starts_with('/')).then(|| view! { <img class=class src=icon.to_string() alt="" loading="lazy"/> }.into_any())
}

/// The widget draws its own frames (a carousel's cards, a watchlist's sections).
pub fn frameless(w: &Value) -> bool {
    let o = &w["options"];
    match s(w, "type").as_str() {
        "watchlist" => !matches!(o["layout"].as_str(), Some("list") | Some("tabs")),
        "rss" => o["style"].as_str() == Some("horizontal-cards"),
        "videos" => true,
        _ => false,
    }
}
/// The widget's sections carry the headers (a watchlist in sections, as Glance's split column):
/// its own shows only while editing the layout.
pub fn headless(w: &Value) -> bool {
    s(w, "type") == "watchlist" && frameless(w)
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
        "usage" => view! { <crate::side::UsageW/> }.into_any(),
        t => {
            let t = t.to_string();
            let every = match t.as_str() { "monitor" => Some(60), "markets" | "watchlist" => Some(300), "weather" => Some(900), _ => None };
            let d = fetch(format!("/widgets/{}/data", js_sys::encode_uri_component(&id)), every);
            let collapse = o["collapse-after"].as_u64().map(|x| x as usize);
            (move || {
                let r = d.get();
                if let Some(v) = loading(&r) {
                    return v;
                }
                let v = r.and_then(|r| r.ok()).unwrap_or_default();
                match t.as_str() {
                    "weather" => weather(&v, &o).into_any(),
                    "monitor" => monitor(&v).into_any(),
                    "markets" => markets(&arr(&v, "markets")).into_any(),
                    "watchlist" => watchlist(&v, &o),
                    "repository" => repository(&v).into_any(),
                    "releases" => view! { <Collapse n=collapse items=arr(&v, "items") row=release/> }.into_any(),
                    "videos" => view! { <Carousel items=arr(&v, "items")/> }.into_any(),
                    "rss" if o["style"].as_str() == Some("horizontal-cards") => view! { <Carousel items=arr(&v, "items")/> }.into_any(),
                    _ => view! { <Collapse n=collapse items=arr(&v, "items") row=story/> }.into_any(),
                }
            })
            .into_any()
        }
    }
}

/// A list that shows `n` rows and a "Show more" (Glance's collapsible list).
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
        {n.map(|_| view! {
            <button class=move || if open.get() { "dw-more open" } else { "dw-more" } aria-expanded=move || open.get().to_string() on:click=move |_| open.update(|o| *o = !*o)>
                {move || if open.get() { "Show less" } else { "Show more" }}<Icon name="chevron" size=12/>
            </button>
        })}
    }
    .into_any()
}

/// "a · b · c", as Glance's horizontal lists.
fn dots(parts: Vec<AnyView>) -> impl IntoView {
    view! { <ul class="dw-dots-list">{parts.into_iter().map(|p| view! { <li>{p}</li> }).collect_view()}</ul> }
}
fn story(it: Value) -> AnyView {
    let mut meta: Vec<AnyView> = vec![];
    let t = n(&it, "time");
    if t > 0 {
        meta.push(ago(t).into_any());
    }
    if it["score"].is_number() {
        meta.push(format!("{} points", n(&it, "score")).into_any());
    }
    let comments = s(&it, "comments_url");
    if !comments.is_empty() {
        meta.push(link(comments, format!("{} comments", n(&it, "comments"))).into_any());
    }
    for k in ["source", "flair"] {
        let x = s(&it, k);
        if !x.is_empty() {
            meta.push(x.into_any());
        }
    }
    let domain = s(&it, "domain");
    if !domain.is_empty() {
        meta.push(view! { <a class="dw-domain" href=s(&it, "url") target="_blank" rel="noopener noreferrer">{domain}</a> }.into_any());
    }
    view! {
        <a class="dw-title" href=s(&it, "url") target="_blank" rel="noopener noreferrer">{s(&it, "title")}</a>
        {dots(meta)}
    }
    .into_any()
}
fn release(it: Value) -> AnyView {
    let mut meta: Vec<AnyView> = vec![];
    if n(&it, "time") > 0 {
        meta.push(ago(n(&it, "time")).into_any());
    }
    meta.push(s(&it, "name").into_any());
    view! {
        <a class="dw-title truncate" href=s(&it, "url") target="_blank" rel="noopener noreferrer">{s(&it, "repo")}</a>
        {dots(meta)}
    }
    .into_any()
}

/// Glance's horizontal cards: a row that scrolls sideways, faded at the cut edges, with arrows
/// (the last page wraps round to the first).
#[component]
fn Carousel(items: Vec<Value>) -> impl IntoView {
    let track = NodeRef::<leptos::html::Div>::new();
    let cut_l = RwSignal::new(false);
    let cut_r = RwSignal::new(false);
    let measure = move || {
        if let Some(t) = track.get_untracked() {
            let (x, w, all) = (t.scroll_left(), t.client_width(), t.scroll_width());
            cut_l.set(x > 2);
            cut_r.set(x + w + 2 < all);
        }
    };
    Effect::new(move |_| {
        if track.get().is_some() {
            set_timeout(move || { let _ = track.try_get_untracked().map(|_| measure()); }, std::time::Duration::from_millis(0));
        }
    });
    let step = move |dir: i32| {
        let Some(t) = track.get_untracked() else { return };
        let (x, w, all) = (t.scroll_left(), t.client_width(), t.scroll_width());
        let to = if dir > 0 && x + w + 2 >= all { 0 } else if dir < 0 && x <= 2 { all } else { x + dir * (w * 4 / 5) };
        t.set_scroll_left(to);
    };
    if items.is_empty() {
        return view! { <div class="dw-empty">"Nothing here."</div> }.into_any();
    }
    view! {
        <div class=move || format!("dw-carousel{}{}", if cut_l.get() { " cut-l" } else { "" }, if cut_r.get() { " cut-r" } else { "" })>
            <div class="dw-track" node_ref=track on:scroll=move |_| measure()>
                {items.into_iter().map(|it| {
                    let img = s(&it, "image");
                    let mut meta: Vec<AnyView> = vec![];
                    if n(&it, "time") > 0 { meta.push(ago(n(&it, "time")).into_any()) }
                    if !s(&it, "source").is_empty() { meta.push(s(&it, "source").into_any()) }
                    view! {
                        <div class="dw-card">
                            {(!img.is_empty()).then(|| view! { <img src=img alt="" loading="lazy"/> })}
                            <div class="dw-card-text">
                                <a class="dw-title" href=s(&it, "url") target="_blank" rel="noopener noreferrer">{s(&it, "title")}</a>
                                {dots(meta)}
                            </div>
                        </div>
                    }
                }).collect_view()}
            </div>
            <button class="dw-car-btn prev" title="Back" aria-label="Back" on:click=move |_| step(-1)><Icon name="chevron-left" size=16/></button>
            <button class="dw-car-btn next" title="More" aria-label="More" on:click=move |_| step(1)><Icon name="chevron-right" size=16/></button>
        </div>
    }
    .into_any()
}

/// 1234.5 → "1,234.50" (4 decimals under 10, as the chart).
pub fn price_text(x: f64, digits: Option<usize>) -> String {
    let d = digits.unwrap_or(if x.abs() < 10.0 { 4 } else { 2 });
    let s = format!("{:.*}", d, x.abs());
    let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), format!(".{b}"))).unwrap_or((s.clone(), String::new()));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{}{out}{frac}", if x < 0.0 { "−" } else { "" })
}
/// The currency's sign, as Glance prints prices ("$349.21", "€31.45"; others bare).
fn currency_sign(c: &str) -> &'static str {
    match c {
        "USD" => "$",
        "EUR" => "€",
        "GBP" | "GBp" => "£",
        "JPY" | "CNY" => "¥",
        "INR" => "₹",
        "KRW" => "₩",
        "BRL" => "R$",
        "CAD" => "C$",
        "AUD" => "A$",
        "CHF" => "Fr",
        _ => "",
    }
}
fn spark(points: &[f64]) -> impl IntoView + use<> {
    let (lo, hi) = points.iter().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
    let k = if hi > lo { hi - lo } else { 1.0 };
    let m = points.len().max(2) - 1;
    let pts = points.iter().enumerate().map(|(i, v)| format!("{:.2},{:.2}", i as f64 * 100.0 / m as f64, 49.0 - (v - lo) / k * 48.0)).collect::<Vec<_>>().join(" ");
    view! {
        <svg class="dw-spark" viewBox="0 0 100 50" preserveAspectRatio="none" aria-hidden="true">
            <polyline points=pts fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>
        </svg>
    }
}
/// Glance's markets: symbol and name, a month's sparkline, the day's change over the price.
/// A row opens its chart.
fn markets(list: &[Value]) -> impl IntoView + use<> {
    view! {
        <ul class="dw-markets">
            {list.iter().map(|m| {
                let ch = m["change"].as_f64().unwrap_or(0.0);
                let price = m["price"].as_f64().unwrap_or(0.0);
                let failed = m["error"].as_bool() == Some(true);
                let pts: Vec<f64> = m["spark"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
                let sym = s(m, "symbol");
                let name = s(m, "name");
                let cur = s(m, "currency");
                let price_s = if failed { "–".to_string() } else { format!("{}{}", currency_sign(&cur), price_text(price, None)) };
                let change_s = if failed { String::new() } else { format!("{}{:.2}%", if ch >= 0.0 { "+" } else { "" }, ch) };
                let item = super::chart::Item { symbol: sym.clone(), name: name.clone(), price: price_s.clone(), change: ch, currency: cur.clone() };
                let label = format!("{sym}: open its chart");
                view! {
                    <li role="button" tabindex="0" aria-label=label
                        on:click={ let item = item.clone(); move |_| super::chart::open(item.clone()) }
                        on:keydown=move |e| if e.key() == "Enter" || e.key() == " " { e.prevent_default(); super::chart::open(item.clone()) }>
                        <span class="dw-mk-name"><b class="truncate">{sym}</b><small class="truncate">{name}</small></span>
                        {(pts.len() > 1).then(|| spark(&pts))}
                        <span class="dw-mk-num">
                            <b class={ if failed { "" } else if ch > 0.0 { "pos" } else if ch < 0.0 { "neg" } else { "" } }>{change_s}</b>
                            <small title=cur>{price_s}</small>
                        </span>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}
/// A watchlist: its categories as Glance's split column (each its own card), as tabs, or as one list.
fn watchlist(v: &Value, o: &Value) -> AnyView {
    let cats = arr(v, "categories");
    match o["layout"].as_str() {
        Some("list") => view! {
            <div class="dw-watch">
                {cats.into_iter().map(|c| view! { <div class="dw-sub">{s(&c, "title")}</div>{markets(&arr(&c, "markets"))} }).collect_view()}
            </div>
        }.into_any(),
        Some("tabs") => {
            let on = RwSignal::new(0usize);
            let titles: Vec<String> = cats.iter().map(|c| s(c, "title")).collect();
            view! {
                <div class="dw-tabs" role="tablist">
                    {titles.into_iter().enumerate().map(|(i, t)| view! {
                        <button role="tab" aria-selected=move || (on.get() == i).to_string() class:on=move || on.get() == i on:click=move |_| on.set(i)>{t}</button>
                    }).collect_view()}
                </div>
                {move || cats.get(on.get()).map(|c| markets(&arr(c, "markets")))}
            }.into_any()
        }
        _ => view! {
            <div class="dw-watch sections">
                {cats.into_iter().map(|c| view! {
                    <div class="dw-wcat">
                        <header class="dw-head"><h2>{s(&c, "title")}</h2></header>
                        <div class="dw-frame">{markets(&arr(&c, "markets"))}</div>
                    </div>
                }).collect_view()}
            </div>
        }.into_any(),
    }
}

const ICON_OK: &str = "M10 18a8 8 0 1 0 0-16 8 8 0 0 0 0 16Zm3.857-9.809a.75.75 0 0 0-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 1 0-1.06 1.061l2.5 2.5a.75.75 0 0 0 1.137-.089l4-5.5Z";
const ICON_WARN: &str = "M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495ZM10 5a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 10 5Zm0 9a1 1 0 1 0 0-2 1 1 0 0 0 0 2Z";
/// What Glance says of a status code.
fn status_text(code: u16, ok: bool) -> String {
    match code {
        _ if ok => "OK".into(),
        401 => "Unauthorized".into(),
        403 => "Forbidden".into(),
        404 => "Not Found".into(),
        500.. => "Server Error".into(),
        400.. => "Client Error".into(),
        c => c.to_string(),
    }
}
/// Glance's monitor: each site's logo, name, status and time, and a check or a warning.
fn monitor(v: &Value) -> impl IntoView + use<> {
    view! {
        <ul class="dw-monitor">
            {arr(v, "sites").into_iter().map(|x| {
                let up = x["up"].as_bool().unwrap_or(false);
                let ok = x["ok"].as_bool().unwrap_or(up);
                let code = x["status"].as_u64().map(|c| c as u16);
                let (state, word) = match (code, x["error"].as_str()) {
                    (Some(c), _) => (if ok { "ok" } else if up { "warn" } else { "down" }, status_text(c, ok)),
                    (None, Some("timeout")) => ("down", "Timed Out".to_string()),
                    _ => ("down", "Error".to_string()),
                };
                let ms = (state != "down" || code.is_some()).then(|| format!("{}ms", n(&x, "ms")));
                let title = s(&x, "title");
                view! {
                    <li class=format!("dw-site {state}")>
                        {logo(&s(&x, "icon"), "dw-site-logo")}
                        <div class="grow" style="min-width:0">
                            <a class="dw-site-name truncate" href=s(&x, "url") target="_blank" rel="noopener noreferrer">{title.clone()}</a>
                            <ul class="dw-dots-list">
                                <li class=if state == "ok" { "" } else { "neg" } title=code.map(|c| c.to_string()).unwrap_or_default()>{word.clone()}</li>
                                {ms.map(|m| view! { <li>{m}</li> })}
                            </ul>
                        </div>
                        <svg class="dw-site-state" viewBox="0 0 20 20" role="img" aria-label=format!("{title}: {word}")>
                            <path fill-rule="evenodd" clip-rule="evenodd" d=if state == "ok" { ICON_OK } else { ICON_WARN }/>
                        </svg>
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
            <ul class="dw-list">{items.into_iter().map(|i| view! { <li><a class="dw-title" href=s(&i, "url") target="_blank" rel="noopener noreferrer">{s(&i, "title")}</a></li> }).collect_view()}</ul>
        })
    };
    view! {
        <a class="dw-title" href=s(v, "url") target="_blank" rel="noopener noreferrer">{s(v, "name")}</a>
        {dots(vec![format!("{} stars", n(v, "stars")).into_any(), format!("{} forks", n(v, "forks")).into_any(), format!("{} open issues", n(v, "open_issues")).into_any()])}
        {section("Pull requests", arr(v, "pulls"))}
        {section("Issues", arr(v, "issues"))}
        {section("Commits", arr(v, "commits"))}
    }
}
/// Glance's bookmarks: groups in their colour ("h s l"), each link with its logo and an arrow.
fn bookmarks(o: &Value) -> impl IntoView + use<> {
    view! {
        <div class="dw-bookmarks">
            {arr(o, "groups").into_iter().map(|g| {
                let hsl: Vec<f64> = s(&g, "color").split_whitespace().filter_map(|x| x.parse().ok()).collect();
                let style = (hsl.len() == 3).then(|| format!("--bm:hsl({} {}% {}%)", hsl[0], hsl[1], hsl[2])).unwrap_or_default();
                view! {
                    <div class="dw-bm-group" style=style>
                        <div class="dw-bm-title">{s(&g, "title")}</div>
                        <ul class="dw-bm-links">
                            {arr(&g, "links").into_iter().map(|l| view! {
                                <li>
                                    {logo(&s(&l, "icon"), "dw-bm-logo").map(|i| view! { <span class="dw-bm-icon">{i}</span> })}
                                    <a href=s(&l, "url") target="_blank" rel="noopener noreferrer">{s(&l, "title")}</a>
                                </li>
                            }).collect_view()}
                        </ul>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// WMO weather codes in Glance's words.
fn sky(code: i64) -> &'static str {
    match code {
        0 => "Clear Sky",
        1 => "Mainly Clear",
        2 => "Partly Cloudy",
        3 => "Overcast",
        45 => "Fog",
        48 => "Rime Fog",
        51..=57 => "Drizzle",
        61 | 80 => "Rain",
        63 | 81 => "Moderate Rain",
        65 | 82 => "Heavy Rain",
        66 | 67 => "Freezing Rain",
        71 | 85 | 86 => "Snow",
        73 => "Moderate Snow",
        75 => "Heavy Snow",
        77 => "Snow Grains",
        95..=99 => "Thunderstorm",
        _ => "",
    }
}
/// Glance's weather: the sky and how it feels, today in twelve two-hour columns (the bar is the
/// temperature; daylight shaded, rain dotted, the current one marked; hover one for its value),
/// and the place.
fn weather(v: &Value, o: &Value) -> impl IntoView + use<> {
    let c = &v["current"];
    let h = &v["hourly"];
    let f = o["units"].as_str() == Some("imperial");
    let h12 = o["hour-format"].as_str() == Some("12h");
    let hour_of = |t: &str| t.get(11..13).and_then(|x| x.parse::<usize>().ok());
    let now = hour_of(c["time"].as_str().unwrap_or("")).unwrap_or(12);
    let rise = hour_of(v["daily"]["sunrise"][0].as_str().unwrap_or("")).map(|x| x / 2).unwrap_or(3);
    let set = hour_of(v["daily"]["sunset"][0].as_str().unwrap_or("")).map(|x| x.saturating_sub(1) / 2).unwrap_or(9);
    let temp = |i: usize| h["temperature_2m"][i].as_f64();
    let cols: Vec<(f64, bool)> = (0..12)
        .map(|i| {
            let t = match (temp(i * 2), temp(i * 2 + 1)) { (Some(a), Some(b)) => (a + b) / 2.0, (Some(a), None) | (None, Some(a)) => a, _ => f64::NAN };
            let rain = [i * 2, i * 2 + 1].iter().any(|k| h["precipitation_probability"][*k].as_i64().unwrap_or(0) >= 50);
            (t, rain)
        })
        .collect();
    let (lo, hi) = cols.iter().filter(|c| c.0.is_finite()).fold((f64::MAX, f64::MIN), |(a, b), c| (a.min(c.0), b.max(c.0)));
    let span = if hi > lo { hi - lo } else { 1.0 };
    let label = |i: usize| {
        let hr = (i * 2 + 2) % 24;
        if h12 { format!("{}{}", if hr % 12 == 0 { 12 } else { hr % 12 }, if hr < 12 { "am" } else { "pm" }) } else { format!("{hr:02}:00") }
    };
    let feels = c["apparent_temperature"].as_f64().unwrap_or(f64::NAN).round();
    let place = s(v, "place");
    view! {
        <div class="dw-weather">
            <div class="dw-sky">{sky(c["weather_code"].as_i64().unwrap_or(-1))}</div>
            <div class="dw-feels">{if feels.is_finite() { format!("Feels like {feels}°{}", if f { "F" } else { "C" }) } else { String::new() }}</div>
            <div class="dw-wcols">
                {cols.into_iter().enumerate().map(|(i, (t, rain))| {
                    let cls = format!("dw-wcol{}", if i == now / 2 { " now" } else { "" });
                    let day = (i >= rise && i <= set).then(|| format!("dw-day-sky{}", if i == rise { " rise" } else if i == set { " set" } else { "" }));
                    let scale = if t.is_finite() { (t - lo) / span } else { 0.0 };
                    view! {
                        <div class=cls>
                            {rain.then(|| view! { <div class="dw-rain"/> })}
                            {day.map(|c| view! { <div class=c/> })}
                            <div class="dw-wval">{if t.is_finite() { format!("{}{}", if t.round() < 0.0 { "-" } else { "" }, t.round().abs()) } else { "–".into() }}</div>
                            <div class="dw-wbar" style=format!("--h:{scale:.2}")/>
                            <div class="dw-wtime">{label(i)}</div>
                        </div>
                    }
                }).collect_view()}
            </div>
            {(o["hide-location"].as_bool() != Some(true) && !place.is_empty()).then(|| view! { <div class="dw-place"><i class="dw-pin"/><span class="truncate">{place}</span></div> })}
        </div>
    }
}

/// A time zone's time now ("HH:MM", or 12 h), None for a zone the browser doesn't know.
fn time_in(tz: Option<&str>, h12: bool) -> Option<String> {
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
    // Through Reflect, so an unknown zone (a RangeError) is an Err here instead of a trap.
    let d = js_sys::Date::new_0();
    let f = js_sys::Reflect::get(&d, &"toLocaleTimeString".into()).ok()?;
    let f: js_sys::Function = f.into();
    let args = js_sys::Array::of2(&"en-GB".into(), &opts);
    js_sys::Reflect::apply(&f, &d, &args).ok()?.as_string()
}
fn minutes_of(t: &str) -> Option<i32> {
    let (h, m) = t.split_once(':')?;
    Some(h.trim().parse::<i32>().ok()? * 60 + m.get(..2)?.parse::<i32>().ok()?)
}
/// Glance's clock: the date and the time, then each zone with how far it is from here.
#[component]
fn ClockW(o: Value) -> impl IntoView {
    let tick = RwSignal::new(0u32);
    if let Ok(h) = set_interval_with_handle(move || tick.update(|n| *n += 1), std::time::Duration::from_secs(5)) {
        on_cleanup(move || h.clear());
    }
    let h12 = o["hour-format"].as_str() == Some("12h");
    let zones = arr(&o, "timezones");
    let day = move |opts: &[(&str, &str)]| crate::pages::calendar::util::fmt_day_with(&today_str(), opts, Some("en-GB"));
    view! {
        <div class="dw-clock">
            <div class="dw-clock-top">
                <div><div class="dw-big">{move || { tick.track(); day(&[("day", "numeric"), ("month", "long")]) }}</div><div>{move || { tick.track(); day(&[("year", "numeric")]) }}</div></div>
                <div class="dw-right"><div class="dw-big">{move || { tick.track(); time_in(None, h12).unwrap_or_default() }}</div><div>{move || { tick.track(); day(&[("weekday", "long")]) }}</div></div>
            </div>
            <ul class="dw-zones">
                {zones.into_iter().map(|z| {
                    let tz = s(&z, "timezone");
                    let label = s(&z, "label");
                    let tz2 = tz.clone();
                    let diff = move || {
                        let (Some(there), Some(here)) = (time_in(Some(&tz2), false).and_then(|t| minutes_of(&t)), time_in(None, false).and_then(|t| minutes_of(&t))) else { return String::new() };
                        let mut d = there - here;
                        if d > 14 * 60 { d -= 1440 }
                        if d < -12 * 60 { d += 1440 }
                        match d { 0 => String::new(), d if d % 60 == 0 => format!("{:+}h", d / 60), d => format!("{}{}h{:02}", if d < 0 { "-" } else { "+" }, d.abs() / 60, d.abs() % 60) }
                    };
                    view! {
                        <li>
                            <span class="grow truncate">{if label.is_empty() { tz.clone() } else { label }}</span>
                            <span class="subdue">{move || { tick.track(); diff() }}</span>
                            <b>{move || { tick.track(); time_in(Some(&tz), h12).unwrap_or_else(|| "–".into()) }}</b>
                        </li>
                    }
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
    let d = fetch("/summary".into(), Some(60));
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
    let d = fetch("/calendar/widget".into(), Some(300));
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

/// The brief, as CERQ's brief.html was: Today's ASCII sky as a strip with the greeting and the
/// temperature, the headline typed out, what matters, and today as a ribbon with a now line.
/// A click on the sky opens Today.
#[component]
fn BriefW() -> impl IntoView {
    use crate::pages::today::{hero, Tx};
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
    let ok = Signal::derive(move || b.with(|r| r.as_ref().and_then(|r| r.as_ref().ok()).cloned()));
    let lang = RwSignal::new(String::new());
    Effect::new(move |_| {
        let l = ok.with(|v| v.as_ref().and_then(|v| v["lang"].as_str()).unwrap_or("").to_string());
        if lang.get_untracked() != l {
            lang.set(l);
        }
    });
    let c = Tx::of(lang);
    let pending = move || ok.with(|v| v.as_ref().is_some_and(|v| v["pending"].as_bool() == Some(true)));
    // The headline, typed out again only when it changes.
    let say = Memo::new(move |_| match b.with(|r| r.clone()) {
        Some(Err(e)) => format!("{} ({e}).", c.t("Could not load the brief")),
        None => c.t("Writing your brief…"),
        Some(Ok(v)) if v["pending"].as_bool() == Some(true) => c.t("Writing your brief…"),
        Some(Ok(v)) => s(&v, "text"),
    });
    let open_today = move |_| navigate("today", Where::Replace, &[]);
    view! {
        <div class="td bw">
            <div on:click=open_today>
                <hero::Hero b=ok c=c before=|| () after=|| ()/>
            </div>
            <main class="bw-main">
                {move || { let t = say.get(); (!t.is_empty()).then(|| view! { <div class="td-say"><hero::Typed text=t/></div> }) }}
                {move || {
                    let matters: Vec<Value> = ok.with(|v| v.as_ref().map(|v| arr(v, "matters")).unwrap_or_default()).into_iter().filter(|x| x["k"] != "now" && x["k"] != "next").take(3).collect();
                    (!matters.is_empty()).then(|| view! {
                        <ul class="td-mts">
                            {matters.into_iter().enumerate().map(|(i, x)| view! {
                                <li class=format!("mt-{} rise", s(&x, "k")) style=format!("--d:{}s", 0.2 + i as f64 * 0.07)>
                                    <span class="ic">{matter_icon(&s(&x, "k"))}</span>
                                    <span class="tx"><span>{s(&x, "text")}</span><small>{s(&x, "detail")}</small></span>
                                </li>
                            }).collect_view()}
                        </ul>
                    })
                }}
                {move || { tick.track(); ok.get().filter(|_| !pending()).map(|v| ribbon(&v)) }}
                {move || {
                    let by_model = ok.with(|v| v.as_ref().is_some_and(|v| v["by_model"].as_bool() == Some(true)));
                    let ready = ok.with(|v| v.is_some()) && !pending();
                    view! {
                        <div class=if by_model { "bw-foot" } else { "bw-foot off" }>
                            <span>{ready.then(|| view! { <b>"●"</b>" "{c.t(if by_model { "Headline by your model" } else { "From your calendar and mail" })}})}</span>
                            <a href="/today" on:click=move |e| { e.prevent_default(); navigate("today", Where::Replace, &[]) }>{c.t("Open Today")}" →"</a>
                        </div>
                    }
                }}
            </main>
        </div>
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
