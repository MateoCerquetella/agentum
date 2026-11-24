//! Repeating events, expanded on read with the `rrule` crate.
//!
//! Rows keep Odysseus' clocks: timed events from a zone are naive UTC
//! (is_utc), the rest are wall-clock times. A UTC series is expanded in the
//! user's zone (CERQ_TZ), so "every Monday 22:00" stays on Mondays and on
//! the same hour across DST; wall-clock series are expanded as is. Keys
//! (occurrence ids, recurrence_exdates) stay in the stored clock.
use chrono::{Duration, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use rrule::{RRule, RRuleSet, Unvalidated};
use std::collections::HashSet;

/// UNTIL out of the rule: (rule without it, UNTIL value).
fn split_until(rule: &str) -> (String, Option<String>) {
    let mut until = None;
    let rest: Vec<&str> = rule.split(';').filter(|p| {
        let p = p.trim();
        if p.len() > 6 && p[..6].eq_ignore_ascii_case("UNTIL=") { until = Some(p[6..].to_string()); false } else { !p.is_empty() }
    }).collect();
    (rest.join(";"), until)
}

/// The UNTIL bound in the clock the series is expanded in. A date-only
/// UNTIL includes that whole day.
fn until_in(v: &str, utc_series: bool, tz: Tz) -> Option<NaiveDateTime> {
    let z = v.ends_with(['Z', 'z']);
    let raw = v.trim_end_matches(['Z', 'z']);
    let t = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S").ok()
        .or_else(|| NaiveDate::parse_from_str(raw, "%Y%m%d").ok().map(|d| d.and_hms_opt(23, 59, 59).unwrap()))?;
    Some(match (utc_series, z) {
        (true, true) | (false, false) => t,
        (true, false) => super::ics::local_to_utc(tz, t),
        // A wall-clock series with a UTC bound: compare as wall-clock, like Odysseus (it dropped the Z).
        (false, true) => t,
    })
}

/// Starts of the occurrences that overlap [lo, hi) (stored clock), skipping
/// `exdates` keys. At most `cap` of them. None if the rule doesn't parse.
pub fn expand(rule: &str, start: NaiveDateTime, dur: Duration, all_day: bool, is_utc: bool, exdates: &HashSet<String>,
              lo: NaiveDateTime, hi: NaiveDateTime, tz: Tz, cap: usize) -> Option<Vec<NaiveDateTime>> {
    let utc_series = is_utc && !all_day;
    let (rest, until) = split_until(rule);
    let mut r: RRule<Unvalidated> = rest.parse().ok()?;
    // Expansion clock: the user's zone for UTC rows, else UTC as a stand-in
    // for "no zone" (no DST jumps).
    let clock: rrule::Tz = if utc_series { tz.into() } else { rrule::Tz::UTC };
    let to_clock = |t: NaiveDateTime| -> chrono::DateTime<rrule::Tz> {
        if utc_series { rrule::Tz::UTC.from_utc_datetime(&t).with_timezone(&clock) } else { rrule::Tz::UTC.from_utc_datetime(&t) }
    };
    let from_clock = |d: chrono::DateTime<rrule::Tz>| -> NaiveDateTime { d.naive_utc() };
    if let Some(u) = until.as_deref().and_then(|u| until_in(u, utc_series, tz)) {
        if u < start { return Some(vec![]); }
        r = r.until(rrule::Tz::UTC.from_utc_datetime(&u));
    }
    let set: RRuleSet = r.build(to_clock(start)).ok()?.limit();
    let from = lo - dur;
    let mut out = Vec::new();
    for d in &set {
        let s = from_clock(d);
        if s >= hi { break; }
        if s < from || s + dur <= lo { continue; }
        if exdates.contains(&super::ics::occ_key(s, all_day)) { continue; }
        out.push(s);
        if out.len() >= cap { break; }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BA: Tz = chrono_tz::America::Argentina::Buenos_Aires;
    fn dt(s: &str) -> NaiveDateTime { NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap() }
    fn fmt(v: &[NaiveDateTime]) -> Vec<String> { v.iter().map(|d| d.format("%Y-%m-%d %H:%M").to_string()).collect() }

    #[test]
    fn weekly_in_local_days_with_exdates() {
        // Mon/Fri 22:00 in Buenos Aires = 01:00 UTC the next day: BYDAY is about the local day.
        let ex: HashSet<String> = ["2026-11-24T01:00".to_string()].into();
        let got = expand("FREQ=WEEKLY;BYDAY=MO,FR", dt("2026-11-17 01:00"), Duration::hours(1), false, true, &ex,
                         dt("2026-11-16 03:00"), dt("2026-11-30 03:00"), BA, 100).unwrap();
        assert_eq!(fmt(&got), ["2026-11-17 01:00", "2026-11-21 01:00", "2026-11-28 01:00"]);
    }

    #[test]
    fn until_count_all_day_and_dst() {
        let none = HashSet::new();
        // UNTIL in UTC (as iCloud writes it), inclusive
        let got = expand("FREQ=WEEKLY;UNTIL=20260819T025959Z;BYDAY=TU,TH", dt("2026-08-11 23:15"), Duration::hours(1), false, true, &none,
                         dt("2026-08-01 00:00"), dt("2026-09-01 00:00"), BA, 100).unwrap();
        assert_eq!(fmt(&got), ["2026-08-11 23:15", "2026-08-13 23:15", "2026-08-18 23:15"]);
        // all-day yearly with an exdate, overlapping the window by its end
        let ex: HashSet<String> = ["2027-08-27".to_string()].into();
        let got = expand("FREQ=YEARLY", dt("2020-08-27 00:00"), Duration::days(1), true, false, &ex,
                         dt("2026-08-27 12:00"), dt("2028-08-28 00:00"), BA, 100).unwrap();
        assert_eq!(fmt(&got), ["2026-08-27 00:00", "2028-08-27 00:00"]);
        let got = expand("FREQ=DAILY;COUNT=3", dt("2026-01-01 09:00"), Duration::minutes(30), false, false, &none,
                         dt("2025-12-01 00:00"), dt("2026-02-01 00:00"), BA, 100).unwrap();
        assert_eq!(got.len(), 3);
        // Madrid 09:00 weekly across the October DST change: 07:00 UTC, then 08:00 UTC
        let got = expand("FREQ=WEEKLY", dt("2026-10-19 07:00"), Duration::hours(1), false, true, &none,
                         dt("2026-10-19 00:00"), dt("2026-11-03 00:00"), chrono_tz::Europe::Madrid, 100).unwrap();
        assert_eq!(fmt(&got), ["2026-10-19 07:00", "2026-10-26 08:00", "2026-11-02 08:00"]);
        assert!(expand("FREQ=SOMETIMES", dt("2026-01-01 09:00"), Duration::hours(1), false, false, &none, dt("2026-01-01 00:00"), dt("2026-02-01 00:00"), BA, 10).is_none());
    }
}
