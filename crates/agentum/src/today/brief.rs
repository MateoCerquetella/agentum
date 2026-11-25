//! The morning brief (brief.py): weather, today's calendar, the shape of the
//! day, what's coming up, what matters, and one headline from your model.
//! Built at CERQ_BRIEF_AT with a push, rebuilt in the background when it's
//! read and stale. The last one is kept in kv "today.brief".
use super::{i18n::{self as L, Lang}, now, hm, hm_env, m, num, dur};
use crate::{calendar, llm, state::App};
use chrono::{Datelike, Duration, NaiveDate, Timelike};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::{atomic::{AtomicBool, Ordering}, LazyLock}};
use tokio::sync::Mutex;

pub fn day_start(app: &App) -> i64 { hm_env(app, "CERQ_DAY_START", "08:00") }
pub fn day_end(app: &App) -> i64 { hm_env(app, "CERQ_DAY_END", "22:00").max(day_start(app) + 60) }
fn fahrenheit(app: &App) -> bool { app.setting("CERQ_UNITS") == "Fahrenheit" }

pub fn part(hour: u32) -> &'static str { if (4..12).contains(&hour) { "morning" } else if (12..19).contains(&hour) { "afternoon" } else { "evening" } }

// ---- weather (Open-Meteo)
pub fn place(app: &App) -> Option<(f64, f64)> { Some((num(app, "CERQ_BRIEF_LAT")?, num(app, "CERQ_BRIEF_LON")?)) }

async fn forecast(app: &App, days: u32, fields: &str) -> Option<Value> {
    let (lat, lon) = place(app)?;
    let url = format!("https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&timezone={}&forecast_days={days}&temperature_unit={}{fields}",
        app.tz().name(), if fahrenheit(app) { "fahrenheit" } else { "celsius" });
    let a = app.clone();
    app.cache.get_or(&format!("today:wx:{url}"), std::time::Duration::from_secs(600), || async move {
        let r = a.http.get(&url).header("User-Agent", "cerq/1.0").timeout(std::time::Duration::from_secs(15)).send().await?;
        if !r.status().is_success() { anyhow::bail!("open-meteo {}", r.status()); }
        Ok(r.json::<Value>().await?)
    }).await.ok()
}

fn rnd(v: &Value) -> Value { v.as_f64().map(|x| json!(x.round() as i64)).unwrap_or(Value::Null) }

/// {place, now, feels, sky, day_sky, high, low, rain, uv, sunrise, sunset, rain_from, rain_to, hours:[{h,t,p,sky}]}
pub async fn weather(app: &App) -> Option<Value> {
    let d = forecast(app, 1, "&current=temperature_2m,apparent_temperature,weather_code&hourly=precipitation_probability,temperature_2m,weather_code&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,weather_code,uv_index_max,sunrise,sunset").await?;
    let l = Lang::of(app);
    let (day, cur, hourly) = (&d["daily"], &d["current"], &d["hourly"]);
    let hour = now(app).format("%H:00").to_string();
    let times: Vec<&str> = hourly["time"].as_array()?.iter().map(|t| t.as_str().unwrap_or("")).collect();
    let at = |k: &str, i: usize| hourly[k].get(i).cloned().unwrap_or(Value::Null);
    let ahead: Vec<(&str, i64)> = times.iter().enumerate().filter_map(|(i, t)| { let p = at("precipitation_probability", i).as_i64()?; (t.get(11..16)? >= hour.as_str()).then(|| (&t[11..16], p)) }).collect();
    let wet: Vec<&str> = ahead.iter().filter(|(_, p)| *p >= 50).map(|(h, _)| *h).collect();
    let sky = |c: &Value| L::sky(l, L::wmo(c.as_i64().unwrap_or(-1)));
    let s16 = |v: &Value| v.as_str().and_then(|s| s.get(11..16)).unwrap_or("").to_string();
    Some(json!({"place": app.setting("CERQ_BRIEF_PLACE"), "now": rnd(&cur["temperature_2m"]), "feels": rnd(&cur["apparent_temperature"]),
        "sky": sky(&cur["weather_code"]), "day_sky": sky(&day["weather_code"][0]), "high": rnd(&day["temperature_2m_max"][0]), "low": rnd(&day["temperature_2m_min"][0]),
        "rain": ahead.iter().map(|(_, p)| *p).max().unwrap_or(0), "uv": day["uv_index_max"][0].as_f64().map(|x| x.round() as i64).unwrap_or(0),
        "sunrise": s16(&day["sunrise"][0]), "sunset": s16(&day["sunset"][0]),
        "rain_from": wet.first().copied().unwrap_or(""), "rain_to": wet.last().copied().unwrap_or(""),
        "hours": times.iter().enumerate().map(|(i, t)| json!({"h": t.get(11..13).unwrap_or(""), "t": rnd(&at("temperature_2m", i)), "p": at("precipitation_probability", i).as_i64().unwrap_or(0), "sky": sky(&at("weather_code", i))})).collect::<Vec<_>>()}))
}

/// Tomorrow: {sky, high, low, rain, rain_from, rain_to}
pub async fn tomorrow_weather(app: &App) -> Option<Value> {
    let d = forecast(app, 2, "&hourly=precipitation_probability&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,weather_code").await?;
    let day = &d["daily"];
    let tday = day["time"][1].as_str()?.to_string();
    let wet: Vec<String> = d["hourly"]["time"].as_array()?.iter().zip(d["hourly"]["precipitation_probability"].as_array()?.iter())
        .filter(|(t, p)| t.as_str().unwrap_or("").starts_with(&tday) && p.as_i64().unwrap_or(0) >= 50).map(|(t, _)| t.as_str().unwrap_or("").get(11..16).unwrap_or("").to_string()).collect();
    Some(json!({"sky": L::sky(Lang::of(app), L::wmo(day["weather_code"][1].as_i64().unwrap_or(-1))), "high": rnd(&day["temperature_2m_max"][1]), "low": rnd(&day["temperature_2m_min"][1]),
        "rain": day["precipitation_probability_max"][1].as_i64().unwrap_or(0), "rain_from": wet.first().cloned().unwrap_or_default(), "rain_to": wet.last().cloned().unwrap_or_default()}))
}

// ---- calendar
/// Each day's events as the brief lists them: one entry per event (its
/// first occurrence that day), the same meeting on two calendars once,
/// all-day first then by time.
pub fn day_items(occ: &[calendar::Occ], day: NaiveDate, done: &HashSet<String>) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    let mut uids = HashSet::new();
    let next = day + Duration::days(1);
    for o in occ {
        let (sd, ed) = (o.start.date_naive(), o.end.date_naive());
        let ends_in = o.end > o.start && (ed > day || (ed == day && o.end.time() > chrono::NaiveTime::MIN));
        let overlaps = sd < next && (if o.all_day { ed > day } else { ends_in || (o.end == o.start && sd == day) });
        if !overlaps || !uids.insert(o.uid.clone()) { continue; }
        let (time, end) = if o.all_day { (String::new(), String::new()) } else {
            (if sd == day { o.start.format("%H:%M").to_string() } else { "00:00".into() }, if ed == day { o.end.format("%H:%M").to_string() } else { "24:00".into() })
        };
        out.push(json!({"title": o.title, "calendar": o.calendar.rsplit(" · ").next().unwrap_or(""), "kind": o.kind, "location": o.location,
            "all_day": o.all_day, "time": time, "end": end, "uid": if o.recurring { String::new() } else { o.uid.clone() }, "series": o.uid,
            "done": done.contains(&format!("{}|{day}", o.uid))}));
    }
    let hol = out.iter().filter(|x| x["kind"] == "holiday").count();
    for x in out.iter_mut().filter(|x| x["kind"] == "holiday") { x["cals"] = json!(hol); }
    // the same meeting on two calendars; a holiday in three holiday calendars (counted)
    let mut uniq: Vec<Value> = vec![];
    for x in out {
        let k = |v: &Value| (if v["kind"] == "holiday" { "holiday".to_string() } else { super::s(v, "title") }, super::s(v, "time"), v["all_day"].as_bool());
        match uniq.iter_mut().find(|u| k(u) == k(&x)) { Some(u) => *u = x, None => uniq.push(x) }
    }
    uniq.sort_by_key(|x| (!x["all_day"].as_bool().unwrap_or(false), super::s(x, "time"), super::s(x, "title").to_lowercase()));
    uniq
}

/// Items for days [from, from+n), one calendar read.
pub async fn days_items(app: &App, from: NaiveDate, n: i64) -> Vec<(NaiveDate, Vec<Value>)> {
    let occ = calendar::occurrences(app, from, from + Duration::days(n)).await.unwrap_or_else(|e| { tracing::warn!("brief calendar: {e:#}"); vec![] });
    let done = calendar::done_keys(app).await.unwrap_or_default();
    (0..n).map(|i| { let d = from + Duration::days(i); (d, day_items(&occ, d, &done)) }).collect()
}

/// When the day is busy and when it's free, what kind of day it is, and the
/// weather at each timed event ("Gym 18:30: 26°, dry"); sets e.wx.
pub fn shape(app: &App, events: &mut [Value], w: Option<&Value>) -> Value { shape_of((day_start(app), day_end(app)), events, w) }

fn shape_of(day: (i64, i64), events: &mut [Value], w: Option<&Value>) -> Value {
    let hours: Vec<&Value> = w.and_then(|w| w["hours"].as_array()).map(|a| a.iter().collect()).unwrap_or_default();
    let mut counts = serde_json::Map::new();
    for k in ["work", "personal", "money", "birthday", "holiday"] { counts.insert(k.into(), json!(events.iter().filter(|e| e["kind"] == k).count())); }
    for e in events.iter_mut().filter(|e| !e["all_day"].as_bool().unwrap_or(false)) {
        let h = super::s(e, "time").get(..2).unwrap_or("").to_string();
        e["wx"] = hours.iter().find(|x| x["h"] == h.as_str()).map(|x| json!({"t": x["t"], "p": x["p"]})).unwrap_or(Value::Null);
    }
    let mut timed: Vec<&Value> = events.iter().filter(|e| !e["all_day"].as_bool().unwrap_or(false)).collect();
    timed.sort_by_key(|e| super::s(e, "time"));
    if timed.is_empty() { return json!({"counts": counts, "free": [], "busy": null, "first": null, "last_end": null}); }
    let (t, e) = (|x: &Value| m(&super::s(x, "time")), |x: &Value| m(&super::s(x, "end")));
    let last_end = timed.iter().map(|x| e(x)).max().unwrap_or(0);
    let (ds, de) = (day.0.min(t(timed[0])), day.1.max(last_end));
    let (mut free, mut cur) = (vec![], ds);
    for x in &timed {
        let (s, en) = (t(x), e(x).max(t(x) + 10));
        if s - cur >= 45 { free.push(json!({"from": hm(cur), "to": hm(s), "min": s - cur})); }
        cur = cur.max(en);
    }
    if de - cur >= 45 { free.push(json!({"from": hm(cur), "to": hm(de), "min": de - cur})); }
    // the busiest stretch: events at most 30 min apart, the one with the most of them
    let (mut best, mut run): (Option<Value>, Vec<&Value>) = (None, vec![timed[0]]);
    for b in timed.iter().skip(1) {
        if t(b) - run.iter().map(|r| e(r)).max().unwrap_or(0) <= 30 { run.push(b); } else { run = vec![b]; }
        if run.len() >= 2 && best.as_ref().map(|x| run.len() as i64 > x["n"].as_i64().unwrap_or(0)).unwrap_or(true) {
            best = Some(json!({"from": super::s(run[0], "time"), "to": hm(run.iter().map(|r| e(r)).max().unwrap_or(0)), "n": run.len()}));
        }
    }
    json!({"counts": counts, "free": free, "busy": best, "first": super::s(timed[0], "time"), "last_end": hm(last_end)})
}

/// Payments, birthdays, holidays and long weekends in the days after
/// `today` (items[0] is today). When holiday calendars disagree on a moved
/// holiday, the date most list wins.
pub fn upcoming(l: Lang, today: NaiveDate, items: &[(NaiveDate, Vec<Value>)]) -> Vec<Value> {
    let (mut found, mut hols): (Vec<Value>, Vec<(NaiveDate, Value)>) = (vec![], vec![]);
    let when = |d: NaiveDate| format!("{} {}", L::dow(l, d), d.day());
    for (d, evs) in items.iter().filter(|(d, _)| *d > today) {
        let i = (*d - today).num_days();
        for e in evs {
            match e["kind"].as_str().unwrap_or("") {
                "holiday" => { hols.retain(|(x, _)| x != d); hols.push((*d, e.clone())); }
                k @ ("money" | "birthday") => found.push(json!({"kind": k, "title": e["title"], "date": d.to_string(), "when": when(*d), "in": i})),
                _ => {}
            }
        }
    }
    let cals = |v: &Value| v["cals"].as_i64().unwrap_or(1);
    for (d, e) in &hols {
        if hols.iter().any(|(x, o)| x != d && (*x - *d).num_days().abs() <= 4 && cals(o) > cals(e)) { continue; }
        let i = (*d - today).num_days();
        found.push(json!({"kind": "holiday", "title": e["title"], "date": d.to_string(), "when": when(*d), "in": i}));
        let wd = d.weekday().num_days_from_monday();
        if wd == 0 || wd == 4 {
            let (a, z) = if wd == 0 { (*d - Duration::days(2), *d) } else { (*d, *d + Duration::days(2)) };
            found.push(json!({"kind": "weekend", "title": L::t0(l, "long_weekend"), "date": a.to_string(), "when": format!("{} – {}", when(a), when(z)), "in": (a - today).num_days().max(0)}));
        }
    }
    found.sort_by_key(|x| x["in"].as_i64().unwrap_or(0));
    found
}

fn in_days(l: Lang, n: i64) -> String { if n == 1 { L::t0(l, "tomorrow") } else { L::t(l, "in_days", &[("n", &n.to_string())]) } }

/// What matters today, most actionable first: things to do or know, never
/// filler (no weather unless it changes plans).
pub fn matters(app: &App, b: &Value, t_now: i64) -> Vec<Value> {
    let f = fahrenheit(app);
    let cfg = Cfg { l: Lang::of(app), day: (day_start(app), day_end(app)),
        hot: num(app, "CERQ_HOT").unwrap_or(if f { 86.0 } else { 30.0 }), cold: num(app, "CERQ_COLD").unwrap_or(if f { 54.0 } else { 12.0 }) };
    matters_of(&cfg, b, t_now)
}

/// The settings `matters` reads.
pub struct Cfg { pub l: Lang, pub day: (i64, i64), pub hot: f64, pub cold: f64 }

pub fn matters_of(cfg: &Cfg, b: &Value, t_now: i64) -> Vec<Value> {
    let l = cfg.l;
    let ev: Vec<&Value> = b["events"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let (w, sh, mail) = (&b["weather"], &b["shape"], &b["mail"]);
    let up: Vec<&Value> = b["upcoming"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let s = super::s;
    let done = |e: &Value| e["done"].as_bool().unwrap_or(false);
    let mut out = vec![];
    for e in ev.iter().filter(|x| x["kind"] == "money" && !done(x)) {
        let t = s(e, "time");
        out.push(json!({"k": "pay", "text": e["title"], "detail": L::t0(l, "today") + &if t.is_empty() { String::new() } else { format!(" · {t}") }}));
    }
    let timed: Vec<&&Value> = ev.iter().filter(|e| !e["all_day"].as_bool().unwrap_or(false) && e["kind"] != "money" && !done(e)).collect();
    let live = timed.iter().find(|e| m(&s(e, "time")) <= t_now && t_now < m(&s(e, "end")));
    let nxt = timed.iter().find(|e| m(&s(e, "time")) > t_now);
    let left: Vec<&&&Value> = timed.iter().filter(|e| m(&s(e, "end")) > t_now).collect();
    if let Some(e) = live { out.push(json!({"k": "now", "text": e["title"], "detail": L::t(l, "now_until", &[("t", &s(e, "end"))]), "time": e["time"]})); }
    if let Some(e) = nxt {
        let free = m(&s(e, "time")) - t_now.max(live.map(|x| m(&s(x, "end"))).unwrap_or(0));
        let extra = if free >= 90 { format!(" · {}", L::t(l, "free_until", &[("d", &dur(free))])) } else { String::new() };
        out.push(json!({"k": "next", "text": e["title"], "detail": s(e, "time") + &extra, "time": e["time"]}));
    }
    if let Some(busy) = sh.get("busy").filter(|x| !x.is_null()) {
        if busy["n"].as_i64().unwrap_or(0) >= 3 && m(&s(busy, "to")) > t_now {
            out.push(json!({"k": "busy", "text": L::t(l, "busy", &[("a", &s(busy, "from")), ("b", &s(busy, "to"))]), "detail": L::t(l, "back_to_back", &[("n", &busy["n"].to_string())])}));
        }
    }
    if left.is_empty() {
        let f = &b["tomorrow"]["first"];
        let detail = if f.is_object() { L::t(l, "tomorrow_first", &[("t", &s(f, "time")), ("title", &s(f, "title")), ("n", &b["tomorrow"]["count"].as_i64().unwrap_or(1).to_string())]) } else { L::t0(l, "tomorrow_free") };
        out.push(json!({"k": "free", "text": L::t0(l, if timed.is_empty() { "free_day" } else { "nothing_else" }), "detail": detail}));
    }
    // weather only when it changes plans
    let (ds, de) = (cfg.day.0 / 60, cfg.day.1 / 60);
    let hours: Vec<&Value> = w["hours"].as_array().map(|a| a.iter().filter(|h| {
        let hh: i64 = s(h, "h").parse().unwrap_or(-1);
        ds <= hh && hh < de && hh * 60 >= t_now - 59
    }).collect()).unwrap_or_default();
    let wet: Vec<i64> = hours.iter().filter(|h| h["p"].as_i64().unwrap_or(0) >= 50).map(|h| s(h, "h").parse().unwrap_or(0)).collect();
    if let (Some(a), Some(z)) = (wet.first(), wet.last().map(|z| z + 1)) {
        let hit = left.iter().find(|e| a * 60 < m(&s(e, "end")) && m(&s(e, "time")) < z * 60).map(|e| s(e, "title"));
        out.push(json!({"k": "rain", "text": L::t(l, "rain", &[("a", &format!("{a:02}:00")), ("b", &format!("{z:02}:00"))]),
            "detail": L::t0(l, "umbrella") + &hit.map(|x| L::t(l, "during", &[("x", &x)])).unwrap_or_default()}));
    }
    let temps: Vec<i64> = hours.iter().filter_map(|h| h["t"].as_i64()).collect();
    let (hot, cold) = (cfg.hot, cfg.cold);
    if let (Some(mx), Some(mn)) = (temps.iter().max(), temps.iter().min()) {
        if *mx as f64 >= hot { out.push(json!({"k": "heat", "text": L::t(l, "hot", &[("t", &mx.to_string())]), "detail": L::t0(l, "hot_tip")})); }
        else if *mn as f64 <= cold { out.push(json!({"k": "cold", "text": L::t(l, "cold", &[("t", &mn.to_string())]), "detail": L::t0(l, "cold_tip")})); }
    }
    let inn = |u: &Value| u["in"].as_i64().unwrap_or(0);
    for u in up.iter().filter(|u| u["kind"] == "money" && inn(u) <= 3) {
        out.push(json!({"k": "pay", "text": u["title"], "detail": format!("{} · {}", s(u, "when"), in_days(l, inn(u)))}));
    }
    if let Some(n) = mail["people_waiting"].as_i64().filter(|n| *n > 0) {
        out.push(json!({"k": "mail", "text": L::t(l, if n == 1 { "waiting_1" } else { "waiting_n" }, &[("n", &n.to_string())]),
            "detail": L::t(l, "oldest", &[("n", &mail["oldest_to_reply_days"].as_i64().unwrap_or(0).to_string())]), "who": mail.get("waiting").cloned().unwrap_or(json!([]))}));
    }
    for u in &up {
        let k = s(u, "kind");
        if k == "weekend" || ((k == "birthday" || k == "holiday") && inn(u) <= 7) {
            // already told as a long weekend
            if k == "holiday" && up.iter().any(|x| x["kind"] == "weekend" && inn(x) <= inn(u) && inn(u) <= inn(x) + 3) { continue; }
            let detail = if inn(u) > 0 { format!("{} · {}", s(u, "when"), in_days(l, inn(u))) } else { s(u, "when") };
            out.push(json!({"k": k, "text": u["title"], "detail": detail}));
        }
    }
    out.truncate(7);
    out
}

/// What the headline is about, by part of the day.
fn focus(part: &str) -> &'static str {
    match part {
        "morning" => "how their day looks: calm or busy and when, and the one thing not to forget. Like: \"A calm Sunday: pay the cards, and rest up for a full Monday.\"",
        "afternoon" => "how the rest of their day looks from now on (ignore what already happened), and the one thing still to do. Like: \"Two meetings left, then a free evening; reply to Ana first.\"",
        _ => "the evening: what is still left tonight, if anything, and how tomorrow starts, so they can wind down and get ready. Never talk about today's earlier events as if they were still ahead. Like: \"A quiet night left; tomorrow starts at 9:00 with the dentist, so get some sleep.\"",
    }
}

/// One headline from your model on top of the list; without an answer no
/// text at all (the list says it) and model_note says why.
async fn write(app: &App, b: &mut Value) -> (String, bool) {
    let t = now(app);
    let t_now = (t.hour() * 60 + t.minute()) as i64;
    let facts = json!({"date": b["date"], "time_now": t.format("%H:%M").to_string(), "part_of_day": b["part"],
        "matters": b["matters"].as_array().map(|a| a.iter().map(|x| { let mut x = x.clone(); x.as_object_mut().map(|o| o.remove("who")); x }).collect::<Vec<_>>()).unwrap_or_default(),
        "timed_today": b["timed"].as_array().map(|a| a.iter().map(|e| json!({"time": e["time"], "end": e["end"], "title": e["title"], "kind": e["kind"],
            "over": m(&super::s(e, "end")) <= t_now, "done": e["done"].as_bool().unwrap_or(false)})).collect::<Vec<_>>()).unwrap_or_default(),
        "tomorrow": b["tomorrow"]});
    let sys = format!("Write in {}: ONE short sentence, at most 20 words, plain text, speaking to the reader as \"you\", about {} Events marked over already happened. Use only the facts given; never invent anything. A list of the details is shown right below, so don't repeat it item by item. Event titles may be in another language; keep them as they are.",
        Lang::of(app).name(), focus(b["part"].as_str().unwrap_or("morning")));
    let out = llm::complete(app, vec![json!({"role": "system", "content": sys}), json!({"role": "user", "content": facts.to_string()})], 120, llm::Prio::Bg).await;
    let text = crate::calendar::clean_model_text(&out.unwrap_or_default(), 700);
    if text.chars().count() > 15 { b["model_note"] = json!(""); return (text, true); }
    b["model_note"] = json!(if llm::configured(app) { "your model didn't answer" } else { "your model isn't set up" });
    (String::new(), false)
}

// ---- build and read
static LAST: LazyLock<Mutex<Option<Value>>> = LazyLock::new(Default::default);
static BUILDING: AtomicBool = AtomicBool::new(false);

async fn last(app: &App) -> Value {
    let mut g = LAST.lock().await;
    if g.is_none() { *g = Some(app.db.kv_get("today.brief").await.ok().flatten().unwrap_or(Value::Null)); }
    g.clone().unwrap_or(Value::Null)
}

/// Today's and tomorrow's events, the next days and the day's shape, into b.
async fn fill_calendar(app: &App, b: &mut Value, day: NaiveDate) {
    b["cal_sig"] = json!(calendar::signature(app).await);
    let items = days_items(app, day, 11).await;
    let mut events = items[0].1.clone();
    let nxt = &items[1].1;
    let all_day = |e: &&Value| e["all_day"].as_bool().unwrap_or(false);
    let first = nxt.iter().find(|e| !all_day(e)).cloned().unwrap_or(Value::Null);
    b["tomorrow_first"] = first.clone();
    b["tomorrow"] = json!({"count": nxt.iter().filter(|e| !all_day(e)).count(), "first": first,
        "all_day": nxt.iter().filter(all_day).map(|e| json!({"title": e["title"], "kind": e["kind"]})).collect::<Vec<_>>()});
    let w = b.get("weather").cloned().filter(|w| !w.is_null());
    b["shape"] = shape(app, &mut events, w.as_ref());
    b["upcoming"] = json!(upcoming(Lang::of(app), day, &items));
    b["timed"] = json!(events.iter().filter(|e| !all_day(e)).collect::<Vec<_>>());
    b["all_day"] = json!(events.iter().filter(all_day).collect::<Vec<_>>());
    b["events"] = json!(events);
}

async fn mail_facts(app: &App) -> Value { crate::mail::brief_mail(app).await.unwrap_or_else(|e| { tracing::warn!("brief mail: {e:#}"); json!({}) }) }

/// Build the brief (one at a time: a second caller gets the last one).
pub async fn build(app: &App, notify: bool) -> Value {
    if BUILDING.swap(true, Ordering::SeqCst) { return last(app).await; }
    let b = build_inner(app, notify).await;
    BUILDING.store(false, Ordering::SeqCst);
    b
}

async fn build_inner(app: &App, notify: bool) -> Value {
    let l = Lang::of(app);
    let t = now(app);
    let today = t.date_naive();
    let p = part(t.hour());
    let mut b = json!({"date": L::long_date(l, today), "day": today.to_string(), "ts": crate::util::now_ts(), "lang": l.code(),
        "part": p, "greeting": L::t0(l, p), "weather": weather(app).await});
    fill_calendar(app, &mut b, today).await;
    b["mail"] = mail_facts(app).await;
    let t_now = (t.hour() * 60 + t.minute()) as i64;
    b["matters"] = json!(matters(app, &b, t_now));
    b["matters_sig"] = json!(b["matters"].as_array().unwrap().iter().map(|x| x["text"].clone()).collect::<Vec<_>>());
    let (text, by_model) = write(app, &mut b).await;
    b["text"] = json!(text);
    b["by_model"] = json!(by_model);
    let prev = last(app).await;
    if prev["pushed_day"] == b["day"] { b["pushed_day"] = b["day"].clone(); }
    if notify {
        let body = if text.is_empty() { b["matters"].as_array().unwrap().iter().take(3).map(|x| format!("{} ({})", super::s(x, "text"), super::s(x, "detail"))).collect::<Vec<_>>().join(" · ") } else { text.clone() };
        crate::push::send_all(app, json!({"title": format!("{} · {}", super::s(&b, "greeting"), super::s(&b, "date")), "body": body.chars().take(240).collect::<String>(), "tag": "cerq-brief", "url": "/?open=morning"})).await;
        b["pushed_day"] = b["day"].clone();
    }
    if let Err(e) = app.db.kv_set("today.brief", &b).await { tracing::warn!("brief save: {e:#}"); }
    *LAST.lock().await = Some(b.clone());
    tracing::info!("brief built ({}, {} events, {} matters)", if by_model { "model" } else { "no model" }, b["events"].as_array().map(|a| a.len()).unwrap_or(0), b["matters"].as_array().map(|a| a.len()).unwrap_or(0));
    app.emit(json!({"kind": "today"}));
    b
}

fn rebuild_soon(app: &App) {
    let a = app.clone();
    tokio::spawn(async move { build(&a, false).await; });
}

/// The last brief with its mail and "what matters" recomputed now (fast), so
/// a reply or a done mark shows at once. When that changed, or the brief is
/// old, it's rebuilt in the background so the headline catches up.
pub async fn get(app: &App) -> Value {
    let prev = last(app).await;
    let l = Lang::of(app);
    let t = now(app);
    let p = part(t.hour());
    if prev.is_null() || prev.get("day").is_none() {
        rebuild_soon(app);
        return json!({"pending": true, "greeting": L::t0(l, p), "text": "Writing your brief…"});
    }
    let mut b = prev.clone();
    let today = t.date_naive();
    let mut cal_changed = false;
    if super::s(&b, "day") != today.to_string() || calendar::signature(app).await != super::s(&b, "cal_sig") {
        fill_calendar(app, &mut b, today).await;
        cal_changed = true;
    }
    b["mail"] = mail_facts(app).await;
    b["matters"] = json!(matters(app, &b, (t.hour() * 60 + t.minute()) as i64));
    let changed = b["matters"].as_array().map(|a| a.iter().map(|x| x["text"].clone()).collect::<Vec<_>>()) != prev["matters_sig"].as_array().cloned();
    b["greeting"] = json!(L::t0(l, p));
    let age = crate::util::now_ts() - prev["ts"].as_i64().unwrap_or(0);
    let old = age > 30 * 60 || super::s(&prev, "day") != today.to_string() || super::s(&prev, "part") != p;
    if old || cal_changed || (changed && age > 20) { rebuild_soon(app); }
    b
}

/// Build and push once a day at CERQ_BRIEF_AT (before noon).
pub async fn tick(app: &App) {
    let t = now(app);
    let at = hm_env(app, "CERQ_BRIEF_AT", "07:00");
    let mins = (t.hour() * 60 + t.minute()) as i64;
    if mins >= at && t.hour() < 12 && super::s(&last(app).await, "pushed_day") != t.date_naive().to_string() {
        build(app, true).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev(title: &str, kind: &str, time: &str, end: &str) -> Value {
        json!({"title": title, "kind": kind, "all_day": time.is_empty(), "time": time, "end": end, "done": false, "location": ""})
    }
    fn d(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    #[test]
    fn matters_most_actionable_first() {
        let mut b = json!({"events": [ev("Pagar tarjeta", "money", "", ""), ev("Standup", "work", "09:00", "09:30"), ev("Dentist", "personal", "16:00", "17:00"),
                                      ev("Gym", "personal", "19:00", "20:00")],
            "weather": {"hours": [{"h": "15", "t": 24, "p": 10}, {"h": "16", "t": 31, "p": 70}, {"h": "17", "t": 29, "p": 60}]},
            "shape": {"busy": null}, "mail": {"people_waiting": 2, "oldest_to_reply_days": 4, "waiting": []},
            "upcoming": [{"kind": "money", "title": "Pagar luz", "when": "Fri 20", "in": 2}, {"kind": "birthday", "title": "Cumple Ana", "when": "Sun 22", "in": 4}],
            "tomorrow": {"count": 1, "first": {"time": "09:00", "title": "X"}}});
        let cfg = Cfg { l: Lang::En, day: (480, 1320), hot: 30.0, cold: 12.0 };
        let got = matters_of(&cfg, &b, 9 * 60 + 10);
        let ks: Vec<&str> = got.iter().map(|x| x["k"].as_str().unwrap()).collect();
        assert_eq!(ks, ["pay", "now", "next", "rain", "heat", "pay", "mail"]);
        assert_eq!(got[2]["detail"], "16:00 · free until then (6h30)");
        assert_eq!(got[3]["detail"], "umbrella · during Dentist");
        // the evening: nothing left, tomorrow's first event; a done payment is not asked again
        b["events"][0]["done"] = json!(true);
        let got = matters_of(&cfg, &b, 21 * 60);
        assert_eq!(got[0]["k"], "free");
        assert_eq!(got[0]["detail"], "tomorrow 09:00 · X · 1 things");
    }

    #[test]
    fn day_shape() {
        let mut evs = vec![ev("A", "work", "09:00", "10:00"), ev("B", "work", "10:15", "11:00"), ev("C", "work", "11:20", "12:00"), ev("D", "personal", "15:00", "16:00")];
        let sh = shape_of((480, 1320), &mut evs, None);
        assert_eq!(sh["free"], json!([{"from": "08:00", "to": "09:00", "min": 60}, {"from": "12:00", "to": "15:00", "min": 180}, {"from": "16:00", "to": "22:00", "min": 360}]));
        assert_eq!(sh["busy"], json!({"from": "09:00", "to": "12:00", "n": 3}));
        assert_eq!(sh["counts"]["work"], 3);
    }

    #[test]
    fn upcoming_long_weekend_and_moved_holiday() {
        let today = d("2026-11-18"); // a Wednesday
        let hol = |t: &str, n: i64| { let mut v = ev(t, "holiday", "", ""); v["cals"] = json!(n); v };
        let items = vec![(d("2026-11-18"), vec![]), (d("2026-11-20"), vec![hol("Soberanía (trasladado)", 1)]), (d("2026-11-23"), vec![hol("Soberanía", 3)]),
                         (d("2026-11-25"), vec![ev("Pagar luz", "money", "", "")])];
        let up = upcoming(Lang::En, today, &items);
        let kinds: Vec<(&str, &str)> = up.iter().map(|u| (u["kind"].as_str().unwrap(), u["date"].as_str().unwrap())).collect();
        // the 23rd (3 calendars) beats the 20th; a Monday holiday makes Sat–Mon a long weekend
        assert_eq!(kinds, [("weekend", "2026-11-21"), ("holiday", "2026-11-23"), ("money", "2026-11-25")]);
        assert_eq!(up[0]["when"], "Sat 21 – Mon 23");
    }
}
