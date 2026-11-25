//! Quick add ("call the bank fri 10:00", "lunch with Ana tomorrow 13:00", also in Spanish): a
//! to-do or an event, its day, time, length and #label. Word by word (no regex in the wasm).
use super::util::{add_days, parts, today_str, weekday, ymd};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Parsed {
    pub event: bool,
    pub title: String,
    pub date: Option<String>,
    pub start: Option<i32>,
    pub end: Option<i32>,
    pub label: Option<String>,
    pub guessed: bool,
}

fn wd(w: &str) -> Option<i64> {
    Some(match w {
        "sun" | "sunday" | "domingo" => 0,
        "mon" | "monday" | "lunes" => 1,
        "tue" | "tues" | "tuesday" | "martes" => 2,
        "wed" | "wednesday" | "miércoles" | "miercoles" => 3,
        "thu" | "thur" | "thurs" | "thursday" | "jueves" => 4,
        "fri" | "friday" | "viernes" => 5,
        "sat" | "saturday" | "sábado" | "sabado" => 6,
        _ => return None,
    })
}
fn month(w: &str) -> Option<i64> {
    if w.chars().count() < 3 || !w.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    let p: String = w.chars().take(3).collect();
    let m = match p.as_str() {
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
        "dec" | "dic" => 11,
        _ => return None,
    };
    // "mar" alone is March; "martes" is a weekday, "marcar" isn't a month.
    let full = ["january", "enero", "february", "febrero", "march", "marzo", "april", "abril", "may", "mayo", "june", "junio", "july", "julio", "august", "agosto", "september", "sept", "septiembre", "october", "octubre", "november", "noviembre", "december", "diciembre"];
    (w.chars().count() == 3 || full.contains(&w)).then_some(m)
}
const EVENTISH: [&str; 27] = ["with", "con", "meeting", "meet", "lunch", "dinner", "breakfast", "coffee", "reunión", "reunion", "almuerzo", "cena", "desayuno", "café", "cafe", "dentist", "dentista", "doctor", "médico", "medico", "padel", "gym", "flight", "vuelo", "turno", "call", "appointment"];

fn on_date(day: i64, mon: i64, today: &str) -> Option<String> {
    let (y, _, _) = parts(today);
    let dim = |y: i64, m: i64| -> i64 {
        match m {
            1 => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 },
            3 | 5 | 8 | 10 => 30,
            _ => 31,
        }
    };
    if !(0..12).contains(&mon) || day < 1 || day > dim(y, mon) {
        return None;
    }
    let d = ymd(y, mon + 1, day);
    if d.as_str() < today {
        if day > dim(y + 1, mon) {
            return None;
        }
        return Some(ymd(y + 1, mon + 1, day));
    }
    Some(d)
}
fn hour(h: i32, m: i32, ap: Option<&str>) -> Option<i32> {
    let mut x = h;
    if ap == Some("pm") && x < 12 {
        x += 12;
    }
    if ap == Some("am") && x == 12 {
        x = 0;
    }
    (x < 24 && m < 60).then_some(x * 60 + m)
}
fn num(s: &str) -> Option<i32> {
    (!s.is_empty() && s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// A clock in one word: "10", "10:30", "10am", "10:30pm", "10h", "@10". Marked = it can't be just a number.
#[derive(Clone, Copy, Debug)]
struct Clock {
    h: i32,
    m: i32,
    ap: Option<&'static str>,
    marked: bool,
}
fn clock(w: &str) -> Option<Clock> {
    let (w, at) = match w.strip_prefix('@') {
        Some(r) => (r, true),
        None => (w, false),
    };
    let (w, ap) = if let Some(r) = w.strip_suffix("am") {
        (r, Some("am"))
    } else if let Some(r) = w.strip_suffix("pm") {
        (r, Some("pm"))
    } else {
        (w, None)
    };
    let (w, hsuf) = match w.strip_suffix('h') {
        Some(r) if ap.is_none() => (r, true),
        _ => (w, false),
    };
    let (h, m, colon) = match w.split_once(':') {
        Some((h, m)) if m.len() == 2 => (num(h)?, num(m)?, true),
        Some(_) => return None,
        None => (num(w)?, 0, false),
    };
    if h > 23 || w.split(':').next().unwrap_or("").len() > 2 {
        return None;
    }
    Some(Clock { h, m, ap, marked: at || colon || ap.is_some() || hsuf })
}

/// Read a quick-add line. Times are minutes after midnight; no date = none.
pub fn parse(text: &str) -> Parsed {
    parse_on(text, &today_str())
}
pub fn parse_on(text: &str, today: &str) -> Parsed {
    // Words with their places in the original (for the title), lower-cased one char for one.
    let chars: Vec<char> = text.chars().collect();
    let mut words: Vec<(usize, usize, String)> = vec![];
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let s = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let mut e = i;
        while e > s + 1 && matches!(chars[e - 1], ',' | '.' | ';' | '!' | '?') {
            e -= 1;
        }
        let w: String = chars[s..e].iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
        words.push((s, e, w));
    }
    let n = words.len();
    let w = |k: usize| words.get(k).map(|x| x.2.as_str()).unwrap_or("");
    let mut cut = vec![false; n];
    let mut r = Parsed::default();
    let mut night = false;
    let mut kw = false;

    // "event …", "todo …", "new task …"
    let k0 = if matches!(w(0), "new" | "add" | "nuevo" | "nueva") { 1 } else { 0 };
    if matches!(w(k0), "event" | "evento" | "task" | "todo" | "to-do" | "tarea" | "pendiente") {
        r.event = w(k0).starts_with("even");
        kw = true;
        for c in cut.iter_mut().take(k0 + 1) {
            *c = true;
        }
    }
    let free = |cut: &[bool], a: usize, b: usize| b <= n && (a..b).all(|k| !cut[k]);
    let take = |cut: &mut Vec<bool>, a: usize, b: usize| {
        for c in cut.iter_mut().take(b).skip(a) {
            *c = true;
        }
    };

    // Relative days.
    for k in 0..n {
        if r.date.is_some() {
            break;
        }
        let pairs: [(&[&str], i64, bool); 10] = [
            (&["day", "after", "tomorrow"], 2, false),
            (&["pasado", "mañana"], 2, false),
            (&["esta", "noche"], 0, true),
            (&["today"], 0, false),
            (&["hoy"], 0, false),
            (&["tonight"], 0, true),
            (&["tomorrow"], 1, false),
            (&["mañana"], 1, false),
            (&["tmrw"], 1, false),
            (&["tmr"], 1, false),
        ];
        for (ws, add, nt) in pairs.iter() {
            if free(&cut, k, k + ws.len()) && ws.iter().enumerate().all(|(j, x)| w(k + j) == *x) {
                r.date = Some(add_days(today, *add));
                night |= *nt;
                take(&mut cut, k, k + ws.len());
                break;
            }
        }
    }
    // Weekdays ("fri", "next monday", "el viernes").
    for k in 0..n {
        if r.date.is_some() {
            break;
        }
        if let Some(d) = wd(w(k)).filter(|_| !cut[k]) {
            let diff = (d - weekday(today) + 7) % 7;
            r.date = Some(add_days(today, if diff == 0 { 7 } else { diff }));
            let mut a = k;
            if k > 0 && !cut[k - 1] && matches!(w(k - 1), "next" | "this" | "el" | "este" | "próximo" | "proximo") {
                a = k - 1;
                if a > 0 && !cut[a - 1] && w(a - 1) == "el" && w(k - 1) == "próximo" {
                    a -= 1;
                }
            }
            take(&mut cut, a, k + 1);
        }
    }
    // "in 3 days", "en 2 semanas".
    for k in 0..n.saturating_sub(2) {
        if r.date.is_some() {
            break;
        }
        if free(&cut, k, k + 3) && matches!(w(k), "in" | "en") {
            if let Some(x) = num(w(k + 1)) {
                let u = w(k + 2);
                let unit = if ["day", "days", "día", "días", "dia", "dias"].contains(&u) { 1 } else if ["week", "weeks", "semana", "semanas"].contains(&u) { 7 } else { 0 };
                if unit > 0 {
                    r.date = Some(add_days(today, x as i64 * unit));
                    take(&mut cut, k, k + 3);
                }
            }
        }
    }
    // "12 nov", "12 de noviembre", "nov 12th".
    for k in 0..n {
        if r.date.is_some() {
            break;
        }
        if cut[k] {
            continue;
        }
        if let Some(d) = num(w(k)) {
            let (mk, end) = if w(k + 1) == "de" { (k + 2, k + 3) } else { (k + 1, k + 2) };
            if let Some(m) = (mk < n && free(&cut, k, end)).then(|| month(w(mk))).flatten() {
                if let Some(x) = on_date(d as i64, m, today) {
                    r.date = Some(x);
                    take(&mut cut, k, end);
                    continue;
                }
            }
        }
        if let Some(m) = month(w(k)) {
            let t = w(k + 1).trim_end_matches("st").trim_end_matches("nd").trim_end_matches("rd").trim_end_matches("th");
            if let Some(d) = (k + 1 < n && !cut[k + 1]).then(|| num(t)).flatten() {
                if let Some(x) = on_date(d as i64, m, today) {
                    r.date = Some(x);
                    take(&mut cut, k, k + 2);
                }
            }
        }
    }
    // "12/11" (day/month).
    for k in 0..n {
        if r.date.is_some() {
            break;
        }
        if let Some((a, b)) = w(k).split_once('/').filter(|_| !cut[k]) {
            if let (Some(d), Some(m)) = (num(a), num(b)) {
                if let Some(x) = on_date(d as i64, m as i64 - 1, today) {
                    r.date = Some(x);
                    take(&mut cut, k, k + 1);
                }
            }
        }
    }
    // Lengths: "for 30 min", "por 2 horas", "45 min".
    let mut len: Option<i32> = None;
    let unit = |u: &str| -> Option<i32> {
        if ["m", "min", "mins", "minute", "minutes", "minuto", "minutos"].contains(&u) {
            Some(1)
        } else if ["h", "hr", "hrs", "hour", "hours", "hora", "horas"].contains(&u) {
            Some(60)
        } else {
            None
        }
    };
    for k in 0..n {
        if cut[k] {
            continue;
        }
        if matches!(w(k), "for" | "por") && free(&cut, k, k + 3) {
            if let (Some(x), Some(u)) = (num(w(k + 1)), unit(w(k + 2))) {
                len = Some(x * u);
                take(&mut cut, k, k + 3);
                continue;
            }
        }
        if free(&cut, k, k + 2) {
            if let (Some(x), true) = (num(w(k)), ["min", "mins", "minutes", "minutos"].contains(&w(k + 1))) {
                len = Some(x);
                take(&mut cut, k, k + 2);
            }
        }
    }
    // Times: a range ("10-11", "from 9 to 10:30", "2pm–4pm"), else one ("at 10", "10:00", "3pm").
    let prefix = |k: usize, cut: &[bool]| -> usize {
        // How many words before k say "at" (0, 1 or 2).
        if k >= 2 && !cut[k - 1] && !cut[k - 2] && matches!((w(k - 2), w(k - 1)), ("a", "las") | ("desde", "las")) {
            2
        } else if k >= 1 && !cut[k - 1] && matches!(w(k - 1), "at" | "from" | "de" | "alle" | "@") {
            1
        } else {
            0
        }
    };
    let ap_after = |k: usize| -> Option<&'static str> {
        match w(k) {
            "am" => Some("am"),
            "pm" => Some("pm"),
            _ => None,
        }
    };
    'range: for k in 0..n {
        if r.start.is_some() || cut[k] {
            break;
        }
        // One word: "10-11", "10:00–11:30".
        let one = w(k).split_once('-').or_else(|| w(k).split_once('–'));
        let cands: Vec<(Clock, Clock, usize)> = if let Some((a, b)) = one.filter(|(a, b)| !a.is_empty() && !b.is_empty()) {
            match (clock(a), clock(b)) {
                (Some(a), Some(b)) => vec![(a, b, k + 1)],
                _ => vec![],
            }
        } else if let Some(mut a) = clock(w(k)) {
            let mut j = k + 1;
            if a.ap.is_none() {
                if let Some(ap) = ap_after(j) {
                    a.ap = Some(ap);
                    a.marked = true;
                    j += 1;
                }
            }
            if j < n && matches!(w(j), "-" | "–" | "to" | "a" | "until" | "hasta") {
                match clock(w(j + 1)) {
                    Some(mut b) if j + 1 < n => {
                        let mut e = j + 2;
                        if b.ap.is_none() {
                            if let Some(ap) = ap_after(e) {
                                b.ap = Some(ap);
                                b.marked = true;
                                e += 1;
                            }
                        }
                        vec![(a, b, e)]
                    }
                    _ => vec![],
                }
            } else {
                vec![]
            }
        } else {
            vec![]
        };
        for (a, b, e) in cands {
            let p = prefix(k, &cut);
            if !free(&cut, k, e) || !(p > 0 || a.marked || b.marked) {
                continue;
            }
            let ap = if a.ap.is_none() && b.ap == Some("pm") && a.h < 12 { Some("pm") } else { a.ap };
            if let (Some(s), Some(en)) = (hour(a.h, a.m, ap), hour(b.h, b.m, b.ap)) {
                if en > s {
                    r.start = Some(s);
                    r.end = Some(en);
                    take(&mut cut, k - p, e);
                    break 'range;
                }
            }
        }
    }
    for k in 0..n {
        if r.start.is_some() {
            break;
        }
        if cut[k] {
            continue;
        }
        if let Some(mut a) = clock(w(k)) {
            let mut e = k + 1;
            if a.ap.is_none() {
                if let Some(ap) = ap_after(e).filter(|_| e < n && !cut[e]) {
                    a.ap = Some(ap);
                    a.marked = true;
                    e += 1;
                }
            }
            let p = prefix(k, &cut);
            if !(p > 0 || a.marked) {
                continue;
            }
            if let Some(mut s) = hour(a.h, a.m, a.ap) {
                if night && s < 720 {
                    s += 720;
                }
                r.start = Some(s);
                take(&mut cut, k - p, e);
            }
        }
    }
    for k in 0..n {
        if r.start.is_none() && !cut[k] && matches!(w(k), "noon" | "midday" | "mediodía" | "mediodia") {
            r.start = Some(720);
            take(&mut cut, k, k + 1);
        }
    }
    for k in 0..n {
        if r.label.is_none() && !cut[k] {
            if let Some(l) = w(k).strip_prefix('#').filter(|l| !l.is_empty()) {
                r.label = Some(l.to_string());
                take(&mut cut, k, k + 1);
            }
        }
    }

    // The title: what's left, without the little words around what was taken.
    let mut left: Vec<String> = words.iter().enumerate().filter(|(k, x)| !cut[*k] && x.2 != "@" && x.2 != "#").map(|(_, x)| chars[x.0..x.1].iter().collect()).collect();
    for _ in 0..3 {
        while left.first().is_some_and(|f| ["on", "at", "el", "la", "for", "para", "the"].contains(&f.to_lowercase().as_str())) {
            left.remove(0);
        }
        if left.len() >= 2 && left[0].eq_ignore_ascii_case("a") && left[1].eq_ignore_ascii_case("las") {
            left.drain(..2);
        }
        while left.last().is_some_and(|f| ["on", "at", "el", "for", "para", "the", "this", "next", "este", "de"].contains(&f.to_lowercase().as_str())) {
            left.pop();
        }
    }
    let t = left.join(" ");
    let mut cs = t.chars();
    r.title = match cs.next() {
        Some(c) => c.to_uppercase().collect::<String>() + cs.as_str(),
        None => String::new(),
    };
    if !kw && r.start.is_some() && words.iter().any(|x| EVENTISH.contains(&x.2.as_str())) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_lines() {
        let t = "2026-11-24"; // a Tuesday
        let p = parse_on("call the bank fri 10:00", t);
        assert_eq!((p.title.as_str(), p.date.as_deref(), p.start, p.event), ("Call the bank", Some("2026-11-27"), Some(600), false));
        let p = parse_on("lunch with Ana tomorrow 1pm", t);
        assert_eq!((p.title.as_str(), p.date.as_deref(), p.start, p.end, p.event), ("Lunch with Ana", Some("2026-11-25"), Some(780), Some(840), true));
        let p = parse_on("fix the light #home", t);
        assert_eq!((p.title.as_str(), p.label.as_deref(), p.date), ("Fix the light", Some("home"), None));
        let p = parse_on("meeting 12 dec from 9 to 10:30", t);
        assert_eq!((p.title.as_str(), p.date.as_deref(), p.start, p.end), ("Meeting", Some("2026-12-12"), Some(540), Some(630)));
        let p = parse_on("pay rent 3 nov", t);
        assert_eq!(p.date.as_deref(), Some("2027-11-03"));
        let p = parse_on("buy 3 apples", t);
        assert_eq!((p.title.as_str(), p.start), ("Buy 3 apples", None));
    }
}
