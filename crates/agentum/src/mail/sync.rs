//! The background mail sync (cerq-mail's runner/run_once): per usable account
//! folder roles, Sent → INBOX → Junk incremental sync, body facts, categories,
//! rules; then push for new mail. Accounts sync in parallel (a few at a time),
//! each on one IMAP connection; fetches go in chunks of 100 headers.
use super::{account::{self, Account}, cache::{self, args, Msg}, categorize::categorize, enrich, imap::{self, Session}, rules, state, views};
use crate::{state::App, util::now_ts};
use futures::StreamExt;
use serde_json::{json, Value};
use std::{collections::HashSet, sync::{atomic::{AtomicI64, Ordering}, LazyLock}, time::Duration};
use tokio::sync::{Mutex, Notify};

/// Messages cached per folder.
const KEEP: usize = 1500;
/// Accounts synced at the same time.
const PARALLEL: usize = 3;
const HEADER_FIELDS: &str = "FROM TO CC SUBJECT DATE MESSAGE-ID IN-REPLY-TO REFERENCES LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST LIST-ID PRECEDENCE AUTO-SUBMITTED CONTENT-TYPE";

/// One sync at a time (the loop and POST /run); never held by UI actions.
static RUN: Mutex<()> = Mutex::const_new(());
pub static LAST_RUN: AtomicI64 = AtomicI64::new(0);
/// Wakes the loop early (after sending mail, applying a rule…).
pub static KICK: LazyLock<Notify> = LazyLock::new(Notify::new);

pub fn start(app: App) {
    tokio::spawn(async move {
        migrate(&app).await;
        loop {
            let _ = run_once(&app).await;
            super::message::expire_uploads(&app).await;
            let secs = app.setting("CERQ_MAIL_INTERVAL").parse::<u64>().unwrap_or(60).max(30);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(secs)) => {}
                _ = KICK.notified() => {}
            }
        }
    });
}

/// Read recent bills and invites again for due dates and times (cerq-mail
/// did this once, at enrich_v 3).
async fn migrate(app: &App) {
    if state::get(app).await.enrich_v >= 3 { return; }
    let since = now_ts() - 45 * 86400;
    let _ = app.db.call(move |c| c.execute("UPDATE msg SET extra=NULL WHERE folder='INBOX' AND category IN ('receipt','personal','calendar') AND ts>?1", [since])).await;
    let _ = state::update(app, |s| s.enrich_v = 3).await;
}

/// Map special-use roles (inbox/sent/junk/archive/trash/all) to folder names; cached in `folders`.
pub fn roles_from(list: &[(String, Vec<String>)]) -> Value {
    let mut roles = serde_json::Map::new();
    roles.insert("inbox".into(), json!("INBOX"));
    for (name, flags) in list {
        for (role, flag) in [("sent", "\\sent"), ("junk", "\\junk"), ("archive", "\\archive"), ("trash", "\\trash"), ("all", "\\all")] {
            if flags.iter().any(|f| f == flag) && !roles.contains_key(role) { roles.insert(role.into(), json!(name)); }
        }
    }
    for (role, guesses) in [("sent", &["sent messages", "sent", "[gmail]/sent mail", "enviados"][..]), ("junk", &["junk", "spam", "[gmail]/spam", "junk e-mail"]),
                            ("archive", &["archive", "archivo"]), ("trash", &["deleted messages", "trash", "[gmail]/trash", "papelera"])] {
        if roles.contains_key(role) { continue; }
        if let Some(g) = guesses.iter().find_map(|g| list.iter().find(|(n, _)| n.to_lowercase() == *g)) { roles.insert(role.into(), json!(g.0)); }
    }
    roles.insert("_all".into(), json!(list.iter().map(|(n, _)| n).collect::<Vec<_>>()));
    Value::Object(roles)
}

/// A cache row from fetched headers.
pub fn parse_header(f: &imap::Fetched, account: &str, folder: &str, uidvalidity: i64) -> Msg {
    let raw = f.header.as_deref().unwrap_or(b"");
    let h = mail_parser::MessageParser::default().parse_headers(raw);
    let raw_h = |name: &str| h.as_ref().and_then(|m| m.header_raw(name)).map(|v| v.trim().to_string()).unwrap_or_default();
    let collapse = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let addrs = |a: Option<&mail_parser::Address>| a.map(|a| a.iter().filter_map(|x| x.address.as_ref().map(|s| s.to_lowercase())).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(",")).unwrap_or_default();
    let (from_name, from_addr) = h.as_ref().and_then(|m| m.from()).and_then(|a| a.first())
        .map(|x| (x.name.as_deref().unwrap_or("").to_string(), x.address.as_deref().unwrap_or("").to_lowercase())).unwrap_or_default();
    let date = h.as_ref().and_then(|m| m.date()).filter(|d| (1971..=2200).contains(&d.year)).map(|d| d.to_timestamp());
    let idate = chrono::DateTime::parse_from_str(f.internal_date.trim(), "%d-%b-%Y %H:%M:%S %z").ok().map(|d| d.timestamp());
    let ctype = h.as_ref().and_then(|m| m.root_part().headers.iter().find(|x| x.name().eq_ignore_ascii_case("content-type")).and_then(|x| x.value().as_content_type()).map(|c| match c.subtype() {
        Some(s) => format!("{}/{}", c.ctype(), s), None => c.ctype().to_string() }.to_lowercase())).unwrap_or_default();
    Msg {
        account: account.into(), folder: folder.into(), uid: f.uid as i64, uidvalidity,
        msgid: raw_h("Message-ID"), in_reply_to: raw_h("In-Reply-To"), refs: collapse(raw_h("References")), thrid: f.thrid.clone(),
        from_name, from_addr, to_addrs: addrs(h.as_ref().and_then(|m| m.to())), cc_addrs: addrs(h.as_ref().and_then(|m| m.cc())),
        subject: h.as_ref().and_then(|m| m.subject()).unwrap_or("").trim().chars().take(400).collect(),
        ts: date.or(idate).unwrap_or_else(now_ts), flags: f.flags.clone(), labels: f.labels.clone(),
        list_unsub: collapse(raw_h("List-Unsubscribe")).chars().take(1000).collect(),
        list_unsub_post: raw_h("List-Unsubscribe-Post").chars().take(200).collect(),
        list_id: h.as_ref().and_then(|m| m.list_id().as_text()).map(|s| s.trim().to_string()).unwrap_or_else(|| raw_h("List-Id")).chars().take(200).collect(),
        precedence: raw_h("Precedence").to_lowercase(), auto_submitted: raw_h("Auto-Submitted").to_lowercase(), ctype,
        ..Default::default()
    }
}

#[derive(Default)]
struct FolderSync { new: usize, changed: usize }

async fn sync_folder(app: &App, s: &mut Session, a: &Account, folder: &str, known: &HashSet<String>, mine: &HashSet<String>, limit: usize) -> anyhow::Result<FolderSync> {
    let mut out = FolderSync::default();
    let Ok(mb) = imap::examine(s, folder).await else { return Ok(out) };
    let uv = mb.uid_validity.unwrap_or(0) as i64;
    let (aid, f) = (a.id.clone(), folder.to_string());
    // A new UIDVALIDITY means every cached UID is stale.
    let last: i64 = app.db.call(move |c| {
        let (mx, cuv): (i64, i64) = c.query_row("SELECT coalesce(max(uid),0), coalesce(max(uidvalidity),0) FROM msg WHERE account=?1 AND folder=?2", [&aid, &f], |r| Ok((cache::num(r, 0), cache::num(r, 1))))?;
        if cuv != 0 && cuv != uv {
            c.execute("DELETE FROM msg WHERE account=?1 AND folder=?2", [&aid, &f])?;
            c.execute("DELETE FROM mail_body WHERE account=?1 AND folder=?2", [&aid, &f])?;
            return Ok(0);
        }
        Ok(mx)
    }).await?;
    let mut uids: Vec<u32> = imap::uid_search(s, &if last > 0 { format!("UID {}:*", last + 1) } else { "ALL".into() }).await?
        .into_iter().filter(|u| *u as i64 > last).collect();
    if uids.len() > limit { uids.drain(..uids.len() - limit); }
    let gm = if a.is_gmail() { " X-GM-THRID X-GM-LABELS" } else { "" };
    for chunk in uids.chunks(100) {
        let mut rows = vec![];
        imap::uid_fetch(s, &imap::set(chunk), &format!("(UID FLAGS INTERNALDATE{gm} BODY.PEEK[HEADER.FIELDS ({HEADER_FIELDS})])"), |f| {
            let mut m = parse_header(&f, &a.id, folder, uv);
            m.category = categorize(&m, known, mine).into();
            rows.push(m);
        }).await?;
        out.new += rows.len();
        app.db.call(move |c| { let tx = c.transaction()?; for m in &rows { m.insert(&tx)?; } tx.commit() }).await?;
    }
    // Drop rows that left the folder, refresh the flags of the newest (read,
    // answered, starred change often), and trim the cache.
    let present = imap::uid_search(s, "ALL").await?;
    let keep: Vec<u32> = present[present.len().saturating_sub(300)..].to_vec();
    let mut flags: Vec<(u32, String)> = Vec::with_capacity(keep.len());
    imap::uid_fetch(s, &imap::set(&keep), "(UID FLAGS)", |f| flags.push((f.uid, f.flags))).await?;
    let (aid, f) = (a.id.clone(), folder.to_string());
    out.changed += app.db.call(move |c| {
        let tx = c.transaction()?;
        let present: HashSet<u32> = present.into_iter().collect();
        let cached: Vec<i64> = tx.prepare("SELECT uid FROM msg WHERE account=?1 AND folder=?2")?.query_map([&aid, &f], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let mut n = 0;
        for u in cached.into_iter().filter(|u| !present.contains(&(*u as u32))) {
            n += tx.execute("DELETE FROM msg WHERE account=?1 AND folder=?2 AND uid=?3", rusqlite::params![aid, f, u])?;
        }
        for (u, fl) in &flags {
            n += tx.execute("UPDATE msg SET flags=?1 WHERE account=?2 AND folder=?3 AND uid=?4 AND flags IS NOT ?1", rusqlite::params![fl, aid, f, u])?;
        }
        tx.execute("DELETE FROM msg WHERE account=?1 AND folder=?2 AND uid NOT IN (SELECT uid FROM msg WHERE account=?1 AND folder=?2 ORDER BY uid DESC LIMIT ?3)", rusqlite::params![aid, f, KEEP as i64])?;
        tx.commit()?;
        Ok(n)
    }).await?;
    Ok(out)
}

/// Body facts for up to 25 recent codes, receipts, people and invites.
async fn enrich(app: &App, s: &mut Session, a: &Account) -> anyhow::Result<usize> {
    let since = now_ts() - 45 * 86400;
    let todo = cache::msgs(app, "WHERE account=?1 AND folder='INBOX' AND extra IS NULL AND ts>?2 AND category IN (?3,?4,?5,?6) ORDER BY ts DESC LIMIT 25",
        args(&[&a.id, &since, &enrich::ENRICH[0], &enrich::ENRICH[1], &enrich::ENRICH[2], &enrich::ENRICH[3]])).await?;
    if todo.is_empty() { return Ok(0); }
    imap::examine(s, "INBOX").await?;
    let today = chrono::Utc::now().with_timezone(&app.tz()).date_naive();
    let month_first = app.setting("CERQ_DATE_ORDER") == "Month first";
    let uids: Vec<u32> = todo.iter().map(|m| m.uid as u32).collect();
    let mut facts: Vec<(i64, String)> = vec![];
    imap::uid_fetch(s, &imap::set(&uids), "(UID BODY.PEEK[]<0.120000>)", |f| {
        let Some(m) = todo.iter().find(|m| m.uid == f.uid as i64) else { return };
        let x = enrich::body_facts(f.body.as_deref().unwrap_or(b""), &m.category, &m.subject, today, month_first);
        facts.push((m.uid, Value::Object(x).to_string()));
    }).await?;
    let n = facts.len();
    let aid = a.id.clone();
    // Mark even the ones without facts, so they aren't fetched again.
    app.db.call(move |c| {
        let tx = c.transaction()?;
        for u in uids {
            let x = facts.iter().find(|(id, _)| *id == u as i64).map(|(_, x)| x.as_str()).unwrap_or("{}");
            tx.execute("UPDATE msg SET extra=?1 WHERE account=?2 AND folder='INBOX' AND uid=?3", rusqlite::params![x, aid, u])?;
        }
        tx.commit()
    }).await?;
    Ok(n)
}

/// Apply the current category rules to cached mail (AI-labelled rows keep theirs).
async fn recategorize(app: &App, aid: &str, known: HashSet<String>, mine: HashSet<String>, allow: Vec<String>) -> anyhow::Result<usize> {
    let aid = aid.to_string();
    app.db.call(move |c| {
        let tx = c.transaction()?;
        let mut n = 0;
        {
            let mut st = tx.prepare(&format!("SELECT {} FROM msg WHERE account=?1 AND ai=0", cache::COLS))?;
            let mut up = tx.prepare("UPDATE msg SET category=?1 WHERE account=?2 AND folder=?3 AND uid=?4")?;
            let mut rows = st.query([&aid])?;
            while let Some(r) = rows.next()? {
                let m = Msg::from_row(r)?;
                let mut cat = categorize(&m, &known, &mine);
                if cat == "cold_email" && allow.contains(&m.from_addr) { cat = "personal"; }
                if cat != m.category { n += up.execute(rusqlite::params![cat, aid, m.folder, m.uid])?; }
            }
        }
        tx.commit()?;
        Ok(n)
    }).await
}

/// Everyone you've ever written to (from cached Sent folders).
pub async fn known_senders(app: &App) -> anyhow::Result<HashSet<String>> {
    app.db.call(|c| {
        let mut st = c.prepare("SELECT to_addrs, cc_addrs FROM msg WHERE folder LIKE '%sent%'")?;
        let mut out = HashSet::new();
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            for x in format!("{},{}", cache::txt(r, 0), cache::txt(r, 1)).split(',') {
                let x = x.trim().to_lowercase();
                if !x.is_empty() { out.insert(x); }
            }
        }
        Ok(out)
    }).await
}

async fn sync_account(app: &App, a: &Account) -> anyhow::Result<Value> {
    let mut s = imap::connect(app, a).await?;
    let r = async {
        let list = imap::list(&mut s).await?;
        let roles = roles_from(&list);
        let (aid, data) = (a.id.clone(), roles.to_string());
        app.db.call(move |c| c.execute("INSERT OR REPLACE INTO folders VALUES (?1,?2)", [aid, data])).await?;
        let st = state::get(app).await;
        let mine = account::my_addresses(&account::accounts(app).await?, &st.aliases);
        let (mut new, mut changed) = (json!({}), 0);
        // Sent first: it decides who is "known".
        if let Some(sent) = roles["sent"].as_str() {
            let r = sync_folder(app, &mut s, a, sent, &HashSet::new(), &mine, 800).await?;
            new["sent"] = json!(r.new); changed += r.new + r.changed;
        }
        let known = known_senders(app).await?;
        let r = sync_folder(app, &mut s, a, "INBOX", &known, &mine, KEEP).await?;
        new["inbox"] = json!(r.new); changed += r.new + r.changed;
        match enrich(app, &mut s, a).await {
            Ok(n) => changed += n,
            Err(e) => tracing::warn!("enrich {}: {e:#}", a.name),
        }
        if let Err(e) = super::message::prefetch(app, &mut s, a).await { tracing::warn!("prefetch {}: {}", a.name, format!("{e:#}").chars().take(300).collect::<String>()); }
        if let Some(junk) = roles["junk"].as_str() {
            let r = sync_folder(app, &mut s, a, junk, &known, &mine, 300).await?;
            new["junk"] = json!(r.new); changed += r.new + r.changed;
        }
        changed += recategorize(app, &a.id, known, mine, st.cold.allow.clone()).await?;
        let acts = rules::run_rules(app, &mut s, a).await?;
        changed += acts.len();
        anyhow::Ok(json!({"new": new, "actions": acts.len(), "changed": changed}))
    }.await;
    imap::logout(s).await;
    r
}

/// Sync every usable account, then push what's new. Returns the run summary
/// {ts, accounts:{name:{new, actions}|{error}}}.
pub async fn run_once(app: &App) -> Value {
    let _one = RUN.lock().await;
    let ts = now_ts();
    let accts: Vec<Account> = account::accounts(app).await.unwrap_or_default().into_iter().filter(|a| a.usable()).collect();
    let results: Vec<(String, Value)> = futures::stream::iter(accts.into_iter().map(|a| async move {
        let r = tokio::time::timeout(Duration::from_secs(900), sync_account(app, &a)).await;
        let v = match r {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => { tracing::warn!("mail sync {}: {e:#}", a.name); json!({"error": format!("{e:#}").chars().take(200).collect::<String>()}) }
            Err(_) => { tracing::warn!("mail sync {}: timed out", a.name); json!({"error": "timed out"}) }
        };
        (a.name, v)
    })).buffer_unordered(PARALLEL).collect().await;
    let mut accounts = serde_json::Map::new();
    let mut changed = false;
    for (name, mut v) in results {
        if let Some(c) = v.as_object_mut().and_then(|o| o.remove("changed")) { changed |= c.as_u64().unwrap_or(0) > 0; }
        accounts.insert(name, v);
    }
    if changed { super::changed(app); }
    if let Err(e) = push_new(app).await { tracing::warn!("push: {e:#}"); }
    let res = json!({"ts": ts, "accounts": accounts});
    let r2 = res.clone();
    let _ = state::update(app, move |s| { s.runs.push(r2); let n = s.runs.len(); if n > 20 { s.runs.drain(..n - 20); } }).await;
    LAST_RUN.store(ts, Ordering::SeqCst);
    res
}

/// Push mail that deserves a notification (same filter as /new) to every
/// device. The first run only sets the mark: no flood of old mail.
async fn push_new(app: &App) -> anyhow::Result<()> {
    let since = state::get(app).await.push_until;
    if since == 0 { return state::update(app, |s| s.push_until = now_ts()).await; }
    if crate::push::subs(app).await.is_empty() {
        return state::update(app, |s| s.push_until = since.max(now_ts() - 60)).await;
    }
    let mut new: Vec<Value> = views::fresh(app, since).await?.into_iter().filter(|m| m["ts"].as_i64().unwrap_or(0) > since).collect();
    if new.is_empty() { return Ok(()); }
    new.sort_by_key(|m| m["ts"].as_i64().unwrap_or(0));
    let until = new.iter().filter_map(|m| m["ts"].as_i64()).max().unwrap_or(since);
    let who = |m: &Value| { let n = m["from_name"].as_str().unwrap_or(""); if n.is_empty() { m["from_addr"].as_str().unwrap_or("").to_string() } else { n.to_string() } };
    let sent = if new.len() > 3 {
        let mut names: Vec<String> = vec![];
        for m in new.iter().rev() { let w = who(m); if !names.contains(&w) { names.push(w); } }
        crate::push::send_all(app, json!({"title": format!("{} new emails", new.len()), "body": names.join(", ").chars().take(180).collect::<String>(), "tag": "cerq-mail-batch", "url": "/?open=mail"})).await
    } else {
        let mut sent = 0;
        for m in &new {
            let subj = m["subject"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no subject)");
            let tag = match m["msgid"].as_str() { Some(id) if !id.is_empty() => id.to_string(), _ => m["uid"].to_string() };
            sent += crate::push::send_all(app, json!({"title": who(m), "body": format!("{subj}\n{}", m["account_name"].as_str().unwrap_or("")), "tag": format!("cerq-{tag}"),
                "url": format!("/?open=mail&msg={}:{}&folder=INBOX", m["account"].as_str().unwrap_or(""), m["uid"])})).await;
        }
        sent
    };
    // Only a batch some device took is done; if every send failed, retry next sync.
    if sent > 0 { state::update(app, |s| s.push_until = until).await?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_parsing() {
        let f = imap::Fetched { uid: 7, flags: "\\Seen".into(), internal_date: "17-Jul-2026 02:44:25 -0700".into(),
            header: Some(b"From: =?utf-8?q?Jos=C3=A9?= <Jose@Example.COM>\r\nTo: a@x.com, \"B\" <B@y.com>\r\nSubject: =?utf-8?b?SG9sYSDDsQ==?=\r\nMessage-ID: <abc@x>\r\nReferences: <a@x>\r\n <b@x>\r\nList-Unsubscribe: <https://u>,\r\n <mailto:u@x>\r\nPrecedence: Bulk\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n".to_vec()), ..Default::default() };
        let m = parse_header(&f, "acc", "INBOX", 5);
        assert_eq!(m.from_name, "José");
        assert_eq!(m.from_addr, "jose@example.com");
        assert_eq!(m.to_addrs, "a@x.com,b@y.com");
        assert_eq!(m.subject, "Hola ñ");
        assert_eq!(m.msgid, "<abc@x>");
        assert_eq!(m.refs, "<a@x> <b@x>");
        assert_eq!(m.list_unsub, "<https://u>, <mailto:u@x>");
        assert_eq!(m.precedence, "bulk");
        assert_eq!(m.ctype, "text/plain");
        // No Date header: INTERNALDATE.
        assert_eq!(m.ts, 1784281465);
    }

    #[test]
    fn roles() {
        let l = |n: &str, f: &[&str]| (n.to_string(), f.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        let r = roles_from(&[l("INBOX", &[]), l("[Gmail]/Sent Mail", &["\\sent"]), l("[Gmail]/Spam", &["\\junk"]), l("Archivo", &[]), l("Papelera", &[])]);
        assert_eq!(r["sent"], "[Gmail]/Sent Mail");
        assert_eq!(r["junk"], "[Gmail]/Spam");
        assert_eq!(r["archive"], "Archivo");
        assert_eq!(r["trash"], "Papelera");
        assert_eq!(r["_all"].as_array().unwrap().len(), 5);
    }
}
