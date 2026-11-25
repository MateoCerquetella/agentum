//! CERQ Mail (owner: mail backend). Port of the earlier Python mail agent plus
//! the Odysseus mail routes the UI used (read, attachments, send, uploads,
//! AI reply). Cross-module API below is a contract: keep the signatures.
mod account;
mod api;
mod bulk;
mod cache;
mod categorize;
mod enrich;
mod imap;
mod message;
mod rules;
mod state;
mod sync;
pub use sync::run_once as sync_mail_now;
pub use cache::Msg as CachedMsg;
mod views;
mod writer;

use crate::state::App;
use axum::Router;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

/// Bumped whenever mail may have changed; the brief's numbers are cached until then.
static VERSION: AtomicU64 = AtomicU64::new(0);

/// Mail changed: caches go stale and open tabs refresh.
fn changed(app: &App) {
    VERSION.fetch_add(1, Ordering::SeqCst);
    app.emit(json!({"kind": "mail"}));
}

pub fn routes() -> Router<App> { api::routes().merge(crate::push::routes()) }
/// Background: sync loop, rules, push for new mail.
pub fn start(app: App) { sync::start(app) }

/// Brief numbers: {to_reply, awaiting, unread_important, new_overnight, auto_handled_24h, people_waiting, oldest_to_reply_days, waiting:[{from,subject,days,id:"acct:uid"}≤3]}
pub async fn brief_mail(app: &App) -> anyhow::Result<Value> { views::brief_mail(app).await }
/// {to_reply:[ViewMsg], awaiting:[ViewMsg+{to,days}]}
/// Account id → name (the journal labels the mail you sent with it).
pub async fn account_names(app: &App) -> std::collections::HashMap<String, String> { account::names(app).await }
/// Cached messages for a WHERE clause (the journal reads the Sent folders).
pub async fn cache_msgs(app: &App, rest: &str, a: &[&(dyn rusqlite::ToSql + Sync)]) -> anyhow::Result<Vec<cache::Msg>> { cache::msgs(app, rest, cache::args(a)).await }
pub async fn reply_zero(app: &App, days: i64) -> anyhow::Result<Value> { views::reply_zero(app, days).await }
/// Dated things found in mail: [{id:"mail:<msgid>", kind:'pay'|'event', title, date, time, from, subject, amount, msg:"acct:uid", account}]
pub async fn mail_reminders(app: &App) -> anyhow::Result<Vec<Value>> { views::mail_reminders(app).await }
/// Search cached mail (subject/from/snippet), newest first. ViewMsg list.
pub async fn search(app: &App, q: &str, limit: usize) -> anyhow::Result<Vec<Value>> { views::search(app, q, limit).await }
/// Full message (live IMAP): {account, folder, uid, subject, from_name, from_address, date, to, cc, body (text), body_html, message_id, references, attachments:[...]}
/// Reading it here doesn't mark it read (the Mail reader does, through GET /api/mail/read).
pub async fn read(app: &App, account: &str, folder: &str, uid: i64) -> anyhow::Result<Value> { message::read(app, account, folder, uid as u32, false).await }
/// Send: {account_id, to, cc, bcc, subject, body, attachments:[upload tokens], in_reply_to?, references?} → {ok}
pub async fn send(app: &App, req: Value) -> anyhow::Result<Value> { message::send(app, &req).await }
/// [{id, name, from, default, usable}]
pub async fn accounts_brief(app: &App) -> anyhow::Result<Vec<Value>> {
    Ok(account::accounts(app).await?.iter().map(|a| json!({"id": a.id, "name": a.name, "from": a.from, "default": a.default, "usable": a.usable()})).collect())
}
