//! Local create/update/delete. The local tables are the truth: a change is
//! saved, marked sync_pending for a remote calendar (a delete leaves a
//! tombstone) and written back in the background (sync::push_*), retried by
//! the sync loop until it goes through.
use super::{ics, store::{self, EvRow}};
use crate::{state::App, util::s};
use anyhow::{anyhow, bail};
use chrono::{Duration, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use rusqlite::params;
use serde_json::{json, Value};

/// An event's time span: dates for all-day (end exclusive), else UTC instants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Span { Days(NaiveDate, NaiveDate), Utc(NaiveDateTime, NaiveDateTime) }

/// An ISO time from the API: a date, a time with an offset, or a bare
/// time read as the user's local time. → (UTC instant, local date).
pub fn parse_time(v: &str, tz: Tz) -> Option<(NaiveDateTime, NaiveDate)> {
    let v = v.trim();
    if v.len() == 10 {
        let d = NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()?;
        return Some((ics::local_to_utc(tz, d.and_hms_opt(0, 0, 0)?), d));
    }
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&v.replace(' ', "T")) {
        let u = t.naive_utc();
        return Some((u, chrono::Utc.from_utc_datetime(&u).with_timezone(&tz).date_naive()));
    }
    let v = v.replace(' ', "T");
    let t = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"].iter().find_map(|f| NaiveDateTime::parse_from_str(&v, f).ok())?;
    Some((ics::local_to_utc(tz, t), t.date()))
}

fn span_of(e: &ics::Event, tz: Tz) -> Span {
    if e.all_day { Span::Days(e.dtstart.date(), e.dtend.date()) }
    else if e.is_utc { Span::Utc(e.dtstart, e.dtend) }
    else { Span::Utc(ics::local_to_utc(tz, e.dtstart), ics::local_to_utc(tz, e.dtend)) }
}
fn set_span(e: &mut ics::Event, sp: Span) {
    match sp {
        Span::Days(a, b) => { e.all_day = true; e.is_utc = false; e.dtstart = a.and_hms_opt(0, 0, 0).unwrap(); e.dtend = b.max(a + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap(); }
        Span::Utc(a, b) => { e.all_day = false; e.is_utc = true; e.dtstart = a; e.dtend = if b > a { b } else { a + Duration::hours(1) }; }
    }
}

/// The span after a patch {dtstart?, dtend?, all_day?}. `shift_from` is the
/// occurrence being edited for a whole-series edit: the series then moves
/// by as much as that occurrence did.
fn patched(cur: Span, p: &Value, tz: Tz, shift_from: Option<Span>) -> anyhow::Result<Span> {
    let t = |k: &str| -> anyhow::Result<Option<(NaiveDateTime, NaiveDate)>> {
        match p.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
            Some(v) => parse_time(v, tz).map(Some).ok_or_else(|| anyhow!("{k}: not a date or time")),
            None => Ok(None),
        }
    };
    let (ns, ne) = (t("dtstart")?, t("dtend")?);
    let all_day = p.get("all_day").and_then(|v| v.as_bool()).unwrap_or(matches!(cur, Span::Days(..)));
    let base = shift_from.unwrap_or(cur);
    let target = match (all_day, base) {
        (true, Span::Days(a, b)) => {
            let na = ns.map(|x| x.1).unwrap_or(a);
            Span::Days(na, ne.map(|x| x.1).unwrap_or(na + (b - a)))
        }
        (true, Span::Utc(a, b)) => {
            let la = chrono::Utc.from_utc_datetime(&a).with_timezone(&tz).date_naive();
            let lb = chrono::Utc.from_utc_datetime(&b).with_timezone(&tz).date_naive();
            let na = ns.map(|x| x.1).unwrap_or(la);
            Span::Days(na, ne.map(|x| x.1).unwrap_or(na + (lb - la).max(Duration::zero()) + Duration::days(1)))
        }
        (false, Span::Utc(a, b)) => { let na = ns.map(|x| x.0).unwrap_or(a); Span::Utc(na, ne.map(|x| x.0).unwrap_or(na + (b - a))) }
        (false, Span::Days(a, _)) => {
            let na = ns.map(|x| x.0).ok_or_else(|| anyhow!("a timed event needs dtstart"))
                .or_else(|_| Ok::<_, anyhow::Error>(ics::local_to_utc(tz, a.and_hms_opt(9, 0, 0).unwrap())))?;
            Span::Utc(na, ne.map(|x| x.0).unwrap_or(na + Duration::hours(1)))
        }
    };
    // A whole-series edit made on one occurrence: move the series by the same amount.
    Ok(match (shift_from, cur, target) {
        (Some(Span::Utc(oa, ob)), Span::Utc(a, b), Span::Utc(na, nb)) => Span::Utc(a + (na - oa), b + (nb - ob)),
        (Some(Span::Days(oa, ob)), Span::Days(a, b), Span::Days(na, nb)) => Span::Days(a + (na - oa), b + (nb - ob)),
        (Some(_), _, t) => t,
        (None, _, t) => t,
    })
}

fn apply_fields(e: &mut ics::Event, color: &mut String, p: &Value) {
    if let Some(v) = p.get("summary").and_then(|v| v.as_str()) { e.summary = v.trim().chars().take(500).collect(); }
    if let Some(v) = p.get("description").and_then(|v| v.as_str()) { e.description = v.chars().take(20000).collect(); }
    if let Some(v) = p.get("location").and_then(|v| v.as_str()) { e.location = v.trim().chars().take(500).collect(); }
    if let Some(v) = p.get("color").and_then(|v| v.as_str()) { *color = v.trim().to_string(); }
}

/// An RRULE from the API ("FREQ=WEEKLY;BYDAY=MO" or "RRULE:…"); "" clears it.
fn clean_rrule(v: &str) -> anyhow::Result<String> {
    let v = v.trim();
    let v = v.strip_prefix("RRULE:").unwrap_or(v);
    if v.is_empty() { return Ok(String::new()); }
    let r = ics::normalize_rrule(v);
    if !r.starts_with("FREQ=") || r.contains('\n') { bail!("rrule: not a recurrence rule"); }
    Ok(r)
}

fn insert_sql(c: &rusqlite::Connection, e: &ics::Event, cal_id: &str, color: &str, pending: Option<&str>) -> rusqlite::Result<usize> {
    let now = store::now_str();
    c.execute("INSERT INTO calendar_events(uid,calendar_id,summary,description,location,dtstart,dtend,all_day,is_utc,rrule,recurrence_exdates,color,status,origin,remote_href,remote_etag,sync_pending,created_at,updated_at)
               VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'confirmed',NULL,NULL,NULL,?13,?14,?14)",
        params![e.uid, cal_id, e.summary, e.description, e.location, store::fmt(e.dtstart), store::fmt(e.dtend), e.all_day as i64, e.is_utc as i64,
                e.rrule, e.exdates, if color.is_empty() { None } else { Some(color) }, pending, now])
}

fn update_sql(c: &rusqlite::Connection, e: &ics::Event, color: &str, pending: Option<&str>) -> rusqlite::Result<usize> {
    c.execute("UPDATE calendar_events SET summary=?2,description=?3,location=?4,dtstart=?5,dtend=?6,all_day=?7,is_utc=?8,rrule=?9,recurrence_exdates=?10,color=?11,
               sync_pending=coalesce(sync_pending, ?12),updated_at=?13 WHERE uid=?1",
        params![e.uid, e.summary, e.description, e.location, store::fmt(e.dtstart), store::fmt(e.dtend), e.all_day as i64, e.is_utc as i64,
                e.rrule, e.exdates, if color.is_empty() { None } else { Some(color) }, pending, store::now_str()])
}

/// Delete a row; a remote one leaves a tombstone for the write-back.
fn delete_sql(c: &rusqlite::Connection, r: &EvRow) -> rusqlite::Result<()> {
    if r.remote() && r.remote_href.is_some() {
        c.execute("INSERT OR REPLACE INTO calendar_tombstones(uid,calendar_id,remote_href,remote_etag,ts) SELECT uid,calendar_id,remote_href,remote_etag,?2 FROM calendar_events WHERE uid=?1",
            params![r.ev.uid, crate::util::now_ts()])?;
    }
    c.execute("DELETE FROM calendar_events WHERE uid=?1", [&r.ev.uid])?;
    Ok(())
}

async fn target_calendar(app: &App, id: &str) -> anyhow::Result<store::Cal> {
    if id.is_empty() { return store::default_calendar(app).await; }
    let c = store::cals(app).await?.into_iter().find(|c| c.id == id).ok_or_else(|| anyhow!("calendar not found"))?;
    if c.read_only { bail!("{} is read-only", c.name); }
    Ok(c)
}

fn changed(app: &App) { app.emit(json!({"kind": "calendar"})); }

/// The stored event as an occurrence (its first one) plus its rule.
pub async fn event_json(app: &App, uid: &str) -> anyhow::Result<Value> {
    let r = store::get_row(app, uid).await?.ok_or_else(|| anyhow!("event not found"))?;
    let o = store::occ_of(&r, r.ev.dtstart, false, app.tz(), &store::work_accounts(app));
    let mut v = serde_json::to_value(&o)?;
    v["recurring"] = json!(!r.ev.rrule.is_empty());
    v["rrule"] = json!(r.ev.rrule);
    v["recurrence_exdates"] = json!(r.exdates());
    Ok(v)
}

pub async fn create(app: &App, b: &Value) -> anyhow::Result<Value> {
    let summary = s(b, "summary");
    if summary.trim().is_empty() { bail!("summary is required"); }
    let tz = app.tz();
    let start = s(b, "dtstart");
    let (st, sd) = parse_time(&start, tz).ok_or_else(|| anyhow!("dtstart: not a date or time"))?;
    let all_day = b["all_day"].as_bool().unwrap_or(start.trim().len() == 10);
    let end = parse_time(&s(b, "dtend"), tz);
    let span = if all_day { Span::Days(sd, end.map(|x| x.1).filter(|d| *d > sd).unwrap_or(sd + Duration::days(1))) }
        else { Span::Utc(st, end.map(|x| x.0).filter(|t| *t > st).unwrap_or(st + Duration::hours(1))) };
    let cal = target_calendar(app, &s(b, "calendar_id")).await?;
    let mut e = ics::Event { uid: uuid::Uuid::new_v4().to_string(), summary: String::new(), description: String::new(), location: String::new(),
        dtstart: st, dtend: st, all_day, is_utc: !all_day, rrule: clean_rrule(&s(b, "rrule"))?, exdates: None, status: "confirmed".into(), people: vec![] };
    let mut color = String::new();
    apply_fields(&mut e, &mut color, b);
    set_span(&mut e, span);
    let pending = (cal.source != "local").then_some("create");
    let (e2, cid) = (e.clone(), cal.id.clone());
    app.db.call(move |c| insert_sql(c, &e2, &cid, &color, pending)).await?;
    if pending.is_some() { super::sync::push_soon(app, &e.uid); }
    changed(app);
    event_json(app, &e.uid).await
}

/// "<uid>::<key>" → (uid, Some(key)).
fn split_id(id: &str) -> (&str, Option<&str>) {
    match id.split_once("::") { Some((u, k)) if !u.is_empty() && !k.is_empty() => (u, Some(k)), _ => (id, None) }
}
/// An occurrence key back to its start in the row's clock.
fn occ_start(r: &EvRow, key: &str) -> anyhow::Result<NaiveDateTime> {
    let bad = || anyhow!("not an occurrence of this event");
    if r.ev.all_day { NaiveDate::parse_from_str(key.get(..10).ok_or_else(bad)?, "%Y-%m-%d").map(|d| d.and_hms_opt(0, 0, 0).unwrap()).map_err(|_| bad()) }
    else { NaiveDateTime::parse_from_str(key.get(..16).ok_or_else(bad)?, "%Y-%m-%dT%H:%M").map_err(|_| bad()) }
}

pub async fn update(app: &App, id: &str, p: &Value) -> anyhow::Result<Value> {
    let tz = app.tz();
    let (uid, key) = split_id(id);
    let r = store::get_row(app, uid).await?.ok_or_else(|| anyhow!("event not found"))?;
    if r.cal_read_only { bail!("{} is read-only", r.cal_name); }
    let recurring = !r.ev.rrule.is_empty();
    let scope = match s(p, "scope").as_str() { "" => if key.is_some() { "this" } else { "all" }.to_string(), x => x.to_string() };
    let to_cal = s(p, "calendar_id");
    let dur = r.ev.dtend - r.ev.dtstart;

    // One occurrence of a series: it becomes its own event, the series skips that date.
    if let (Some(k), true, "this") = (key, recurring, scope.as_str()) {
        let os = occ_start(&r, k)?;
        let mut one = r.ev.clone();
        one.uid = uuid::Uuid::new_v4().to_string();
        one.rrule.clear();
        one.exdates = None;
        one.dtstart = os;
        one.dtend = os + dur;
        let mut color = r.color.clone();
        apply_fields(&mut one, &mut color, p);
        let sp = patched(span_of(&one, tz), p, tz, None)?;
        set_span(&mut one, sp);
        let cal = if to_cal.is_empty() { store::cals(app).await?.into_iter().find(|c| c.id == r.calendar_id).ok_or_else(|| anyhow!("calendar not found"))? } else { target_calendar(app, &to_cal).await? };
        let mut master = r.ev.clone();
        let mut ex = r.exdates();
        ex.push(ics::occ_key(os, r.ev.all_day));
        ex.sort(); ex.dedup();
        master.exdates = Some(ics::ex_json(&ex));
        let (m2, o2, mc, cid, mp, op) = (master.clone(), one.clone(), r.color.clone(), cal.id.clone(), r.remote().then_some("update"), (cal.source != "local").then_some("create"));
        app.db.call(move |c| { let tx = c.transaction()?; update_sql(&tx, &m2, &mc, mp)?; insert_sql(&tx, &o2, &cid, &color, op)?; tx.commit() }).await?;
        if mp.is_some() { super::sync::push_soon(app, &master.uid); }
        if op.is_some() { super::sync::push_soon(app, &one.uid); }
        changed(app);
        return event_json(app, &one.uid).await;
    }

    let mut e = r.ev.clone();
    let mut color = r.color.clone();
    apply_fields(&mut e, &mut color, p);
    if let Some(v) = p.get("rrule").and_then(|v| v.as_str()) { e.rrule = clean_rrule(v)?; if e.rrule.is_empty() { e.exdates = None; } }
    let shift = match key { Some(k) if recurring => { let os = occ_start(&r, k)?; let mut o = r.ev.clone(); o.dtstart = os; o.dtend = os + dur; Some(span_of(&o, tz)) } _ => None };
    let had_floating = !e.all_day && !e.is_utc;
    let sp = patched(span_of(&e, tz), p, tz, shift)?;
    set_span(&mut e, sp);
    // Skipped dates follow the series: into UTC for a wall-clock series, and
    // by as much as the series moved (they're keyed by occurrence start).
    if e.exdates.is_some() && e.all_day == r.ev.all_day {
        let to_utc = had_floating && e.is_utc;
        let old_start = if to_utc { ics::local_to_utc(tz, r.ev.dtstart) } else { r.ev.dtstart };
        let delta = e.dtstart - old_start;
        let ex: Vec<String> = r.exdates().iter().filter_map(|k| {
            let t = if e.all_day { NaiveDate::parse_from_str(k.get(..10)?, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0)? } else { NaiveDateTime::parse_from_str(k.get(..16)?, "%Y-%m-%dT%H:%M").ok()? };
            Some(ics::occ_key(if to_utc { ics::local_to_utc(tz, t) } else { t } + delta, e.all_day))
        }).collect();
        e.exdates = if ex.is_empty() { None } else { Some(ics::ex_json(&ex)) };
    }

    // Another calendar: re-created there under a new uid, deleted here (both servers follow).
    if !to_cal.is_empty() && to_cal != r.calendar_id {
        let cal = target_calendar(app, &to_cal).await?;
        let old = r.clone();
        e.uid = uuid::Uuid::new_v4().to_string();
        let (e2, cid, op) = (e.clone(), cal.id.clone(), (cal.source != "local").then_some("create"));
        app.db.call(move |c| { let tx = c.transaction()?; delete_sql(&tx, &old)?; insert_sql(&tx, &e2, &cid, &color, op)?; tx.commit() }).await?;
        if r.remote() { super::sync::push_soon(app, &r.ev.uid); }
        if op.is_some() { super::sync::push_soon(app, &e.uid); }
        changed(app);
        return event_json(app, &e.uid).await;
    }

    let (e2, pending) = (e.clone(), r.remote().then_some("update"));
    app.db.call(move |c| update_sql(c, &e2, &color, pending)).await?;
    if pending.is_some() { super::sync::push_soon(app, &e.uid); }
    changed(app);
    event_json(app, &e.uid).await
}

/// Delete an event, or with an occurrence id and scope "this" (the default
/// for occurrence ids) only that date of the series.
pub async fn delete(app: &App, id: &str, scope: &str) -> anyhow::Result<()> {
    let (uid, key) = split_id(id);
    let r = store::get_row(app, uid).await?.ok_or_else(|| anyhow!("event not found"))?;
    if r.cal_read_only { bail!("{} is read-only", r.cal_name); }
    let scope = if scope.is_empty() { if key.is_some() { "this" } else { "all" } } else { scope };
    if let (Some(k), false, "this") = (key, r.ev.rrule.is_empty(), scope) {
        let mut ex = r.exdates();
        ex.push(ics::occ_key(occ_start(&r, k)?, r.ev.all_day));
        ex.sort(); ex.dedup();
        let (ex, uid2, pending) = (ics::ex_json(&ex), uid.to_string(), r.remote().then_some("update"));
        app.db.call(move |c| c.execute("UPDATE calendar_events SET recurrence_exdates=?2, sync_pending=coalesce(sync_pending, ?3), updated_at=?4 WHERE uid=?1",
            params![uid2, ex, pending, store::now_str()])).await?;
        if pending.is_some() { super::sync::push_soon(app, uid); }
    } else {
        let r2 = r.clone();
        app.db.call(move |c| delete_sql(c, &r2)).await?;
        if r.remote() && r.remote_href.is_some() { super::sync::push_soon(app, uid); }
    }
    changed(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const BA: Tz = chrono_tz::America::Argentina::Buenos_Aires;
    fn dt(s: &str) -> NaiveDateTime { NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap() }
    fn d(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    #[test]
    fn times_and_patches() {
        assert_eq!(parse_time("2026-11-20T10:00", BA), Some((dt("2026-11-20 13:00"), d("2026-11-20"))));
        assert_eq!(parse_time("2026-11-20T23:30:00-03:00", BA), Some((dt("2026-11-21 02:30"), d("2026-11-20"))));
        assert_eq!(parse_time("2026-11-20", BA).map(|x| x.1), Some(d("2026-11-20")));
        let cur = Span::Utc(dt("2026-11-20 13:00"), dt("2026-11-20 14:00"));
        // moving the start keeps the length
        assert_eq!(patched(cur, &json!({"dtstart": "2026-11-21T09:00"}), BA, None).unwrap(), Span::Utc(dt("2026-11-21 12:00"), dt("2026-11-21 13:00")));
        // to all-day
        assert_eq!(patched(cur, &json!({"all_day": true}), BA, None).unwrap(), Span::Days(d("2026-11-20"), d("2026-11-21")));
        // a series edited from its occurrence of the 27th moves by one hour
        let occ = Span::Utc(dt("2026-11-27 13:00"), dt("2026-11-27 14:00"));
        assert_eq!(patched(cur, &json!({"dtstart": "2026-11-27T11:00", "dtend": "2026-11-27T12:30"}), BA, Some(occ)).unwrap(),
                   Span::Utc(dt("2026-11-20 14:00"), dt("2026-11-20 15:30")));
        assert_eq!(split_id("abc::2026-11-27T13:00"), ("abc", Some("2026-11-27T13:00")));
        assert_eq!(clean_rrule("RRULE:BYDAY=MO;FREQ=WEEKLY").unwrap(), "FREQ=WEEKLY;BYDAY=MO");
    }
}
