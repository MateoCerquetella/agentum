//! Calendar helpers. Dates are "YYYY-MM-DD" strings and times are minutes after midnight, read
//! straight from the hub's RFC 3339 wall clock (already in your time zone), so the browser's own
//! time zone never shifts an event.
use serde::Deserialize;
use wasm_bindgen::JsValue;

/// One occurrence from GET /api/calendar/events.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Occ {
    pub id: String,
    pub uid: String,
    pub calendar_id: String,
    pub calendar: String,
    pub title: String,
    pub location: String,
    pub description: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub recurring: bool,
    pub kind: String,
    pub color: String,
    pub read_only: bool,
    pub done: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Cal {
    pub id: String,
    pub name: String,
    pub account: String,
    pub color: String,
    pub source: String,
    pub read_only: bool,
    pub default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalView {
    Month,
    Week,
    Day,
    Agenda,
}
impl CalView {
    pub const ALL: [(CalView, &'static str, &'static str); 4] = [(CalView::Month, "month", "Month"), (CalView::Week, "week", "Week"), (CalView::Day, "day", "Day"), (CalView::Agenda, "agenda", "List")];
    pub fn parse(s: &str) -> Option<CalView> {
        Self::ALL.iter().find(|v| v.1 == s).map(|v| v.0)
    }
    pub fn id(self) -> &'static str {
        Self::ALL.iter().find(|v| v.0 == self).map(|v| v.1).unwrap_or("week")
    }
}

// ---- civil dates (no time zones involved)
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}
pub fn parts(s: &str) -> (i64, i64, i64) {
    let n = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    (n(0, 4).unwrap_or(1970), n(5, 7).unwrap_or(1), n(8, 10).unwrap_or(1))
}
pub fn ymd(y: i64, m: i64, d: i64) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}
fn serial(s: &str) -> i64 {
    let (y, m, d) = parts(s);
    days_from_civil(y, m, d)
}
pub fn add_days(s: &str, n: i64) -> String {
    let (y, m, d) = civil_from_days(serial(s) + n);
    ymd(y, m, d)
}
/// The first of the month `n` months from s's.
pub fn first_of_month(s: &str, n: i64) -> String {
    let (y, m, _) = parts(s);
    let k = y * 12 + (m - 1) + n;
    ymd(k.div_euclid(12), k.rem_euclid(12) + 1, 1)
}
/// 0 = Sunday.
pub fn weekday(s: &str) -> i64 {
    (serial(s) + 4).rem_euclid(7)
}
pub fn day_num(s: &str) -> i64 {
    parts(s).2
}
pub fn today_str() -> String {
    let d = js_sys::Date::new_0();
    ymd(d.get_full_year() as i64, d.get_month() as i64 + 1, d.get_date() as i64)
}
pub fn is_ymd(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}
pub fn day_of(iso: &str) -> String {
    iso.get(..10).unwrap_or(iso).to_string()
}
pub fn min_of(iso: &str) -> i32 {
    if iso.len() > 15 {
        let n = |a: usize, b: usize| iso.get(a..b).and_then(|x| x.parse::<i32>().ok()).unwrap_or(0);
        n(11, 13) * 60 + n(14, 16)
    } else {
        0
    }
}
pub fn hm(m: i32) -> String {
    format!("{:02}:{:02}", m.div_euclid(60).rem_euclid(24), m.rem_euclid(60))
}
pub fn mins(t: &str) -> i32 {
    let mut it = t.split(':').map(|x| x.trim().parse::<i32>().unwrap_or(0));
    it.next().unwrap_or(0) * 60 + it.next().unwrap_or(0)
}
pub fn now_min() -> i32 {
    let d = js_sys::Date::new_0();
    (d.get_hours() * 60 + d.get_minutes()) as i32
}
pub fn local_iso(day: &str, m: i32) -> String {
    format!("{day}T{}:00", hm(m.min(1439)))
}

pub fn kind_color(k: &str) -> &'static str {
    match k {
        "work" => "var(--primary)",
        "personal" => "var(--positive)",
        "money" => "var(--money)",
        "birthday" | "holiday" => "var(--negative)",
        _ => "var(--positive)",
    }
}
/// Calendar colours come from the servers: only plain hex is used in styles.
pub fn safe_color(c: &str) -> String {
    let ok = c.len() >= 4 && c.len() <= 9 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
    if ok { c.to_string() } else { "var(--subdue)".into() }
}

/// The days an occurrence covers. All-day ends are exclusive, and a timed event ending at 00:00
/// doesn't reach into that day.
pub fn days_of(o: &Occ) -> Vec<String> {
    let s = day_of(&o.start);
    let mut e = day_of(if o.end.is_empty() { &o.start } else { &o.end });
    if e > s && (o.all_day || min_of(&o.end) == 0) {
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
pub fn covers(o: &Occ, day: &str) -> bool {
    days_of(o).iter().any(|d| d == day)
}
/// A timed occurrence's [start, end) minutes on one of its days.
pub fn segment(o: &Occ, day: &str) -> (i32, i32) {
    let s = if day_of(&o.start).as_str() < day { 0 } else { min_of(&o.start) };
    let e = if day_of(&o.end).as_str() > day { 1440 } else { min_of(&o.end) };
    (s, e.max(s + 15))
}
/// All-day first, then the earliest, then by title.
pub fn by_time(a: &Occ, b: &Occ) -> std::cmp::Ordering {
    b.all_day.cmp(&a.all_day).then_with(|| a.start.cmp(&b.start)).then_with(|| a.title.cmp(&b.title))
}
/// The done mark of an occurrence: "<series uid>|<its local day>".
pub fn done_key(o: &Occ) -> String {
    format!("{}|{}", o.uid.split("::").next().unwrap_or(""), day_of(&o.start))
}
pub fn on_day(events: &[Occ], d: &str) -> Vec<Occ> {
    let mut v: Vec<Occ> = events.iter().filter(|o| covers(o, d)).cloned().collect();
    v.sort_by(by_time);
    v
}

pub fn week_start_of(s: &str, sunday_first: bool) -> String {
    let dow = weekday(s);
    add_days(s, -(if sunday_first { dow } else { (dow + 6) % 7 }))
}
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub start: String,
    pub end: String,
    pub days: Vec<String>,
}
/// What a view shows: [start, end) and its days.
pub fn range_for(view: CalView, date: &str, sunday_first: bool) -> Range {
    let (start, n) = match view {
        CalView::Week => (week_start_of(date, sunday_first), 7),
        CalView::Month => (week_start_of(&first_of_month(date, 0), sunday_first), 42),
        CalView::Agenda => (date.to_string(), 30),
        CalView::Day => (date.to_string(), 1),
    };
    let days = (0..n).map(|i| add_days(&start, i)).collect();
    Range { end: add_days(&start, n), start, days }
}

/// Side-by-side lanes for overlapping items: clusters of overlapping items share the width, each
/// item in the first free lane. Items are (s, e, T); out: (s, e, lane, lanes, T).
pub fn layout_lanes<T>(mut items: Vec<(i32, i32, T)>) -> Vec<(i32, i32, usize, usize, T)> {
    items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<(i32, i32, usize, usize, T)> = vec![];
    let mut group_start = 0;
    let mut end = -1;
    let mut lane_ends: Vec<i32> = vec![];
    let close = |out: &mut Vec<(i32, i32, usize, usize, T)>, from: usize| {
        let n = out[from..].iter().map(|x| x.2 + 1).max().unwrap_or(1).max(1);
        for x in &mut out[from..] {
            x.3 = n;
        }
    };
    for (s, e, t) in items {
        if s >= end {
            close(&mut out, group_start);
            group_start = out.len();
            lane_ends.clear();
        }
        let l = match lane_ends.iter().position(|x| *x <= s) {
            Some(l) => l,
            None => {
                lane_ends.push(0);
                lane_ends.len() - 1
            }
        };
        lane_ends[l] = e;
        out.push((s, e, l, 1, t));
        end = end.max(e);
    }
    close(&mut out, group_start);
    out
}

fn locale() -> String {
    web_sys::window().and_then(|w| w.navigator().language()).unwrap_or_else(|| "en-GB".into())
}
/// A day in the browser's language: `fmt_day(d, &[("weekday", "short")])`.
pub fn fmt_day_with(s: &str, opts: &[(&str, &str)], lang: Option<&str>) -> String {
    let (y, m, d) = parts(s);
    let date = js_sys::Date::new_with_year_month_day_hr_min_sec(y as u32, m as i32 - 1, d as i32, 12, 0, 0);
    let o = js_sys::Object::new();
    for (k, v) in opts {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), &JsValue::from_str(v));
    }
    let loc = lang.map(str::to_string).unwrap_or_else(locale);
    date.to_locale_date_string(&loc, &o).into()
}
pub fn fmt_day(s: &str, opts: &[(&str, &str)]) -> String {
    fmt_day_with(s, opts, None)
}
pub const SHORT: &[(&str, &str)] = &[("weekday", "short"), ("day", "numeric"), ("month", "short")];
pub fn fmt_short(s: &str) -> String {
    fmt_day(s, SHORT)
}
/// "Wed 18 Nov" in English (to-do toasts, like Today's).
pub fn day_name(iso: &str) -> String {
    fmt_day_with(&iso[..10.min(iso.len())], SHORT, Some("en-GB"))
}

/// "Wed 18 Nov · 09:00–10:00", "Wed 18 Nov · all day", across days "Wed 18 Nov 22:00 – Thu 19 Nov 01:00".
pub fn when_text(start: &str, end: &str, all_day: bool) -> String {
    let s = day_of(start);
    let e = day_of(if end.is_empty() { start } else { end });
    if all_day {
        let last = if e > s { add_days(&e, -1) } else { s.clone() };
        return if last == s { format!("{} · all day", fmt_short(&s)) } else { format!("{} – {} · all day", fmt_short(&s), fmt_short(&last)) };
    }
    let a = hm(min_of(start));
    let b = hm(min_of(if end.is_empty() { start } else { end }));
    if e == s || (e == add_days(&s, 1) && min_of(end) == 0) {
        return format!("{} · {a}–{}", fmt_short(&s), if e == s { b } else { "24:00".into() });
    }
    format!("{} {a} – {} {b}", fmt_short(&s), fmt_short(&e))
}
pub fn when_of(o: &Occ) -> String {
    when_text(&o.start, &o.end, o.all_day)
}

// ---- to-dos (notes with note_type "todo")
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub due: String,
    pub label: String,
    pub done: bool,
}
impl Todo {
    pub fn from(v: &serde_json::Value) -> Todo {
        let s = |k: &str| match &v[k] {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        Todo { id: s("id"), title: s("title"), due: s("due"), label: s("label"), done: v["done"].as_bool().unwrap_or(false) }
    }
    pub fn due(&self) -> Option<Due> {
        due_of(&self.due)
    }
}
/// "Reminder: …" notes are reminders for calendar events, not to-dos (as on the hub).
pub fn is_todo(n: &serde_json::Value) -> bool {
    n["note_type"].as_str() == Some("todo") && !n["archived"].as_bool().unwrap_or(false) && !n["title"].as_str().unwrap_or("").trim_start().to_lowercase().starts_with("reminder:")
}
/// Open to-dos, and the ones finished today (shown done until tomorrow).
pub fn keep(n: &serde_json::Value) -> bool {
    if !n["done"].as_bool().unwrap_or(false) {
        return true;
    }
    let u = &n["updated_at"];
    let d = match u {
        serde_json::Value::Number(x) => {
            let ms = x.as_f64().unwrap_or(0.0);
            js_sys::Date::new(&JsValue::from_f64(if ms < 1e12 { ms * 1000.0 } else { ms }))
        }
        serde_json::Value::String(s) if !s.is_empty() => js_sys::Date::new(&JsValue::from_str(s)),
        _ => return false,
    };
    !d.get_time().is_nan() && ymd(d.get_full_year() as i64, d.get_month() as i64 + 1, d.get_date() as i64) == today_str()
}
/// A to-do being dragged (onto a day, the calendar grid, the Inbox).
pub const DRAG: &str = "text/cerq-todo";

#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    pub day: String,
    pub min: Option<i32>,
}
/// A due is "YYYY-MM-DD" (a day) or a time: times Agentum writes carry the offset, and a time
/// without one is UTC, as the hub reads it.
pub fn due_of(due: &str) -> Option<Due> {
    let v = due.trim();
    if v.is_empty() {
        return None;
    }
    if is_ymd(v) {
        return Some(Due { day: v.into(), min: None });
    }
    let s = v.replacen(' ', "T", 1);
    let tail = s.len().saturating_sub(6);
    let has_zone = s.ends_with('Z') || s.ends_with('z') || s.get(tail..).is_some_and(|t| (t.starts_with('+') || t.starts_with('-')) && t[1..].chars().filter(|c| c.is_ascii_digit()).count() == 4) || s.get(s.len().saturating_sub(5)..).is_some_and(|t| (t.starts_with('+') || t.starts_with('-')) && t[1..].chars().all(|c| c.is_ascii_digit()));
    let iso = if has_zone { s.clone() } else { format!("{}Z", s.get(..19).unwrap_or(&s)) };
    let d = js_sys::Date::new(&JsValue::from_str(&iso));
    if d.get_time().is_nan() {
        return v.get(..10).filter(|x| is_ymd(x)).map(|x| Due { day: x.into(), min: None });
    }
    Some(Due { day: ymd(d.get_full_year() as i64, d.get_month() as i64 + 1, d.get_date() as i64), min: Some((d.get_hours() * 60 + d.get_minutes()) as i32) })
}
/// A due for the hub: the day, or the local time with its offset.
pub fn due_str(day: &str, min: Option<i32>) -> String {
    let Some(m) = min else { return day.to_string() };
    let (y, mo, d) = parts(day);
    let dt = js_sys::Date::new_with_year_month_day_hr_min_sec(y as u32, mo as i32 - 1, d as i32, m / 60, m % 60, 0);
    let o = -(dt.get_timezone_offset() as i32);
    let a = o.abs();
    format!("{day}T{}:00{}{:02}:{:02}", hm(m), if o >= 0 { '+' } else { '-' }, a / 60, a % 60)
}
