//! Body facts for the list (cerq-mail's enrich): a one-line preview, a sign-in
//! code, an amount, a due date and an invitation's date and time. Read from
//! the first 120 KB of codes, receipts, people and invites, a few per sync;
//! stored in msg.extra as JSON. Python's regexes, lookarounds included.
use chrono::{Datelike, Duration, NaiveDate};
use fancy_regex::Regex as Fx;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

pub const ENRICH: [&str; 4] = ["otp", "receipt", "personal", "calendar"];

fn fx(s: &str) -> Fx { Fx::new(s).unwrap() }
fn rx(s: &str) -> Regex { Regex::new(s).unwrap() }

static CODE: LazyLock<Fx> = LazyLock::new(|| fx(r"(?i)(c[oó]digo|code|verificaci[oó]n|verification|otp|pin|token|clave|contrase[nñ]a)[^0-9\n]{0,60}?(?<![\d.,])(\d{4,8})(?![\d,]|\.\d)"));
static CODE6: LazyLock<Fx> = LazyLock::new(|| fx(r"(?<![\d.,#/-])(\d{6})(?![\d,/-]|\.\d)"));
static TOTAL: LazyLock<Fx> = LazyLock::new(|| fx(r"(?i)((?<!sub)total|payment of|pago de|pagaste|importe|monto|charged|you paid|amount paid|cobro|consumo de|compra de|transferencia de|transferiste)"));
static AMT: LazyLock<Regex> = LazyLock::new(|| rx(r"(US\$|U\$S|USD|ARS|AR\$|R\$|\$|€|EUR|£)\s?(\d{1,3}(?:[.,]\d{3})+(?:[.,]\d{1,2})?|\d+(?:[.,]\d{1,2})?)"));
static DUE: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)(vence|vencimiento|vto\.|fecha l[ií]mite de pago|due date|due on|payment due|is due|pay by|pag[aá] antes del?)"));
/// Purchases, deliveries and returns carry dates too ("devolvé hasta el…"): not bills.
static NOT_BILL: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)compra|pedido|order|env[ií]o|lleg[oó]|despach|devoluci|return|shipped|entrega|delivery|cup[oó]n|coupon"));
static TIME: LazyLock<Fx> = LazyLock::new(|| fx(r"(?i)(?<![\d:])([01]?\d|2[0-3])(?::|\.|h)([0-5]\d)\s*(hs|h|am|pm)?(?!\d)"));
static SNIP: LazyLock<Regex> = LazyLock::new(|| rx(r"\(?<?https?://\S+|-+>|[-=_*~#|•·]{3,}|\[image:[^\]]*\]"));
static WS: LazyLock<Regex> = LazyLock::new(|| rx(r"\s+"));
static ZW: LazyLock<Regex> = LazyLock::new(|| rx("[\u{200b}-\u{200f}\u{2060}\u{feff}\u{034f}\u{ad}]+"));

const MONTH_NAMES: [&str; 12] = ["ene enero jan january", "feb febrero february", "mar marzo march", "abr abril apr april", "may mayo",
    "jun junio june", "jul julio july", "ago agosto aug august", "sep sept septiembre setiembre september",
    "oct octubre october", "nov noviembre november", "dic diciembre dec december"];

fn month(name: &str) -> Option<u32> {
    let n = name.to_lowercase();
    MONTH_NAMES.iter().position(|names| names.split(' ').any(|x| x == n)).map(|i| i as u32 + 1)
}

static DATES: LazyLock<[Fx; 4]> = LazyLock::new(|| {
    let mut names: Vec<&str> = MONTH_NAMES.iter().flat_map(|n| n.split(' ')).collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mon = names.join("|");
    [
        fx(r"(?<!\d)(\d{4})-(\d{2})-(\d{2})(?!\d)"),                                          // 2026-11-20
        fx(r"(?<![\d/])(\d{1,2})[/.-](\d{1,2})(?:[/.-](\d{2,4}))?(?![\d/])"),                // 20/11(/2026)
        fx(&format!(r"(?i)(?<!\w)(\d{{1,2}})(?: de)? ({mon})\.?(?:,? (?:de )?(\d{{4}}))?(?!\w)")), // 20 de noviembre
        fx(&format!(r"(?i)(?<!\w)({mon})\.? (\d{{1,2}})(?:st|nd|rd|th)?(?:,? (\d{{4}}))?(?!\w)")),  // November 20, 2026
    ]
});

/// Byte index of the n-th character (or the end).
fn at_char(s: &str, n: usize) -> usize { s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len()) }
fn head(s: &str, n: usize) -> &str { &s[..at_char(s, n)] }

/// The first date in `text`, in the coming year: (date, start, end) in bytes.
pub fn a_date(text: &str, today: NaiveDate, month_first: bool) -> Option<(NaiveDate, usize, usize)> {
    let mut best: Option<(NaiveDate, usize, usize)> = None;
    for (i, re) in DATES.iter().enumerate() {
        let Ok(Some(m)) = re.captures(text) else { continue };
        let whole = m.get(0).unwrap();
        if best.is_some_and(|b| whole.start() >= b.1) { continue; }
        let g = |k: usize| m.get(k).map(|x| x.as_str()).unwrap_or("");
        let n = |k: usize| g(k).parse::<i32>().ok();
        let (y, mo, d): (Option<i32>, Option<u32>, Option<u32>) = match i {
            0 => (n(1), n(2).map(|x| x as u32), n(3).map(|x| x as u32)),
            1 => {
                let (mut d, mut mo) = (n(1).map(|x| x as u32), n(2).map(|x| x as u32));
                if month_first { std::mem::swap(&mut d, &mut mo); }
                let y = n(3).map(|y| if g(3).len() == 2 { y + 2000 } else { y });
                (y, mo, d)
            }
            2 => (n(3), month(g(2)), n(1).map(|x| x as u32)),
            _ => (n(3), month(g(1)), n(2).map(|x| x as u32)),
        };
        let (Some(mo), Some(d)) = (mo, d) else { continue };
        let Some(mut got) = NaiveDate::from_ymd_opt(y.unwrap_or(today.year()), mo, d) else { continue };
        if y.is_none() && got < today - Duration::days(30) {
            let Some(next) = NaiveDate::from_ymd_opt(today.year() + 1, mo, d) else { continue };
            got = next;
        }
        best = Some((got, whole.start(), whole.end()));
    }
    best
}

fn in_window(d: NaiveDate, today: NaiveDate) -> bool { today <= d && d <= today + Duration::days(120) }

/// Due dates ("vence el 20/11") and invitation times ("el jueves 20/11 a las 18:30").
pub fn due_facts(flat: &str, category: &str, today: NaiveDate, subject: &str, month_first: bool) -> Map<String, Value> {
    let mut out = Map::new();
    if matches!(category, "receipt" | "personal") && !NOT_BILL.is_match(subject) {
        for k in DUE.find_iter(head(flat, 6000)) {
            let after = &flat[k.end()..];
            if let Some((d, _, _)) = a_date(head(after, 60), today, month_first) {
                if in_window(d, today) { out.insert("due".into(), json!(d.to_string())); break; }
            }
        }
    }
    if category == "calendar" {
        if let Some((d, _, end)) = a_date(head(flat, 3000), today, month_first) {
            if in_window(d, today) {
                let mut when = d.to_string();
                if let Ok(Some(tm)) = TIME.captures(head(&flat[end..], 40)) {
                    let h: u32 = tm[1].parse().unwrap_or(0);
                    let pm = tm.get(3).is_some_and(|x| x.as_str().eq_ignore_ascii_case("pm"));
                    when += &format!("T{:02}:{}", if pm && h < 12 { h + 12 } else { h }, &tm[2]);
                }
                out.insert("when".into(), json!(when));
            }
        }
    }
    out
}

/// The text of a message: the first text/plain part that isn't an attachment,
/// else the first HTML part with tags stripped.
pub fn body_text(raw: &[u8]) -> String {
    let Some(msg) = mail_parser::MessageParser::default().parse(raw) else { return String::new() };
    let (mut plain, mut html) = (None, None);
    for p in &msg.parts {
        use mail_parser::{MimeHeaders, PartType};
        if p.attachment_name().is_some() { continue; }
        match &p.body {
            PartType::Text(t) if plain.is_none() && p.content_type().is_none_or(|c| c.subtype().is_none_or(|s| s.eq_ignore_ascii_case("plain"))) => plain = Some(t.to_string()),
            PartType::Html(h) if html.is_none() => html = Some(h.to_string()),
            _ => {}
        }
    }
    let text = match (plain, html) {
        (Some(p), _) if !p.is_empty() => p,
        (_, Some(h)) => strip_html(&h),
        _ => String::new(),
    };
    ZW.replace_all(&text, "").into_owned()
}

static STYLE: LazyLock<Regex> = LazyLock::new(|| rx(r"(?is)<style[^>]*>.*?</style>"));
static SCRIPT: LazyLock<Regex> = LazyLock::new(|| rx(r"(?is)<script[^>]*>.*?</script>"));
static HEAD: LazyLock<Regex> = LazyLock::new(|| rx(r"(?is)<head[^>]*>.*?</head>"));
static TAG: LazyLock<Regex> = LazyLock::new(|| rx(r"(?s)<[^>]+>"));

pub fn strip_html(h: &str) -> String {
    let h = STYLE.replace_all(h, " ");
    let h = SCRIPT.replace_all(&h, " ");
    let h = HEAD.replace_all(&h, " ");
    html_escape::decode_html_entities(&TAG.replace_all(&h, " ")).into_owned()
}

/// "1.234,56" → 1234.56, "1,234.56" → 1234.56.
pub fn amount_value(n: &str) -> f64 {
    static DEC_COMMA: LazyLock<Regex> = LazyLock::new(|| rx(r",\d{1,2}$"));
    let n = if DEC_COMMA.is_match(n) { n.replace('.', "").replace(',', ".") } else { n.replace(',', "") };
    n.parse().unwrap_or(0.0)
}

pub fn body_facts(raw: &[u8], category: &str, subject: &str, today: NaiveDate, month_first: bool) -> Map<String, Value> {
    facts(&body_text(raw), category, subject, today, month_first)
}

/// The facts from a message's text.
pub fn facts(text: &str, category: &str, subject: &str, today: NaiveDate, month_first: bool) -> Map<String, Value> {
    let flat = WS.replace_all(text, " ").trim().to_string();
    let snip = SNIP.replace_all(&flat, " ");
    let snip = WS.replace_all(&snip, " ");
    let snip = snip.trim_matches(|c: char| c.is_whitespace() || " .,:;-".contains(c));
    let mut out = Map::new();
    out.insert("snippet".into(), json!(head(snip, 160)));
    if category == "otp" {
        let m = CODE.captures(text).ok().flatten().and_then(|c| c.get(2)).or_else(|| CODE6.captures(text).ok().flatten().and_then(|c| c.get(1)));
        if let Some(m) = m { out.insert("code".into(), json!(m.as_str())); }
    }
    // Personal mail too: bank notices ("consumo de $ 40.000").
    if matches!(category, "receipt" | "personal") {
        let hay = if category == "receipt" { flat.as_str() } else { head(&flat, 4000) };
        let found: Vec<(usize, &str, &str)> = AMT.captures_iter(hay).map(|c| (c.get(0).unwrap().start(), c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str()))
            .filter(|f| amount_value(f.2) > 0.0).collect();
        // The one a "total" / "payment of" points at, else (receipts) the biggest.
        let said = found.iter().find(|f| {
            let before = &flat[..f.0];
            let start = before.char_indices().rev().nth(44).map(|(i, _)| i).unwrap_or(0);
            TOTAL.is_match(&before[start..]).unwrap_or(false)
        });
        let pick = said.or_else(|| if category == "receipt" {
            found.iter().fold(None, |best: Option<&(usize, &str, &str)>, f| match best { Some(b) if amount_value(b.2) >= amount_value(f.2) => Some(b), _ => Some(f) })
        } else { None });
        if let Some((_, cur, n)) = pick {
            let cur = match *cur { "U$S" | "USD" => "US$", "AR$" | "ARS" => "$", c => c };
            out.insert("amount".into(), json!(format!("{cur} {n}")));
        }
    }
    out.extend(due_facts(&flat, category, today, subject, month_first));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(s: &str) -> NaiveDate { s.parse().unwrap() }
    const TODAY: &str = "2026-11-18";

    #[test]
    fn dates() {
        let t = d(TODAY);
        assert_eq!(a_date("vence 2026-12-01 hoy", t, false).unwrap().0, d("2026-12-01"));
        assert_eq!(a_date("el 20/11", t, false).unwrap().0, d("2026-11-20"));
        assert_eq!(a_date("el 03/04/27", t, false).unwrap().0, d("2027-04-03"));
        assert_eq!(a_date("el 03/04/27", t, true).unwrap().0, d("2027-03-04"));
        assert_eq!(a_date("el 20 de noviembre", t, false).unwrap().0, d("2026-11-20"));
        assert_eq!(a_date("due by November 25th, 2026", t, false).unwrap().0, d("2026-11-25"));
        assert_eq!(a_date("on Dec. 3", t, false).unwrap().0, d("2026-12-03"));
        // A day with no year long past means next year.
        assert_eq!(a_date("el 5/1", t, false).unwrap().0, d("2027-01-05"));
        // The earliest date in the text wins, whatever the format.
        assert_eq!(a_date("Nov 30 or 2026-12-01", t, false).unwrap().0, d("2026-11-30"));
        assert!(a_date("no dates 31/02 here", t, false).is_none());
    }

    #[test]
    fn due_and_when() {
        let t = d(TODAY);
        let f = due_facts("Tu factura vence el 25/11. Total $ 1.234,50", "receipt", t, "Factura disponible", false);
        assert_eq!(f["due"], "2026-11-25");
        // Orders and deliveries carry dates that aren't bills.
        assert!(due_facts("Llega el 25/11", "receipt", t, "Tu pedido", false).get("due").is_none());
        let f = due_facts("Te invito el jueves 20/11 a las 18:30 en casa", "calendar", t, "", false);
        assert_eq!(f["when"], "2026-11-20T18:30");
        let f = due_facts("Meeting November 21 at 3.15 pm", "calendar", t, "", false);
        assert_eq!(f["when"], "2026-11-21T15:15");
        // Too far ahead: not a reminder.
        assert!(due_facts("vence el 2027-08-01", "personal", t, "", false).get("due").is_none());
    }

    #[test]
    fn codes_and_amounts() {
        let t = d(TODAY);
        assert_eq!(facts("Your verification code is 482913. It expires in 10 min.", "otp", "", t, false)["code"], "482913");
        assert_eq!(facts("Use 1234 as your PIN: 5678", "otp", "", t, false)["code"], "5678");
        assert_eq!(facts("Ingresá 123456 para continuar", "otp", "", t, false)["code"], "123456");
        assert!(facts("Order 1234567 shipped", "otp", "", t, false).get("code").is_none());
        let f = facts("Subtotal $ 100,00 envío $ 20,00 Total $ 120,00", "receipt", "", t, false);
        assert_eq!(f["amount"], "$ 120,00");
        let f = facts("Items: USD 5.00, USD 40.00, USD 7.50", "receipt", "", t, false);
        assert_eq!(f["amount"], "US$ 40.00");
        let f = facts("Realizaste un consumo de $ 40.000 en COTO", "personal", "", t, false);
        assert_eq!(f["amount"], "$ 40.000");
        assert!(facts("Te debo $ 500 del asado", "personal", "", t, false).get("amount").is_none());
        assert_eq!(amount_value("1.234,56"), 1234.56);
        assert_eq!(amount_value("1,234.56"), 1234.56);
        assert_eq!(amount_value("40.000"), 40.0);
    }

    #[test]
    fn snippet_and_html() {
        let t = d(TODAY);
        let f = facts("Hi Ana,\n\nSee https://example.com/x ----- [image: logo] thanks.", "personal", "", t, false);
        assert_eq!(f["snippet"], "Hi Ana, See thanks");
        let raw = b"Content-Type: text/html; charset=utf-8\r\n\r\n<html><head><style>p{}</style></head><body><p>Hola &amp; chau</p></body></html>";
        assert_eq!(WS.replace_all(body_text(raw).trim(), " "), "Hola & chau");
        let raw = b"Content-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nplain\xe2\x80\x8b text\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>html</p>\r\n--b--\r\n";
        assert_eq!(body_text(raw).trim(), "plain text");
    }
}
