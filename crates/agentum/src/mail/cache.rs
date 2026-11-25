//! The mail cache (table `msg`, cerq-mail's cache.db as imported): one row per
//! message header, plus helpers to read rows and turn them into the list
//! shape the UI gets (ViewMsg).
use crate::state::App;
use rusqlite::types::{ToSql, Value as Sql, ValueRef};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub account: String,
    pub folder: String,
    pub uid: i64,
    pub uidvalidity: i64,
    pub msgid: String,
    pub in_reply_to: String,
    pub refs: String,
    pub thrid: String,
    pub from_name: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub subject: String,
    pub ts: i64,
    pub flags: String,
    pub labels: String,
    pub list_unsub: String,
    pub list_unsub_post: String,
    pub list_id: String,
    pub precedence: String,
    pub auto_submitted: String,
    pub ctype: String,
    pub category: String,
    pub ai: i64,
    pub extra: Option<String>,
}

/// A column as text whatever SQLite stored (NULL → "").
pub fn txt(r: &rusqlite::Row, i: usize) -> String {
    match r.get_ref(i) {
        Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
        Ok(ValueRef::Integer(n)) => n.to_string(),
        Ok(ValueRef::Real(f)) => f.to_string(),
        _ => String::new(),
    }
}
pub fn num(r: &rusqlite::Row, i: usize) -> i64 {
    match r.get_ref(i) {
        Ok(ValueRef::Integer(n)) => n,
        Ok(ValueRef::Real(f)) => f as i64,
        Ok(ValueRef::Text(t)) => std::str::from_utf8(t).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        _ => 0,
    }
}

pub const COLS: &str = "account,folder,uid,uidvalidity,msgid,in_reply_to,refs,thrid,from_name,from_addr,to_addrs,cc_addrs,subject,ts,flags,labels,list_unsub,list_unsub_post,list_id,precedence,auto_submitted,ctype,category,ai,extra";

impl Msg {
    /// From a row selected with COLS, in that order.
    pub fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Msg> {
        Ok(Msg {
            account: txt(r, 0), folder: txt(r, 1), uid: num(r, 2), uidvalidity: num(r, 3), msgid: txt(r, 4), in_reply_to: txt(r, 5),
            refs: txt(r, 6), thrid: txt(r, 7), from_name: txt(r, 8), from_addr: txt(r, 9), to_addrs: txt(r, 10), cc_addrs: txt(r, 11),
            subject: txt(r, 12), ts: num(r, 13), flags: txt(r, 14), labels: txt(r, 15), list_unsub: txt(r, 16), list_unsub_post: txt(r, 17),
            list_id: txt(r, 18), precedence: txt(r, 19), auto_submitted: txt(r, 20), ctype: txt(r, 21), category: txt(r, 22), ai: num(r, 23),
            extra: r.get::<_, Option<String>>(24).ok().flatten(),
        })
    }
    pub fn seen(&self) -> bool { self.flags.contains("\\Seen") }
    pub fn extra_json(&self) -> Value {
        self.extra.as_deref().and_then(|e| serde_json::from_str(e).ok()).unwrap_or(Value::Null)
    }
    /// ViewMsg, the list shape the UI and the other modules use.
    pub fn view(&self, names: &HashMap<String, String>) -> Value {
        json!({
            "account": self.account, "account_name": names.get(&self.account).map(String::as_str).unwrap_or("?"),
            "folder": self.folder, "uid": self.uid, "msgid": self.msgid, "from_name": self.from_name, "from_addr": self.from_addr,
            "subject": self.subject, "ts": self.ts, "unread": !self.seen(), "answered": self.flags.contains("\\Answered"),
            "starred": self.flags.contains("\\Flagged"), "category": self.category, "unsubscribe": !self.list_unsub.is_empty(),
            "to": self.to_addrs.split(',').take(3).collect::<Vec<_>>().join(", "), "thrid": self.thrid, "extra": self.extra_json(),
        })
    }
    pub fn insert(&self, c: &rusqlite::Connection) -> rusqlite::Result<usize> {
        c.execute(&format!("INSERT OR REPLACE INTO msg ({COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)"),
            rusqlite::params![self.account, self.folder, self.uid, self.uidvalidity, self.msgid, self.in_reply_to, self.refs, self.thrid,
                self.from_name, self.from_addr, self.to_addrs, self.cc_addrs, self.subject, self.ts, self.flags, self.labels, self.list_unsub,
                self.list_unsub_post, self.list_id, self.precedence, self.auto_submitted, self.ctype, self.category, self.ai, self.extra])
    }
}

/// SQL arguments that can travel to the blocking pool.
pub fn args(v: &[&(dyn ToSql + Sync)]) -> Vec<Sql> {
    v.iter().map(|x| match x.to_sql() {
        Ok(rusqlite::types::ToSqlOutput::Owned(o)) => o,
        Ok(rusqlite::types::ToSqlOutput::Borrowed(b)) => b.into(),
        _ => Sql::Null,
    }).collect()
}

/// Cached messages: `where` is everything after "FROM msg".
pub async fn msgs(app: &App, rest: &str, a: Vec<Sql>) -> anyhow::Result<Vec<Msg>> {
    let sql = format!("SELECT {COLS} FROM msg {rest}");
    app.db.call(move |c| {
        let mut st = c.prepare(&sql)?;
        let r = st.query_map(rusqlite::params_from_iter(a), Msg::from_row)?.collect();
        r
    }).await
}

/// Any query as JSON objects (column name → value).
pub async fn json_rows(app: &App, sql: &str, a: Vec<Sql>) -> anyhow::Result<Vec<Value>> {
    let sql = sql.to_string();
    app.db.call(move |c| {
        let mut st = c.prepare(&sql)?;
        let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        let r = st.query_map(rusqlite::params_from_iter(a), |r| {
            let mut o = serde_json::Map::new();
            for (i, n) in names.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(x) => json!(x),
                    ValueRef::Real(x) => json!(x),
                    ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    ValueRef::Blob(_) => Value::Null,
                };
                o.insert(n.clone(), v);
            }
            Ok(Value::Object(o))
        })?.collect();
        r
    }).await
}

/// One number from a query.
pub async fn count(app: &App, sql: &str, a: Vec<Sql>) -> anyhow::Result<i64> {
    let sql = sql.to_string();
    app.db.call(move |c| c.query_row(&sql, rusqlite::params_from_iter(a), |r| Ok(num(r, 0)))).await
}
