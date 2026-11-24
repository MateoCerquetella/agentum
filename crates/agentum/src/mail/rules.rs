//! Rules (conditions → actions), the cold-email blocker, single actions on
//! messages with their log and undo, and batch actions over many messages.
use super::{account::{self, Account}, cache::{self, args, Msg}, imap::{self, Session}, state};
use crate::{llm, state::App, util::now_ts};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const ACTIONS: [&str; 7] = ["archive", "delete", "label", "mark_read", "move", "spam", "star"];
pub const COLD_MODES: [&str; 4] = ["off", "list", "label", "archive_label"];
const FLAG_ACTS: [&str; 4] = ["mark_read", "mark_unread", "star", "unstar"];
const MOVES: [&str; 4] = ["archive", "move", "spam", "delete"];

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Action {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Conditions {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub category: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(from = "Value")]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub operator: String,
    pub conditions: Conditions,
    pub instructions: String,
    pub actions: Vec<Action>,
    /// Account ids; empty means all.
    pub accounts: Vec<String>,
    pub created: i64,
}
impl From<Value> for Rule { fn from(v: Value) -> Rule { clean_rule(&v) } }

fn text(v: Option<&Value>) -> String {
    match v { None | Some(Value::Null) => String::new(), Some(Value::String(s)) => s.clone(), Some(x) => x.to_string() }
}
fn cut(s: &str, n: usize) -> String { s.chars().take(n).collect() }
fn truthy(v: Option<&Value>) -> bool {
    match v { None | Some(Value::Null) => false, Some(Value::Bool(b)) => *b, Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(), Some(Value::Array(a)) => !a.is_empty(), Some(Value::Object(o)) => !o.is_empty() }
}

/// Normalise a rule from the UI (or the old state file).
pub fn clean_rule(r: &Value) -> Rule {
    let id = text(r.get("id"));
    let ok_id = (6..=32).contains(&id.len()) && id.chars().all(|c| matches!(c, 'a'..='f' | '0'..='9'));
    let actions = r["actions"].as_array().map(|a| a.iter().filter(|x| ACTIONS.contains(&text(x.get("type")).as_str())).map(|x| Action {
        kind: cut(&text(x.get("type")), 120),
        label: x.get("label").map(|v| cut(&text(Some(v)), 120)),
        folder: x.get("folder").map(|v| cut(&text(Some(v)), 120)),
    }).collect()).unwrap_or_default();
    let c = |k: &str| cut(&text(r["conditions"].get(k)), 400);
    let name = text(r.get("name"));
    Rule {
        id: if ok_id { id } else { uuid::Uuid::new_v4().simple().to_string()[..10].to_string() },
        name: cut(if name.is_empty() { "Rule" } else { &name }, 80),
        enabled: truthy(r.get("enabled")),
        operator: if r["operator"] == "OR" { "OR".into() } else { "AND".into() },
        conditions: Conditions { from: c("from"), to: c("to"), subject: c("subject"), category: c("category") },
        instructions: cut(&text(r.get("instructions")), 600),
        actions,
        accounts: r["accounts"].as_array().map(|a| a.iter().take(10).map(|x| text(Some(x))).collect()).unwrap_or_default(),
        created: r["created"].as_f64().filter(|x| *x != 0.0).map(|x| x as i64).unwrap_or_else(now_ts),
    }
}

/// A condition's alternatives: split on | , or newline, lowercased.
pub fn alts(v: &str) -> Vec<String> {
    v.split(['|', ',', '\n']).map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()
}

/// The checks that don't need the model.
pub fn checks(rule: &Rule, m: &Msg) -> Vec<bool> {
    let c = &rule.conditions;
    let mut out = vec![];
    for (cond, val) in [(&c.from, format!("{} {}", m.from_addr, m.from_name).to_lowercase()),
                        (&c.to, format!("{},{}", m.to_addrs, m.cc_addrs).to_lowercase()),
                        (&c.subject, m.subject.to_lowercase())] {
        let a = alts(cond);
        if !a.is_empty() { out.push(a.iter().any(|x| val.contains(x.as_str()))); }
    }
    if !c.category.is_empty() { out.push(alts(&c.category).contains(&m.category)); }
    out
}

pub fn combine(rule: &Rule, checks: &[bool]) -> bool {
    if checks.is_empty() { return false; }
    if rule.operator == "OR" { checks.iter().any(|x| *x) } else { checks.iter().all(|x| *x) }
}

/// Does the model think this mail matches the rule's instructions? None when
/// it couldn't answer (adds no check).
async fn llm_matches(app: &App, instructions: &str, m: &Msg) -> Option<bool> {
    let out = llm::complete(app, vec![
        json!({"role": "system", "content": "You decide whether an email matches a user's rule. Answer only YES or NO."}),
        json!({"role": "user", "content": format!("Rule: {instructions}\n\nFrom: {} <{}>\nSubject: {}\nCategory guess: {}", m.from_name, m.from_addr, m.subject, m.category)}),
    ], 3, llm::Prio::Bg).await?;
    Some(out.trim().to_uppercase().starts_with('Y'))
}

pub async fn rule_matches(app: &App, rule: &Rule, m: &Msg, use_ai: bool) -> bool {
    let mut c = checks(rule, m);
    if use_ai && !rule.instructions.is_empty() {
        if let Some(x) = llm_matches(app, &rule.instructions, m).await { c.push(x); }
    }
    combine(rule, &c)
}

/// Gmail label or IMAP keyword name for a label ("$Name_ok", ≤40).
fn keyword(name: &str) -> String {
    format!("${}", name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).take(40).collect::<String>())
}

async fn roles(app: &App, account: &str) -> Value {
    let a = account.to_string();
    app.db.call(move |c| { use rusqlite::OptionalExtension; c.query_row("SELECT data FROM folders WHERE account=?1", [a], |r| r.get::<_, String>(0)).optional() }).await
        .ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({"inbox": "INBOX"}))
}
pub async fn role(app: &App, account: &str, role: &str) -> Option<String> {
    if role == "inbox" { return Some("INBOX".into()); }
    roles(app, account).await[role].as_str().map(String::from)
}

async fn log(app: &App, e: &Value) -> anyhow::Result<()> {
    let e = e.clone();
    app.db.call(move |c| log_row(c, &e)).await
}
fn log_row(c: &rusqlite::Connection, e: &Value) -> rusqlite::Result<()> {
    c.execute("INSERT INTO actlog (id,ts,account,rule,action,msgid,subject,from_addr,src,dst,status,detail) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        rusqlite::params![e["id"].as_str(), e["ts"].as_i64(), e["account"].as_str(), e["rule"].as_str(), e["action"].as_str(), e["msgid"].as_str(),
            e["subject"].as_str(), e["from_addr"].as_str(), e["src"].as_str(), e["dst"].as_str(), e["status"].as_str(), e["detail"].as_str()]).map(|_| ())
}

/// Run one action on one message; returns its log entry (what undo needs).
pub async fn apply_action(app: &App, s: &mut Session, a: &Account, folder: &str, m: &Msg, act: &Action, rule_name: &str) -> Value {
    let uid = match imap::find_uid(s, folder, &m.msgid).await { Some(u) => u, None => { let _ = imap::select(s, folder).await; m.uid as u32 } };
    let mut e = json!({"id": uuid::Uuid::new_v4().simple().to_string(), "ts": now_ts(), "account": a.id, "rule": rule_name, "action": act.kind,
        "msgid": m.msgid, "subject": m.subject, "from_addr": m.from_addr, "src": folder, "dst": "", "status": "applied", "detail": ""});
    let res: anyhow::Result<String> = async {
        let u = uid.to_string();
        Ok(match act.kind.as_str() {
            "archive" if a.is_gmail() => { imap::uid_store(s, &u, "-X-GM-LABELS (\\Inbox)").await?; "(gmail archive)".into() }
            "archive" => {
                let dst = role(app, &a.id, "archive").await.unwrap_or_else(|| "Archive".into());
                imap::ensure_folder(s, &dst).await;
                imap::mv(s, uid, &dst).await?;
                dst
            }
            "label" => {
                let name = act.label.as_deref().unwrap_or("").trim().to_string();
                if name.is_empty() { anyhow::bail!("label name missing"); }
                if a.is_gmail() {
                    imap::ensure_folder(s, &name).await;
                    imap::uid_store(s, &u, &format!("+X-GM-LABELS ({})", imap::q(&name))).await?;
                } else if imap::uid_store(s, &u, &format!("+FLAGS ({})", keyword(&name))).await.is_err() {
                    anyhow::bail!("this server doesn't support labels; use Move instead");
                }
                name
            }
            "move" => {
                let dst = act.folder.as_deref().unwrap_or("").trim().to_string();
                if dst.is_empty() { anyhow::bail!("folder missing"); }
                imap::ensure_folder(s, &dst).await;
                imap::mv(s, uid, &dst).await?;
                dst
            }
            "mark_read" => { imap::uid_store(s, &u, "+FLAGS (\\Seen)").await?; String::new() }
            "mark_unread" => { imap::uid_store(s, &u, "-FLAGS (\\Seen)").await?; String::new() }
            "star" => { imap::uid_store(s, &u, "+FLAGS (\\Flagged)").await?; String::new() }
            "unstar" => { imap::uid_store(s, &u, "-FLAGS (\\Flagged)").await?; String::new() }
            "spam" | "delete" => {
                let (r, d) = if act.kind == "spam" { ("junk", "Junk") } else { ("trash", "Trash") };
                let dst = role(app, &a.id, r).await.unwrap_or_else(|| d.into());
                imap::mv(s, uid, &dst).await?;
                dst
            }
            other => anyhow::bail!("unknown action {other}"),
        })
    }.await;
    match res {
        Ok(dst) => e["dst"] = json!(dst),
        Err(err) => { e["status"] = json!("error"); e["detail"] = json!(cut(&format!("{err:#}"), 300)); }
    }
    if let Err(err) = log(app, &e).await { tracing::warn!("actlog: {err:#}"); }
    e
}

/// Undo a logged action (moves go back, labels and flags are reversed).
pub async fn undo(app: &App, id: &str) -> Value {
    let e = cache::json_rows(app, "SELECT * FROM actlog WHERE id=?1", args(&[&id])).await.ok().and_then(|mut v| v.pop());
    let Some(e) = e.filter(|e| e["undone"].as_i64() != Some(1) && e["status"] == "applied") else { return json!({"ok": false, "error": "nothing to undo"}) };
    let Some(a) = account::accounts(app).await.ok().and_then(|v| v.into_iter().find(|a| a.id == e["account"])) else { return json!({"ok": false, "error": "account gone"}) };
    let (t, src, dst, msgid) = (e["action"].as_str().unwrap_or(""), e["src"].as_str().unwrap_or(""), e["dst"].as_str().unwrap_or(""), e["msgid"].as_str().unwrap_or(""));
    let res: anyhow::Result<()> = async {
        let mut s = imap::connect(app, &a).await?;
        let r = async {
            if ["move", "spam", "delete"].contains(&t) || (t == "archive" && !a.is_gmail()) {
                let uid = imap::find_uid(&mut s, dst, msgid).await.ok_or_else(|| anyhow::anyhow!("message not found in {dst}"))?;
                return imap::mv(&mut s, uid, src).await;
            }
            let folder = if t == "archive" { role(app, &a.id, "all").await.unwrap_or_else(|| "[Gmail]/All Mail".into()) } else { src.to_string() };
            let uid = imap::find_uid(&mut s, &folder, msgid).await.ok_or_else(|| anyhow::anyhow!("message not found"))?.to_string();
            let q = match t {
                "archive" => "+X-GM-LABELS (\\Inbox)".to_string(),
                "label" if a.is_gmail() => format!("-X-GM-LABELS ({})", imap::q(dst)),
                "label" => format!("-FLAGS ({})", keyword(dst)),
                "mark_read" => "-FLAGS (\\Seen)".into(),
                "star" => "-FLAGS (\\Flagged)".into(),
                "mark_unread" => "+FLAGS (\\Seen)".into(),
                "unstar" => "+FLAGS (\\Flagged)".into(),
                _ => return Ok(()),
            };
            imap::uid_store(&mut s, &uid, &q).await
        }.await;
        imap::logout(s).await;
        r
    }.await;
    match res {
        Ok(()) => {
            let id = id.to_string();
            let _ = app.db.call(move |c| c.execute("UPDATE actlog SET undone=1 WHERE id=?1", [id])).await;
            super::changed(app);
            json!({"ok": true})
        }
        Err(e) => json!({"ok": false, "error": cut(&format!("{e:#}"), 300)}),
    }
}

/// Apply enabled rules (and the cold-email blocker) to INBOX mail that
/// arrived since the last run, on the sync's own connection. The first run
/// only sets the cursor: rules never touch old mail.
pub async fn run_rules(app: &App, s: &mut Session, a: &Account) -> anyhow::Result<Vec<Value>> {
    let st = state::get(app).await;
    let Some(&cursor) = st.cursor.get(&a.id) else {
        let mx = cache::count(app, "SELECT coalesce(max(uid),0) FROM msg WHERE account=?1 AND folder='INBOX'", args(&[&a.id])).await?;
        state::update(app, |s| { s.cursor.insert(a.id.clone(), mx); }).await?;
        return Ok(vec![]);
    };
    let rows = cache::msgs(app, "WHERE account=?1 AND folder='INBOX' AND uid>?2 ORDER BY uid", args(&[&a.id, &cursor])).await?;
    if rows.is_empty() { return Ok(vec![]); }
    let rules: Vec<&Rule> = st.rules.iter().filter(|r| r.enabled && (r.accounts.is_empty() || r.accounts.contains(&a.id))).collect();
    let mut done = vec![];
    for m in &rows {
        let mut plan: Vec<(String, Action)> = vec![];
        if (st.cold.mode == "label" || st.cold.mode == "archive_label") && m.category == "cold_email" && !st.cold.allow.contains(&m.from_addr) {
            plan.push(("Cold email blocker".into(), Action { kind: "label".into(), label: Some("Cold email".into()), folder: None }));
            if st.cold.mode == "archive_label" { plan.push(("Cold email blocker".into(), Action { kind: "archive".into(), ..Default::default() })); }
        }
        for r in &rules {
            if rule_matches(app, r, m, true).await { plan.extend(r.actions.iter().map(|x| (r.name.clone(), x.clone()))); }
        }
        for (name, act) in plan { done.push(apply_action(app, s, a, "INBOX", m, &act, &name).await); }
    }
    let mx = rows.iter().map(|r| r.uid).max().unwrap_or(cursor);
    state::update(app, |s| { s.cursor.insert(a.id.clone(), mx); }).await?;
    Ok(done)
}

/// What a successful action does to the cache row.
async fn cache_after(app: &App, aid: &str, folder: &str, uid: i64, act: &str) {
    let (aid, folder, act) = (aid.to_string(), folder.to_string(), act.to_string());
    let _ = app.db.call(move |c| {
        if MOVES.contains(&act.as_str()) {
            c.execute("DELETE FROM msg WHERE account=?1 AND folder=?2 AND uid=?3", rusqlite::params![aid, folder, uid])?;
        } else if let Some((add, flag)) = flag_op(&act) {
            set_flag(c, &aid, &folder, uid, add, flag)?;
        }
        Ok(())
    }).await;
}
/// Add or remove one IMAP flag on a cached message.
fn set_flag(c: &rusqlite::Connection, aid: &str, folder: &str, uid: i64, add: bool, flag: &str) -> rusqlite::Result<()> {
    use rusqlite::OptionalExtension;
    let fl: Option<String> = c.query_row("SELECT flags FROM msg WHERE account=?1 AND folder=?2 AND uid=?3", rusqlite::params![aid, folder, uid], |r| Ok(cache::txt(r, 0))).optional()?;
    if let Some(fl) = fl {
        let mut set: std::collections::BTreeSet<&str> = fl.split_whitespace().filter(|f| *f != flag).collect();
        if add { set.insert(flag); }
        c.execute("UPDATE msg SET flags=?1 WHERE account=?2 AND folder=?3 AND uid=?4", rusqlite::params![set.into_iter().collect::<Vec<_>>().join(" "), aid, folder, uid])?;
    }
    Ok(())
}

fn flag_op(act: &str) -> Option<(bool, &'static str)> {
    match act { "mark_read" => Some((true, "\\Seen")), "mark_unread" => Some((false, "\\Seen")), "star" => Some((true, "\\Flagged")), "unstar" => Some((false, "\\Flagged")), _ => None }
}

/// Flag changes on many mails at once: one SELECT and UID STOREs of 200 per
/// folder, UIDs from the cache (no Message-ID search per mail).
async fn store_flags(app: &App, s: &mut Session, aid: &str, act: &str, todo: &[Msg], rule_name: &str) -> (usize, Vec<String>) {
    let (add, flag) = flag_op(act).unwrap();
    let mut by_folder: Vec<(String, Vec<&Msg>)> = vec![];
    for m in todo {
        match by_folder.iter_mut().find(|(f, _)| *f == m.folder) { Some((_, v)) => v.push(m), None => by_folder.push((m.folder.clone(), vec![m])) }
    }
    let (mut done, mut errors, now) = (0, vec![], now_ts());
    for (folder, items) in by_folder {
        if imap::select(s, &folder).await.is_err() { errors.push(format!("can't open {folder}")); continue; }
        for chunk in items.chunks(200) {
            let set = chunk.iter().map(|m| m.uid.to_string()).collect::<Vec<_>>().join(",");
            let r = imap::uid_store(s, &set, &format!("{}FLAGS ({flag})", if add { "+" } else { "-" })).await;
            if let Err(e) = &r { errors.push(cut(&format!("{e:#}"), 200)); }
            // The log entries and the cache rows of the whole chunk in one transaction.
            let ok = r.is_ok();
            let rows: Vec<Value> = chunk.iter().map(|m| json!({"id": uuid::Uuid::new_v4().simple().to_string(), "ts": now, "account": aid, "rule": rule_name, "action": act, "msgid": m.msgid,
                "subject": m.subject, "from_addr": m.from_addr, "src": folder, "dst": "", "status": if ok { "applied" } else { "error" }, "detail": if ok { "" } else { "store failed" }, "uid": m.uid})).collect();
            let (aid2, folder2) = (aid.to_string(), folder.clone());
            let _ = app.db.call(move |c| {
                let tx = c.transaction()?;
                for e in &rows {
                    log_row(&tx, e)?;
                    if ok { set_flag(&tx, &aid2, &folder2, e["uid"].as_i64().unwrap_or(0), add, flag)?; }
                }
                tx.commit()
            }).await;
            if r.is_ok() { done += chunk.len(); }
        }
    }
    (done, errors)
}

/// One action on many cached messages: one IMAP connection per account,
/// accounts in parallel. Flag changes skip mail already in that state, and an
/// account with nothing left to do isn't even connected to.
pub async fn batch_action(app: &App, items: &[Value], act: &str, extra: &Action, rule_name: &str) -> anyhow::Result<Value> {
    if !ACTIONS.contains(&act) && !FLAG_ACTS.contains(&act) { anyhow::bail!("unknown action"); }
    let mut by_acct: Vec<(String, Vec<(String, i64)>)> = vec![];
    for it in items.iter().take(2000) {
        let aid = text(it.get("account"));
        let key = (text(it.get("folder")), it["uid"].as_i64().or_else(|| text(it.get("uid")).parse().ok()).unwrap_or(0));
        match by_acct.iter_mut().find(|(a, _)| *a == aid) { Some((_, v)) => v.push(key), None => by_acct.push((aid, vec![key])) }
    }
    let already = |m: &Msg| match act { "mark_read" => m.seen(), "mark_unread" => !m.seen(), "star" => m.flags.contains("\\Flagged"), "unstar" => !m.flags.contains("\\Flagged"), _ => false };
    let one = |aid: String, targets: Vec<(String, i64)>| async move {
        let (mut done, mut skipped, mut errors, mut entries, mut todo) = (0usize, 0usize, Vec::<String>::new(), Vec::<Value>::new(), Vec::<Msg>::new());
        for (folder, uid) in targets {
            match cache::msgs(app, "WHERE account=?1 AND folder=?2 AND uid=?3", args(&[&aid, &folder, &uid])).await?.pop() {
                None => errors.push("not in cache; refresh".into()),
                Some(m) if already(&m) => skipped += 1,
                Some(m) => todo.push(m),
            }
        }
        if todo.is_empty() { return anyhow::Ok((done, skipped, errors, entries)); }
        let a = account::account(app, &aid).await?;
        let mut s = imap::connect(app, &a).await?;
        if flag_op(act).is_some() {
            let (n, errs) = store_flags(app, &mut s, &aid, act, &todo, rule_name).await;
            done += n;
            errors.extend(errs);
        } else {
            let action = Action { kind: act.to_string(), ..extra.clone() };
            for m in &todo {
                let e = apply_action(app, &mut s, &a, &m.folder, m, &action, rule_name).await;
                if e["status"] == "applied" { done += 1; cache_after(app, &aid, &m.folder, m.uid, act).await; } else { errors.push(text(e.get("detail"))); }
                entries.push(e);
            }
        }
        tokio::spawn(imap::logout(s)); // nobody waits for the goodbye
        Ok((done, skipped, errors, entries))
    };
    use futures::StreamExt;
    let results: Vec<_> = futures::stream::iter(by_acct.into_iter().map(|(a, t)| one(a, t))).buffer_unordered(8).collect().await;
    let (mut done, mut skipped, mut errors, mut entries) = (0, 0, vec![], vec![]);
    for r in results {
        match r {
            Ok((d, s, e, en)) => { done += d; skipped += s; errors.extend(e); entries.extend(en); }
            // One account failing (sign-in, network) doesn't stop the others.
            Err(e) => errors.push(cut(&format!("{e:#}"), 200)),
        }
    }
    if done > 0 { super::changed(app); }
    tracing::info!("bulk {act}: {done} done, {skipped} skipped, {} errors", errors.len());
    errors.truncate(5);
    entries.truncate(1);
    Ok(json!({"ok": errors.is_empty(), "done": done, "skipped": skipped, "errors": errors, "entries": entries}))
}

/// For the UI: rules with the vocabularies.
pub fn rules_json(rules: &[Rule]) -> Value {
    json!({"rules": rules, "categories": super::categorize::CATEGORIES, "actions": ACTIONS})
}

/// Senders an enabled rule names exactly, for the senders table.
pub fn blocked(rules: &[Rule]) -> HashSet<String> {
    rules.iter().filter(|r| r.enabled).flat_map(|r| alts(&r.conditions.from)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m() -> Msg { Msg { from_addr: "news@shop.com".into(), from_name: "Shop News".into(), to_addrs: "me@x.com".into(), subject: "Big SALE today".into(), category: "marketing".into(), ..Default::default() } }

    #[test]
    fn clean() {
        let r = clean_rule(&json!({"id": "nothex!", "name": "", "enabled": 1, "operator": "or", "conditions": {"from": 5},
            "actions": [{"type": "archive"}, {"type": "explode"}, {"type": "label", "label": "X", "junk": 1}], "accounts": ["a", 2]}));
        assert_eq!(r.id.len(), 10);
        assert_eq!(r.name, "Rule");
        assert!(r.enabled);
        assert_eq!(r.operator, "AND");
        assert_eq!(r.conditions.from, "5");
        assert_eq!(r.actions, vec![Action { kind: "archive".into(), ..Default::default() }, Action { kind: "label".into(), label: Some("X".into()), folder: None }]);
        assert_eq!(r.accounts, vec!["a", "2"]);
        assert_eq!(clean_rule(&json!({"id": "abcdef12"})).id, "abcdef12");
    }

    #[test]
    fn matching() {
        let r = |v: Value| clean_rule(&v);
        assert!(combine(&r(json!({"conditions": {"from": "shop.com"}})), &checks(&r(json!({"conditions": {"from": "shop.com"}})), &m())));
        let and = r(json!({"conditions": {"from": "other.com | shop news", "subject": "sale"}}));
        assert!(combine(&and, &checks(&and, &m())));
        let and2 = r(json!({"conditions": {"from": "shop.com", "subject": "invoice"}}));
        assert!(!combine(&and2, &checks(&and2, &m())));
        let or = r(json!({"operator": "OR", "conditions": {"from": "shop.com", "subject": "invoice"}}));
        assert!(combine(&or, &checks(&or, &m())));
        let cat = r(json!({"conditions": {"category": "newsletter, marketing"}}));
        assert!(combine(&cat, &checks(&cat, &m())));
        let to = r(json!({"conditions": {"to": "ME@x.com"}}));
        assert!(combine(&to, &checks(&to, &m())));
        // No conditions: never matches.
        let none = r(json!({}));
        assert!(!combine(&none, &checks(&none, &m())));
        assert_eq!(keyword("Cold email!"), "$Cold_email_");
    }
}
