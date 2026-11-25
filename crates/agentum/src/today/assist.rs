//! Assist (assist.py): the AI around your day beside the brief — to-dos
//! fitted into free time, when to leave, dates found in mail, people
//! waiting, the evening wrap-up and the week ahead, plus their pushes.
//! kv "today.assist" = {hidden, pushed, model}; kv "today.travel" = travel times.
use super::{brief, hm, hm_env, i18n::{self as L, Lang}, m, now, num, s};
use crate::{llm, state::App};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;
use tokio::sync::Mutex;

// ---- state
static STATE: LazyLock<Mutex<Option<Value>>> = LazyLock::new(Default::default);

async fn with_state<T>(app: &App, f: impl FnOnce(&mut Value) -> (T, bool)) -> T {
    let mut g = STATE.lock().await;
    if g.is_none() {
        let mut v = app.db.kv_get("today.assist").await.ok().flatten().unwrap_or(json!({}));
        for (k, d) in [("hidden", json!([])), ("pushed", json!({})), ("model", json!({}))] { if !v[k].is_array() && !v[k].is_object() { v[k] = d; } }
        *g = Some(v);
    }
    let st = g.as_mut().unwrap();
    let (out, dirty) = f(st);
    if dirty {
        // Keep it small: the last 300 hidden, pushes of the last 14 days, 40 model answers.
        let now = crate::util::now_ts();
        if let Some(h) = st["hidden"].as_array_mut() { let n = h.len(); if n > 300 { h.drain(..n - 300); } }
        if let Some(p) = st["pushed"].as_object_mut() { p.retain(|_, t| now - (t.as_f64().unwrap_or(0.0) as i64) < 14 * 86400); }
        if let Some(mo) = st["model"].as_object_mut() {
            if mo.len() > 60 {
                let mut v: Vec<(String, Value)> = std::mem::take(mo).into_iter().collect();
                v.sort_by_key(|(_, x)| x["t"].as_f64().unwrap_or(0.0) as i64);
                *mo = v.into_iter().rev().take(40).collect();
            }
        }
        if let Err(e) = app.db.kv_set("today.assist", st).await { tracing::warn!("assist save: {e:#}"); }
    }
    out
}

async fn hidden(app: &App) -> Vec<String> {
    with_state(app, |st| (st["hidden"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(), false)).await
}

/// "Not now" (or added): hidden from Today; "ev:<uid>" also marks today done.
pub async fn hide(app: &App, id: &str) -> anyhow::Result<()> {
    let id2 = id.to_string();
    with_state(app, |st| {
        let h = st["hidden"].as_array_mut().unwrap();
        if h.iter().any(|x| x == &json!(id2)) { ((), false) } else { h.push(json!(id2)); ((), true) }
    }).await;
    if let Some(uid) = id.strip_prefix("ev:") {
        let key = format!("{}|{}", uid.split("::").next().unwrap_or(uid), now(app).date_naive());
        app.db.call(move |c| c.execute("INSERT OR REPLACE INTO calendar_done(key,ts) VALUES(?1,?2)", rusqlite::params![key, crate::util::now_ts()])).await?;
        app.emit(json!({"kind": "calendar"}));
    }
    *CACHE.lock().await = None;
    Ok(())
}

/// The model's answer for these facts, cached 6 h. None: no answer.
async fn ask(app: &App, kind: &str, facts: &Value, system: &str, max_tokens: u32) -> Option<String> {
    let key: String = format!("{kind}:{facts}").chars().take(2000).collect();
    let k2 = key.clone();
    let hit = with_state(app, |st| (st["model"][&k2].as_object().filter(|h| crate::util::now_ts() - (h["t"].as_f64().unwrap_or(0.0) as i64) < 6 * 3600).and_then(|h| h["a"].as_str().map(String::from)), false)).await;
    if hit.is_some() { return hit; }
    let out = llm::complete(app, vec![json!({"role": "system", "content": system}), json!({"role": "user", "content": facts.to_string()})], max_tokens, llm::Prio::Bg).await?;
    let a = out.clone();
    with_state(app, |st| { st["model"][&key] = json!({"a": a, "t": crate::util::now_ts()}); ((), true) }).await;
    Some(out)
}

// ---- to-dos
/// (open to-dos sorted by due, finished today) as {id, title, due "YYYY-MM-DD HH:MM"}.
async fn todos(app: &App) -> (Vec<Value>, Vec<Value>) {
    static REMINDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*reminder:").unwrap());
    let tz = app.tz();
    let today = now(app).date_naive();
    let due = |v: &str| -> String {
        let v = v.trim();
        if v.is_empty() { return String::new(); }
        if v.len() == 10 { return format!("{v} 00:00"); }
        if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&v.replace(' ', "T")) { return t.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string(); }
        // A due time without a zone is UTC (as Odysseus stored it).
        NaiveDateTime::parse_from_str(&v.replace('T', " ").chars().take(19).collect::<String>(), "%Y-%m-%d %H:%M:%S").ok()
            .or_else(|| NaiveDateTime::parse_from_str(&v.replace('T', " ").chars().take(16).collect::<String>(), "%Y-%m-%d %H:%M").ok())
            .map(|t| chrono::Utc.from_utc_datetime(&t).with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
    };
    let item = |n: &Value| {
        let t = s(n, "title");
        let title = if t.trim().is_empty() { "(untitled)".to_string() } else { t.trim().to_string() };
        json!({"id": s(n, "id"), "title": title, "due": due(&s(n, "due"))})
    };
    let mut open: Vec<Value> = crate::notes::open_todos(app).await.unwrap_or_default().iter().filter(|n| !REMINDER.is_match(&s(n, "title"))).map(item).collect();
    open.sort_by_key(|x| { let d = s(x, "due"); if d.is_empty() { "9999".to_string() } else { d } });
    let done = crate::notes::all(app, false).await.unwrap_or_default().iter()
        .filter(|n| n["note_type"] == "todo" && n["done"].as_bool().unwrap_or(false) && !REMINDER.is_match(&s(n, "title")))
        .filter(|n| {
            let u = s(n, "updated_at");
            let t = chrono::DateTime::parse_from_rfc3339(&u).map(|t| t.naive_utc()).ok()
                .or_else(|| NaiveDateTime::parse_from_str(u.get(..19).unwrap_or(""), "%Y-%m-%d %H:%M:%S").ok());
            t.map(|t| chrono::Utc.from_utc_datetime(&t).with_timezone(&tz).date_naive() == today).unwrap_or(false)
        }).map(item).collect();
    (open, done)
}

fn lang_name(app: &App) -> &'static str { Lang::of(app).name() }

/// Free slots of at least 30 min from now (the rest of a free day).
pub fn free_slots(free: &[Value], has_timed: bool, t_now: i64, day_start: i64, day_end: i64, date: &str) -> Vec<Value> {
    let mut out = vec![];
    for g in free {
        let (a, z) = (m(&s(g, "from")).max(t_now + 5), m(&s(g, "to")));
        if z - a >= 30 && a < day_end { out.push(json!({"date": date, "from": hm(a), "to": hm(z), "min": z - a})); }
    }
    if !has_timed {
        let a = (t_now + 5).max(day_start);
        if day_end - a >= 30 { out.push(json!({"date": date, "from": hm(a), "to": hm(day_end), "min": day_end - a})); }
    }
    out
}

/// Today's free time from now on, each slot with the to-do that fits it.
async fn gaps(app: &App, b: &Value, open: &[Value]) -> Vec<Value> {
    let t = now(app);
    let t_now = (t.hour() * 60 + t.minute()) as i64;
    let free: Vec<Value> = b["shape"]["free"].as_array().cloned().unwrap_or_default();
    let has_timed = b["timed"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
    let mut out = free_slots(&free, has_timed, t_now, brief::day_start(app), brief::day_end(app), &t.date_naive().to_string());
    if out.is_empty() || open.is_empty() { return out; }
    let list: Vec<&Value> = open.iter().take(12).collect();
    let sys = format!("You plan someone's free time. Given free slots of today and their open to-dos, pick for each slot the to-do that best fits (short tasks in short slots, errands that need shops open during the day, urgent or overdue first). Each to-do at most once; a slot may get none. Answer only JSON: a list of {{\"slot\": index, \"todo\": id, \"why\": \"at most 8 words\"}}. To-do titles may be in another language. Write each why in {}.", lang_name(app));
    let ans = ask(app, "gaps", &json!({"now": t.format("%H:%M").to_string(), "free": out, "todos": list}), &sys, 300).await.and_then(|a| llm::extract_json(&a));
    match ans.as_ref().and_then(|a| a.as_array()) {
        Some(picks) => for p in picks {
            let (Some(i), id) = (p["slot"].as_u64().map(|i| i as usize), p["todo"].as_str().map(String::from).unwrap_or_else(|| p["todo"].to_string())) else { continue };
            let Some(td) = open.iter().find(|x| s(x, "id") == id) else { continue };
            if let Some(g) = out.get_mut(i).filter(|g| g.get("todo").is_none()) {
                g["todo"] = td.clone();
                g["why"] = json!(crate::calendar::clean_model_text(&p["why"].as_str().map(String::from).unwrap_or_default(), 80));
                g["by"] = json!("model");
            }
        },
        // No model: the most urgent to-dos in order, so the slots still say something.
        None => for (g, td) in out.iter_mut().zip(open) { g["todo"] = td.clone(); g["why"] = json!("due soonest"); g["by"] = json!("due date"); },
    }
    out
}

// ---- travel (Nominatim + OSRM)
static ONLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)https?://|zoom|meet\.google|teams|webex|online|virtual|discord|slack|phone|tel[eé]fono|llamada").unwrap());
static TRAVEL: LazyLock<Mutex<()>> = LazyLock::new(Default::default);
const UA: &str = "cerq-homelab/1.0 (self-hosted dashboard)";

fn home(app: &App) -> Option<(f64, f64)> {
    match (num(app, "CERQ_HOME_LAT"), num(app, "CERQ_HOME_LON")) { (Some(a), Some(b)) => Some((a, b)), _ => brief::place(app) }
}

/// Minutes from home to a place by your mode (car ×1.3 for traffic) plus
/// the buffer; cached 30 days; errors are not cached.
async fn travel_minutes(app: &App, place: &str) -> Option<i64> {
    let key = place.trim().to_lowercase();
    let (hlat, hlon) = home(app)?;
    if key.is_empty() || ONLINE.is_match(&key) { return None; }
    let mode = app.setting("CERQ_TRAVEL_MODE");
    let buffer = num(app, "CERQ_TRAVEL_BUFFER").unwrap_or(10.0) as i64;
    let key = format!("{}:{buffer}:{key}", mode.to_lowercase());
    let _g = TRAVEL.lock().await;
    let mut cache = app.db.kv_get("today.travel").await.ok().flatten().unwrap_or(json!({}));
    if let Some(hit) = cache.get(&key).filter(|h| crate::util::now_ts() - (h["t"].as_f64().unwrap_or(0.0) as i64) < 30 * 86400) { return hit["min"].as_i64(); }
    let router = match mode.as_str() { "Bike" => "https://routing.openstreetmap.de/routed-bike/route/v1/driving", "Walking" => "https://routing.openstreetmap.de/routed-foot/route/v1/driving", _ => "https://router.project-osrm.org/route/v1/driving" };
    let get = |url: String, q: Vec<(&'static str, String)>| { let a = app.clone(); async move { a.http.get(url).query(&q).header("User-Agent", UA).timeout(std::time::Duration::from_secs(15)).send().await?.error_for_status()?.json::<Value>().await } };
    let viewbox = format!("{},{},{},{}", hlon - 1.5, hlat + 1.5, hlon + 1.5, hlat - 1.5);
    let res: Result<Option<i64>, reqwest::Error> = async {
        let g = get("https://nominatim.openstreetmap.org/search".into(), vec![("q", place.to_string()), ("format", "json".into()), ("limit", "1".into()), ("viewbox", viewbox)]).await?;
        let mut mins = None;
        if let Some(p) = g.get(0) {
            let (lat, lon) = (s(p, "lat").parse::<f64>().unwrap_or(0.0), s(p, "lon").parse::<f64>().unwrap_or(0.0));
            let r = get(format!("{router}/{hlon},{hlat};{lon},{lat}"), vec![("overview", "false".into())]).await?;
            if let Some(secs) = r["routes"][0]["duration"].as_f64() { mins = Some((secs / 60.0 * if mode == "Bike" || mode == "Walking" { 1.0 } else { 1.3 }) as i64 + buffer); }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Nominatim: one request a second
        Ok(mins)
    }.await;
    match res {
        Ok(mins) => {
            cache[&key] = json!({"min": mins, "t": crate::util::now_ts()});
            let _ = app.db.kv_set("today.travel", &cache).await;
            mins
        }
        Err(e) => { tracing::warn!("travel: {e}"); None }
    }
}

/// Timed events still ahead with a place: when to leave (rain adds 10 min).
async fn leave(app: &App, b: &Value) -> Vec<Value> {
    let t = now(app);
    let t_now = (t.hour() * 60 + t.minute()) as i64;
    let mut out = vec![];
    for e in b["timed"].as_array().cloned().unwrap_or_default() {
        let loc = s(&e, "location");
        if loc.is_empty() || m(&s(&e, "time")) <= t_now || e["done"].as_bool().unwrap_or(false) { continue; }
        let Some(mins) = travel_minutes(app, &loc).await.filter(|m| *m > 0) else { continue };
        let h = s(&e, "time").get(..2).unwrap_or("").to_string();
        let rain = b["weather"]["hours"].as_array().and_then(|a| a.iter().find(|x| x["h"] == h.as_str())).map(|x| x["p"].as_i64().unwrap_or(0) >= 50).unwrap_or(false);
        let go = m(&s(&e, "time")) - mins - if rain { 10 } else { 0 };
        out.push(json!({"title": e["title"], "time": e["time"], "location": loc, "travel": mins, "leave": hm(go.max(0)), "rain": rain}));
    }
    out
}

/// Dates read from mail, not hidden, in the next 60 days, not already on
/// the calendar that day (same sender words in a title).
async fn reminders(app: &App) -> Vec<Value> {
    let today = now(app).date_naive();
    let hid = hidden(app).await;
    let all = crate::mail::mail_reminders(app).await.unwrap_or_else(|e| { tracing::warn!("mail reminders: {e:#}"); vec![] });
    let cands: Vec<Value> = all.into_iter().filter(|r| !hid.contains(&s(r, "id"))).filter(|r| NaiveDate::parse_from_str(&s(r, "date"), "%Y-%m-%d").map(|d| today <= d && d <= today + Duration::days(60)).unwrap_or(false)).collect();
    if cands.is_empty() { return vec![]; }
    let days = brief::days_items(app, today, 61).await;
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w{4,}").unwrap());
    let mut out: Vec<Value> = cands.into_iter().filter(|r| {
        let titles: String = days.iter().find(|(d, _)| d.to_string() == s(r, "date")).map(|(_, v)| v.iter().map(|e| s(e, "title").to_lowercase()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let from = s(r, "from").to_lowercase();
        let words: Vec<&str> = WORD.find_iter(&from).map(|w| w.as_str()).filter(|w| !matches!(*w, "noreply" | "info" | "mail")).take(2).collect();
        !(words.iter().any(|w| titles.contains(w)))
    }).collect();
    out.sort_by_key(|r| s(r, "date"));
    out.truncate(8);
    out
}

/// The evening: what got done, what slipped, errands to check, tomorrow,
/// and one thing to prepare tonight.
async fn wrapup(app: &App, b: &Value, open: &[Value], done_today: &[Value]) -> Value {
    let l = Lang::of(app);
    let t = now(app);
    let t_now = (t.hour() * 60 + t.minute()) as i64;
    let timed: Vec<Value> = b["timed"].as_array().cloned().unwrap_or_default();
    let mut done: Vec<String> = timed.iter().filter(|e| m(&s(e, "end")) <= t_now || e["done"].as_bool().unwrap_or(false)).map(|e| s(e, "title"))
        .chain(done_today.iter().map(|x| s(x, "title"))).collect();
    let today = t.date_naive().to_string();
    let mut slipped: Vec<Value> = open.iter().filter(|x| { let d = s(x, "due"); !d.is_empty() && d.get(..10).unwrap_or("") <= today.as_str() }).cloned().collect();
    // Errands kept as calendar events: today's personal one-offs that are over. Today asks: Done, or Move to tomorrow.
    let hid = hidden(app).await;
    let check: Vec<Value> = timed.iter().filter(|e| !s(e, "uid").is_empty() && e["kind"] == "personal" && m(&s(e, "end")) <= t_now
        && !hid.contains(&format!("ev:{}", s(e, "uid"))) && !e["done"].as_bool().unwrap_or(false))
        .map(|e| json!({"uid": e["uid"], "title": e["title"], "time": e["time"], "end": e["end"]})).collect();
    let tw = brief::tomorrow_weather(app).await;
    let first = b["tomorrow"]["first"].clone();
    let count = b["tomorrow"]["count"].as_i64().unwrap_or(0);
    let facts = json!({"done_today": done.iter().take(10).collect::<Vec<_>>(), "not_done_due_today_or_before": slipped.iter().take(8).map(|x| x["title"].clone()).collect::<Vec<_>>(),
        "tomorrow_first": if first.is_object() { json!({"time": first["time"], "title": first["title"], "location": first["location"]}) } else { Value::Null },
        "tomorrow_count": count, "tomorrow_weather": tw});
    let sys = format!("Write in {}, plain text, speaking to the reader as \"you\": ONE thing to prepare tonight for tomorrow, at most 18 words, concrete (pack, charge, print, lay out clothes, set an alarm, umbrella...). Base it only on the facts. Titles may be in another language; keep them as they are.", lang_name(app));
    let from_model = crate::calendar::clean_model_text(&ask(app, "wrapup", &facts, &sys, 80).await.unwrap_or_default(), 200);
    let line = if !from_model.is_empty() { from_model.clone() }
        else if let Some(p) = tw.as_ref().and_then(|w| w["rain"].as_i64()).filter(|p| *p >= 50) { L::t(l, "wrap_umbrella", &[("p", &p.to_string())]) }
        else if first.is_object() { L::t(l, "wrap_first", &[("t", &s(&first, "time")), ("title", &s(&first, "title"))]) }
        else { L::t0(l, "wrap_easy") };
    // each thing once: an errand that's both an event and a to-do is asked about, not "done"
    static NONWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\W+").unwrap());
    let key = |x: &str| -> String { NONWORD.replace_all(&x.to_lowercase(), " ").trim().chars().take(40).collect() };
    let asked: Vec<String> = check.iter().map(|x| key(&s(x, "title"))).collect();
    slipped.retain(|x| !asked.contains(&key(&s(x, "title"))));
    let pending: Vec<String> = asked.iter().cloned().chain(slipped.iter().map(|x| key(&s(x, "title")))).collect();
    done.retain(|x| !pending.contains(&key(x)));
    done.truncate(10);
    json!({"done": done, "check": check.into_iter().take(8).collect::<Vec<_>>(), "slipped": slipped.into_iter().take(8).collect::<Vec<_>>(),
        "tomorrow": first, "tomorrow_count": count, "weather": tw, "prepare": line, "by_model": !from_model.is_empty()})
}

fn owner(app: &App) -> String {
    let n = std::env::var("CERQ_OWNER_NAME").ok().filter(|x| !x.trim().is_empty()).unwrap_or_else(|| app.setting("CERQ_MY_NAME"));
    n.trim().to_lowercase().split(' ').next().unwrap_or("").to_string()
}

/// The next 7 days: how heavy each is, what's coming, what to plan.
async fn week(app: &App) -> Value {
    let l = Lang::of(app);
    let today = now(app).date_naive();
    let items = brief::days_items(app, today, 9).await;
    let days: Vec<Value> = items.iter().skip(1).take(7).map(|(d, ev)| {
        let timed: Vec<&Value> = ev.iter().filter(|e| !e["all_day"].as_bool().unwrap_or(false)).collect();
        let hours = timed.iter().map(|e| (m(&s(e, "end")) - m(&s(e, "time"))).clamp(0, 720)).sum::<i64>() as f64 / 60.0;
        json!({"date": d.to_string(), "dow": L::dow(l, *d), "day": d.day(), "n": timed.len(), "hours": (hours * 10.0).round() / 10.0,
               "first": timed.first().map(|e| s(e, "time")).unwrap_or_default(), "all_day": ev.iter().filter(|e| e["all_day"].as_bool().unwrap_or(false)).take(3).map(|e| e["title"].clone()).collect::<Vec<_>>()})
    }).collect();
    let busy: Vec<Value> = days.iter().filter(|d| d["n"].as_i64().unwrap_or(0) >= 4 || d["hours"].as_f64().unwrap_or(0.0) >= 5.0).map(|d| d["date"].clone()).collect();
    let up = brief::upcoming(l, today, &items);
    let me = owner(app);
    let facts = json!({"days": days.iter().map(|d| json!({"dow": d["dow"], "day": d["day"], "n": d["n"], "hours": d["hours"], "first": d["first"], "all_day": d["all_day"]})).collect::<Vec<_>>(),
        "coming_up": up.iter().map(|u| json!({"kind": u["kind"], "when": u["when"],
            "title": if u["kind"] == "birthday" && !me.is_empty() && s(u, "title").to_lowercase().contains(&me) { L::t0(l, "own_birthday") } else { s(u, "title") }})).collect::<Vec<_>>()});
    let sys = format!("Write in {}, plain text, speaking to the reader as \"you\", at most 35 words: how next week looks (which days are heavy or light) and what to plan now: gifts for birthdays, trips for long weekends, payments, prep for heavy days. For the reader's own birthday, no gift: maybe plan something for themselves. Use only the facts. Titles may be in another language; keep them as they are.", lang_name(app));
    let line = crate::calendar::clean_model_text(&ask(app, "week", &facts, &sys, 120).await.unwrap_or_default(), 400);
    json!({"days": days, "busy": busy, "coming_up": up, "text": line, "by_model": !line.is_empty()})
}

fn week_day(app: &App) -> u32 {
    ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"].iter().position(|d| *d == app.setting("CERQ_WEEK_DAY").to_lowercase()).unwrap_or(6) as u32
}

static CACHE: LazyLock<Mutex<Option<(i64, Value)>>> = LazyLock::new(Default::default);

/// Everything for a read (90 s cache unless fresh).
pub async fn get(app: &App, fresh: bool) -> Value {
    if !fresh { if let Some((t, v)) = CACHE.lock().await.as_ref() { if crate::util::now_ts() - t < 90 { return v.clone(); } } }
    let b = brief::get(app).await;
    let (open, done_today) = todos(app).await;
    let t = now(app);
    let wd = week_day(app);
    let dow = t.weekday().num_days_from_monday();
    let v = json!({"ts": crate::util::now_ts(), "part": brief::part(t.hour()), "todos": open.iter().take(20).collect::<Vec<_>>(),
        "gaps": gaps(app, &b, &open).await, "leave": leave(app, &b).await, "reminders": reminders(app).await,
        "replies": b["mail"].get("waiting").cloned().unwrap_or(json!([])),
        "wrapup": if t.hour() >= 18 || t.hour() < 4 { wrapup(app, &b, &open, &done_today).await } else { Value::Null },
        "week": if [(wd + 6) % 7, wd, (wd + 1) % 7].contains(&dow) { week(app).await } else { Value::Null }});
    *CACHE.lock().await = Some((crate::util::now_ts(), v.clone()));
    v
}

/// Each push goes out once (kv "today.assist".pushed).
async fn once(app: &App, key: &str) -> bool {
    let k = key.to_string();
    with_state(app, |st| if st["pushed"].get(&k).is_some() { (false, false) } else { st["pushed"][&k] = json!(crate::util::now_ts()); (true, true) }).await
}

/// Every minute: the wrap-up, the plan for today, the week ahead, leave-by.
pub async fn tick(app: &App) -> anyhow::Result<()> {
    let l = Lang::of(app);
    let t = now(app);
    let mins = (t.hour() * 60 + t.minute()) as i64;
    let day = t.date_naive().to_string();
    let push = |p: Value| async move { crate::push::send_all(app, p).await };
    let wrap = hm_env(app, "CERQ_WRAPUP_AT", "21:00");
    if (wrap..wrap + 120).contains(&mins) && once(app, &format!("wrapup:{day}")).await {
        let v = get(app, true).await;
        let w = if v["wrapup"].is_object() { v["wrapup"].clone() } else { let (o, d) = todos(app).await; wrapup(app, &brief::get(app).await, &o, &d).await };
        let mut bits = vec![L::t(l, "push_done", &[("n", &w["done"].as_array().map(|a| a.len()).unwrap_or(0).to_string())])];
        let sl = w["slipped"].as_array().map(|a| a.len()).unwrap_or(0);
        if sl > 0 { bits.push(L::t(l, "push_slipped", &[("n", &sl.to_string())])); }
        if w["tomorrow"].is_object() { bits.push(L::t(l, "push_tomorrow", &[("t", &s(&w["tomorrow"], "time")), ("title", &s(&w["tomorrow"], "title"))])); }
        push(json!({"title": format!("{} · {}", L::t0(l, "push_tonight"), bits.join(" · ").chars().take(80).collect::<String>()), "body": s(&w, "prepare").chars().take(240).collect::<String>(), "tag": "cerq-wrapup", "url": "/?open=morning"})).await;
    }
    let at = hm_env(app, "CERQ_BRIEF_AT", "07:00");
    if (at + 5..(at / 60 + 3) * 60).contains(&mins) && s(&brief::get(app).await, "day") == day && once(app, &format!("plan:{day}")).await {
        let plan: Vec<Value> = get(app, true).await["gaps"].as_array().cloned().unwrap_or_default().into_iter().filter(|g| g["todo"].is_object()).collect();
        if !plan.is_empty() {
            let list = plan.iter().take(3).map(|g| format!("{} {}", s(g, "from"), s(&g["todo"], "title"))).collect::<Vec<_>>().join(" · ");
            push(json!({"title": format!("{}: {}", L::t0(l, "push_plan"), list.chars().take(90).collect::<String>()), "body": L::t0(l, "push_plan_body"), "tag": "cerq-plan", "url": "/?open=morning"})).await;
        }
    }
    let wk = hm_env(app, "CERQ_WEEK_AT", "19:00");
    if t.weekday().num_days_from_monday() == week_day(app) && (wk..(wk / 60 + 3) * 60).contains(&mins) && once(app, &format!("week:{day}")).await {
        let w = week(app).await;
        let body = if s(&w, "text").is_empty() { w["days"].as_array().map(|a| a.iter().map(|d| format!("{} {}", s(d, "dow"), d["n"])).collect::<Vec<_>>().join(", ")).unwrap_or_default() } else { s(&w, "text") };
        push(json!({"title": L::t0(l, "push_week"), "body": body.chars().take(240).collect::<String>(), "tag": "cerq-week", "url": "/?open=morning"})).await;
    }
    let b = brief::get(app).await;
    for x in leave(app, &b).await {
        let left = m(&s(&x, "leave")) - mins;
        if (0..=5).contains(&left) && once(app, &format!("leave:{day}:{}:{}", s(&x, "time"), s(&x, "title"))).await {
            let body = L::t(l, "push_leave_body", &[("t", &s(&x, "time")), ("place", &s(&x, "location")), ("m", &x["travel"].to_string())]) + &if x["rain"].as_bool().unwrap_or(false) { L::t0(l, "push_rain") } else { String::new() };
            push(json!({"title": L::t(l, "push_leave", &[("t", &s(&x, "leave")), ("title", &s(&x, "title"))]), "body": body, "tag": "cerq-leave", "url": "/?open=morning"})).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn free_gaps_from_now() {
        let free = vec![serde_json::json!({"from": "08:00", "to": "10:00", "min": 120}), serde_json::json!({"from": "12:00", "to": "12:40", "min": 40}), serde_json::json!({"from": "18:00", "to": "22:00", "min": 240})];
        // at 09:40 the first slot has 15 min left (dropped), 12:00–12:40 stays, the evening stays
        let got = super::free_slots(&free, true, 9 * 60 + 40, 480, 1320, "2026-11-18");
        let v: Vec<(String, String)> = got.iter().map(|g| (super::s(g, "from"), super::s(g, "to"))).collect();
        assert_eq!(v, [("12:00".into(), "12:40".into()), ("18:00".into(), "22:00".into())]);
        // a free day: the rest of it, from now + 5 min
        let got = super::free_slots(&[], false, 14 * 60, 480, 1320, "2026-11-18");
        assert_eq!((super::s(&got[0], "from"), got[0]["min"].as_i64()), ("14:05".into(), Some(475)));
    }
}
