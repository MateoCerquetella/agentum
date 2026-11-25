//! Generative UI (crates/agentum/src/genui.rs): a small JSON component tree that the hub and your
//! model write and this draws. One node: `{t: type, ...}`. Text only ever becomes text (no HTML);
//! links are http(s) or in-app. The Secretary draws a ```ui block in an answer with it.
use agentum_ui::nav::Where;
use agentum_ui::prelude::*;

/// The node types drawn here (a ```json block of one of these is drawn too).
pub const UI_TYPES: [&str; 21] = ["card", "row", "stack", "text", "heading", "callout", "bullets", "metric", "badge", "keyvalue", "table", "bars", "sparkline", "progress", "timeline", "steps", "people", "refs", "button", "divider", "details"];

/// http(s), or a path in the app.
pub fn safe(u: &str) -> bool {
    u.starts_with("https://") || u.starts_with("http://") || (u.starts_with('/') && !u.starts_with("//"))
}

/// Open a link: the web in a new tab; the app's own (/calendar?date=…, /mail?msg=…) beside what's open.
pub fn open_url(url: &str) {
    if url.is_empty() {
        return;
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        if let Some(w) = web_sys::window() {
            let _ = w.open_with_url_and_target_and_features(url, "_blank", "noopener,noreferrer");
        }
        return;
    }
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let page = path.trim_start_matches('/').split('/').next().filter(|p| !p.is_empty()).unwrap_or("today").to_string();
    let mut params: Vec<(String, String)> = vec![];
    if let Ok(q) = web_sys::UrlSearchParams::new_with_str(query) {
        for e in js_sys::try_iter(&q.entries()).ok().flatten().into_iter().flatten().flatten() {
            let a = js_sys::Array::from(&e);
            if let (Some(k), Some(v)) = (a.get(0).as_string(), a.get(1).as_string()) {
                params.push((k, v));
            }
        }
    }
    let refs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    navigate(&page, Where::Right, &refs);
}

fn str_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// **bold**, *italic*, `code` and [links](…), as views.
pub fn inline(s: &str) -> AnyView {
    let mut out: Vec<AnyView> = vec![];
    let mut text = String::new();
    let mut rest = s;
    let flush = |text: &mut String, out: &mut Vec<AnyView>| {
        if !text.is_empty() {
            out.push(std::mem::take(text).into_any());
        }
    };
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("**") {
            if let Some(end) = r.find("**").filter(|e| *e > 0) {
                flush(&mut text, &mut out);
                out.push(view! { <b>{r[..end].to_string()}</b> }.into_any());
                rest = &r[end + 2..];
                continue;
            }
        }
        if c == '`' {
            if let Some(end) = rest[1..].find('`').filter(|e| *e > 0) {
                flush(&mut text, &mut out);
                out.push(view! { <code>{rest[1..1 + end].to_string()}</code> }.into_any());
                rest = &rest[end + 2..];
                continue;
            }
        }
        if c == '*' && rest[1..].chars().next().is_some_and(|n| n != '*' && !n.is_whitespace()) {
            if let Some(end) = rest[1..].find('*') {
                flush(&mut text, &mut out);
                out.push(view! { <em>{rest[1..1 + end].to_string()}</em> }.into_any());
                rest = &rest[end + 2..];
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = rest.find("](") {
                let label = &rest[1..close];
                let after = &rest[close + 2..];
                if let Some(end) = after.find(')').filter(|e| !after[..*e].contains(char::is_whitespace)) {
                    let url = after[..end].to_string();
                    if !label.contains(']') {
                        flush(&mut text, &mut out);
                        let label = label.to_string();
                        out.push(if safe(&url) { view! { <a on:click=move |_| open_url(&url)>{label}</a> }.into_any() } else { label.into_any() });
                        rest = &after[end + 1..];
                        continue;
                    }
                }
            }
        }
        text.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut text, &mut out);
    out.into_iter().collect_view().into_any()
}

fn source_icon(s: &str) -> &'static str {
    match s {
        "github" => "github",
        "bb" | "code" => "bot",
        "calendar" => "calendar",
        "mail" => "mail",
        "todo" => "check",
        "note" => "notes",
        _ => "link",
    }
}

fn ref_chip(r: &Value) -> AnyView {
    let title = str_of(if r["title"].is_null() { &r["text"] } else { &r["title"] });
    let url = r["url"].as_str().unwrap_or("").to_string();
    let ok = safe(&url);
    view! {
        <button type="button" class="su-ref" title=title.clone() disabled=!ok on:click=move |_| open_url(&url)>
            <Icon name=source_icon(r["source"].as_str().unwrap_or("")) size=11/><span class="truncate">{title.clone()}</span>
        </button>
    }
    .into_any()
}

/// Each source shows once per list: a chip repeated under every bullet is noise.
fn first_refs(items: &[Value], seen: &mut Vec<String>) -> Vec<Value> {
    items
        .iter()
        .map(|it| {
            let mut it = it.clone();
            let refs: Vec<Value> = it["refs"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| {
                let k = str_of(if r["url"].is_null() { &r["title"] } else { &r["url"] });
                if seen.contains(&k) {
                    false
                } else {
                    seen.push(k);
                    true
                }
            }).collect();
            if it.is_object() {
                it["refs"] = json!(refs);
                if let Some(sub) = it["items"].as_array().cloned() {
                    it["items"] = json!(first_refs(&sub, seen));
                }
            }
            it
        })
        .collect()
}

fn text_of(it: &Value) -> String {
    str_of(if it["text"].is_null() { &it["title"] } else { &it["text"] })
}
fn title_of(it: &Value) -> String {
    str_of(if it["title"].is_null() { &it["text"] } else { &it["title"] })
}

fn bullets(items: &[Value], depth: usize) -> AnyView {
    let xs = if depth == 0 { first_refs(items, &mut vec![]) } else { items.to_vec() };
    view! {
        <ul class={ if depth > 0 { "gu-bul sub" } else { "gu-bul" } }>
            {xs.into_iter().map(|it| {
                let cls = format!("{}{}", if it["kind"] == "group" { "grp " } else { "" }, it["tone"].as_str().map(|t| format!("tone-{t}")).unwrap_or_default());
                let url = it["url"].as_str().unwrap_or("").to_string();
                let tx = text_of(&it);
                let refs = it["refs"].as_array().cloned().unwrap_or_default();
                let sub = it["items"].as_array().cloned().unwrap_or_default();
                view! {
                    <li class=cls>
                        <span class="tx">{if safe(&url) { view! { <a on:click=move |_| open_url(&url)>{inline(&tx)}</a> }.into_any() } else { inline(&tx) }}</span>
                        {(!refs.is_empty()).then(|| view! { <span class="su-refs">{refs.iter().map(ref_chip).collect_view()}</span> })}
                        {(!sub.is_empty()).then(|| bullets(&sub, depth + 1))}
                    </li>
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

fn act(a: &Value) {
    if let Some(u) = a["open"].as_str().filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
        open_url(u);
    } else if let Some(g) = a["go"].as_str().filter(|g| safe(g)) {
        open_url(g);
    } else if let Some(q) = a.get("ask").filter(|q| !q.is_null()) {
        navigate("chat", Where::Right, &[("q", &str_of(q))]);
    }
}

fn kids(n: &Value) -> AnyView {
    n["children"].as_array().cloned().unwrap_or_default().iter().map(node).collect_view().into_any()
}

fn num(v: &Value) -> f64 {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(0.0)
}
fn short(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

/// Draw one node (nothing for an unknown one).
pub fn node(n: &Value) -> AnyView {
    if !n.is_object() {
        return ().into_any();
    }
    let tone = n["tone"].as_str().map(|t| format!(" tone-{t}")).unwrap_or_default();
    let s = |k: &str| str_of(&n[k]);
    match n["t"].as_str().unwrap_or("") {
        "card" => view! {
            <section class="gu-card">
                {(!n["title"].is_null() || !n["sub"].is_null()).then(|| view! {
                    <header>
                        {n["icon"].as_str().map(|i| view! { <Icon name=i.to_string() size=15/> })}
                        <span class="hi">{s("title")}</span>
                        {(!n["sub"].is_null()).then(|| view! { <span class="subdue small">{s("sub")}</span> })}
                    </header>
                })}
                {kids(n)}
            </section>
        }.into_any(),
        "stack" => view! { <div class="gu-stack">{kids(n)}</div> }.into_any(),
        "row" => view! { <div class="gu-row">{kids(n)}</div> }.into_any(),
        "heading" => view! { <div class="gu-h">{s("text")}</div> }.into_any(),
        "text" => view! { <p class=format!("gu-text{tone}")>{inline(&s("text"))}</p> }.into_any(),
        "callout" => view! {
            <div class=format!("gu-callout{}", if tone.is_empty() { " tone-info".to_string() } else { tone.clone() })>
                {(!n["title"].is_null()).then(|| view! { <b>{s("title")}</b> })}
                <span>{inline(&s("text"))}</span>
            </div>
        }.into_any(),
        "bullets" => match n["items"].as_array().filter(|a| !a.is_empty()) {
            Some(items) => bullets(items, 0),
            None => ().into_any(),
        },
        "metric" => view! {
            <div class=format!("gu-metric{tone}")>
                <small>{s("label")}</small><b>{s("value")}</b>
                {(!n["note"].is_null()).then(|| view! { <small>{s("note")}</small> })}
            </div>
        }.into_any(),
        "badge" => view! { <span class=format!("gu-badge{tone}")>{s("text")}</span> }.into_any(),
        "divider" => view! { <hr class="gu-hr"/> }.into_any(),
        "details" => {
            let open = RwSignal::new(n["open"].as_bool().unwrap_or(false));
            let body = n.clone();
            view! {
                <div class="gu-details">
                    <button type="button" class="gu-sum" aria-expanded=move || open.get().to_string() on:click=move |_| open.update(|o| *o = !*o)>
                        <Icon name=Signal::derive(move || if open.get() { "chevronD".to_string() } else { "chevron-right".to_string() }) size=12/>{s("title")}
                    </button>
                    {move || open.get().then(|| kids(&body))}
                </div>
            }.into_any()
        }
        "keyvalue" => view! {
            <dl class="gu-kv">
                {n["pairs"].as_array().cloned().unwrap_or_default().into_iter().map(|p| view! { <dt>{str_of(&p[0])}</dt><dd>{inline(&str_of(&p[1]))}</dd> }).collect_view()}
            </dl>
        }.into_any(),
        "table" => {
            let cols: Vec<String> = n["columns"].as_array().into_iter().flatten().map(str_of).collect();
            view! {
                <div class="gu-table"><table>
                    {(!cols.is_empty()).then(|| view! { <thead><tr>{cols.into_iter().map(|c| view! { <th>{c}</th> }).collect_view()}</tr></thead> })}
                    <tbody>{n["rows"].as_array().cloned().unwrap_or_default().into_iter().map(|r| view! {
                        <tr>{r.as_array().cloned().unwrap_or_default().into_iter().map(|c| view! { <td>{inline(&str_of(&c))}</td> }).collect_view()}</tr>
                    }).collect_view()}</tbody>
                </table></div>
            }.into_any()
        }
        "bars" => {
            let vs: Vec<f64> = n["values"].as_array().into_iter().flatten().map(num).collect();
            let max = vs.iter().copied().fold(1.0_f64, f64::max);
            let unit = n["unit"].as_str().map(|u| format!(" {u}")).unwrap_or_default();
            view! {
                <div class="gu-bars">{vs.into_iter().enumerate().map(|(i, v)| {
                    let label = str_of(&n["labels"][i]);
                    view! {
                        <div title=format!("{label}: {}{unit}", short(v))>
                            <div class="bar"><i style=format!("height:{}%", v / max * 100.0)/></div>
                            <b>{if v != 0.0 { short(v) } else { String::new() }}</b><small>{label.clone()}</small>
                        </div>
                    }
                }).collect_view()}</div>
            }.into_any()
        }
        "sparkline" => {
            let vs: Vec<f64> = n["values"].as_array().into_iter().flatten().map(num).collect();
            if vs.len() < 2 {
                return ().into_any();
            }
            let (max, min) = (vs.iter().copied().fold(f64::MIN, f64::max), vs.iter().copied().fold(f64::MAX, f64::min));
            let sp = if max - min == 0.0 { 1.0 } else { max - min };
            let pts = vs.iter().enumerate().map(|(i, v)| format!("{:.2},{:.2}", i as f64 / (vs.len() - 1) as f64 * 100.0, 22.0 - (v - min) / sp * 20.0)).collect::<Vec<_>>().join(" ");
            view! { <svg class="gu-spark" viewBox="0 0 100 24" preserveAspectRatio="none"><polyline fill="none" stroke="currentColor" stroke-width="1.5" points=pts/></svg> }.into_any()
        }
        "progress" => {
            let max = Some(num(&n["max"])).filter(|m| *m > 0.0).unwrap_or(100.0);
            let v = num(&n["value"]).clamp(0.0, max);
            view! {
                <div class="gu-prog">
                    {(!n["label"].is_null()).then(|| view! { <small>{s("label")}</small> })}
                    <div><i style=format!("width:{}%", v / max * 100.0)/></div>
                </div>
            }.into_any()
        }
        "timeline" => view! {
            <div class="gu-tl">{n["items"].as_array().cloned().unwrap_or_default().into_iter().map(|it| {
                let url = it["url"].as_str().unwrap_or("").to_string();
                let t = title_of(&it);
                view! {
                    <div class=format!("gu-tl-i{}", it["tone"].as_str().map(|t| format!(" tone-{t}")).unwrap_or_default())>
                        <span class="t">{str_of(&it["time"])}</span><i/>
                        <span class="ti">
                            {if safe(&url) { view! { <a on:click=move |_| open_url(&url)>{t.clone()}</a> }.into_any() } else { t.clone().into_any() }}
                            {(!it["detail"].is_null()).then(|| view! { <small>{str_of(&it["detail"])}</small> })}
                        </span>
                    </div>
                }
            }).collect_view()}</div>
        }.into_any(),
        "steps" => view! {
            <ol class="gu-steps">{n["items"].as_array().cloned().unwrap_or_default().into_iter().enumerate().map(|(i, it)| {
                let st = it["status"].as_str().unwrap_or("todo").to_string();
                let dot = match st.as_str() { "done" => "✓".to_string(), "blocked" => "!".to_string(), _ => (i + 1).to_string() };
                view! {
                    <li class=format!("st-{st}")>
                        <span class="dot">{dot}</span>
                        <span class="ti">{title_of(&it)}{(!it["detail"].is_null()).then(|| view! { <small>{str_of(&it["detail"])}</small> })}</span>
                    </li>
                }
            }).collect_view()}</ol>
        }.into_any(),
        "people" => view! {
            <div class="gu-people">{n["items"].as_array().cloned().unwrap_or_default().into_iter().map(|p| {
                let name = title_of(&p);
                let initial = name.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                view! { <span class="gu-person" title=str_of(&p["detail"])><i>{initial}</i>{name.clone()}</span> }
            }).collect_view()}</div>
        }.into_any(),
        "refs" => view! { <div class="su-refs">{n["items"].as_array().cloned().unwrap_or_default().iter().map(ref_chip).collect_view()}</div> }.into_any(),
        "button" => {
            let a = n["action"].clone();
            view! {
                <button type="button" class="btn small" on:click=move |_| act(&a)>
                    {n["icon"].as_str().map(|i| view! { <Icon name=i.to_string() size=12/> })}{s("label")}
                </button>
            }.into_any()
        }
        _ => ().into_any(),
    }
}

/// `{type: …}` works as well as `{t: …}`, at any depth (models write either).
fn norm(v: Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.into_iter().map(norm).collect()),
        Value::Object(o) => {
            let has_t = o.contains_key("t");
            Value::Object(o.into_iter().map(|(k, x)| (if k == "type" && !has_t { "t".to_string() } else { k }, norm(x))).collect())
        }
        v => v,
    }
}

/// Top-level JSON values written one after another (`{…}\n{…}`), as small models do.
fn several(text: &str) -> Option<Value> {
    let (mut out, mut depth, mut start, mut in_str, mut esc) = (vec![], 0i32, None, false, false);
    for (i, c) in text.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start.take() {
                        out.push(serde_json::from_str::<Value>(&text[s..=i]).ok()?);
                    }
                }
            }
            _ => {}
        }
    }
    (!out.is_empty()).then_some(Value::Array(out))
}

fn is_node(x: &Value) -> bool {
    x.is_object() && x["t"].is_string()
}

/// A ```ui block's text: one node, or several (metrics side by side become a row). None when it
/// isn't UI.
pub fn parse_ui(text: &str) -> Option<Value> {
    let v = serde_json::from_str::<Value>(text).ok().or_else(|| several(text))?;
    let v = norm(v);
    if let Value::Array(a) = v {
        let nodes: Vec<Value> = a.into_iter().flat_map(|x| if let Value::Array(i) = x { i } else { vec![x] }).filter(is_node).collect();
        return match nodes.len() {
            0 => None,
            1 => nodes.into_iter().next(),
            _ if nodes.iter().all(|x| x["t"] == "metric") => Some(json!({"t": "row", "children": nodes})),
            _ => {
                // Runs of metrics inside a list become rows.
                let mut kids: Vec<Value> = vec![];
                for x in nodes {
                    let metric = x["t"] == "metric";
                    match kids.last_mut() {
                        Some(last) if metric && last["t"] == "row" && last["children"].as_array().is_some_and(|c| c.iter().all(|c| c["t"] == "metric")) => {
                            if let Some(c) = last["children"].as_array_mut() {
                                c.push(x)
                            }
                        }
                        _ => kids.push(if metric { json!({"t": "row", "children": [x]}) } else { x }),
                    }
                }
                Some(json!({"t": "stack", "children": kids}))
            }
        };
    }
    is_node(&v).then_some(v)
}
