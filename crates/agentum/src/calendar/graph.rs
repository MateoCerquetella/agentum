//! Outlook.com calendars over Microsoft Graph (no CalDAV there), as
//! cerq_calendar.GraphClient: calendars, calendarView for the window
//! (recurring meetings come as their occurrences), and write-back. Events
//! made in CERQ carry their CERQ uid in an extended property.
use super::{dav::RemoteCal, ics::Event};
use anyhow::bail;
use chrono::{Duration, NaiveDateTime};
use serde_json::{json, Value};

pub const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const UID_PROP: &str = "String {7c3f6a1e-2b1d-4c6e-9f0a-1d2e3c4b5a69} Name CerqUid";
const FIELDS: &str = "id,subject,start,end,isAllDay,location,bodyPreview,webLink,changeKey,type,isCancelled,organizer,attendees";

fn expand() -> String { format!("singleValueExtendedProperties($filter=id eq '{UID_PROP}')") }

async fn call(token: &str, method: reqwest::Method, url: &str, query: &[(&str, String)], body: Option<Value>) -> anyhow::Result<Value> {
    let mut r = super::dav::client().request(method.clone(), url).bearer_auth(token).header("Prefer", "outlook.timezone=\"UTC\"");
    if !query.is_empty() { r = r.query(query); }
    if let Some(b) = body { r = r.json(&b); }
    let r = r.send().await?;
    let st = r.status();
    let text = r.text().await.unwrap_or_default();
    if !st.is_success() {
        if method == reqwest::Method::DELETE && st == reqwest::StatusCode::NOT_FOUND { return Ok(json!({})); }
        bail!("Graph {method} {}: {st} {}", url.split('?').next().unwrap_or(url), text.chars().take(200).collect::<String>());
    }
    Ok(if text.is_empty() { json!({}) } else { serde_json::from_str(&text)? })
}

pub async fn calendars(token: &str) -> anyhow::Result<Vec<RemoteCal>> {
    let v = call(token, reqwest::Method::GET, &format!("{GRAPH}/me/calendars"), &[("$top", "100".into())], None).await?;
    Ok(v["value"].as_array().cloned().unwrap_or_default().iter().map(|c| RemoteCal {
        url: format!("{GRAPH}/me/calendars/{}", c["id"].as_str().unwrap_or("")),
        name: c["name"].as_str().unwrap_or("Outlook").to_string(),
        ctag: String::new(),
        color: c["hexColor"].as_str().filter(|h| h.len() == 7).unwrap_or("").to_string(),
        read_only: !c["canEdit"].as_bool().unwrap_or(true),
    }).collect())
}

fn time(v: &Value) -> NaiveDateTime {
    let s = v["dateTime"].as_str().unwrap_or("");
    NaiveDateTime::parse_from_str(&s[..s.len().min(19)], "%Y-%m-%dT%H:%M:%S").unwrap_or_default()
}

fn uid_of(ev: &Value) -> String {
    ev["singleValueExtendedProperties"].as_array().and_then(|ps| ps.iter().find(|p| p["id"].as_str().map(|i| i.eq_ignore_ascii_case(UID_PROP)).unwrap_or(false)))
        .and_then(|p| p["value"].as_str()).filter(|v| !v.is_empty()).map(String::from)
        .unwrap_or_else(|| format!("outlook-{}", ev["id"].as_str().unwrap_or("")))
}

/// A Graph event as a row (+ its href and change key).
pub fn to_event(ev: &Value) -> (Event, String, String) {
    let (s, e) = (time(&ev["start"]), time(&ev["end"]));
    let all_day = ev["isAllDay"].as_bool().unwrap_or(false);
    // An all-day event starts at local midnight, anywhere from 12:00 the day before to 12:00 that day in UTC: +12h finds the day.
    let day = |t: NaiveDateTime| (t + Duration::hours(12)).date().and_hms_opt(0, 0, 0).unwrap();
    let (dtstart, dtend) = if all_day { let a = day(s); (a, day(e).max(a + Duration::days(1))) } else { (s, if e > s { e } else { s + Duration::hours(1) }) };
    let event = Event {
        uid: uid_of(ev), summary: ev["subject"].as_str().unwrap_or("").to_string(),
        description: ev["bodyPreview"].as_str().unwrap_or("").to_string(),
        location: ev["location"]["displayName"].as_str().unwrap_or("").to_string(),
        dtstart, dtend, all_day, is_utc: !all_day, rrule: String::new(), exdates: None, status: "confirmed".into(),
        people: std::iter::once(&ev["organizer"]).chain(ev["attendees"].as_array().into_iter().flatten()).filter_map(|a| {
            let (n, m) = (a["emailAddress"]["name"].as_str().unwrap_or("").trim(), a["emailAddress"]["address"].as_str()?.trim().to_lowercase());
            m.contains('@').then(|| if n.is_empty() || n.eq_ignore_ascii_case(&m) { m.clone() } else { format!("{n} <{m}>") })
        }).fold(vec![], |mut v: Vec<String>, p| { if !v.contains(&p) { v.push(p); } v }),
    };
    (event, format!("{GRAPH}/me/events/{}", ev["id"].as_str().unwrap_or("")), ev["changeKey"].as_str().unwrap_or("").to_string())
}

/// Every event of a calendar in [start, end) (UTC).
pub async fn events(token: &str, cal_url: &str, start: NaiveDateTime, end: NaiveDateTime) -> anyhow::Result<Vec<(Event, String, String)>> {
    let iso = |d: NaiveDateTime| d.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut out = vec![];
    let mut page = call(token, reqwest::Method::GET, &format!("{cal_url}/calendarView"),
        &[("startDateTime", iso(start)), ("endDateTime", iso(end)), ("$top", "200".into()), ("$select", FIELDS.into()), ("$expand", expand())], None).await?;
    loop {
        for ev in page["value"].as_array().cloned().unwrap_or_default() {
            if !ev["isCancelled"].as_bool().unwrap_or(false) { out.push(to_event(&ev)); }
        }
        match page["@odata.nextLink"].as_str() {
            Some(next) => { let next = next.to_string(); page = call(token, reqwest::Method::GET, &next, &[], None).await?; }
            None => break,
        }
    }
    Ok(out)
}

/// cerq_calendar.from_ical: a row as a Graph body (times in UTC).
fn body(e: &Event) -> Value {
    let f = |t: NaiveDateTime| json!({"dateTime": t.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"});
    json!({"subject": e.summary, "isAllDay": e.all_day, "start": f(e.dtstart), "end": f(e.dtend),
           "location": {"displayName": e.location}, "body": {"contentType": "text", "content": e.description}})
}

pub async fn create(token: &str, cal_url: &str, e: &Event) -> anyhow::Result<(String, String)> {
    let mut b = body(e);
    b["singleValueExtendedProperties"] = json!([{"id": UID_PROP, "value": e.uid}]);
    let v = call(token, reqwest::Method::POST, &format!("{cal_url}/events"), &[], Some(b)).await?;
    Ok((format!("{GRAPH}/me/events/{}", v["id"].as_str().unwrap_or("")), v["changeKey"].as_str().unwrap_or("").to_string()))
}

pub async fn update(token: &str, href: &str, e: &Event) -> anyhow::Result<String> {
    let v = call(token, reqwest::Method::PATCH, href, &[], Some(body(e))).await?;
    Ok(v["changeKey"].as_str().unwrap_or("").to_string())
}

pub async fn delete(token: &str, href: &str) -> anyhow::Result<()> {
    call(token, reqwest::Method::DELETE, href, &[], None).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn graph_all_day_and_uid() {
        let ev = serde_json::json!({"id": "AB", "subject": "Feriado", "isAllDay": true,
            "start": {"dateTime": "2026-11-23T03:00:00.0000000"}, "end": {"dateTime": "2026-11-24T03:00:00.0000000"},
            "singleValueExtendedProperties": [{"id": super::UID_PROP, "value": "cerq-1"}]});
        let (e, href, _) = super::to_event(&ev);
        assert_eq!((e.uid.as_str(), e.all_day, e.is_utc), ("cerq-1", true, false));
        assert_eq!(e.dtstart.format("%F %R").to_string(), "2026-11-23 00:00");
        assert_eq!(e.dtend.format("%F").to_string(), "2026-11-24");
        assert!(href.ends_with("/me/events/AB"));
    }
}
