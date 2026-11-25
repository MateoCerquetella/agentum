//! iCalendar (RFC 5545) for the sync: just enough to read VEVENTs the way
//! Odysseus' caldav_sync stored them (timed → naive UTC + is_utc, floating →
//! naive local, all-day → dates) and to write one back.
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

/// One content line: NAME;PARAM=V:value (value still escaped).
#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}
impl Prop {
    pub fn param(&self, k: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

/// Unfold continuation lines (CRLF + space/tab) into logical lines.
/// Trailing blanks of the last physical line of each are dropped (iCloud
/// writes "SUMMARY:Gym "), as python-caldav's fix-up did, so the stored text
/// matches what Odysseus stored.
pub fn unfold(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut piece = 0; // where the last physical line starts in out.last()
    let trim = |out: &mut Vec<String>, piece: usize| if let Some(l) = out.last_mut() {
        let keep = piece + l[piece..].trim_end_matches([' ', '\t']).len();
        l.truncate(keep);
    };
    for raw in s.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = out.last_mut() { piece = last.len(); last.push_str(rest); continue; }
        }
        if !line.is_empty() { trim(&mut out, piece); piece = 0; out.push(line.to_string()); }
    }
    trim(&mut out, piece);
    out
}

pub fn parse_line(line: &str) -> Option<Prop> {
    // The name/params part ends at the first ':' outside double quotes.
    let (mut in_q, mut split) = (false, None);
    for (i, c) in line.char_indices() {
        match c { '"' => in_q = !in_q, ':' if !in_q => { split = Some(i); break; } _ => {} }
    }
    let i = split?;
    let (head, value) = (&line[..i], &line[i + 1..]);
    let mut parts = Vec::new();
    let (mut cur, mut in_q) = (String::new(), false);
    for c in head.chars() {
        match c { '"' => in_q = !in_q, ';' if !in_q => parts.push(std::mem::take(&mut cur)), _ => cur.push(c) }
    }
    parts.push(cur);
    let name = parts.first()?.to_ascii_uppercase();
    let params = parts[1..].iter().filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_ascii_uppercase(), v.to_string()))).collect();
    Some(Prop { name, params, value: value.to_string() })
}

pub fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() { Some('n') | Some('N') => out.push('\n'), Some(x) => out.push(x), None => {} }
        } else { out.push(c); }
    }
    out
}
pub fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace('\n', "\\n")
}

/// A component's raw text plus its own properties (sub-components such as
/// VALARM are skipped; their properties never leak into the parent).
#[derive(Debug, Clone)]
pub struct Comp {
    pub kind: String,
    pub props: Vec<Prop>,
    /// The BEGIN…END lines as they came (to copy a component back verbatim).
    pub raw: String,
}
impl Comp {
    pub fn get(&self, name: &str) -> Option<&Prop> { self.props.iter().find(|p| p.name == name) }
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a { self.props.iter().filter(move |p| p.name == name) }
    pub fn text(&self, name: &str) -> String { self.get(name).map(|p| unescape(&p.value)).unwrap_or_default() }
}

/// The top-level components inside VCALENDAR (VEVENT, VTIMEZONE, VTODO…).
pub fn components(ics: &str) -> Vec<Comp> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur: Option<Comp> = None;
    for line in unfold(ics) {
        let up = line.to_ascii_uppercase();
        if let Some(k) = up.strip_prefix("BEGIN:") {
            depth += 1;
            if depth == 2 { cur = Some(Comp { kind: k.trim().to_string(), props: vec![], raw: String::new() }); }
        }
        if let Some(c) = cur.as_mut() {
            c.raw.push_str(&line);
            c.raw.push_str("\r\n");
            if depth == 2 && !up.starts_with("BEGIN:") && !up.starts_with("END:") {
                if let Some(p) = parse_line(&line) { c.props.push(p); }
            }
        }
        if up.starts_with("END:") {
            if depth == 2 { if let Some(c) = cur.take() { out.push(c); } }
            depth = depth.saturating_sub(1);
        }
    }
    out
}

/// A DTSTART/DTEND/EXDATE/RECURRENCE-ID value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum When {
    Date(NaiveDate),
    /// No zone: the reader's wall clock.
    Floating(NaiveDateTime),
    /// An instant (from Z or a TZID), as naive UTC.
    Utc(NaiveDateTime),
}
impl When {
    /// (naive value as stored, is_utc, all_day) — caldav_sync._to_utc_naive.
    pub fn stored(self) -> (NaiveDateTime, bool, bool) {
        match self {
            When::Date(d) => (d.and_hms_opt(0, 0, 0).unwrap(), false, true),
            When::Floating(t) => (t, false, false),
            When::Utc(t) => (t, true, false),
        }
    }
}

/// Windows zone names that show up in invites from Outlook/Exchange.
const WINDOWS_TZ: &[(&str, &str)] = &[
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"), ("SA Eastern Standard Time", "America/Cayenne"),
    ("E. South America Standard Time", "America/Sao_Paulo"), ("Pacific SA Standard Time", "America/Santiago"),
    ("Montevideo Standard Time", "America/Montevideo"), ("Eastern Standard Time", "America/New_York"),
    ("Central Standard Time", "America/Chicago"), ("Mountain Standard Time", "America/Denver"),
    ("Pacific Standard Time", "America/Los_Angeles"), ("GMT Standard Time", "Europe/London"),
    ("W. Europe Standard Time", "Europe/Berlin"), ("Romance Standard Time", "Europe/Paris"),
    ("Central Europe Standard Time", "Europe/Budapest"), ("Central European Standard Time", "Europe/Warsaw"),
    ("UTC", "UTC"), ("Greenwich Standard Time", "Atlantic/Reykjavik"),
];

/// A TZID to a zone: IANA names (also "/mozilla.org/…/Europe/Berlin"),
/// common Windows names, else the user's own zone.
pub fn zone(tzid: &str, fallback: Tz) -> Tz {
    let t = tzid.trim().trim_matches('"');
    if let Ok(z) = t.parse::<Tz>() { return z; }
    if let Some((_, iana)) = WINDOWS_TZ.iter().find(|(w, _)| w.eq_ignore_ascii_case(t)) { return iana.parse().unwrap_or(fallback); }
    let segs: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
    for n in [3, 2] {
        if segs.len() >= n { if let Ok(z) = segs[segs.len() - n..].join("/").parse::<Tz>() { return z; } }
    }
    fallback
}

/// Local wall time → UTC (the earlier instant when ambiguous; skipped
/// hours move forward).
pub fn local_to_utc(tz: Tz, t: NaiveDateTime) -> NaiveDateTime {
    match tz.from_local_datetime(&t) {
        chrono::LocalResult::Single(d) | chrono::LocalResult::Ambiguous(d, _) => d.naive_utc(),
        chrono::LocalResult::None => tz.from_local_datetime(&(t + chrono::Duration::hours(1))).earliest().map(|d| d.naive_utc()).unwrap_or(t),
    }
}
pub fn at_local(tz: Tz, t: NaiveDateTime) -> DateTime<Tz> { Utc.from_utc_datetime(&local_to_utc(tz, t)).with_timezone(&tz) }

fn parse_dt(v: &str) -> Option<NaiveDateTime> {
    let v = v.trim().trim_end_matches(['Z', 'z']);
    NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok().or_else(|| NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M").ok())
}

/// The values of a date/date-time property (EXDATE may list several).
pub fn whens(p: &Prop, fallback: Tz) -> Vec<When> {
    let is_date = p.param("VALUE").map(|v| v.eq_ignore_ascii_case("DATE")).unwrap_or(false);
    p.value.split(',').filter_map(|v| {
        let v = v.trim();
        if is_date || (v.len() == 8 && v.bytes().all(|b| b.is_ascii_digit())) {
            return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(When::Date);
        }
        let t = parse_dt(v)?;
        if v.ends_with(['Z', 'z']) { return Some(When::Utc(t)); }
        match p.param("TZID") { Some(id) => Some(When::Utc(local_to_utc(zone(id, fallback), t))), None => Some(When::Floating(t)) }
    }).collect()
}
pub fn when(p: &Prop, fallback: Tz) -> Option<When> { whens(p, fallback).into_iter().next() }

/// recurrence_exdates as Odysseus wrote it (Python's json.dumps spacing).
pub fn ex_json(keys: &[String]) -> String {
    format!("[{}]", keys.iter().map(|k| serde_json::to_string(k).unwrap()).collect::<Vec<_>>().join(", "))
}

/// The key a date uses in recurrence_exdates and occurrence ids.
pub fn occ_key(t: NaiveDateTime, all_day: bool) -> String {
    if all_day { t.format("%Y-%m-%d").to_string() } else { t.format("%Y-%m-%dT%H:%M").to_string() }
}

/// RRULE parts in icalendar's canonical order (what Odysseus stored), so a
/// re-sync of an unchanged series doesn't look like an edit.
pub fn normalize_rrule(v: &str) -> String {
    const ORDER: &[&str] = &["RSCALE", "FREQ", "UNTIL", "COUNT", "INTERVAL", "BYSECOND", "BYMINUTE", "BYHOUR", "BYDAY", "BYWEEKDAY",
        "BYMONTHDAY", "BYYEARDAY", "BYWEEKNO", "BYMONTH", "BYSETPOS", "WKST", "SKIP"];
    let mut parts: Vec<(String, String)> = v.split(';').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().to_string())).collect();
    parts.sort_by_key(|(k, _)| ORDER.iter().position(|o| o == k).unwrap_or(ORDER.len()));
    parts.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";")
}

/// A VEVENT as a calendar_events row.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub dtstart: NaiveDateTime,
    pub dtend: NaiveDateTime,
    pub all_day: bool,
    pub is_utc: bool,
    pub rrule: String,
    /// JSON list or None (as Odysseus stored it).
    pub exdates: Option<String>,
    pub status: String,
    /// Who's in it: "Name <email>" (the organizer first). Kept in calendar_people, not in the event row.
    pub people: Vec<String>,
}

/// ORGANIZER and ATTENDEE as "Name <email>", once each.
fn people_of(c: &Comp) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for p in c.all("ORGANIZER").chain(c.all("ATTENDEE")) {
        let v = p.value.trim();
        let mail = if v.len() > 7 && v[..7].eq_ignore_ascii_case("mailto:") { &v[7..] } else { v };
        if !mail.contains('@') { continue; }
        let name = p.param("CN").unwrap_or("").trim_matches('"').trim();
        let who = if name.is_empty() || name.eq_ignore_ascii_case(mail) { mail.to_lowercase() } else { format!("{name} <{}>", mail.to_lowercase()) };
        if !out.iter().any(|x| x.contains(&mail.to_lowercase())) { out.push(who); }
    }
    out
}

/// Every VEVENT of a calendar object. An edited occurrence (RECURRENCE-ID)
/// becomes its own event "<uid>_<stamp>" and its date an EXDATE of the
/// series, as cerq_calendar.split_overrides did, so the series still repeats.
pub fn parse_events(ics: &str, tz: Tz) -> Vec<Event> {
    let comps: Vec<Comp> = components(ics).into_iter().filter(|c| c.kind == "VEVENT").collect();
    let mut out: Vec<Event> = Vec::new();
    let mut overridden: Vec<(String, When)> = Vec::new();
    for c in &comps {
        let Some(start) = c.get("DTSTART").and_then(|p| when(p, tz)) else { continue };
        let mut uid = c.text("UID");
        if uid.is_empty() { uid = uuid::Uuid::new_v4().to_string(); }
        if let Some(rid) = c.get("RECURRENCE-ID") {
            if let Some(w) = when(rid, tz) { overridden.push((uid.clone(), w)); }
            uid = format!("{uid}_{}", rid.value.trim().trim_end_matches(['Z', 'z']));
        }
        let (dtstart, is_utc, all_day) = start.stored();
        let mut dtend = match c.get("DTEND").and_then(|p| when(p, tz)) {
            Some(e) => e.stored().0,
            None => match c.get("DURATION").and_then(|p| duration(&p.value)) {
                Some(d) => dtstart + d,
                None => dtstart + if all_day { chrono::Duration::days(1) } else { chrono::Duration::hours(1) },
            },
        };
        if dtend <= dtstart { dtend = dtstart + if all_day { chrono::Duration::days(1) } else { chrono::Duration::hours(1) }; }
        let rrule = c.get("RRULE").map(|p| normalize_rrule(&p.value)).unwrap_or_default();
        let mut ex: Vec<String> = c.all("EXDATE").flat_map(|p| whens(p, tz)).map(|w| exdate_key(w, all_day)).collect();
        ex.sort(); ex.dedup();
        out.push(Event {
            uid, summary: c.text("SUMMARY"), description: c.text("DESCRIPTION"), location: c.text("LOCATION"),
            dtstart, dtend, all_day, is_utc, rrule,
            exdates: if ex.is_empty() { None } else { Some(ex_json(&ex)) },
            status: { let s = c.text("STATUS").to_lowercase(); if s.is_empty() { "confirmed".into() } else { s } },
            people: people_of(c),
        });
    }
    for (uid, w) in overridden {
        if let Some(m) = out.iter_mut().find(|e| e.uid == uid && !e.rrule.is_empty()) {
            let mut ex: Vec<String> = m.exdates.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
            ex.push(exdate_key(w, m.all_day));
            ex.sort(); ex.dedup();
            m.exdates = Some(ex_json(&ex));
        }
    }
    out
}

/// cerq_calendar.exdates_json: "YYYY-MM-DD" for all-day series, else the
/// UTC (or floating) "YYYY-MM-DDTHH:MM".
pub fn exdate_key(w: When, all_day: bool) -> String {
    match w {
        When::Date(d) => d.format("%Y-%m-%d").to_string(),
        When::Floating(t) | When::Utc(t) => occ_key(t, all_day),
    }
}

/// An ISO 8601 duration (P1D, PT1H30M, -PT15M).
pub fn duration(v: &str) -> Option<chrono::Duration> {
    let v = v.trim();
    let (neg, v) = match v.strip_prefix('-') { Some(r) => (true, r), None => (false, v.strip_prefix('+').unwrap_or(v)) };
    let v = v.strip_prefix('P')?;
    let (mut secs, mut num, mut in_time) = (0i64, String::new(), false);
    for c in v.chars() {
        match c {
            'T' => in_time = true,
            d if d.is_ascii_digit() => num.push(d),
            u => {
                let n: i64 = num.parse().ok()?; num.clear();
                secs += n * match (u, in_time) { ('W', _) => 604800, ('D', _) => 86400, ('H', true) => 3600, ('M', true) => 60, ('S', true) => 1, _ => return None };
            }
        }
    }
    Some(chrono::Duration::seconds(if neg { -secs } else { secs }))
}

/// Fold a content line at 75 octets (on char boundaries).
fn fold(line: &str, out: &mut String) {
    let mut n = 0;
    for c in line.chars() {
        let l = c.len_utf8();
        if n + l > 75 { out.push_str("\r\n "); n = 1; }
        out.push(c);
        n += l;
    }
    out.push_str("\r\n");
}

fn fmt_time(name: &str, t: NaiveDateTime, all_day: bool, is_utc: bool) -> String {
    if all_day { format!("{name};VALUE=DATE:{}", t.format("%Y%m%d")) }
    else if is_utc { format!("{name}:{}", t.format("%Y%m%dT%H%M%SZ")) }
    else { format!("{name}:{}", t.format("%Y%m%dT%H%M%S")) }
}

/// The master VEVENT of a local row (caldav_writeback.build_event_ical).
pub fn build_vevent(e: &Event) -> String {
    let mut out = String::new();
    let mut l = |s: String| fold(&s, &mut out);
    l("BEGIN:VEVENT".into());
    l(format!("UID:{}", e.uid));
    l(format!("DTSTAMP:{}", Utc::now().format("%Y%m%dT%H%M%SZ")));
    l(format!("SUMMARY:{}", escape(&e.summary)));
    if !e.description.is_empty() { l(format!("DESCRIPTION:{}", escape(&e.description))); }
    if !e.location.is_empty() { l(format!("LOCATION:{}", escape(&e.location))); }
    l(fmt_time("DTSTART", e.dtstart, e.all_day, e.is_utc));
    l(fmt_time("DTEND", e.dtend, e.all_day, e.is_utc));
    if !e.rrule.is_empty() { l(format!("RRULE:{}", e.rrule)); }
    let ex: Vec<String> = e.exdates.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    for k in ex {
        let t = if e.all_day { NaiveDate::parse_from_str(&k[..k.len().min(10)], "%Y-%m-%d").ok().map(|d| d.and_hms_opt(0, 0, 0).unwrap()) }
            else { NaiveDateTime::parse_from_str(&k[..k.len().min(16)], "%Y-%m-%dT%H:%M").ok() };
        if let Some(t) = t { l(fmt_time("EXDATE", t, e.all_day, e.is_utc)); }
    }
    if e.status == "cancelled" { l("STATUS:CANCELLED".into()); }
    l("END:VEVENT".into());
    out
}

/// A whole calendar object: our master plus components kept from the
/// server's copy (time zones and edited occurrences, see sync::put_event).
pub fn build_calendar(master: &str, keep: &[&Comp]) -> String {
    let mut s = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//CERQ//Calendar//EN\r\n");
    for c in keep.iter().filter(|c| c.kind == "VTIMEZONE") { s.push_str(&c.raw); }
    s.push_str(master);
    for c in keep.iter().filter(|c| c.kind != "VTIMEZONE") { s.push_str(&c.raw); }
    s.push_str("END:VCALENDAR\r\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    const BA: Tz = chrono_tz::America::Argentina::Buenos_Aires;
    fn dt(s: &str) -> NaiveDateTime { NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap() }

    #[test]
    fn tzid_all_day_and_floating() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Madrid\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Dentist\\, Dr. X\r\nDESCRIPTION:line1\\nline2 that is fol\r\n ded\r\nDTSTART;TZID=Europe/Madrid:20260715T100000\r\nDTEND;TZID=Europe/Madrid:20260715T110000\r\n"
            .to_string() + "BEGIN:VALARM\r\nDESCRIPTION:alarm\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:20261225\r\nDTEND;VALUE=DATE:20261225\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\n"
            + "BEGIN:VEVENT\r\nUID:c\r\nDTSTART:20261101T090000\r\nDURATION:PT30M\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:d\r\nDTSTART:20261101T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let ev = parse_events(&ics, BA);
        assert_eq!(ev.len(), 4);
        assert_eq!((ev[0].summary.as_str(), ev[0].description.as_str()), ("Dentist, Dr. X", "line1\nline2 that is folded"));
        // 10:00 in Madrid (summer, +02:00) is 08:00 UTC
        assert_eq!((ev[0].dtstart, ev[0].dtend, ev[0].is_utc, ev[0].all_day), (dt("2026-07-15 08:00"), dt("2026-07-15 09:00"), true, false));
        // all-day with DTEND == DTSTART gets one day
        assert_eq!((ev[1].dtstart, ev[1].dtend, ev[1].all_day, ev[1].is_utc, ev[1].status.as_str()), (dt("2026-12-25 00:00"), dt("2026-12-26 00:00"), true, false, "cancelled"));
        assert_eq!((ev[2].dtstart, ev[2].dtend, ev[2].is_utc), (dt("2026-11-01 09:00"), dt("2026-11-01 09:30"), false));
        assert_eq!((ev[3].dtend, ev[3].is_utc), (dt("2026-11-01 13:00"), true));
    }

    #[test]
    fn trailing_blanks_like_the_old_app() {
        assert_eq!(unfold("SUMMARY:Gym \r\nDTSTART:1\r\n"), ["SUMMARY:Gym", "DTSTART:1"]);
        assert_eq!(unfold("SUMMARY:x d\r\n ía \r\n \r\nEND:VEVENT"), ["SUMMARY:x día ", "END:VEVENT"]);
    }

    #[test]
    fn overrides_split_and_exdates() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:gym\nDTSTART;TZID=America/Argentina/Buenos_Aires:20261102T190000\nDTEND;TZID=America/Argentina/Buenos_Aires:20261102T200000\nRRULE:BYDAY=MO,FR;FREQ=WEEKLY\nEXDATE;TZID=America/Argentina/Buenos_Aires:20261106T190000,20261109T190000\nEND:VEVENT\nBEGIN:VEVENT\nUID:gym\nRECURRENCE-ID;TZID=America/Argentina/Buenos_Aires:20261113T190000\nDTSTART;TZID=America/Argentina/Buenos_Aires:20261113T200000\nDTEND;TZID=America/Argentina/Buenos_Aires:20261113T210000\nSUMMARY:Gym late\nEND:VEVENT\nEND:VCALENDAR\n";
        let ev = parse_events(ics, BA);
        assert_eq!(ev[0].rrule, "FREQ=WEEKLY;BYDAY=MO,FR");
        assert_eq!(ev[0].exdates.as_deref(), Some(r#"["2026-11-06T22:00", "2026-11-09T22:00", "2026-11-13T22:00"]"#));
        assert_eq!(ev[1].uid, "gym_20261113T190000");
        assert_eq!(ev[1].dtstart, dt("2026-11-13 23:00"));
    }

    #[test]
    fn reads_who_is_in_it() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:r1\r\nDTSTART:20261120T153000Z\r\nSUMMARY:Retrospective\r\n\
ORGANIZER;CN=Ada Lovelace:mailto:Ada@Example.org\r\nATTENDEE;CN=\"Pavel\";PARTSTAT=ACCEPTED:mailto:pavel@example.org\r\n\
ATTENDEE;CN=ada@example.org:mailto:ada@example.org\r\nATTENDEE:mailto:sam@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let e = &parse_events(ics, chrono_tz::UTC)[0];
        assert_eq!(e.people, vec!["Ada Lovelace <ada@example.org>", "Pavel <pavel@example.org>", "sam@example.org"]);
    }
    #[test]
    fn writes_back_what_it_reads() {
        let e = Event { uid: "x@y".into(), summary: "Lunch; with Ana, Bob".into(), description: "a\nb".into(), location: String::new(),
            dtstart: dt("2026-11-20 15:00"), dtend: dt("2026-11-20 16:00"), all_day: false, is_utc: true, rrule: "FREQ=WEEKLY".into(),
            exdates: Some(r#"["2026-11-27T15:00"]"#.into()), status: "confirmed".into(), people: vec![] };
        let ics = build_calendar(&build_vevent(&e), &[]);
        assert!(ics.contains("EXDATE:20261127T150000Z"));
        let back = parse_events(&ics, BA).remove(0);
        assert_eq!(back, e);
        assert_eq!(zone("/mozilla.org/20070129_1/Europe/Berlin", BA), chrono_tz::Europe::Berlin);
        assert_eq!(zone("Argentina Standard Time", chrono_tz::UTC), BA);
        assert_eq!(duration("-PT15M"), Some(chrono::Duration::minutes(-15)));
    }
}
