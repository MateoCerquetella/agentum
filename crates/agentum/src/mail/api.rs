//! HTTP routes under /api/mail (agent.py's endpoints, same shapes) plus the
//! reader/composer routes and /api/summary.
use super::{account, bulk, cache::{self, args}, changed, message, rules::{self, Action, COLD_MODES}, state, sync, views, writer};
use crate::{llm, state::App, util::*};
use axum::{body::Body, extract::{DefaultBodyLimit, Multipart, Path, Query, State}, http::header, response::{IntoResponse, Response}, routing::{delete, get, post}, Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;

type Q = Query<HashMap<String, String>>;

fn q<'a>(q: &'a HashMap<String, String>, k: &str) -> &'a str { q.get(k).map(String::as_str).unwrap_or("") }
fn qn(q: &HashMap<String, String>, k: &str, default: i64) -> i64 { q.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(default) }

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/summary", get(summary))
        .route("/api/mail/accounts", get(accounts))
        .route("/api/mail/inbox", get(inbox))
        .route("/api/mail/counts", get(|State(app): State<App>| async move { R::Ok(Json(views::counts(&app).await?)) }))
        .route("/api/mail/new", get(new))
        .route("/api/mail/reply-zero", get(|State(app): State<App>, Query(p): Q| async move { R::Ok(Json(views::reply_zero(&app, qn(&p, "days", 30)).await?)) }))
        .route("/api/mail/senders", get(senders))
        .route("/api/mail/analytics", get(|State(app): State<App>, Query(p): Q| async move { R::Ok(Json(views::analytics(&app, qn(&p, "days", 30)).await?)) }))
        .route("/api/mail/aliases", get(aliases).post(set_aliases))
        .route("/api/mail/rules", get(|State(app): State<App>| async move { Json(rules::rules_json(&state::get(&app).await.rules)) }).post(save_rule))
        .route("/api/mail/cold", get(cold).post(set_cold))
        .route("/api/mail/log", get(log))
        .route("/api/mail/run", post(|State(app): State<App>| async move { Json(sync::run_once(&app).await) }))
        .route("/api/mail/message", post(message_action))
        .route("/api/mail/messages", post(messages_action))
        .route("/api/mail/dismiss", post(dismiss))
        .route("/api/mail/undismiss", post(undismiss))
        .route("/api/mail/rules/delete", post(delete_rule))
        .route("/api/mail/rules/preview", post(preview_rule))
        .route("/api/mail/rules/apply", post(apply_rule))
        .route("/api/mail/undo", post(|State(app): State<App>, Json(d): Json<Value>| async move { Json(rules::undo(&app, &s(&d, "id")).await) }))
        .route("/api/mail/bulk/archive", post(|State(app): State<App>, Json(d): Json<Value>| async move { R::Ok(Json(bulk::archive_sender(&app, &s(&d, "account"), &s(&d, "sender")).await?)) }))
        .route("/api/mail/bulk/archive-old", post(archive_old))
        .route("/api/mail/bulk/unsubscribe", post(|State(app): State<App>, Json(d): Json<Value>| async move { R::Ok(Json(bulk::unsubscribe(&app, &s(&d, "account"), &s(&d, "sender")).await?)) }))
        .route("/api/mail/bulk/block", post(block))
        .route("/api/mail/write", post(write))
        .route("/api/mail/read", get(read))
        .route("/api/mail/attachment", get(attachment))
        .route("/api/mail/upload", post(upload).layer(DefaultBodyLimit::max(message::UPLOAD_MAX + 64 * 1024)))
        .route("/api/mail/upload/{token}", delete(|State(app): State<App>, Path(t): Path<String>| async move { message::delete_upload(&app, &t).await?; R::Ok(Json(json!({"ok": true}))) }))
        .route("/api/mail/send", post(|State(app): State<App>, Json(d): Json<Value>| async move { message::send(&app, &d).await.map(Json).map_err(|e| bad(format!("{e:#}"))) }))
        .route("/api/mail/ai-reply", post(|State(app): State<App>, Json(d): Json<Value>| async move { message::ai_reply(&app, &d).await.map(Json).map_err(|e| bad(format!("{e:#}"))) }))
}

async fn summary(State(app): State<App>) -> R<Json<Value>> { Ok(Json(views::summary(&app).await?)) }

async fn accounts(State(app): State<App>) -> R<Json<Value>> {
    let accts: Vec<Value> = account::accounts(&app).await?.iter().map(|a| a.public()).collect();
    Ok(Json(json!({"accounts": accts, "llm": llm::configured(&app), "interval": app.setting("CERQ_MAIL_INTERVAL").parse::<i64>().unwrap_or(60), "last_run": sync::LAST_RUN.load(std::sync::atomic::Ordering::SeqCst)})))
}

async fn inbox(State(app): State<App>, Query(p): Q) -> R<Json<Value>> {
    let iq = views::InboxQuery {
        category: q(&p, "category").into(), account: q(&p, "account").into(), q: q(&p, "q").into(),
        limit: qn(&p, "limit", 100).clamp(1, 300) as usize, folder: match q(&p, "folder") { "" => "inbox".into(), f => f.into() }, unread: q(&p, "unread") == "1",
    };
    Ok(Json(json!({"messages": views::inbox(&app, &iq).await?})))
}

async fn new(State(app): State<App>, Query(p): Q) -> R<Json<Value>> {
    Ok(Json(json!({"messages": views::fresh(&app, qn(&p, "since", 0)).await?, "now": now_ts(), "last_run": sync::LAST_RUN.load(std::sync::atomic::Ordering::SeqCst)})))
}

async fn senders(State(app): State<App>, Query(p): Q) -> R<Json<Value>> {
    Ok(Json(json!({"senders": views::senders(&app, q(&p, "account"), qn(&p, "limit", 80).clamp(1, 300)).await?})))
}

async fn aliases_json(app: &App) -> Value {
    let mut mine: Vec<String> = account::mine(app).await.into_iter().collect();
    mine.sort();
    json!({"aliases": state::get(app).await.aliases, "mine": mine})
}
async fn aliases(State(app): State<App>) -> Json<Value> { Json(aliases_json(&app).await) }
async fn set_aliases(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let mut list: Vec<String> = d["aliases"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_lowercase()).filter(|x| x.contains('@')).collect()).unwrap_or_default();
    list.sort();
    list.dedup();
    list.truncate(50);
    state::update(&app, |s| s.aliases = list).await?;
    changed(&app);
    let mut v = aliases_json(&app).await;
    v["ok"] = json!(true);
    Ok(Json(v))
}

async fn cold(State(app): State<App>) -> R<Json<Value>> {
    let iq = views::InboxQuery { category: "cold_email".into(), account: String::new(), q: String::new(), limit: 100, folder: "inbox".into(), unread: false };
    Ok(Json(json!({"cold": state::get(&app).await.cold, "messages": views::inbox(&app, &iq).await?})))
}
async fn set_cold(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let mode = s(&d, "mode");
    let allow = s(&d, "allow").to_lowercase();
    let cold = state::update(&app, |st| {
        if COLD_MODES.contains(&mode.as_str()) { st.cold.mode = mode; }
        if !allow.is_empty() && !st.cold.allow.contains(&allow) { st.cold.allow.push(allow.clone()); st.cold.allow.sort(); }
        st.cold.clone()
    }).await?;
    if !allow.is_empty() {
        app.db.call(move |c| c.execute("UPDATE msg SET category='personal' WHERE from_addr=?1", [allow])).await?;
    }
    changed(&app);
    Ok(Json(json!({"ok": true, "cold": cold})))
}

async fn log(State(app): State<App>, Query(p): Q) -> R<Json<Value>> {
    let n = qn(&p, "limit", 100).clamp(1, 300);
    Ok(Json(json!({"log": cache::json_rows(&app, "SELECT * FROM actlog ORDER BY ts DESC LIMIT ?1", args(&[&n])).await?})))
}

/// The label/folder that go with an action.
fn extra(d: &Value) -> Action {
    let opt = |k: &str| d[k].as_str().filter(|x| !x.is_empty()).map(String::from);
    Action { kind: String::new(), label: opt("label"), folder: opt("folder_to") }
}

async fn message_action(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let item = json!({"account": d["account"], "folder": d["folder"], "uid": d["uid"]});
    let r = rules::batch_action(&app, &[item], &s(&d, "action"), &extra(&d), "Manual").await.map_err(|e| bad(e.to_string()))?;
    let entry = r["entries"].get(0).cloned().unwrap_or_else(|| json!({"detail": r["errors"].get(0).cloned().unwrap_or(json!("failed"))}));
    Ok(Json(json!({"ok": r["done"] == 1, "entry": entry})))
}

async fn messages_action(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let items = d["items"].as_array().cloned().unwrap_or_default();
    Ok(Json(rules::batch_action(&app, &items, &s(&d, "action"), &extra(&d), "Manual").await.map_err(|e| bad(e.to_string()))?))
}

async fn archive_old(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let found = views::old_mail(&app, d["days"].as_i64().unwrap_or(30), &s(&d, "category"), &s(&d, "account")).await?;
    if d["dry_run"].as_bool().unwrap_or(false) {
        let mut cats: HashMap<String, usize> = HashMap::new();
        for m in &found { *cats.entry(m.category.clone()).or_default() += 1; }
        return Ok(Json(json!({"ok": true, "count": found.len(), "categories": cats})));
    }
    let items: Vec<Value> = found.iter().map(|m| json!({"account": m.account, "folder": m.folder, "uid": m.uid})).collect();
    Ok(Json(rules::batch_action(&app, &items, "archive", &Action::default(), "Bulk archive (old)").await?))
}

/// Reply Zero "Done": one (msgid) or many (msgids).
async fn dismiss(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let ids: Vec<String> = match d["msgids"].as_array() { Some(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(), None => vec![s(&d, "msgid")] }
        .into_iter().filter(|x| !x.is_empty()).take(500).collect();
    let n = ids.len();
    state::update(&app, |st| {
        for id in ids { if !st.dismissed.contains(&id) { st.dismissed.push(id); } }
        let len = st.dismissed.len();
        if len > 2000 { st.dismissed.drain(..len - 2000); }
    }).await?;
    changed(&app);
    Ok(Json(json!({"ok": true, "dismissed": n})))
}
async fn undismiss(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let ids: Vec<String> = d["msgids"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    state::update(&app, |st| st.dismissed.retain(|x| !ids.contains(x))).await?;
    changed(&app);
    Ok(Json(json!({"ok": true})))
}

async fn save_rule(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let r = rules::clean_rule(&d);
    let r2 = r.clone();
    state::update(&app, |st| { st.rules.retain(|x| x.id != r2.id); st.rules.push(r2); }).await?;
    Ok(Json(json!({"ok": true, "rule": r})))
}
async fn delete_rule(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let id = s(&d, "id");
    state::update(&app, |st| st.rules.retain(|x| x.id != id)).await?;
    Ok(Json(json!({"ok": true})))
}

/// Which recent mail a rule would hit, without touching anything.
async fn preview_rule(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let r = rules::clean_rule(&d);
    let cand = cache::msgs(&app, "WHERE folder='INBOX' ORDER BY ts DESC LIMIT 300", vec![]).await?;
    let names = account::names(&app).await;
    let use_ai = d["ai"].as_bool().unwrap_or(false);
    let mut hits = vec![];
    for m in &cand {
        if (r.accounts.is_empty() || r.accounts.contains(&m.account)) && rules::rule_matches(&app, &r, m, use_ai).await { hits.push(m.view(&names)); }
        if hits.len() >= 100 { break; }
    }
    Ok(Json(json!({"matches": hits, "scanned": cand.len()})))
}

/// Run a rule now on matching inbox mail (after previewing), then sync.
async fn apply_rule(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let r = rules::clean_rule(&d);
    let mut done = vec![];
    for a in account::accounts(&app).await?.into_iter().filter(|a| a.usable() && (r.accounts.is_empty() || r.accounts.contains(&a.id))) {
        let mut hits = vec![];
        for m in cache::msgs(&app, "WHERE account=?1 AND folder='INBOX' ORDER BY ts DESC LIMIT 300", args(&[&a.id])).await? {
            if rules::rule_matches(&app, &r, &m, true).await { hits.push(m); }
        }
        if hits.is_empty() { continue; }
        let mut sess = super::imap::connect(&app, &a).await?;
        for m in &hits { for act in &r.actions { done.push(rules::apply_action(&app, &mut sess, &a, "INBOX", m, act, &r.name).await); } }
        super::imap::logout(sess).await;
    }
    sync::KICK.notify_one();
    let errors: Vec<&Value> = done.iter().filter(|x| x["status"] == "error").map(|x| &x["detail"]).take(5).collect();
    Ok(Json(json!({"ok": true, "applied": done.iter().filter(|x| x["status"] == "applied").count(), "errors": errors})))
}

/// A rule: anything from this sender → archive (or delete).
async fn block(State(app): State<App>, Json(d): Json<Value>) -> R<Json<Value>> {
    let sender = s(&d, "sender").to_lowercase();
    if sender.is_empty() { return Err(bad("sender missing")); }
    let act = if d["delete"].as_bool().unwrap_or(false) { "delete" } else { "archive" };
    let acct = s(&d, "account");
    let r = rules::clean_rule(&json!({"name": format!("Block {sender}"), "enabled": true, "conditions": {"from": sender}, "actions": [{"type": act}],
        "accounts": if acct.is_empty() { json!([]) } else { json!([acct]) }}));
    let r2 = r.clone();
    state::update(&app, |st| st.rules.push(r2)).await?;
    Ok(Json(json!({"ok": true, "rule": r})))
}

/// Writing tools: NDJSON chunk… done | error.
async fn write(State(app): State<App>, Json(d): Json<Value>) -> R<Response> {
    if !llm::configured(&app) { return Err(bad("No model is set up yet. Pick one in Settings → Your model.")); }
    let (msgs, budget) = writer::messages(&d, &app.setting("CERQ_MY_NAME"), &app.setting("CERQ_WRITE_LANG2")).map_err(bad)?;
    let body = Body::from_stream(futures::StreamExt::map(writer::frames(app, msgs, budget), Ok::<_, std::convert::Infallible>));
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8"), (header::CACHE_CONTROL, "no-store"), (header::HeaderName::from_static("x-accel-buffering"), "no")], body).into_response())
}

fn uid_of(p: &HashMap<String, String>) -> Result<u32, AppError> { p.get("uid").and_then(|u| u.parse().ok()).ok_or_else(|| bad("uid missing")) }

/// The Mail reader: the whole message; marks it read unless peek=1.
async fn read(State(app): State<App>, Query(p): Q) -> R<Json<Value>> {
    let folder = match q(&p, "folder") { "" => "INBOX", f => f };
    Ok(Json(message::read(&app, q(&p, "account"), folder, uid_of(&p)?, q(&p, "peek") != "1").await?))
}

async fn attachment(State(app): State<App>, Query(p): Q) -> R<Response> {
    let folder = match q(&p, "folder") { "" => "INBOX", f => f };
    let index = p.get("index").and_then(|i| i.parse().ok()).ok_or_else(|| bad("index missing"))?;
    let (name, ct, bytes) = message::attachment(&app, q(&p, "account"), folder, uid_of(&p)?, index).await?;
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let enc: String = name.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let ct = if ct.is_empty() || ct.contains(['\r', '\n']) { "application/octet-stream".to_string() } else { ct };
    Ok(([(header::CONTENT_TYPE, ct), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}")),
         (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()), (header::CONTENT_SECURITY_POLICY, "sandbox".into()),
         (header::CACHE_CONTROL, "private, max-age=86400".into())], bytes).into_response())
}

async fn upload(State(app): State<App>, mut mp: Multipart) -> R<Json<Value>> {
    while let Some(field) = mp.next_field().await.map_err(|e| bad(e.to_string()))? {
        if field.name() != Some("file") { continue; }
        let name = field.file_name().unwrap_or("file").to_string();
        let ct = field.content_type().unwrap_or("application/octet-stream").to_string();
        return Ok(Json(message::upload(&app, &name, &ct, field).await.map_err(|e| bad(format!("{e:#}")))?));
    }
    Err(bad("no file"))
}
