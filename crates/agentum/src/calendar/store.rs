//! Reading the calendar tables: rows, occurrences in the user's zone, what
//! kind of thing an event is, the calendar list and the done marks.
use super::{ics, recur, Occ};
use crate::state::App;
use chrono::{Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use rusqlite::{params, OptionalExtension, Row};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::LazyLock};

/// Stored format of dtstart/dtend (what Odysseus wrote).
pub const FMT: &str = "%Y-%m-%d %H:%M:%S%.6f";
pub fn fmt(t: NaiveDateTime) -> String { t.format(FMT).to_string() }
pub fn now_str() -> String { fmt(Utc::now().naive_utc()) }
pub fn parse_stored(s: &str) -> NaiveDateTime {
    let s = s.replace('T', " ");
    NaiveDateTime::parse_from_str(&s[..s.len().min(19)], "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDate::parse_from_str(&s[..s.len().min(10)], "%Y-%m-%d").map(|d| d.and_hms_opt(0, 0, 0).unwrap()))
        .unwrap_or_default()
}

/// One calendar_events row with its calendar.
#[derive(Debug, Clone)]
pub struct EvRow {
    pub ev: ics::Event,
    pub calendar_id: String,
    pub cal_name: String,
    pub cal_color: String,
    pub cal_source: String,
    pub cal_read_only: bool,
    pub color: String,
    pub remote_href: Option<String>,
    pub sync_pending: Option<String>,
}
impl EvRow {
    pub fn exdates(&self) -> Vec<String> { self.ev.exdates.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default() }
    pub fn remote(&self) -> bool { self.cal_source != "local" }
}

pub const SELECT: &str = "SELECT e.uid, e.calendar_id, e.summary, e.description, e.location, e.dtstart, e.dtend, e.all_day, e.is_utc, e.rrule,
    e.recurrence_exdates, e.color, e.status, e.remote_href, e.sync_pending, c.name, c.color, c.source, c.read_only
    FROM calendar_events e JOIN calendars c ON c.id = e.calendar_id";

pub fn row(r: &Row) -> rusqlite::Result<EvRow> {
    let o = |i: usize| -> rusqlite::Result<String> { Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default()) };
    let name = o(15)?;
    Ok(EvRow {
        ev: ics::Event {
            uid: r.get(0)?, summary: o(2)?, description: o(3)?, location: o(4)?,
            dtstart: parse_stored(&o(5)?), dtend: parse_stored(&o(6)?),
            all_day: r.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0, is_utc: r.get::<_, Option<i64>>(8)?.unwrap_or(0) != 0,
            rrule: o(9)?, exdates: r.get::<_, Option<String>>(10)?.filter(|s| !s.is_empty() && s != "[]"),
            status: { let s = o(12)?; if s.is_empty() { "confirmed".into() } else { s } },
            people: vec![],
        },
        calendar_id: o(1)?, cal_read_only: r.get::<_, Option<i64>>(18)?.unwrap_or(0) != 0 || SPECIAL.is_match(&name),
        cal_name: name, cal_color: o(16)?, cal_source: { let s = o(17)?; if s.is_empty() { "local".into() } else { s } },
        color: o(11)?, remote_href: r.get::<_, Option<String>>(13)?.filter(|s| !s.is_empty()), sync_pending: r.get(14)?,
    })
}

pub async fn get_row(app: &App, uid: &str) -> anyhow::Result<Option<EvRow>> {
    let uid = uid.to_string();
    app.db.call(move |c| c.query_row(&format!("{SELECT} WHERE e.uid = ?1"), [uid], row).optional()).await
}

// ---- kind (brief.py)
static HOLIDAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)festivo|holiday|feriado").unwrap());
static BIRTHDAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)cumplea|birthday").unwrap());
pub static MONEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(pagar|cancelar|renovar|vence|vencimiento|abonar)\b").unwrap());
/// Holiday and birthday calendars are the server's, never yours to edit.
static SPECIAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)festivo|holiday|feriado|cumplea|birthday").unwrap());
const PERSONAL_ACCOUNTS: &[&str] = &["icloud", "gmail", "outlook", "personal", ""];

/// The account part of a "Calendario · iCloud" label.
pub fn account_of(label: &str) -> &str { label.rsplit_once(" · ").map(|(_, a)| a.trim()).unwrap_or("") }

/// holiday | birthday | money | work | personal, from the calendar and the
/// title. Work accounts are CERQ_WORK_CALENDARS; without it, anything but
/// iCloud/Gmail/Outlook/CERQ's own is work.
pub fn kind(title: &str, cal: &str, work: &[String]) -> &'static str {
    if HOLIDAY.is_match(cal) { return "holiday"; }
    if BIRTHDAY.is_match(cal) || title.to_lowercase().starts_with("cumple") { return "birthday"; }
    if MONEY.is_match(title) { return "money"; }
    let account = account_of(cal).to_lowercase();
    if !work.is_empty() { return if work.contains(&account) { "work" } else { "personal" }; }
    if PERSONAL_ACCOUNTS.contains(&account.as_str()) { "personal" } else { "work" }
}
pub fn work_accounts(app: &App) -> Vec<String> {
    app.setting("CERQ_WORK_CALENDARS").split(',').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()
}

/// A stored time to the user's zone.
pub fn to_local(t: NaiveDateTime, all_day: bool, is_utc: bool, tz: Tz) -> chrono::DateTime<Tz> {
    if is_utc && !all_day { Utc.from_utc_datetime(&t).with_timezone(&tz) } else { ics::at_local(tz, t) }
}

pub fn occ_of(r: &EvRow, start: NaiveDateTime, recurring: bool, tz: Tz, work: &[String]) -> Occ {
    let e = &r.ev;
    let end = start + (e.dtend - e.dtstart).max(Duration::zero());
    let title = { let t = e.summary.trim(); if t.is_empty() { "(untitled)".to_string() } else { t.to_string() } };
    Occ {
        id: if recurring { format!("{}::{}", e.uid, ics::occ_key(start, e.all_day)) } else { e.uid.clone() },
        uid: e.uid.clone(), calendar_id: r.calendar_id.clone(), calendar: r.cal_name.clone(),
        kind: kind(&title, &r.cal_name, work).to_string(), title,
        location: e.location.trim().to_string(), description: e.description.clone(),
        start: to_local(start, e.all_day, e.is_utc, tz), end: to_local(end, e.all_day, e.is_utc, tz),
        all_day: e.all_day, recurring,
        color: if r.color.is_empty() { r.cal_color.clone() } else { r.color.clone() },
        read_only: r.cal_read_only,
    }
}

/// Occurrence starts of one row in [lo, hi) (its own clock).
pub fn starts_of(r: &EvRow, lo: NaiveDateTime, hi: NaiveDateTime, tz: Tz) -> Vec<NaiveDateTime> {
    let e = &r.ev;
    let dur = (e.dtend - e.dtstart).max(Duration::zero());
    if !e.rrule.trim().is_empty() {
        let ex: HashSet<String> = r.exdates().into_iter().collect();
        if let Some(v) = recur::expand(&e.rrule, e.dtstart, dur, e.all_day, e.is_utc, &ex, lo, hi, tz, 1000) { return v; }
        tracing::debug!("unreadable rrule {:?} on {}", e.rrule, e.uid);
    }
    if e.dtstart < hi && (e.dtend > lo || (e.dtend == e.dtstart && e.dtstart >= lo)) { vec![e.dtstart] } else { vec![] }
}

/// Every occurrence overlapping local days [from, to), sorted by start.
pub async fn occurrences(app: &App, from: NaiveDate, to: NaiveDate) -> anyhow::Result<Vec<Occ>> {
    let tz = app.tz();
    let (lo_l, hi_l) = (from.and_hms_opt(0, 0, 0).unwrap(), to.and_hms_opt(0, 0, 0).unwrap());
    let (lo_u, hi_u) = (ics::local_to_utc(tz, lo_l), ics::local_to_utc(tz, hi_l));
    let (qlo, qhi) = (fmt(lo_l.min(lo_u)), fmt(hi_l.max(hi_u)));
    let rows = app.db.call(move |c| {
        let mut st = c.prepare(&format!("{SELECT} WHERE coalesce(e.status,'') != 'cancelled' AND e.dtstart < ?2 AND (coalesce(e.rrule,'') != '' OR e.dtend > ?1)"))?;
        st.query_map(params![qlo, qhi], row)?.collect::<rusqlite::Result<Vec<_>>>()
    }).await?;
    let work = work_accounts(app);
    let mut out = vec![];
    for r in &rows {
        let (lo, hi) = if r.ev.is_utc && !r.ev.all_day { (lo_u, hi_u) } else { (lo_l, hi_l) };
        let recurring = !r.ev.rrule.trim().is_empty();
        for s in starts_of(r, lo, hi, tz) { out.push(occ_of(r, s, recurring, tz, &work)); }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then(b.all_day.cmp(&a.all_day)).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())));
    Ok(out)
}

/// A moved holiday that holiday calendars disagree on: the date most of them
/// list wins (±4 days), so the 20th and the 23rd don't both show it. Then one
/// holiday per day (the same holiday in three calendars).
pub fn one_holiday(occ: Vec<Occ>) -> Vec<Occ> {
    use std::collections::HashMap;
    let mut per_day: HashMap<NaiveDate, usize> = HashMap::new();
    for o in occ.iter().filter(|o| o.kind == "holiday") { *per_day.entry(o.start.date_naive()).or_default() += 1; }
    let beaten: HashSet<NaiveDate> = per_day.iter().filter(|(d, n)| per_day.iter().any(|(x, m)| x != *d && (*x - **d).num_days().abs() <= 4 && m > n)).map(|(d, _)| *d).collect();
    let mut seen = HashSet::new();
    occ.into_iter().filter(|o| o.kind != "holiday" || (!beaten.contains(&o.start.date_naive()) && seen.insert(o.start.date_naive()))).collect()
}

// ---- calendars
#[derive(Debug, Clone)]
pub struct Cal { pub id: String, pub name: String, pub color: String, pub source: String, pub read_only: bool, pub account_id: String, pub remote_url: String }

pub async fn cals(app: &App) -> anyhow::Result<Vec<Cal>> {
    app.db.call(|c| {
        let mut st = c.prepare("SELECT id, coalesce(name,''), coalesce(color,''), coalesce(source,'local'), coalesce(read_only,0), coalesce(account_id,''), coalesce(remote_url,'') FROM calendars ORDER BY source = 'local', name")?;
        st.query_map([], |r| {
            let name: String = r.get(1)?;
            Ok(Cal { id: r.get(0)?, read_only: r.get::<_, i64>(4)? != 0 || SPECIAL.is_match(&name), name, color: r.get(2)?, source: r.get(3)?, account_id: r.get(5)?, remote_url: r.get(6)? })
        })?.collect()
    }).await
}

/// Where new events go: CERQ_DEFAULT_CALENDAR (a calendar id; an account
/// name like "iCloud" also works, its "Calendar…/Calendario…" first), else
/// the first synced calendar that takes events, else the local Personal one.
pub async fn default_calendar(app: &App) -> anyhow::Result<Cal> {
    let all = cals(app).await?;
    let want = app.setting("CERQ_DEFAULT_CALENDAR").trim().to_string();
    let ok = |c: &&Cal| !c.read_only;
    if !want.is_empty() {
        if let Some(c) = all.iter().filter(ok).find(|c| c.id == want) { return Ok(c.clone()); }
        let named: Vec<&Cal> = all.iter().filter(ok).filter(|c| c.name.ends_with(&format!(" · {want}"))).collect();
        if let Some(c) = named.iter().find(|c| c.name.to_lowercase().starts_with("calend")).or(named.first()) { return Ok((*c).clone()); }
    }
    if let Some(c) = all.iter().filter(ok).find(|c| c.source != "local") { return Ok(c.clone()); }
    if let Some(c) = all.iter().find(|c| c.source == "local") { return Ok(c.clone()); }
    let id = uuid::Uuid::new_v4().to_string();
    let i2 = id.clone();
    app.db.call(move |c| c.execute("INSERT INTO calendars(id,name,color,source,read_only,created_at,updated_at) VALUES(?1,'Personal','#5b8abf','local',0,?2,?2)", params![i2, now_str()])).await?;
    Ok(Cal { id, name: "Personal".into(), color: "#5b8abf".into(), source: "local".into(), read_only: false, account_id: String::new(), remote_url: String::new() })
}

pub async fn calendars_json(app: &App) -> anyhow::Result<Vec<Value>> {
    let def = default_calendar(app).await?.id;
    Ok(cals(app).await?.into_iter().map(|c| {
        let (name, account) = match c.name.rsplit_once(" · ") { Some((n, a)) => (n.to_string(), a.to_string()), None => (c.name.clone(), if c.source == "local" { "CERQ".into() } else { String::new() }) };
        json!({"id": c.id, "name": name, "account": account, "label": c.name, "color": c.color, "source": c.source, "read_only": c.read_only, "default": c.id == def})
    }).collect())
}

// ---- done marks ("<uid>|YYYY-MM-DD", the series uid for repeating events)
pub fn done_key(uid: &str, day: NaiveDate) -> String { format!("{}|{day}", uid.split("::").next().unwrap_or(uid)) }

pub async fn done_keys(app: &App) -> anyhow::Result<HashSet<String>> {
    app.db.call(|c| { let mut st = c.prepare("SELECT key FROM calendar_done")?; st.query_map([], |r| r.get(0))?.collect() }).await
}

pub async fn signature(app: &App) -> String {
    app.db.call(|c| c.query_row(
        "SELECT count(*) || '|' || coalesce(max(updated_at),'') || '|' || (SELECT count(*) FROM calendars) || '|' || (SELECT count(*) || ':' || coalesce(max(ts),0) FROM calendar_done) FROM calendar_events",
        [], |r| r.get::<_, String>(0))).await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds() {
        let none: Vec<String> = vec![];
        assert_eq!(kind("Día de la Soberanía", "Festivos en Argentina · Gmail", &none), "holiday");
        assert_eq!(kind("Ana", "Calendario de cumpleaños · Outlook", &none), "birthday");
        assert_eq!(kind("Cumple de Juan", "Calendar · iCloud", &none), "birthday");
        assert_eq!(kind("Pagar tarjeta", "Calendar · iCloud", &none), "money");
        assert_eq!(kind("Pagaré", "Calendar · iCloud", &none), "personal");
        assert_eq!(kind("Daily Sync", "mcerquetella@goempirical.com · GoEmpirical", &none), "work");
        assert_eq!(kind("Dentist", "Personal", &none), "personal");
        let work = vec!["freebee".to_string()];
        assert_eq!(kind("Standup", "mateo@ridefreebee.com · Freebee", &work), "work");
        assert_eq!(kind("Standup", "x · GoEmpirical", &work), "personal");
    }
}
