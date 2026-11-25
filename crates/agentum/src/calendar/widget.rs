//! The dashboard's Calendar widget, Calendar's ✨ Summarize and quick add
//! (calwidget.py and Odysseus' quick-parse).
use super::{store, Occ};
use crate::{llm, state::App, today::i18n::{self as L, Lang}};
use chrono::{Datelike, Duration, NaiveDate, Timelike};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashMap, HashSet}, sync::{LazyLock, Mutex}};

/// Occurrences for a range, the same meeting on two calendars once, moved
/// holidays resolved, with their done marks.
async fn occ_done(app: &App, from: NaiveDate, to: NaiveDate) -> anyhow::Result<Vec<(Occ, bool)>> {
    let occ = store::one_holiday(super::occurrences(app, from, to).await?);
    let done = store::done_keys(app).await?;
    let mut seen = HashSet::new();
    Ok(occ.into_iter().filter(|o| o.kind == "holiday" || seen.insert((o.title.to_lowercase(), o.start, o.all_day)))
        .map(|o| { let d = done.contains(&store::done_key(&o.uid, o.start.date_naive())); (o, d) }).collect())
}

/// The days an occurrence shows on (all-day ends are exclusive), at most 40.
fn days_of(o: &Occ) -> Vec<NaiveDate> {
    let first = o.start.date_naive();
    let last = if o.end > o.start { (o.end - Duration::seconds(1)).date_naive() } else { first };
    let last = last.min(first + Duration::days(40)).max(first);
    (0..=(last - first).num_days()).map(|i| first + Duration::days(i)).collect()
}

fn hm(o: &chrono::DateTime<chrono_tz::Tz>) -> String { o.format("%H:%M").to_string() }

static WIDGET: LazyLock<Mutex<Option<(String, Value)>>> = LazyLock::new(Default::default);

pub async fn widget(app: &App) -> anyhow::Result<Value> {
    let tz = app.tz();
    let l = Lang::of(app);
    let now = chrono::Utc::now().with_timezone(&tz);
    let today = now.date_naive();
    let sunday_first = app.setting("CERQ_WEEK_START") == "Sunday";
    let key = format!("{}|{}|{}|{}", super::signature(app).await, now.format("%F %R"), l.code(), sunday_first);
    if let Some((k, v)) = WIDGET.lock().unwrap().as_ref() { if *k == key { return Ok(v.clone()); } }
    let first = today.with_day(1).unwrap();
    let off = if sunday_first { first.weekday().num_days_from_sunday() } else { first.weekday().num_days_from_monday() };
    let grid_lo = first - Duration::days(off as i64);
    let next_month = (first + Duration::days(32)).with_day(1).unwrap();
    let weeks = ((next_month - grid_lo).num_days() + 6) / 7;
    let grid_hi = grid_lo + Duration::days(7 * weeks);
    let occ = occ_done(app, grid_lo, grid_hi.max(today + Duration::days(15))).await?;
    let mut per: BTreeMap<NaiveDate, Vec<&(Occ, bool)>> = BTreeMap::new();
    for o in &occ { for d in days_of(&o.0) { per.entry(d).or_default().push(o); } }
    let order = |k: &str| match k { "birthday" => 0, "money" => 1, "work" => 2, "personal" => 3, _ => 4 };
    let days: Vec<Value> = (0..7 * weeks).map(|i| {
        let d = grid_lo + Duration::days(i);
        let evs = per.get(&d).cloned().unwrap_or_default();
        let hol: Vec<&str> = evs.iter().filter(|o| o.0.kind == "holiday").map(|o| o.0.title.as_str()).collect();
        let mut rest: Vec<&Occ> = evs.iter().map(|o| &o.0).filter(|o| o.kind != "holiday").collect();
        rest.sort_by_key(|o| (!o.all_day, o.start));
        let mut by_kind = rest.clone();
        by_kind.sort_by_key(|o| order(&o.kind));
        let dots: Vec<&str> = by_kind.iter().take(3).map(|o| o.kind.as_str()).collect();
        let tip: Vec<String> = hol.first().map(|h| h.to_string()).into_iter()
            .chain(rest.iter().take(6).map(|o| format!("{} {}", if o.all_day { L::t0(l, "all_day") } else { hm(&o.start) }, o.title))).collect();
        json!({"d": d.day(), "date": d.to_string(), "month": d.month() == today.month(), "today": d == today, "past": d < today,
               "n": rest.len(), "dots": dots, "holiday": !hol.is_empty(), "tip": tip.join(" · ")})
    }).collect();
    let when = |o: &Occ| -> String {
        let d = o.start.date_naive();
        let day = if d == today { L::t0(l, "Today") } else if d == today + Duration::days(1) { L::t0(l, "Tomorrow") }
            else if (d - today).num_days() < 7 { format!("{} {}", L::dow(l, d), d.day()) } else { L::short_date(l, d) };
        if o.all_day { format!("{day} · {}", L::t0(l, "all_day")) }
        else if o.start <= now && now < o.end { L::t(l, "Now_until", &[("t", &hm(&o.end))]) }
        else { format!("{day} · {}", hm(&o.start)) }
    };
    let next: Vec<Value> = occ.iter().filter(|(o, done)| o.kind != "holiday" && o.end > now && !done && !(o.all_day && o.start.date_naive() < today))
        .take(4).map(|(o, _)| json!({"when": when(o), "title": o.title, "kind": o.kind, "date": o.start.date_naive().to_string(), "now": !o.all_day && o.start <= now && now < o.end})).collect();
    let todays: Vec<&Occ> = per.get(&today).map(|v| v.iter().map(|o| &o.0).filter(|o| o.kind != "holiday").collect()).unwrap_or_default();
    let left = todays.iter().filter(|o| !o.all_day && o.start > now).count();
    let dow: Vec<&str> = if l == Lang::Es { L::DOW_MIN_ES.to_vec() } else { L::DOW_MIN_EN.to_vec() };
    let dow = if sunday_first { [&dow[6..], &dow[..6]].concat() } else { dow };
    let v = json!({"title": L::capitalize(&L::month_year(l, today)), "dow": dow, "days": days, "next": next, "today_left": left,
        "labels": {"today": L::t0(l, "w_today"), "left": L::t(l, "w_left", &[("n", &left.to_string())]), "none": L::t0(l, "w_none")},
        "today_n": todays.len()});
    *WIDGET.lock().unwrap() = Some((key, v.clone()));
    Ok(v)
}

/// The model's text without think blocks or markdown marks.
pub fn clean(s: &str, n: usize) -> String {
    static THINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<think>.*?(</think>|$)").unwrap());
    static MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_#`]+").unwrap());
    let t = THINK.replace_all(s, "");
    MARKS.replace_all(&t, "").trim().chars().take(n).collect()
}

static SUMS: LazyLock<Mutex<HashMap<String, Value>>> = LazyLock::new(Default::default);

/// A day, week or month (1–42 days) in two sentences by your model, plus the
/// facts it was given (busiest days, clashes, birthdays, free evenings).
pub async fn summary(app: &App, start: NaiveDate, end: NaiveDate) -> anyhow::Result<Value> {
    let n = (end - start).num_days();
    if !(1..=42).contains(&n) { anyhow::bail!("range must be 1 to 42 days"); }
    let kind = if n == 1 { "day" } else if n <= 7 { "week" } else { "month" };
    let tz = app.tz();
    let l = Lang::of(app);
    let now = chrono::Utc::now().with_timezone(&tz);
    let key = format!("{start}|{end}|{}|{}|{}|{}", super::signature(app).await, now.date_naive(), now.hour(), l.code());
    if let Some(v) = SUMS.lock().unwrap().get(&key) { return Ok(v.clone()); }
    // 5 days either side: a moved holiday's winning date may sit just outside the range.
    let occ = occ_done(app, start - Duration::days(5), end + Duration::days(5)).await?;
    let mut per: BTreeMap<NaiveDate, Vec<&(Occ, bool)>> = BTreeMap::new();
    for o in &occ { for d in days_of(&o.0) { if start <= d && d < end { per.entry(d).or_default().push(o); } } }
    let (mut rows, mut clashes, mut notable, mut free) = (vec![], vec![], vec![], vec![]);
    for i in 0..n {
        let d = start + Duration::days(i);
        let evs = per.get(&d).cloned().unwrap_or_default();
        let mut timed: Vec<&Occ> = evs.iter().map(|o| &o.0).filter(|o| !o.all_day && o.kind != "holiday").collect();
        timed.sort_by_key(|o| o.start);
        let meets: Vec<&&Occ> = timed.iter().filter(|o| (o.kind == "work" || o.kind == "personal") && !store::MONEY.is_match(&o.title)).collect();
        for w in meets.windows(2) {
            let (a, b) = (w[0], w[1]);
            if (a.end.min(b.end) - b.start).num_seconds() >= 900 && a.title.to_lowercase() != b.title.to_lowercase() {
                clashes.push(json!({"date": d.to_string(), "a": a.title, "b": b.title, "time": hm(&b.start)}));
            }
        }
        for (o, _) in evs.iter().map(|x| (&x.0, x.1)) {
            if matches!(o.kind.as_str(), "holiday" | "birthday" | "money") { notable.push(json!({"date": d.to_string(), "kind": o.kind, "title": o.title})); }
        }
        if d.weekday().num_days_from_monday() < 5 && !timed.iter().any(|o| o.end.hour() >= 18 || o.start.hour() >= 18) { free.push(d.to_string()); }
        let mut all: Vec<&(Occ, bool)> = evs.clone();
        all.sort_by_key(|x| (!x.0.all_day, x.0.start));
        rows.push(json!({"date": d.to_string(), "day": format!("{} {}", L::dow(l, d), d.day()), "count": timed.len(),
            "first": timed.first().map(|o| hm(&o.start)).unwrap_or_default(), "last": timed.last().map(|o| hm(&o.end)).unwrap_or_default(),
            "past": d < now.date_naive(),
            "events": all.iter().take(12).map(|(o, done)| format!("{}{} ({}{})", if o.all_day { String::new() } else { hm(&o.start) + " " }, o.title, o.kind, if *done { ", done" } else { "" })).collect::<Vec<_>>()}));
    }
    let total: i64 = rows.iter().map(|r| r["count"].as_i64().unwrap_or(0)).sum();
    let mut busy: Vec<&Value> = rows.iter().filter(|r| r["count"].as_i64().unwrap_or(0) > 0).collect();
    busy.sort_by_key(|r| -r["count"].as_i64().unwrap_or(0));
    let last = end - Duration::days(1);
    let label = if kind == "day" { L::long_date(l, start) } else if kind == "month" && start.day() == 1 { L::month_year(l, start) }
        else { L::t(l, "range_week", &[("a", &L::short_date(l, start)), ("b", &L::short_date(l, last))]) };
    let label = L::capitalize(&label);
    let mut facts = json!({"range": label, "today": format!("{} {}", L::short_date(l, now.date_naive()), now.format("%H:%M")), "timed_events": total,
        "days": if n <= 14 { json!(rows) } else { json!(rows.iter().map(|r| json!({"day": r["day"], "count": r["count"], "first": r["first"], "last": r["last"]})).collect::<Vec<_>>()) },
        "clashes": clashes.iter().take(6).collect::<Vec<_>>(), "birthdays_payments_holidays": notable.iter().take(12).collect::<Vec<_>>(), "weekdays_with_a_free_evening": free});
    if n > 14 { facts["notable_days"] = json!(rows.iter().filter(|r| r["count"].as_i64().unwrap_or(0) >= 5).take(6).collect::<Vec<_>>()); }
    let span = match kind { "day" => "this day", "week" => "this week", _ => "this month" };
    let has_model = llm::configured(app);
    let text = if has_model {
        let out = llm::complete(app, vec![
            json!({"role": "system", "content": format!("Write in {}: at most 2 short sentences and 30 words in all, plain text, speaking to the reader as \"you\", that sum up {span} of their calendar: the one thing that matters most (the busiest day, a clash, a birthday or payment not to miss), and the best free stretch. No lists, no counting every event. Skip past days and events marked done. Use only the facts given; never invent anything. Event titles may be in another language; keep them as they are.", l.name())}),
            json!({"role": "user", "content": facts.to_string()}),
        ], 90, llm::Prio::Fg).await;
        clean(&out.unwrap_or_default(), 700)
    } else { String::new() };
    let done_n = per.values().flatten().filter(|(o, d)| *d && o.kind != "holiday").count();
    let v = json!({"kind": kind, "start": start.to_string(), "end": end.to_string(), "label": label, "text": text, "done": done_n, "events": total,
        "busy": busy.iter().take(3).map(|r| json!({"date": r["date"], "day": r["day"], "count": r["count"]})).collect::<Vec<_>>(),
        "clashes": clashes.iter().take(6).collect::<Vec<_>>(), "notable": notable.iter().take(8).collect::<Vec<_>>(),
        "free_evenings": facts["weekdays_with_a_free_evening"].as_array().map(|a| a.iter().take(7).cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "by_model": !text.is_empty()});
    let mut sums = SUMS.lock().unwrap();
    if sums.len() > 50 { sums.clear(); }
    // No answer from your model: ask again next time, don't keep the empty one.
    if !text.is_empty() || !has_model { sums.insert(key, v.clone()); }
    Ok(v)
}

/// "lunch with Ana tomorrow 1pm" → {summary, dtstart, dtend, all_day, location} (local times).
pub async fn quick_parse(app: &App, text: &str) -> anyhow::Result<Value> {
    if !llm::configured(app) { anyhow::bail!("No model is set up. Pick one in Settings → Your model."); }
    let now = chrono::Utc::now().with_timezone(&app.tz()).naive_local();
    let days: Vec<String> = (0..8).map(|i| (now + Duration::days(i)).format("%A %Y-%m-%d").to_string()).collect();
    let system = format!("You are a calendar event parser. Read the user's one-line description and emit STRICT JSON describing the event. The current user-local timestamp is {}. Resolve relative dates (\"tomorrow\", \"friday\", \"next monday\", \"in 30 minutes\") against today. Default duration is 60 minutes when no end time is given. If the text mentions a date with no time, treat it as an all-day event.\n\nOutput ONLY this JSON shape, nothing else:\n{{\n  \"summary\": \"<event title, capitalized>\",\n  \"dtstart\": \"<YYYY-MM-DDTHH:MM:00>\",\n  \"dtend\":   \"<YYYY-MM-DDTHH:MM:00>\",\n  \"all_day\": <true|false>,\n  \"location\": \"<place or empty>\"\n}}\nFor all-day events use \"YYYY-MM-DD\" (no time) for both fields.\nToday is {}. The next days: {}. The text may be in Spanish: lunes=Monday, martes=Tuesday, miércoles=Wednesday, jueves=Thursday, viernes=Friday, sábado=Saturday, domingo=Sunday, hoy=today, mañana=tomorrow (or morning, after a day), pasado mañana=in two days, 17hs=17:00, mediodía=12:00, a la tarde=afternoon. The title is only what the event is, in the user's language, without the date or time words (\"gym el sábado a las 10\" → \"Gym\").",
        now.format("%Y-%m-%dT%H:%M:%S"), now.format("%A %Y-%m-%d"), days.join("; "));
    let out = llm::complete(app, vec![json!({"role": "system", "content": system}), json!({"role": "user", "content": text})], 300, llm::Prio::Fg).await
        .ok_or_else(|| anyhow::anyhow!("Your model didn't answer. Try again in a moment."))?;
    let p = llm::extract_json(&out).filter(|v| v.is_object()).ok_or_else(|| anyhow::anyhow!("Your model's answer wasn't an event."))?;
    // The model writes local wall time; drop any zone it adds anyway.
    static TZ_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(z|[+-]\d{2}:?\d{2})$").unwrap());
    let strip = |k: &str| -> String { let v = crate::util::s(&p, k); if v.len() > 10 { TZ_SUFFIX.replace(v.trim(), "").to_string() } else { v.trim().to_string() } };
    let (start, mut end) = (strip("dtstart"), strip("dtend"));
    if start.is_empty() { anyhow::bail!("Your model didn't find a start time."); }
    let all_day = p["all_day"].as_bool().unwrap_or(start.len() == 10);
    let start = if all_day { start.chars().take(10).collect() } else { start };
    if all_day {
        // An all-day end is exclusive: the day after the last one.
        let s = NaiveDate::parse_from_str(&start, "%Y-%m-%d")?;
        let e = NaiveDate::parse_from_str(end.get(..10).unwrap_or(""), "%Y-%m-%d").unwrap_or(s);
        end = (if e > s { e } else { s } + Duration::days(1)).to_string();
    } else if super::edit::parse_time(&end, chrono_tz::UTC).zip(super::edit::parse_time(&start, chrono_tz::UTC)).map(|(e, s)| e.0 <= s.0).unwrap_or(true) {
        let s = super::edit::parse_time(&start, chrono_tz::UTC).ok_or_else(|| anyhow::anyhow!("Your model's start time isn't a time."))?.0;
        end = (s + Duration::minutes(60)).format("%Y-%m-%dT%H:%M:00").to_string();
    }
    static NOISE: LazyLock<[Regex; 3]> = LazyLock::new(|| [Regex::new(r"(?i)\bin\s+\d+\s*(min|minute|hour|hr|day)s?\b").unwrap(),
        Regex::new(r"\(\s*\d{1,2}:\d{2}\s*\)").unwrap(), Regex::new(r"(?i)\b\d{1,2}(:\d{2})?\s*(am|pm)\b").unwrap()]);
    let mut summary = { let s = crate::util::s(&p, "summary"); if s.trim().is_empty() { text.to_string() } else { s } };
    for re in NOISE.iter() { summary = re.replace_all(&summary, "").to_string(); }
    let summary: String = summary.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(|c: char| " -—,@".contains(c)).chars().take(200).collect();
    Ok(json!({"summary": summary, "dtstart": start, "dtend": end, "all_day": all_day, "location": crate::util::s(&p, "location").trim().chars().take(200).collect::<String>()}))
}
