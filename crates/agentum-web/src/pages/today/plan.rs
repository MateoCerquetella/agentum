//! Today's planner logic: dates (local 'YYYY-MM-DD' strings, minutes after midnight), quick add
//! ("call the bank fri 10:00", "lunch with Ana tomorrow 13:00", also in Spanish) and to-do due
//! dates. A due is 'YYYY-MM-DD' (a day) or a time; times Agentum writes carry the offset, and a time
//! without one is UTC, as the hub reads it.
use crate::pages::mail::lib::fmt_date;
use agentum_ui::prelude::*;
use regex_lite::Regex;
use std::sync::OnceLock;
use wasm_bindgen::JsValue;

// ---------------------------------------------------------------- dates ----
pub fn ymd(d: &js_sys::Date) -> String {
    format!("{}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date())
}
/// Noon of a day: date maths at noon never trips over a DST change.
pub fn to_date(s: &str) -> js_sys::Date {
    let p = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<u32>().ok()).unwrap_or(1);
    js_sys::Date::new_with_year_month_day_hr_min_sec(p(0, 4), p(5, 7) as i32 - 1, p(8, 10) as i32, 12, 0, 0)
}
pub fn add_days(s: &str, n: i32) -> String {
    let d = to_date(s);
    d.set_date((d.get_date() as i32 + n) as u32);
    ymd(&d)
}
pub fn today_str() -> String {
    ymd(&js_sys::Date::new_0())
}
pub fn is_ymd(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}
pub fn day_of(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}
pub fn min_of(iso: &str) -> i32 {
    if iso.len() > 15 { iso[11..13].parse::<i32>().unwrap_or(0) * 60 + iso[14..16].parse::<i32>().unwrap_or(0) } else { 0 }
}
pub fn hm(m: i32) -> String {
    format!("{:02}:{:02}", (m / 60).rem_euclid(24), m.rem_euclid(60))
}
pub fn mins(t: &str) -> i32 {
    let mut it = t.split(':');
    let h = it.next().and_then(|x| x.trim().parse::<i32>().ok()).unwrap_or(0);
    let m = it.next().and_then(|x| x.trim().parse::<i32>().ok()).unwrap_or(0);
    h * 60 + m
}
pub fn now_min() -> i32 {
    let d = js_sys::Date::new_0();
    (d.get_hours() * 60 + d.get_minutes()) as i32
}
pub fn local_iso(day: &str, m: i32) -> String {
    format!("{day}T{}:00", hm(m.min(1439)))
}
pub fn locale(lang: &str) -> &'static str {
    if lang == "es" { "es-AR" } else { "en-GB" }
}
fn noon_ms(iso: &str) -> f64 {
    to_date(day_of(iso)).get_time()
}
/// "Wed, 18 Nov".
pub fn day_name(iso: &str, lang: &str) -> String {
    fmt_date(noon_ms(iso), Some(locale(lang)), &[("weekday", "short"), ("day", "numeric"), ("month", "short")])
}
pub fn fmt_day(s: &str) -> String {
    fmt_date(noon_ms(s), None, &[("weekday", "short"), ("day", "numeric"), ("month", "short")])
}
pub fn dur(m: i32) -> String {
    if m >= 60 { format!("{}h{}", m / 60, if m % 60 > 0 { format!("{:02}", m % 60) } else { String::new() }) } else { format!("{m} min") }
}
/// "Wed 18 Nov · 09:00–10:00", "Wed 18 Nov · all day".
pub fn when_text(start: &str, end: &str, all_day: bool) -> String {
    let (s, e) = (day_of(start).to_string(), day_of(if end.is_empty() { start } else { end }).to_string());
    if all_day {
        let last = if e > s { add_days(&e, -1) } else { s.clone() };
        return if last == s { format!("{} · all day", fmt_day(&s)) } else { format!("{} – {} · all day", fmt_day(&s), fmt_day(&last)) };
    }
    let (a, b) = (hm(min_of(start)), hm(min_of(if end.is_empty() { start } else { end })));
    if e == s || (e == add_days(&s, 1) && min_of(end) == 0) {
        return format!("{} · {a}–{}", fmt_day(&s), if e == s { b } else { "24:00".into() });
    }
    format!("{} {a} – {} {b}", fmt_day(&s), fmt_day(&e))
}
/// The days an occurrence covers. All-day ends are exclusive, and a timed event ending at 00:00
/// doesn't reach into that day.
pub fn days_of(o: &Value) -> Vec<String> {
    let start = o["start"].as_str().unwrap_or("");
    let end = o["end"].as_str().filter(|e| !e.is_empty()).unwrap_or(start);
    let s = day_of(start).to_string();
    let mut e = day_of(end).to_string();
    if e > s && (o["all_day"] == true || min_of(end) == 0) {
        e = add_days(&e, -1);
    }
    let mut out = vec![];
    let mut d = s.clone();
    while d <= e && out.len() < 366 {
        out.push(d.clone());
        d = add_days(&d, 1);
    }
    if out.is_empty() { vec![s] } else { out }
}
pub fn evening() -> bool {
    let h = js_sys::Date::new_0().get_hours();
    h >= 19 || h < 4
}

// ---------------------------------------------------------------- to-dos ----
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub due: String,
    pub label: String,
    pub done: bool,
}
impl Todo {
    pub fn of(v: &Value) -> Todo {
        let s = |k: &str| match &v[k] {
            Value::String(x) => x.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        Todo { id: s("id"), title: s("title"), due: s("due"), label: s("label"), done: v["done"] == true || v["done"] == 1 }
    }
}
/// "Reminder: …" notes are reminders for calendar events, not to-dos (as on the hub).
pub fn is_todo(n: &Value) -> bool {
    n["note_type"] == "todo" && n["archived"] != true && n["archived"] != 1 && !n["title"].as_str().unwrap_or("").trim_start().to_lowercase().starts_with("reminder:")
}
/// Open to-dos, and the ones finished today (shown done until tomorrow).
pub fn keep(n: &Value) -> bool {
    let done = n["done"] == true || n["done"] == 1;
    if !done {
        return true;
    }
    let u = &n["updated_at"];
    let ms = u.as_f64().map(|x| if x < 1e11 { x * 1000.0 } else { x }).or_else(|| u.as_str().map(|s| js_sys::Date::parse(s)));
    ms.is_some_and(|ms| !ms.is_nan() && ymd(&js_sys::Date::new(&JsValue::from_f64(ms))) == today_str())
}
/// A to-do being dragged (onto a day, the Inbox).
pub const DRAG: &str = "text/cerq-todo";

#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    pub day: String,
    pub min: Option<i32>,
}
pub fn due_of(due: &str) -> Option<Due> {
    let v = due.trim();
    if v.is_empty() {
        return None;
    }
    if is_ymd(v) {
        return Some(Due { day: v.into(), min: None });
    }
    let s = v.replacen(' ', "T", 1);
    let tz = s.ends_with('Z') || s.len() > 6 && { let t = &s[s.len().saturating_sub(6)..]; (t.starts_with('+') || t.starts_with('-')) && t[1..3].bytes().all(|b| b.is_ascii_digit()) } || s.len() > 5 && { let t = &s[s.len() - 5..]; (t.starts_with('+') || t.starts_with('-')) && t[1..].bytes().all(|b| b.is_ascii_digit()) };
    let src = if tz { s.clone() } else { format!("{}Z", s.get(..19).unwrap_or(&s)) };
    let ms = js_sys::Date::parse(&src);
    if ms.is_nan() {
        return (v.len() >= 10 && is_ymd(&v[..10])).then(|| Due { day: v[..10].into(), min: None });
    }
    let d = js_sys::Date::new(&JsValue::from_f64(ms));
    Some(Due { day: ymd(&d), min: Some((d.get_hours() * 60 + d.get_minutes()) as i32) })
}
/// A due for the hub: the day, or the local time with its offset.
pub fn due_str(day: &str, min: Option<i32>) -> String {
    let Some(m) = min else { return day.to_string() };
    let d = to_date(day);
    d.set_hours((m / 60) as u32);
    d.set_minutes((m % 60) as u32);
    let o = -(d.get_timezone_offset() as i32);
    let a = o.abs();
    format!("{day}T{}:00{}{:02}:{:02}", hm(m), if o >= 0 { '+' } else { '-' }, a / 60, a % 60)
}

// ------------------------------------------------------------- quick add ----
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub event: bool,
    pub title: String,
    pub date: Option<String>,
    pub start: Option<i32>,
    pub end: Option<i32>,
    pub label: Option<String>,
    pub guessed: bool,
}

fn wd(w: &str) -> u32 {
    match w {
        "sun" | "sunday" | "domingo" => 0,
        "mon" | "monday" | "lunes" => 1,
        "tue" | "tues" | "tuesday" | "martes" => 2,
        "wed" | "wednesday" | "miércoles" | "miercoles" => 3,
        "thu" | "thur" | "thurs" | "thursday" | "jueves" => 4,
        "fri" | "friday" | "viernes" => 5,
        _ => 6,
    }
}
fn mon(m: &str) -> i32 {
    match m.get(..3).unwrap_or(m) {
        "jan" | "ene" => 0,
        "feb" => 1,
        "mar" => 2,
        "apr" | "abr" => 3,
        "may" => 4,
        "jun" => 5,
        "jul" => 6,
        "aug" | "ago" => 7,
        "sep" => 8,
        "oct" => 9,
        "nov" => 10,
        _ => 11,
    }
}
fn hour(h: &str, m: Option<&str>, ap: Option<&str>) -> Option<i32> {
    let mut x: i32 = h.parse().ok()?;
    let y: i32 = m.and_then(|m| m.parse().ok()).unwrap_or(0);
    if ap == Some("pm") && x < 12 {
        x += 12;
    }
    if ap == Some("am") && x == 12 {
        x = 0;
    }
    (x < 24 && y < 60).then_some(x * 60 + y)
}
fn on_date(day: i32, mon: i32, today: &str) -> Option<String> {
    let t = to_date(today);
    let d = js_sys::Date::new_with_year_month_day_hr_min_sec(t.get_full_year(), mon, day, 12, 0, 0);
    if d.get_month() as i32 != mon {
        return None;
    }
    if ymd(&d).as_str() < today {
        return Some(ymd(&js_sys::Date::new_with_year_month_day_hr_min_sec(t.get_full_year() + 1, mon, day, 12, 0, 0)));
    }
    Some(ymd(&d))
}

struct Res {
    kw: Regex,
    rel: Regex,
    wday: Regex,
    inn: Regex,
    month: Regex,
    slash: Regex,
    len: Regex,
    range: Regex,
    at: Regex,
    noon: Regex,
    label: Regex,
    eventish: Regex,
    lead: Regex,
    trail: Regex,
}
fn res() -> &'static Res {
    static R: OnceLock<Res> = OnceLock::new();
    R.get_or_init(|| {
        let r = |s: &str| Regex::new(s).expect("quick-add regex");
        let mre = "(jan|ene|feb|mar|apr|abr|may|jun|jul|aug|ago|sept?|oct|nov|dec|dic)[a-zé]*";
        Res {
            kw: r(r"^\s*(?:(?:new|add|nuevo|nueva)\s+)?(event|evento|task|todo|to-do|tarea|pendiente)\b"),
            rel: r(r"\b(day after tomorrow|pasado mañana|today|tonight|esta noche|hoy|tomorrow|tmrw?|mañana)\b"),
            wday: r(r"\b(?:(?:next|this|el|este|el próximo|próximo|proximo)\s+)?(sunday|sun|monday|mon|tuesday|tues|tue|wednesday|wed|thursday|thurs|thur|thu|friday|fri|saturday|sat|domingo|lunes|martes|miércoles|miercoles|jueves|viernes|sábado|sabado)\b"),
            inn: r(r"\b(?:in|en)\s+(\d{1,2})\s+(days?|weeks?|días?|dias?|semanas?)\b"),
            month: r(&format!(r"\b(\d{{1,2}})\s+(?:de\s+)?{mre}\b|\b{mre}\s+(\d{{1,2}})(?:st|nd|rd|th)?\b")),
            slash: r(r"\b(\d{1,2})/(\d{1,2})\b"),
            len: r(r"\b(?:for|por)\s+(\d{1,3})\s*(m|min|mins|minutes?|minutos?|h|hrs?|hours?|horas?)\b|\b(\d{1,3})\s*(min|mins|minutes|minutos)\b"),
            range: r(r"(?:\b(at|from|de|a las|desde las)\s+|@\s*)?\b(\d{1,2})(?::(\d{2}))?\s*(am|pm|h)?\s*(?:-|–|\bto\b|\ba\b|\buntil\b|\bhasta\b)\s*(\d{1,2})(?::(\d{2}))?\s*(am|pm|h)?\b"),
            at: r(r"(?:\b(at|a las|alle)\s+|@\s*)?\b(\d{1,2})(?::(\d{2}))?(?:\s*(am|pm)\b|h\b)?"),
            noon: r(r"\b(noon|midday|mediodía|mediodia)\b"),
            label: r(r"(?:^|\s)#([\wáéíóúñ-]+)"),
            eventish: r(r"\b(with|con|meeting|meet|lunch|dinner|breakfast|coffee|call with|reunión|reunion|almuerzo|cena|desayuno|café|cafe|dentist|dentista|doctor|médico|medico|padel|gym|flight|vuelo|turno)\b"),
            lead: r(r"(?i)^(on|at|el|la|a las|for|para|the)\s+"),
            trail: r(r"(?i)\s+(on|at|el|a las|for|para|the|this|next|este|de)$"),
        }
    })
}

/// Read a quick-add line. Times are minutes after midnight; no date = none.
pub fn parse(text: &str, today: &str) -> Parsed {
    let low = text.to_lowercase();
    // Slicing the original by the lowercase text's offsets needs the same byte lengths.
    let src = if low.len() == text.len() { text.to_string() } else { low.clone() };
    let x = res();
    let mut cut: Vec<(usize, usize)> = vec![];
    let mut r = Parsed { event: false, title: String::new(), date: None, start: None, end: None, label: None, guessed: false };
    let mut night = false;
    let each = |re: &Regex, cut: &mut Vec<(usize, usize)>, f: &mut dyn FnMut(&regex_lite::Captures) -> bool| {
        for m in re.captures_iter(&low) {
            let all = m.get(0).unwrap();
            let t = all.as_str();
            if t.is_empty() {
                continue;
            }
            let s = all.start() + (t.len() - t.trim_start().len());
            let e = all.start() + t.trim_end().len();
            if cut.iter().any(|(a, b)| s < *b && e > *a) {
                continue;
            }
            if !f(&m) {
                continue;
            }
            cut.push((s, e));
        }
    };
    let mut kw = false;
    if let Some(m) = x.kw.captures(&low) {
        kw = true;
        r.event = m[1].starts_with("even");
        cut.push((0, m.get(0).unwrap().end()));
    }
    let g = |m: &regex_lite::Captures, i: usize| m.get(i).map(|x| x.as_str().to_string());
    each(&x.rel, &mut cut, &mut |m| {
        if r.date.is_some() {
            return false;
        }
        let w = &m[1];
        r.date = Some(if w.contains("after") || w.contains("pasado") { add_days(today, 2) } else if w.contains("tom") || w.contains("tmr") || w.contains("mañana") { add_days(today, 1) } else { today.to_string() });
        if w.contains("night") || w.contains("noche") {
            night = true;
        }
        true
    });
    each(&x.wday, &mut cut, &mut |m| {
        if r.date.is_some() {
            return false;
        }
        let diff = ((wd(&m[1]) as i32 - to_date(today).get_day() as i32 + 7) % 7).max(0);
        r.date = Some(add_days(today, if diff == 0 { 7 } else { diff }));
        true
    });
    each(&x.inn, &mut cut, &mut |m| {
        if r.date.is_some() {
            return false;
        }
        let n: i32 = m[1].parse().unwrap_or(0);
        r.date = Some(add_days(today, n * if m[2].starts_with('w') || m[2].starts_with('s') { 7 } else { 1 }));
        true
    });
    each(&x.month, &mut cut, &mut |m| {
        if r.date.is_some() {
            return false;
        }
        let day: i32 = g(m, 1).or_else(|| g(m, 4)).and_then(|d| d.parse().ok()).unwrap_or(0);
        let mo = mon(&g(m, 2).or_else(|| g(m, 3)).unwrap_or_default());
        match on_date(day, mo, today) {
            Some(d) => {
                r.date = Some(d);
                true
            }
            None => false,
        }
    });
    each(&x.slash, &mut cut, &mut |m| {
        if r.date.is_some() {
            return false;
        }
        match on_date(m[1].parse().unwrap_or(0), m[2].parse::<i32>().unwrap_or(0) - 1, today) {
            Some(d) => {
                r.date = Some(d);
                true
            }
            None => false,
        }
    });
    let mut len: Option<i32> = None;
    each(&x.len, &mut cut, &mut |m| {
        let n: i32 = g(m, 1).or_else(|| g(m, 3)).and_then(|d| d.parse().ok()).unwrap_or(0);
        len = Some(if g(m, 2).or_else(|| g(m, 4)).unwrap_or_default().starts_with('h') { n * 60 } else { n });
        true
    });
    each(&x.range, &mut cut, &mut |m| {
        let has = |i: usize| m.get(i).is_some();
        if r.start.is_some() || !(has(1) || has(3) || has(4) || has(6) || has(7) || m[0].contains('@')) {
            return false;
        }
        let mut ap = g(m, 4);
        if ap.is_none() && g(m, 7).as_deref() == Some("pm") && m[2].parse::<i32>().unwrap_or(0) < 12 {
            ap = Some("pm".into());
        }
        let (Some(s), Some(e)) = (hour(&m[2], m.get(3).map(|x| x.as_str()), ap.as_deref()), hour(&m[5], m.get(6).map(|x| x.as_str()), m.get(7).map(|x| x.as_str()))) else { return false };
        if e <= s {
            return false;
        }
        r.start = Some(s);
        r.end = Some(e);
        true
    });
    each(&x.at, &mut cut, &mut |m| {
        let t = &m[0];
        if r.start.is_some() || !(m.get(1).is_some() || m.get(3).is_some() || m.get(4).is_some() || t.ends_with('h') || t.contains('@')) {
            return false;
        }
        let Some(mut s) = hour(&m[2], m.get(3).map(|x| x.as_str()), m.get(4).map(|x| x.as_str())) else { return false };
        if night && s < 720 {
            s += 720;
        }
        r.start = Some(s);
        true
    });
    each(&x.noon, &mut cut, &mut |_| {
        if r.start.is_some() {
            return false;
        }
        r.start = Some(720);
        true
    });
    each(&x.label, &mut cut, &mut |m| {
        if r.label.is_some() {
            return false;
        }
        r.label = Some(m[1].to_string());
        true
    });

    cut.sort();
    let (mut t, mut i) = (String::new(), 0usize);
    for (s, e) in &cut {
        if *s > i {
            t += &src[i..*s];
        }
        t.push(' ');
        i = i.max(*e);
    }
    t += src.get(i..).unwrap_or("");
    let mut t = t.split_whitespace().filter(|w| *w != "@" && *w != "#").collect::<Vec<_>>().join(" ");
    for _ in 0..3 {
        t = x.trail.replace(x.lead.replace(&t, "").as_ref(), "").trim().to_string();
    }
    let mut ch = t.chars();
    r.title = ch.next().map(|f| f.to_uppercase().collect::<String>() + ch.as_str()).unwrap_or_default();
    if !kw && r.start.is_some() && x.eventish.is_match(&low) {
        r.event = true;
        r.guessed = true;
    }
    if night && r.start.is_none() && r.event {
        r.start = Some(1200);
    }
    if let (Some(s), None) = (r.start, r.end) {
        r.end = Some((s + len.unwrap_or(if r.event { 60 } else { 30 })).min(1439));
    }
    r
}

/// To-do writes, shared by the list, the strip and the Inbox.
#[derive(Clone, Copy)]
pub struct TodoActs {
    pub todos: RwSignal<Vec<Todo>>,
    pub lang: Signal<String>,
}
impl TodoActs {
    fn put(self, t: &Todo, body: Value, said: Option<String>) {
        let id = t.id.clone();
        let todos = self.todos;
        spawn_local(async move {
            match api_put::<Value>(&format!("/todos/{}", js_sys::encode_uri_component(&id)), &body).await {
                Ok(x) => {
                    let n = Todo::of(&x);
                    todos.update(|ts| {
                        for y in ts.iter_mut().filter(|y| y.id == id) {
                            if x["title"].is_string() {
                                *y = n.clone();
                            } else {
                                if let Some(d) = body["done"].as_bool() {
                                    y.done = d;
                                }
                                if let Some(d) = body["due"].as_str() {
                                    y.due = d.into();
                                }
                            }
                        }
                    });
                    if let Some(s) = said {
                        toast(s);
                    }
                }
                Err(e) => toast_bad(e.message),
            }
        });
    }
    pub fn done(self, t: &Todo) {
        self.put(t, json!({"done": !t.done}), None)
    }
    /// A new day keeps the to-do's time unless one is given; '' takes the date off (back to the Inbox).
    pub fn due(self, t: &Todo, day: &str, min: Option<Option<i32>>) {
        let m = min.unwrap_or_else(|| due_of(&t.due).and_then(|d| d.min));
        let lang = self.lang.get_untracked();
        let said = if day.is_empty() { format!("{} → {}", t.title, super::i18n::tr(&lang, "Inbox")) } else { format!("{} → {}{}", t.title, day_name(day, &lang), min.flatten().map(|m| format!(" {}", hm(m))).unwrap_or_default()) };
        self.put(t, json!({"due": if day.is_empty() { String::new() } else { due_str(day, m) }}), Some(said))
    }
    pub async fn add(self, title: String, due: String, label: Option<String>) -> bool {
        let lang = self.lang.get_untracked();
        match api_post::<Value>("/todos", &json!({"title": title, "due": due, "label": label.unwrap_or_default()})).await {
            Ok(x) => {
                self.todos.update(|ts| ts.push(Todo::of(&x)));
                let tr = |s: &str| super::i18n::tr(&lang, s);
                toast(match due_of(&due) {
                    Some(d) => format!("{} {}{}", tr("To-do for"), day_name(&d.day, &lang), d.min.map(|m| format!(" {}", hm(m))).unwrap_or_default()),
                    None => tr("To-do in your Inbox"),
                });
                true
            }
            Err(e) => {
                toast_bad(e.message);
                false
            }
        }
    }
}
