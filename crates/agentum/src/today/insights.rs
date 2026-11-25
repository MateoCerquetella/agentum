//! Today's extra cards from what CERQ already has, no model (insights.py):
//! this month's money from mail, parcels on their way, the hub's health.
use super::{i18n::{self as L, Lang}, now, s};
use crate::state::App;
use chrono::{Datelike, TimeZone};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

// ---- The hub's machine (an ops script writes the health file once a minute)
fn iso_ts(v: &Value) -> i64 {
    let t = v.as_str().unwrap_or("");
    chrono::DateTime::parse_from_rfc3339(t).map(|d| d.timestamp()).ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(t.get(..19).unwrap_or(""), "%Y-%m-%dT%H:%M:%S").ok().map(|d| d.and_utc().timestamp()))
        .unwrap_or(0)
}

pub fn health() -> Value {
    let path = std::env::var("CERQ_HEALTH_FILE").unwrap_or_else(|_| "/var/lib/cerq-health/health.json".into());
    let Some(mut v) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(|v| v.is_object()) else { return json!({"tracking": false}) };
    health_of(&mut v, crate::util::now_ts());
    v
}

/// The derived fields: stale heartbeat, boot time, reboots this week, backup
/// within 2 days, and whether the card should show at all.
fn health_of(v: &mut Value, now: i64) {
    let uptime = v["uptime_s"].as_f64().unwrap_or(1e9);
    let stale = now - v["ts"].as_f64().unwrap_or(0.0) as i64 > 300;
    let week = now - 7 * 86400;
    let reboots: Vec<Value> = v["boots"].as_array().cloned().unwrap_or_default().into_iter().filter(|b| iso_ts(&b["at"]) > week).collect();
    let bk = &v["backup"];
    let backup_ok = bk["ok"].as_bool().unwrap_or(false) && now - iso_ts(&bk["at"]) < 2 * 86400;
    v["tracking"] = json!(true);
    v["stale"] = json!(stale);
    v["booted_at"] = json!(now - uptime as i64);
    v["reboots_7d"] = json!(reboots);
    v["backup_ok"] = json!(backup_ok);
    v["show"] = json!(stale || !backup_ok || uptime < 86400.0);
}

// ---- money
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(US\$|U\$S|USD|AR\$|ARS|R\$|\$|€|EUR|£)\s*([\d.,]+)").unwrap());
static BILL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bvence|vencid|vencimiento|factura (est[aá] |ya est[aá] )?disponible|tu (última |ultima |nueva )?factura|infracci|multa|resumen de (cuenta|tarjeta)|statement|payment due|bill is (ready|due)").unwrap());
static BANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)novedades de tus transacciones|movimientos?\b|transferencia recibida|acreditamos|te enviaron|recibiste (dinero|una transferencia)").unwrap());

/// "$ 40.000", "US$ 79.99", "1.234,56 €"-style amounts → (currency, value).
pub fn value(amount: &str) -> Option<(String, f64)> {
    let c = AMOUNT.captures(amount)?;
    let raw = c.get(2)?.as_str();
    let n = raw.trim_end_matches(['.', ',']);
    static COMMA_DEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",\d{1,2}$").unwrap());
    static DOT_THOUSANDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.\d{3}$").unwrap());
    static DOT_CENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.\d{2}$").unwrap());
    let n = if COMMA_DEC.is_match(n) { n.replace('.', "").replace(',', ".") } else if DOT_THOUSANDS.is_match(n) { n.replace('.', "") } else { n.replace(',', "") };
    let v: f64 = n.parse().ok()?;
    let cur = match &c[1] { "US$" | "U$S" | "USD" => "US$", "$" | "AR$" | "ARS" => "$", "€" | "EUR" => "€", "£" => "£", "R$" => "R$", x => x }.to_string();
    // "$ 100.00", "$ 0.99": a dollar receipt; pesos write "$ 416" or "$ 1.234,56"
    let cur = if cur == "$" && DOT_CENTS.is_match(raw) && v < 1000.0 { "US$".to_string() } else { cur };
    Some((cur, v))
}

/// A row: (msgid, ts, from_name, from_addr, subject, extra)
type MoneyRow = (String, i64, String, String, String, String);

/// Paid this month by currency, the biggest payments, bank movements, and
/// this month's bills with whether a payment of that amount followed.
pub fn money_of(rows: Vec<MoneyRow>, reminders: &[Value], tz: chrono_tz::Tz, first_day: chrono::NaiveDate, month: String) -> Value {
    let seen = rows.len();
    let mut ids = std::collections::HashSet::new();
    let mut kept: Vec<Value> = vec![];
    for (msgid, ts, from_name, from_addr, subject, extra) in rows {
        if !msgid.is_empty() && !ids.insert(msgid) { continue; } // the same mail in two folders or accounts
        let Some(amount) = serde_json::from_str::<Value>(&extra).ok().and_then(|x| x["amount"].as_str().map(String::from)) else { continue };
        let Some((cur, val)) = value(&amount).filter(|(_, v)| *v > 0.0) else { continue };
        let kind = if BILL.is_match(&subject) { "bill" } else if BANK.is_match(&subject) { "bank" } else { "paid" };
        let who: String = { let w = if from_name.trim().is_empty() { &from_addr } else { &from_name }; w.trim().chars().take(40).collect() };
        let same = |k: &Value| k["kind"] == kind && k["cur"] == cur.as_str() && (k["value"].as_f64().unwrap_or(0.0) - val).abs() < 0.01;
        // one payment, two mails (Mercado Pago's and the shop's); the same bill reminded again
        if kind == "paid" && kept.iter().any(|k| same(k) && (k["ts"].as_i64().unwrap_or(0) - ts).abs() < 3 * 86400) { continue; }
        if kind == "bill" && kept.iter().any(same) { continue; }
        kept.push(json!({"ts": ts, "cur": cur, "value": val, "amount": amount, "kind": kind, "from": who, "subject": subject.chars().take(80).collect::<String>(),
            "date": tz.timestamp_opt(ts, 0).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()}));
    }
    let mut totals = serde_json::Map::new();
    for k in &kept {
        let g = totals.entry(s(k, "kind")).or_insert(json!({})).as_object_mut().unwrap().entry(s(k, "cur")).or_insert(json!({"total": 0.0, "n": 0}));
        g["total"] = json!(g["total"].as_f64().unwrap_or(0.0) + k["value"].as_f64().unwrap_or(0.0));
        g["n"] = json!(g["n"].as_i64().unwrap_or(0) + 1);
    }
    let paid: Vec<&Value> = kept.iter().filter(|k| k["kind"] == "paid").collect();
    let mut bills_in: Vec<&Value> = kept.iter().filter(|k| k["kind"] == "bill").collect();
    bills_in.sort_by_key(|k| -k["ts"].as_i64().unwrap_or(0));
    let (mut bills, mut latest): (Vec<Value>, Vec<String>) = (vec![], vec![]);
    for k in bills_in {
        let from = s(k, "from").to_lowercase();
        if latest.contains(&from) { continue; } // a newer bill from the same sender replaces it
        latest.push(from.clone());
        let settled = paid.iter().find(|p| p["cur"] == k["cur"] && (p["value"].as_f64().unwrap_or(0.0) - k["value"].as_f64().unwrap_or(0.0)).abs() < 0.01 && p["ts"].as_i64().unwrap_or(0) >= k["ts"].as_i64().unwrap_or(0) - 86400);
        let due = reminders.iter().find(|r| r["kind"] == "pay" && s(r, "from").to_lowercase() == from).map(|r| s(r, "date")).unwrap_or_default();
        let mut b = k.clone();
        b["paid"] = json!(settled.is_some());
        b["paid_on"] = json!(settled.map(|p| s(p, "date")).unwrap_or_default());
        b["due"] = json!(due);
        bills.push(b);
    }
    let first = first_day.to_string();
    for r in reminders {
        // due dates read from mail without an amount line here
        if r["kind"] == "pay" && s(r, "date") >= first && !latest.contains(&s(r, "from").to_lowercase()) {
            bills.push(json!({"from": r["from"], "amount": r["amount"], "subject": r["subject"], "date": "", "due": r["date"], "paid": false, "paid_on": ""}));
        }
    }
    bills.sort_by(|a, b| {
        let key = |x: &Value| (x["paid"].as_bool().unwrap_or(false), { let d = s(x, "due"); if d.is_empty() { "9999".to_string() } else { d } });
        key(a).cmp(&key(b)).then(b["value"].as_f64().unwrap_or(0.0).total_cmp(&a["value"].as_f64().unwrap_or(0.0)))
    });
    bills.truncate(8);
    let mut top: Vec<&Value> = paid.clone();
    top.sort_by(|a, b| b["value"].as_f64().unwrap_or(0.0).total_cmp(&a["value"].as_f64().unwrap_or(0.0)));
    json!({"month": month, "paid": totals.get("paid").cloned().unwrap_or(json!({})), "bank": totals.get("bank").cloned().unwrap_or(json!({})),
        "top": top.into_iter().take(5).collect::<Vec<_>>(), "bills": bills, "counted": kept.len(), "seen": seen})
}

const NOT_JUNK: &str = "folder NOT LIKE '%Spam%' AND folder NOT LIKE '%Junk%' AND folder NOT LIKE '%Sent%'";

pub async fn money(app: &App) -> anyhow::Result<Value> {
    let tz = app.tz();
    let t = now(app);
    let first = t.date_naive().with_day(1).unwrap();
    let since = tz.from_local_datetime(&first.and_hms_opt(0, 0, 0).unwrap()).earliest().map(|d| d.timestamp()).unwrap_or(0);
    let rows: Vec<MoneyRow> = app.db.call(move |c| {
        let mut st = c.prepare(&format!("SELECT coalesce(msgid,''), ts, coalesce(from_name,''), coalesce(from_addr,''), coalesce(subject,''), coalesce(extra,'') FROM msg
            WHERE ts >= ?1 AND extra LIKE '%\"amount\"%' AND {NOT_JUNK} AND folder NOT LIKE '%Trash%' ORDER BY ts"))?;
        st.query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?.collect()
    }).await?;
    let reminders = crate::mail::mail_reminders(app).await.unwrap_or_default();
    Ok(money_of(rows, &reminders, tz, first, L::capitalize(&L::month_year(Lang::of(app), t.date_naive()))))
}

// ---- deliveries
static STATUS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| vec![
    ("delivered", Regex::new(r"(?i)lleg[oó] tu (compra|pedido|paquete|env[ií]o)|entregad[oa]|fue entregad|delivered|ya lo ten[eé]s|recibiste tu (compra|pedido)").unwrap()),
    ("today", Regex::new(r"(?i)llega hoy|lo recib[ií]s hoy|arriv\w* today|out for delivery|en reparto|sali[oó] a reparto").unwrap()),
    ("shipped", Regex::new(r"(?i)en camino|despachad|est[aá] en viaje|enviamos tu|tu (compra|pedido|paquete) (sali[oó]|va en camino)|shipped|on (its|the) way|in transit|seguimiento|tracking").unwrap()),
    ("ordered", Regex::new(r"(?i)(confirmamos|recibimos) tu (compra|pedido)|compra confirmada|order (confirmed|received|placed)|gracias por tu compra|preparando tu (pedido|env[ií]o)").unwrap()),
]);
static NOT_PARCEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)factura|invoice|suscripci|subscription|newsletter|github|pull request|\bPR\b|webhook|vuelo|flight|check-?in").unwrap());
static ORDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:#|n[°º]\s*|order\s*|pedido\s*|compra\s*)(\d{6,})").unwrap());
static NOISE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(tu|your|compra|pedido|order|paquete|package|env[ií]o|est[aá]|en camino|lleg[oó]|llega hoy|que la disfrutes|despachad[oa])\b|[!¡,.]").unwrap());

/// The status a shipping mail tells (the first that matches, in order).
pub fn status_of(text: &str) -> Option<&'static str> { STATUS.iter().find(|(_, rx)| rx.is_match(text)).map(|(s, _)| *s) }

/// A row: (ts, from_name, from_addr, subject, extra)
type ParcelRow = (i64, String, String, String, String);

/// One line per order with its latest status. "Arrives today" only on the
/// day that mail came (an older one most likely arrived without a mail
/// saying so), "Delivered" only on the day it was, "On its way" for six
/// days (most arrive sooner, often without a mail) and "Ordered" for ten.
pub fn deliveries_of(rows: Vec<ParcelRow>, l: Lang, tz: chrono_tz::Tz, now: i64) -> Value {
    let mut orders: Vec<((String, String), Value)> = vec![];
    for (ts, from_name, from_addr, subject, extra) in rows {
        if NOT_PARCEL.is_match(&subject) { continue; }
        let snip = serde_json::from_str::<Value>(&extra).ok().map(|x| s(&x, "snippet")).unwrap_or_default();
        let text = format!("{subject} {snip}");
        let Some(status) = status_of(&text) else { continue };
        let dom = from_addr.rsplit('@').next().unwrap_or("").to_lowercase();
        let dom = if dom.matches('.').count() >= 2 { dom.split('.').rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(".") } else { dom };
        let num = ORDER.captures(&text).map(|c| c[1].to_string());
        let what: String = NOISE.replace_all(&subject, " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase().chars().take(30).collect();
        let key = (dom.clone(), num.clone().unwrap_or(what));
        let i = match orders.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                let from: String = if from_name.trim().is_empty() { dom.as_str() } else { from_name.as_str() }.trim().chars().take(30).collect();
                orders.push((key, json!({"from": from})));
                orders.len() - 1
            }
        };
        let o = &mut orders[i].1;
        o["status"] = json!(status); o["ts"] = json!(ts); o["subject"] = json!(subject.chars().take(80).collect::<String>()); o["order"] = json!(num.unwrap_or_default());
    }
    let label = |st: &str| match (l, st) {
        (Lang::Es, "ordered") => "Comprado", (Lang::Es, "shipped") => "En camino", (Lang::Es, "today") => "Llega hoy", (Lang::Es, _) => "Entregado",
        (_, "ordered") => "Ordered", (_, "shipped") => "On its way", (_, "today") => "Arrives today", _ => "Delivered",
    };
    let rank = |st: &str| match st { "today" => 0, "shipped" => 1, "ordered" => 2, _ => 3 };
    let day = |ts: i64| tz.timestamp_opt(ts, 0).single().map(|d| d.date_naive());
    let today = day(now);
    let mut out: Vec<Value> = orders.into_iter().map(|(_, o)| o).filter(|o| {
        let ts = o["ts"].as_i64().unwrap_or(0);
        let (age, same_day) = (now - ts, day(ts) == today);
        match o["status"].as_str().unwrap_or("") {
            "today" | "delivered" => same_day,
            "shipped" => age <= 6 * 86400,
            _ => age <= 10 * 86400,
        }
    }).map(|o| {
        let st = s(&o, "status");
        json!({"from": o["from"], "status": st, "label": label(&st), "when": tz.timestamp_opt(o["ts"].as_i64().unwrap_or(0), 0).single().map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default(),
               "subject": o["subject"], "order": o["order"]})
    }).collect();
    out.sort_by_key(|x| (rank(&s(x, "status")), s(x, "when")));
    out.truncate(8);
    json!({"items": out})
}

pub async fn deliveries(app: &App) -> anyhow::Result<Value> {
    let since = crate::util::now_ts() - 21 * 86400;
    let rows: Vec<ParcelRow> = app.db.call(move |c| {
        let mut st = c.prepare(&format!("SELECT ts, coalesce(from_name,''), coalesce(from_addr,''), coalesce(subject,''), coalesce(extra,'') FROM msg
            WHERE ts > ?1 AND {NOT_JUNK} AND coalesce(category,'') NOT IN ('newsletter', 'cold_email') ORDER BY ts"))?;
        st.query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect()
    }).await?;
    Ok(deliveries_of(rows, Lang::of(app), app.tz(), crate::util::now_ts()))
}

#[cfg(test)]
mod tests {
    use super::*;
    const BA: chrono_tz::Tz = chrono_tz::America::Argentina::Buenos_Aires;
    #[test]
    fn amounts() {
        assert_eq!(value("$ 40.000"), Some(("$".into(), 40000.0)));
        assert_eq!(value("$ 1.234,56"), Some(("$".into(), 1234.56)));
        assert_eq!(value("US$ 1,234.56"), Some(("US$".into(), 1234.56)));
        assert_eq!(value("$ 100.00"), Some(("US$".into(), 100.0)));
        assert_eq!(value("ARS 416"), Some(("$".into(), 416.0)));
        assert_eq!(value("€12,5"), Some(("€".into(), 12.5)));
        assert_eq!(value("about 5"), None);
    }
    #[test]
    fn money_dedup_and_bills() {
        let d = 86400;
        let row = |id: &str, ts: i64, from: &str, subj: &str, amt: &str| (id.to_string(), ts, from.to_string(), String::new(), subj.to_string(), json!({"amount": amt}).to_string());
        let rows = vec![
            row("a", 1_000_000, "Mercado Pago", "Pago aprobado", "$ 40.000"),
            row("b", 1_000_000 + d, "Shop", "Tu compra", "$ 40.000"),        // same payment, second mail
            row("a", 1_000_000 + 2 * d, "Mercado Pago", "Pago aprobado", "$ 40.000"), // same msgid
            row("c", 1_000_000 + 2 * d, "Edenor", "Vence tu factura", "$ 12.500"),
            row("d", 1_000_000 + 3 * d, "Edenor", "Mañana vence tu factura", "$ 12.500"), // reminded again
            row("e", 1_000_000 + 4 * d, "Banco", "Pago de servicios", "$ 12.500"),
            row("f", 1_000_000 + 4 * d, "Banco", "Transferencia recibida", "$ 5.000"),
        ];
        let rem = vec![json!({"kind": "pay", "from": "Personal", "date": "2026-11-25", "amount": "$ 9.000", "subject": "Tu factura"})];
        let v = money_of(rows, &rem, BA, chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(), "x".into());
        assert_eq!(v["paid"]["$"]["n"], 2); // 40.000 once + 12.500
        assert_eq!(v["paid"]["$"]["total"], 52500.0);
        assert_eq!(v["bank"]["$"]["n"], 1);
        let bills = v["bills"].as_array().unwrap();
        assert_eq!(bills.len(), 2);
        // unpaid first: the reminder-only bill, then Edenor paid by the later 12.500
        assert_eq!((s(&bills[0], "from"), bills[0]["paid"].as_bool()), ("Personal".into(), Some(false)));
        assert_eq!((s(&bills[1], "from"), bills[1]["paid"].as_bool()), ("Edenor".into(), Some(true)));
        assert_eq!(v["seen"], 7);
    }
    #[test]
    fn delivery_status() {
        assert_eq!(status_of("Llegó tu compra"), Some("delivered"));
        assert_eq!(status_of("Your package is out for delivery"), Some("today"));
        assert_eq!(status_of("Tu pedido está en camino"), Some("shipped"));
        assert_eq!(status_of("Gracias por tu compra"), Some("ordered"));
        assert_eq!(status_of("Newsletter de ofertas"), None);
        let now = 2_000_000;
        let r = |ts: i64, subj: &str| (ts, "Tienda".to_string(), "envios@mail.shop.example".to_string(), subj.to_string(), "{}".to_string());
        let rows = vec![r(now - 3 * 86400, "Confirmamos tu compra #1234567"), r(now - 86400, "Tu compra #1234567 está en camino"),
                        r(now - 3 * 86400, "Llegó tu pedido #7654321"), r(now - 3600, "Tu factura #1234567 en camino")];
        let v = deliveries_of(rows, Lang::En, BA, now);
        let items = v["items"].as_array().unwrap();
        // one line for order 1234567 (latest: shipped); the old delivery is gone; the invoice is not a parcel
        assert_eq!(items.len(), 1);
        assert_eq!((s(&items[0], "status"), s(&items[0], "label"), s(&items[0], "order")), ("shipped".into(), "On its way".into(), "1234567".into()));
        // "arrives today" holds only on its own day; a delivery shows on the day it happened
        let noon = 1_800_000_000 - 1_800_000_000 % 86400 + 15 * 3600; // 12:00 in Buenos Aires
        let rows = vec![r(noon - 3 * 86400, "Tu pedido #1111111 llega hoy"), r(noon - 2 * 3600, "Tu pedido #2222222 llega hoy"),
                        r(noon - 86400, "Llegó tu pedido #3333333"), r(noon - 3600, "Llegó tu pedido #4444444"),
                        r(noon - 20 * 86400, "Tu pedido #5555555 está en camino")];
        let v = deliveries_of(rows, Lang::En, BA, noon);
        let got: Vec<String> = v["items"].as_array().unwrap().iter().map(|x| format!("{}:{}", s(x, "order"), s(x, "status"))).collect();
        assert_eq!(got, vec!["2222222:today".to_string(), "4444444:delivered".to_string()]);
    }
    #[test]
    fn health_fields() {
        let now = 1_800_000_000;
        let mut v = json!({"ts": now - 30, "uptime_s": 3600, "boots": [{"at": "2027-01-15T07:00:00+00:00", "why": "power"}], "backup": {"ok": true, "at": "2027-01-15T03:00:00+00:00"}});
        health_of(&mut v, now);
        assert_eq!((v["stale"].as_bool(), v["show"].as_bool(), v["booted_at"].as_i64()), (Some(false), Some(true), Some(now - 3600)));
        assert_eq!(v["reboots_7d"].as_array().unwrap().len(), 1);
        assert_eq!(v["backup_ok"].as_bool(), Some(true));
    }
}
