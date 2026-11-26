//! Read-only views over the cache: lists, counts, Reply Zero, senders,
//! analytics, the summary and the brief's mail numbers, mail reminders.
use super::{account, cache::{self, args, Msg}, categorize::{BULK_SYSTEMS, CATEGORIES, FREEMAIL, NOREPLY}, rules, state, sync};
use crate::{state::App, util::now_ts};
use regex::Regex;
use rusqlite::types::Value as Sql;
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, sync::{atomic::Ordering, LazyLock, Mutex}};

/// Kinds that never notify: everything not ticked in Settings → Mail.
pub fn quiet(app: &App) -> Vec<String> {
    let notify: HashSet<String> = app.setting("CERQ_MAIL_NOTIFY").split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    let q: Vec<String> = CATEGORIES.iter().filter(|c| !notify.contains(**c)).map(|c| c.to_string()).collect();
    if q.is_empty() { vec!["__none__".into()] } else { q }
}
fn placeholders(from: usize, n: usize) -> String { (from..from + n).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",") }

pub struct InboxQuery { pub category: String, pub account: String, pub q: String, pub limit: usize, pub folder: String, pub unread: bool }

/// A list across every usable account (folder by role; a missing role skips that account).
pub async fn inbox(app: &App, iq: &InboxQuery) -> anyhow::Result<Vec<Value>> {
    let names = account::names(app).await;
    let mut out: Vec<Msg> = vec![];
    for a in account::accounts(app).await?.into_iter().filter(|a| a.usable() && (iq.account.is_empty() || a.id == iq.account)) {
        let Some(folder) = rules::role(app, &a.id, &iq.folder).await else { continue };
        let mut sql = "WHERE account=?1 AND folder=?2".to_string();
        let mut a2: Vec<Sql> = args(&[&a.id, &folder]);
        if iq.unread { sql += " AND flags NOT LIKE '%Seen%'"; }
        if !iq.category.is_empty() { a2.push(iq.category.clone().into()); sql += &format!(" AND category=?{}", a2.len()); }
        if !iq.q.is_empty() {
            a2.push(format!("%{}%", iq.q).into());
            let n = a2.len();
            sql += &format!(" AND (subject LIKE ?{n} OR from_addr LIKE ?{n} OR from_name LIKE ?{n})");
        }
        a2.push((iq.limit as i64).into());
        sql += &format!(" ORDER BY ts DESC LIMIT ?{}", a2.len());
        out.extend(cache::msgs(app, &sql, a2).await?);
    }
    out.sort_by_key(|m| -m.ts);
    out.truncate(iq.limit);
    Ok(out.iter().map(|m| m.view(&names)).collect())
}

/// Unread in each inbox, the important ones, and what's in each Junk folder.
pub async fn counts(app: &App) -> anyhow::Result<Value> {
    let accts = account::accounts(app).await?;
    let (mut unread, mut junk) = (serde_json::Map::new(), serde_json::Map::new());
    let (mut total, mut junk_total) = (0, 0);
    for a in accts.iter().filter(|a| a.usable()) {
        let n = cache::count(app, "SELECT count(*) FROM msg WHERE account=?1 AND folder='INBOX' AND flags NOT LIKE '%Seen%'", args(&[&a.id])).await?;
        total += n;
        unread.insert(a.id.clone(), json!(n));
        if let Some(f) = rules::role(app, &a.id, "junk").await {
            let j = cache::count(app, "SELECT count(*) FROM msg WHERE account=?1 AND folder=?2", args(&[&a.id, &f])).await?;
            junk_total += j;
            junk.insert(a.id.clone(), json!(j));
        }
    }
    let q = quiet(app);
    let important = cache::count(app, &format!("SELECT count(*) FROM msg WHERE folder='INBOX' AND flags NOT LIKE '%Seen%' AND category NOT IN ({})", placeholders(1, q.len())),
        q.into_iter().map(Sql::from).collect()).await?;
    let names: serde_json::Map<String, Value> = accts.into_iter().map(|a| (a.id, json!(a.name))).collect();
    Ok(json!({"unread": unread, "total": total, "junk": junk, "junk_total": junk_total, "important": important, "names": names}))
}

/// Unread inbox mail newer than `since` that deserves a notification (≤20).
pub async fn fresh(app: &App, since: i64) -> anyhow::Result<Vec<Value>> {
    let names = account::names(app).await;
    let ok: HashSet<String> = account::accounts(app).await?.into_iter().filter(|a| a.usable()).map(|a| a.id).collect();
    let q = quiet(app);
    let mut a: Vec<Sql> = vec![since.into()];
    let ph = placeholders(2, q.len());
    a.extend(q.into_iter().map(Sql::from));
    let rows = cache::msgs(app, &format!("WHERE folder='INBOX' AND ts>?1 AND flags NOT LIKE '%Seen%' AND category NOT IN ({ph}) ORDER BY ts DESC LIMIT 20"), a).await?;
    Ok(rows.iter().filter(|m| ok.contains(&m.account)).map(|m| m.view(&names)).collect())
}

/// Reply Zero rows nobody can answer by mail: calendar invitations (answered
/// in Calendar), "No responder" senders, and bill notices (the brief lists bills).
static NOT_A_REPLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(updated )?invitation:|^invitaci[oó]n|^accepted:|^declined:|^tentative:|^aceptad|^rechazad|^declinad|^provisional|no[- ]?responder|no[- ]?reply|do not reply|pr[oó]ximos? vencimientos?|factura (est[aá]|ya est[aá]) disponible").unwrap());
/// "Ana reacted to your message" (Outlook), "… reacted via Gmail": a reaction
/// to your reply, not a mail to answer. Matched on the subject and the snippet.
static REACTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\breacted (to your (message|email|mail)|via gmail)\b|reaccion[oó] (a tu (mensaje|correo|email)|a trav[eé]s de gmail)").unwrap());
pub fn is_reaction(m: &Msg) -> bool {
    m.ctype.contains("email-reaction") || REACTION.is_match(&m.subject)
        || m.extra.as_deref().and_then(|x| serde_json::from_str::<serde_json::Value>(x).ok()).is_some_and(|v| REACTION.is_match(v["snippet"].as_str().unwrap_or("")))
}
static UNSUB_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(unsubscribe|remove|stop|baja|desuscribir)\s*$").unwrap());

/// What Reply Zero needs besides the cache rows.
pub struct Who<'a> { pub mine: &'a HashSet<String>, pub dismissed: &'a HashSet<String>, pub known: &'a HashSet<String>, pub people: &'a HashSet<String> }

/// The company domain of an address (None for freemail).
fn domain_of(a: &str) -> Option<&str> { if FREEMAIL.is_match(a) { None } else { a.split_once('@').map(|(_, d)| d) } }

fn later(map: &mut HashMap<String, i64>, k: String, ts: i64) { let e = map.entry(k).or_insert(0); *e = (*e).max(ts); }

/// To reply: people wrote to you and you haven't answered. Awaiting: you
/// wrote and they haven't (with the recipients and days waited). Pure:
/// `sent` and `inbox` are the cache rows of the window.
pub fn reply_zero_of<'a>(sent: &'a [Msg], inbox: &'a [Msg], w: &Who, now: i64) -> (Vec<&'a Msg>, Vec<(&'a Msg, Vec<String>, i64)>) {
    let mut replied_to: HashSet<&str> = HashSet::new();
    // Address / company domain → when you last wrote to it.
    let (mut wrote_to, mut wrote_domain) = (HashMap::new(), HashMap::new());
    for s in sent {
        replied_to.extend(s.in_reply_to.split_whitespace().chain(s.refs.split_whitespace()));
        for a in format!("{},{}", s.to_addrs, s.cc_addrs).split(',') {
            let a = a.trim().to_lowercase();
            if !a.contains('@') { continue; }
            if let Some(d) = domain_of(&a) { later(&mut wrote_domain, d.to_string(), s.ts); }
            later(&mut wrote_to, a, s.ts);
        }
    }
    let mut to_reply = vec![];
    for r in inbox {
        if !(r.category == "personal" || r.category == "to_reply") || w.mine.contains(&r.from_addr) { continue; }
        if r.flags.contains("\\Answered") || replied_to.contains(r.msgid.as_str()) || w.dismissed.contains(&r.msgid) { continue; }
        if NOT_A_REPLY.is_match(&r.subject) || is_reaction(r) { continue; }
        // You wrote to them after this mail, in any thread (accepting their
        // invitation counts), or to their company (a recruiter loops in a
        // colleague, you answer the colleague): answered.
        let who = r.from_addr.to_lowercase();
        if wrote_to.get(&who).copied().unwrap_or(0) > r.ts { continue; }
        if who.contains('@') && domain_of(&who).is_some_and(|d| wrote_domain.get(d).copied().unwrap_or(0) > r.ts) { continue; }
        // Only when you were a direct recipient.
        if !r.to_addrs.split(',').any(|x| w.mine.contains(x)) { continue; }
        // A person: someone you've written to, someone on a personal mail
        // service, or a sender you approved. Unknown company senders
        // (welcome mails, policy notices) don't count.
        if !(w.known.contains(&r.from_addr) || FREEMAIL.is_match(&r.from_addr) || w.people.contains(&r.from_addr)) { continue; }
        to_reply.push(r);
    }
    let mut answered: HashSet<&str> = HashSet::new();
    let (mut heard_from, mut heard_domain) = (HashMap::new(), HashMap::new());
    for r in inbox {
        answered.extend(r.in_reply_to.split_whitespace().chain(r.refs.split_whitespace()));
        let a = r.from_addr.to_lowercase();
        if !a.contains('@') { continue; }
        if let Some(d) = domain_of(&a) { later(&mut heard_domain, d.to_string(), r.ts); }
        later(&mut heard_from, a, r.ts);
    }
    let mut awaiting = vec![];
    for s in sent {
        if answered.contains(s.msgid.as_str()) || w.dismissed.contains(&s.msgid) || now - s.ts < 86400 { continue; }
        // Your "Accepted: …" to an invitation waits for nothing.
        if NOT_A_REPLY.is_match(&s.subject) { continue; }
        let to: Vec<String> = s.to_addrs.split(',').filter(|x| !x.is_empty() && !w.mine.contains(*x)).map(String::from).collect();
        if to.is_empty() || to.iter().any(|x| NOREPLY.is_match(x) || BULK_SYSTEMS.is_match(x)) { continue; }
        // They (or their company) wrote back after it, in any thread: not
        // waiting (the same mail sent from two accounts has two ids).
        if to.iter().any(|x| {
            let x = x.trim().to_lowercase();
            heard_from.get(&x).copied().unwrap_or(0) > s.ts || (x.contains('@') && domain_of(&x).is_some_and(|d| heard_domain.get(d).copied().unwrap_or(0) > s.ts))
        }) { continue; }
        if UNSUB_ONLY.is_match(&s.subject) { continue; }
        awaiting.push((s, to.into_iter().take(3).collect(), (now - s.ts) / 86400));
    }
    to_reply.sort_by_key(|m| -m.ts);
    awaiting.sort_by_key(|(m, _, _)| -m.ts);
    awaiting.truncate(60);
    (to_reply, awaiting)
}

pub async fn reply_zero(app: &App, days: i64) -> anyhow::Result<Value> {
    let names = account::names(app).await;
    let st = state::get(app).await;
    let mine = account::my_addresses(&account::accounts(app).await?, &st.aliases);
    let since = now_ts() - days * 86400;
    let sent = cache::msgs(app, "WHERE folder LIKE '%sent%' AND ts>?1", args(&[&since])).await?;
    let inbox = cache::msgs(app, "WHERE folder='INBOX' AND ts>?1", args(&[&since])).await?;
    let known = sync::known_senders(app).await?;
    let dismissed: HashSet<String> = st.dismissed.iter().cloned().collect();
    let people: HashSet<String> = st.cold.allow.iter().cloned().collect();
    let (to_reply, awaiting) = reply_zero_of(&sent, &inbox, &Who { mine: &mine, dismissed: &dismissed, known: &known, people: &people }, now_ts());
    Ok(json!({
        "to_reply": to_reply.iter().map(|m| m.view(&names)).collect::<Vec<_>>(),
        "awaiting": awaiting.iter().map(|(m, to, days)| { let mut v = m.view(&names); v["to"] = json!(to); v["days"] = json!(days); v }).collect::<Vec<_>>(),
    }))
}

pub async fn senders(app: &App, account: &str, limit: i64) -> anyhow::Result<Vec<Value>> {
    let mut sql = r"SELECT from_addr, max(from_name) name, count(*) n, sum(CASE WHEN flags NOT LIKE '%\Seen%' THEN 1 ELSE 0 END) unread,
        max(ts) last, max(list_unsub != '') unsub, max(category) cat, group_concat(DISTINCT account) accts
        FROM msg WHERE folder='INBOX' AND from_addr != ''".to_string();
    let mut a: Vec<Sql> = vec![];
    if !account.is_empty() { a.push(account.to_string().into()); sql += " AND account=?1"; }
    a.push(limit.into());
    sql += &format!(" GROUP BY from_addr ORDER BY n DESC LIMIT ?{}", a.len());
    let mut out = cache::json_rows(app, &sql, a).await?;
    let blocked = rules::blocked(&state::get(app).await.rules);
    for o in &mut out {
        let (n, u) = (o["n"].as_i64().unwrap_or(0), o["unread"].as_i64().unwrap_or(0));
        o["unread_pct"] = json!((100.0 * u as f64 / n.max(1) as f64).round() as i64);
        o["blocked"] = json!(blocked.contains(o["from_addr"].as_str().unwrap_or("")));
    }
    Ok(out)
}

pub async fn analytics(app: &App, days: i64) -> anyhow::Result<Value> {
    let since = now_ts() - days * 86400;
    let tz = app.tz();
    let rows = cache::json_rows(app, "SELECT folder, ts FROM msg WHERE ts>?1", args(&[&since])).await?;
    let mut daily: std::collections::BTreeMap<String, [i64; 3]> = Default::default();
    for r in rows {
        let Some(ts) = r["ts"].as_i64() else { continue };
        let d = chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default().with_timezone(&tz).format("%Y-%m-%d").to_string();
        let f = r["folder"].as_str().unwrap_or("").to_lowercase();
        let k = if f.contains("sent") { 1 } else if f.contains("junk") || f.contains("spam") { 2 } else { 0 };
        daily.entry(d).or_default()[k] += 1;
    }
    let cats = cache::json_rows(app, "SELECT category, count(*) n FROM msg WHERE folder='INBOX' AND ts>?1 GROUP BY category ORDER BY n DESC", args(&[&since])).await?;
    let total = cache::count(app, "SELECT count(*) FROM msg WHERE folder='INBOX'", vec![]).await?;
    let unread = cache::count(app, r"SELECT coalesce(sum(CASE WHEN flags NOT LIKE '%\Seen%' THEN 1 ELSE 0 END),0) FROM msg WHERE folder='INBOX'", vec![]).await?;
    let domains = cache::json_rows(app, "SELECT substr(from_addr, instr(from_addr,'@')+1) dom, count(*) n FROM msg WHERE folder='INBOX' AND ts>?1 GROUP BY dom ORDER BY n DESC LIMIT 10", args(&[&since])).await?;
    // Your reply time: sent replies matched to the message they answer.
    let waits = app.db.call(|c| {
        let mut inbox_ts: HashMap<String, i64> = HashMap::new();
        let mut st = c.prepare("SELECT msgid, ts FROM msg WHERE folder='INBOX' AND msgid != ''")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? { inbox_ts.insert(cache::txt(r, 0), cache::num(r, 1)); }
        let mut st = c.prepare("SELECT in_reply_to, ts FROM msg WHERE folder LIKE '%sent%' AND in_reply_to != ''")?;
        let mut rows = st.query([])?;
        let mut w = vec![];
        while let Some(r) = rows.next()? {
            if let Some(t) = inbox_ts.get(&cache::txt(r, 0)) { let ts = cache::num(r, 1); if ts > *t { w.push(ts - t); } }
        }
        w.sort_unstable();
        Ok(w)
    }).await?;
    let median = waits.get(waits.len() / 2).map(|w| (*w as f64 / 3600.0 * 10.0).round() / 10.0);
    Ok(json!({
        "days": daily.into_iter().map(|(d, [r, s, j])| json!({"day": d, "received": r, "sent": s, "junk": j})).collect::<Vec<_>>(),
        "categories": cats, "inbox_total": total, "inbox_unread": unread, "top_domains": domains, "median_reply_hours": median,
    }))
}

pub async fn summary(app: &App) -> anyhow::Result<Value> {
    let rz = reply_zero(app, 30).await?;
    let day = now_ts() - 86400;
    let today: serde_json::Map<String, Value> = cache::json_rows(app, "SELECT category, count(*) n FROM msg WHERE folder='INBOX' AND ts>?1 GROUP BY category", args(&[&day])).await?
        .into_iter().map(|r| (r["category"].as_str().unwrap_or("").to_string(), r["n"].clone())).collect();
    let acts = cache::count(app, "SELECT count(*) FROM actlog WHERE ts>?1 AND status='applied' AND undone=0", args(&[&day])).await?;
    let last = state::get(app).await.runs.last().cloned().unwrap_or(json!({}));
    let top: Vec<Value> = rz["to_reply"].as_array().map(|a| a.iter().take(5).cloned().collect()).unwrap_or_default();
    Ok(json!({"to_reply": rz["to_reply"].as_array().map_or(0, |a| a.len()), "awaiting": rz["awaiting"].as_array().map_or(0, |a| a.len()),
        "unread": counts(app).await?["important"], "today": today, "actions_today": acts, "top_to_reply": top, "last_run": last}))
}

/// The brief's mail numbers, cached until mail changes (or 5 min): Reply Zero
/// takes a moment on the Pi.
static BRIEF: Mutex<Option<(u64, i64, Value)>> = Mutex::new(None);

pub async fn brief_mail(app: &App) -> anyhow::Result<Value> {
    let ver = super::VERSION.load(Ordering::SeqCst);
    if let Some((v, t, val)) = BRIEF.lock().unwrap().as_ref() {
        if *v == ver && now_ts() - t < 300 { return Ok(val.clone()); }
    }
    let now = now_ts();
    let rz = reply_zero(app, 30).await?;
    let q = quiet(app);
    let mut a: Vec<Sql> = vec![(now - 12 * 3600).into()];
    let ph = placeholders(2, q.len());
    a.extend(q.into_iter().map(Sql::from));
    let new = cache::count(app, &format!("SELECT count(*) FROM msg WHERE folder='INBOX' AND ts>?1 AND category NOT IN ({ph})"), a).await?;
    let acts = cache::count(app, "SELECT count(*) FROM actlog WHERE ts>?1 AND status='applied' AND undone=0", args(&[&(now - 86400)])).await?;
    // Already without invitations, no-reply senders and bills.
    let mut people: Vec<Value> = rz["to_reply"].as_array().cloned().unwrap_or_default();
    people.sort_by_key(|m| m["ts"].as_i64().unwrap_or(0));
    let days = |m: &Value| (now - m["ts"].as_i64().unwrap_or(now)) / 86400;
    let v = json!({
        "to_reply": rz["to_reply"].as_array().map_or(0, |a| a.len()), "awaiting": rz["awaiting"].as_array().map_or(0, |a| a.len()),
        "unread_important": counts(app).await?["important"], "new_overnight": new, "auto_handled_24h": acts,
        "people_waiting": people.len(), "oldest_to_reply_days": people.first().map(days).unwrap_or(0),
        "waiting": people.iter().take(3).map(|m| {
            let from = m["from_name"].as_str().filter(|s| !s.is_empty()).or(m["from_addr"].as_str()).unwrap_or("");
            json!({"from": from, "subject": m["subject"].as_str().unwrap_or("").chars().take(90).collect::<String>(), "days": days(m), "id": format!("{}:{}", m["account"].as_str().unwrap_or(""), m["uid"])})
        }).collect::<Vec<_>>(),
    });
    *BRIEF.lock().unwrap() = Some((ver, now, v.clone()));
    Ok(v)
}

/// Due dates and invitations read from inbox mail (msg.extra), one per thing.
pub async fn mail_reminders(app: &App) -> anyhow::Result<Vec<Value>> {
    let since = now_ts() - 45 * 86400;
    let names = account::names(app).await;
    let rows = cache::msgs(app, r#"WHERE folder='INBOX' AND ts>?1 AND category IN ('receipt','personal','calendar') AND (extra LIKE '%"due"%' OR extra LIKE '%"when"%') ORDER BY ts DESC LIMIT 60"#, args(&[&since])).await?;
    let mut out: Vec<Value> = vec![];
    let mut seen = HashSet::new();
    for r in rows {
        let x = r.extra_json();
        let who = if r.from_name.is_empty() { r.from_addr.clone() } else { r.from_name.clone() };
        let rid = format!("mail:{}", if r.msgid.is_empty() { format!("{}:{}", r.account, r.uid) } else { r.msgid.clone() });
        let link = format!("{}:{}", r.account, r.uid);
        let acct = names.get(&r.account).cloned().unwrap_or_default();
        let subject: String = r.subject.chars().take(90).collect();
        let item = if let Some(due) = x["due"].as_str() {
            let amount = x["amount"].as_str().unwrap_or("");
            let title = if amount.is_empty() { format!("Pay {who}") } else { format!("Pay {who} {amount}") };
            json!({"id": rid, "kind": "pay", "title": title.chars().take(90).collect::<String>(), "date": due, "time": "", "from": who, "subject": subject, "amount": amount, "msg": link, "account": acct})
        } else if let Some(when) = x["when"].as_str() {
            let title = if r.subject.is_empty() { who.clone() } else { r.subject.clone() };
            json!({"id": rid, "kind": "event", "title": title.chars().take(90).collect::<String>(), "date": when.get(..10).unwrap_or(when), "time": when.get(11..16).unwrap_or(""),
                "from": who, "subject": subject, "amount": "", "msg": link, "account": acct})
        } else { continue };
        // One offer per thing (the same bill mailed twice).
        if seen.insert((item["kind"].to_string(), item["date"].to_string(), who.to_lowercase())) { out.push(item); }
    }
    Ok(out)
}

/// Cached mail matching words in the subject, sender or preview, newest first (no junk).
pub async fn search(app: &App, q: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let q = q.trim();
    if q.is_empty() { return Ok(vec![]); }
    let names = account::names(app).await;
    let rows = cache::msgs(app, "WHERE (subject LIKE ?1 OR from_addr LIKE ?1 OR from_name LIKE ?1 OR extra LIKE ?1) AND lower(folder) NOT LIKE '%junk%' AND lower(folder) NOT LIKE '%spam%' ORDER BY ts DESC LIMIT ?2",
        args(&[&format!("%{q}%"), &(limit as i64)])).await?;
    Ok(rows.iter().map(|m| m.view(&names)).collect())
}

/// INBOX mail older than `days` (the Bulk Archiver); starred mail is kept.
pub async fn old_mail(app: &App, days: i64, category: &str, account_id: &str) -> anyhow::Result<Vec<Msg>> {
    let mut sql = "WHERE folder='INBOX' AND ts < ?1 AND flags NOT LIKE '%Flagged%'".to_string();
    let mut a: Vec<Sql> = vec![(now_ts() - days.max(1) * 86400).into()];
    if !category.is_empty() { a.push(category.to_string().into()); sql += &format!(" AND category=?{}", a.len()); }
    if !account_id.is_empty() { a.push(account_id.to_string().into()); sql += &format!(" AND account=?{}", a.len()); }
    let ok: HashSet<String> = account::accounts(app).await?.into_iter().filter(|a| a.usable()).map(|a| a.id).collect();
    Ok(cache::msgs(app, &sql, a).await?.into_iter().filter(|m| ok.contains(&m.account)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn msg(folder: &str, from: &str, to: &str, subject: &str, ts: i64) -> Msg {
        Msg { folder: folder.into(), from_addr: from.into(), to_addrs: to.into(), subject: subject.into(), ts, category: "personal".into(),
            msgid: format!("<{subject}@{from}>"), ..Default::default() }
    }

    #[test]
    fn reply_zero_filters() {
        let now = 1_000_000;
        let mine: HashSet<String> = ["me@gmail.com".to_string()].into();
        let (dismissed, known, people) = (HashSet::new(), HashSet::new(), HashSet::new());
        let w = Who { mine: &mine, dismissed: &dismissed, known: &known, people: &people };
        let inbox = vec![
            msg("INBOX", "ana@gmail.com", "me@gmail.com", "Dinner?", now - 3600),             // waits for you
            msg("INBOX", "bob@acme.com", "me@gmail.com", "Hello", now - 3600),                // unknown company sender
            msg("INBOX", "cat@gmail.com", "list@x.com,me@gmail.com", "Cc'd", now - 3600),     // a direct recipient among others
            msg("INBOX", "dan@gmail.com", "other@gmail.com", "Not to you", now - 3600),       // cc'd or bcc'd only
            msg("INBOX", "eve@gmail.com", "me@gmail.com", "Invitation: Party", now - 3600),   // a calendar invite
            Msg { flags: "\\Answered".into(), ..msg("INBOX", "fay@gmail.com", "me@gmail.com", "Answered", now - 3600) },
            msg("INBOX", "gus@gmail.com", "me@gmail.com", "You wrote after", now - 7200),
            // a reaction to your reply (Outlook puts it in the snippet; Gmail in the subject or a reaction part)
            Msg { extra: Some(r#"{"snippet":"[heart] Nat reacted to your message: Hi Nat, thanks"}"#.into()), ..msg("INBOX", "nat@gmail.com", "me@gmail.com", "RE: Resume", now - 1800) },
            msg("INBOX", "olga@gmail.com", "me@gmail.com", "Olga reacted via Gmail", now - 1800),
            Msg { ctype: "text/vnd.google.email-reaction+json".into(), ..msg("INBOX", "pia@gmail.com", "me@gmail.com", "Re: Plans", now - 1800) },
        ];
        let sent = vec![
            msg("Sent", "me@gmail.com", "gus@gmail.com", "Later", now - 3000),
            msg("Sent", "me@gmail.com", "zoe@gmail.com", "Any news?", now - 3 * 86400),       // awaiting
            msg("Sent", "me@gmail.com", "ana@gmail.com", "Old one", now - 2 * 86400),         // Ana wrote since
            msg("Sent", "me@gmail.com", "noreply@shop.com", "Question", now - 2 * 86400),     // a system address
            msg("Sent", "me@gmail.com", "kim@gmail.com", "Fresh", now - 3600),                // less than a day
            msg("Sent", "me@gmail.com", "list@x.com", "unsubscribe", now - 2 * 86400),
        ];
        let (to_reply, awaiting) = reply_zero_of(&sent, &inbox, &w, now);
        let subj: Vec<&str> = to_reply.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(subj, vec!["Dinner?", "Cc'd"]);
        let aw: Vec<(&str, i64)> = awaiting.iter().map(|(m, _, d)| (m.subject.as_str(), *d)).collect();
        assert_eq!(aw, vec![("Any news?", 3)]);
        assert_eq!(awaiting[0].1, vec!["zoe@gmail.com"]);
        // Dismissed mail drops out; a known company sender counts as a person.
        let dismissed: HashSet<String> = ["<Dinner?@ana@gmail.com>".to_string()].into();
        let known: HashSet<String> = ["bob@acme.com".to_string()].into();
        let w = Who { mine: &mine, dismissed: &dismissed, known: &known, people: &people };
        let (to_reply, _) = reply_zero_of(&sent, &inbox, &w, now);
        let subj: Vec<&str> = to_reply.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(subj, vec!["Hello", "Cc'd"]);
    }
}
