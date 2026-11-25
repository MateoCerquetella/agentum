//! What the Mail reader and composer need from a live server (what Odysseus
//! used to do): read a whole message, download an attachment, compose
//! uploads, send over SMTP (copy to Sent, mark the source answered) and an
//! AI reply draft. Small messages are fetched whole; big ones (attachments)
//! by BODYSTRUCTURE, fetching only the text parts or the one attachment asked for.
use super::{account::{self, Account}, cache::args, enrich::strip_html, imap::{self, Part, Session}, rules};
use crate::{llm, state::App, util::now_ts};
use anyhow::{anyhow, bail};
use lettre::{message::{header::ContentType, Attachment, Mailbox, Mailboxes, MultiPart, SinglePart}, transport::smtp::authentication::{Credentials, Mechanism}, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use mail_parser::{MessageParser, MimeHeaders, PartType};
use serde_json::{json, Value};
use std::time::Duration;

/// Messages up to this size are fetched whole.
const WHOLE: u32 = 1_500_000;
/// Compose uploads: at most this big, kept a day.
pub const UPLOAD_MAX: usize = 25 * 1024 * 1024;

/// RFC 2047 ("=?utf-8?b?…?=") or RFC 2231 ("utf-8''%E2%82…") encoded names.
fn decode_name(v: &str) -> String {
    if let Some((cs, rest)) = v.split_once('\'') {
        if let Some((_, enc)) = rest.split_once('\'') {
            if !cs.is_empty() && cs.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                let mut out = vec![];
                let b = enc.as_bytes();
                let mut i = 0;
                while i < b.len() {
                    if b[i] == b'%' && i + 2 < b.len() {
                        if let Some(x) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) { out.push(x); i += 3; continue; }
                    }
                    out.push(b[i]);
                    i += 1;
                }
                return String::from_utf8_lossy(&out).into_owned();
            }
        }
    }
    if v.contains("=?") {
        let h = format!("Subject: {v}\r\n\r\n");
        if let Some(s) = MessageParser::default().parse_headers(h.as_bytes()).and_then(|m| m.subject().map(String::from)) { return s; }
    }
    v.to_string()
}

fn ext_of(ct: &str) -> &str { if ct == "message/rfc822" { "eml" } else { ct.rsplit('/').next().filter(|_| ct.contains('/')).unwrap_or("bin") } }

/// Attachment metadata as the UI gets it.
fn att(index: usize, filename: &str, ct: &str, size: usize, inline: bool) -> Value {
    let mut name = if filename.is_empty() { format!("attachment_{index}.{}", ext_of(ct)) } else { decode_name(filename) };
    if ct == "message/rfc822" && !name.rsplit_once('.').is_some_and(|(_, e)| (1..=8).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric())) { name += ".eml"; }
    json!({"index": index, "filename": name, "size": size, "content_type": ct, "is_inline": inline})
}

/// The attachments of a BODYSTRUCTURE: every leaf but the message's own text.
fn structure_atts(parts: &[Part]) -> Vec<&Part> {
    parts.iter().filter(|p| !((p.ctype == "text/plain" || p.ctype == "text/html") && p.disposition != "attachment")).collect()
}

/// The attachments of a parsed message, in order: (part index, metadata).
fn parsed_atts(m: &mail_parser::Message) -> Vec<(usize, Value)> {
    if !m.root_part().is_multipart() { return vec![]; }
    let mut out = vec![];
    for (i, p) in m.parts.iter().enumerate().skip(1) {
        if p.is_multipart() { continue; }
        let ct = p.content_type().map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("")).to_lowercase()).unwrap_or_else(|| "text/plain".into());
        let disp = p.content_disposition().map(|d| d.ctype().to_lowercase()).unwrap_or_default();
        if (ct == "text/plain" || ct == "text/html") && disp != "attachment" { continue; }
        let size = match &p.body { PartType::Message(x) => x.raw_message.len(), b => b.len() };
        let idx = out.len();
        out.push((i, att(idx, p.attachment_name().unwrap_or(""), &ct, size, disp == "inline")));
    }
    out
}

fn part_bytes(p: &mail_parser::MessagePart) -> Vec<u8> {
    match &p.body {
        PartType::Text(t) | PartType::Html(t) => t.as_bytes().to_vec(),
        PartType::Binary(b) | PartType::InlineBinary(b) => b.to_vec(),
        PartType::Message(m) => m.raw_message.to_vec(),
        PartType::Multipart(_) => vec![],
    }
}

fn addr_list(a: Option<&mail_parser::Address>) -> String {
    a.map(|a| a.iter().map(|x| match (x.name.as_deref(), x.address.as_deref()) {
        (Some(n), Some(e)) if !n.is_empty() => format!("{n} <{e}>"),
        (_, Some(e)) => e.to_string(),
        (Some(n), None) => n.to_string(),
        _ => String::new(),
    }).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// Size and headers first (and the structure of a big one); then the rest as needed.
async fn head(s: &mut Session, uid: u32) -> anyhow::Result<imap::Fetched> {
    let mut got = None;
    imap::uid_fetch(s, &uid.to_string(), "(UID RFC822.SIZE BODY.PEEK[HEADER])", |f| got = Some(f)).await?;
    let mut f = got.ok_or_else(|| anyhow!("message {uid} not found"))?;
    // Only big messages need BODYSTRUCTURE, and some servers send ones the
    // parser can't read (iCloud: NIL transfer encoding on plain text).
    if f.size > WHOLE {
        let mut st = None;
        imap::uid_fetch(s, &uid.to_string(), "(UID BODYSTRUCTURE)", |x| st = x.structure).await?;
        f.structure = st;
    }
    Ok(f)
}

async fn whole(s: &mut Session, uid: u32) -> anyhow::Result<Vec<u8>> {
    let mut raw = None;
    imap::uid_fetch(s, &uid.to_string(), "(UID BODY.PEEK[])", |f| raw = f.body).await?;
    raw.ok_or_else(|| anyhow!("message {uid} not found"))
}

/// One part of a big message, decoded: its MIME headers and body parsed as a message of its own.
async fn one_part(s: &mut Session, uid: u32, section: &str) -> anyhow::Result<Vec<u8>> {
    let mut f = None;
    imap::uid_fetch(s, &uid.to_string(), &format!("(UID BODY.PEEK[{section}.MIME] BODY.PEEK[{section}])"), |x| f = Some(x)).await?;
    let mut f = f.ok_or_else(|| anyhow!("message {uid} not found"))?;
    let mut raw = f.parts.remove(&format!("{section}.MIME")).unwrap_or_default();
    raw.extend(f.parts.remove(section).unwrap_or_default());
    let m = MessageParser::default().parse(&raw).ok_or_else(|| anyhow!("unreadable part"))?;
    Ok(part_bytes(m.root_part()))
}

fn big(f: &imap::Fetched) -> bool { f.size > WHOLE && f.structure.as_ref().is_some_and(|p| p.iter().all(|x| x.section != "TEXT")) }

/// Opened and prefetched messages kept in `mail_body`, newest first.
const CACHED: i64 = 500;
/// Answers bigger than this (lots of pictures) aren't kept.
const CACHE_MAX: usize = 6_000_000;
/// The newest inbox messages per account the sync fetches ahead.
const PREFETCH: i64 = 40;
/// Bytes of cid: pictures embedded into one email.
const EMBED_MAX: usize = 8_000_000;

/// A cid: picture in the HTML becomes a data: URL (the email's frame is
/// sandboxed and couldn't fetch it); true if it was used.
fn embed(html: &mut String, cid: &str, ct: &str, bytes: &[u8], budget: &mut usize) -> bool {
    use base64::Engine;
    let id = cid.trim().trim_start_matches('<').trim_end_matches('>').trim();
    if id.is_empty() || !ct.starts_with("image/") || bytes.len() > *budget { return false; }
    let needle = format!("cid:{id}");
    if !html.contains(&needle) { return false; }
    *budget -= bytes.len();
    *html = html.replace(&needle, &format!("data:{ct};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
    true
}

/// The full message off the server (folder already open): {account, folder, uid, subject,
/// from_name, from_address, date, to, cc, body, body_html, message_id, references, attachments}.
async fn full(s: &mut Session, aid: &str, folder: &str, uid: u32) -> anyhow::Result<Value> {
    let f = head(s, uid).await?;
    let header = f.header.clone().unwrap_or_default();
    let (mut text, mut html, mut attachments);
    let mut budget = EMBED_MAX;
    if big(&f) {
        let parts = f.structure.clone().unwrap_or_default();
        let first = |ct: &str| parts.iter().find(|p| p.ctype == ct && p.disposition != "attachment").map(|p| p.section.clone());
        text = match first("text/plain") { Some(sec) => String::from_utf8_lossy(&one_part(s, uid, &sec).await?).into_owned(), None => String::new() };
        html = match first("text/html") { Some(sec) => String::from_utf8_lossy(&one_part(s, uid, &sec).await?).into_owned(), None => String::new() };
        attachments = vec![];
        for (i, p) in structure_atts(&parts).into_iter().enumerate() {
            let size = if p.base64 { p.octets as usize * 57 / 78 } else { p.octets as usize };
            let mut v = att(i, &p.filename, &p.ctype, size, p.disposition == "inline");
            let id = p.cid.trim_matches(['<', '>']);
            if !id.is_empty() && size <= budget && html.contains(&format!("cid:{id}")) {
                let bytes = one_part(s, uid, &p.section).await?;
                if embed(&mut html, &p.cid, &p.ctype, &bytes, &mut budget) { v["embedded"] = json!(true); }
            }
            attachments.push(v);
        }
    } else {
        let raw = whole(s, uid).await?;
        let m = MessageParser::default().parse(&raw).ok_or_else(|| anyhow!("unreadable message"))?;
        text = m.text_bodies().find(|p| p.is_text() && !p.is_text_html()).and_then(|p| p.text_contents()).unwrap_or("").to_string();
        html = m.html_bodies().find(|p| p.is_text_html()).and_then(|p| p.text_contents()).unwrap_or("").to_string();
        attachments = vec![];
        for (i, mut v) in parsed_atts(&m) {
            let p = &m.parts[i];
            if let Some(cid) = p.content_id() {
                if embed(&mut html, cid, v["content_type"].as_str().unwrap_or(""), &part_bytes(p), &mut budget) { v["embedded"] = json!(true); }
            }
            attachments.push(v);
        }
    }
    if text.trim().is_empty() && !html.is_empty() { text = strip_html(&html).lines().map(str::trim).collect::<Vec<_>>().join("\n"); }
    if html.trim().is_empty() { html.clear(); }
    let h = MessageParser::default().parse_headers(&header).ok_or_else(|| anyhow!("unreadable headers"))?;
    let from = h.from().and_then(|x| x.first());
    let raw_h = |n: &str| h.header_raw(n).map(|v| v.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    Ok(json!({
        "account": aid, "folder": folder, "uid": uid,
        "subject": h.subject().unwrap_or("(no subject)"),
        "from_name": from.and_then(|x| x.name.as_deref()).unwrap_or(""), "from_address": from.and_then(|x| x.address.as_deref()).unwrap_or(""),
        "date": h.date().map(|d| d.to_rfc3339()).unwrap_or_default(),
        "to": addr_list(h.to()), "cc": addr_list(h.cc()),
        "body": text, "body_html": html,
        "message_id": raw_h("Message-ID"), "references": raw_h("References"),
        "attachments": attachments,
    }))
}

async fn cached(app: &App, aid: &str, folder: &str, uid: u32) -> Option<Value> {
    let (aid, folder) = (aid.to_string(), folder.to_string());
    let data: Option<String> = app.db.call(move |c| {
        use rusqlite::OptionalExtension;
        c.query_row("SELECT data FROM mail_body WHERE account=?1 AND folder=?2 AND uid=?3", rusqlite::params![aid, folder, uid], |r| r.get(0)).optional()
    }).await.ok().flatten();
    serde_json::from_str(&data?).ok()
}

async fn remember(app: &App, aid: &str, folder: &str, uid: u32, v: &Value) {
    let data = v.to_string();
    if data.len() > CACHE_MAX { return; }
    let (aid, folder) = (aid.to_string(), folder.to_string());
    let _ = app.db.call(move |c| {
        c.execute("INSERT OR REPLACE INTO mail_body (account,folder,uid,ts,data) VALUES (?1,?2,?3,?4,?5)", rusqlite::params![aid, folder, uid, now_ts(), data])?;
        c.execute("DELETE FROM mail_body WHERE rowid NOT IN (SELECT rowid FROM mail_body ORDER BY ts DESC LIMIT ?1)", [CACHED])
    }).await;
}

/// The full message (see `full`), from the cache when it was opened or
/// prefetched before. `mark_seen` also sets \Seen on the server and in the cache.
pub async fn read(app: &App, account: &str, folder: &str, uid: u32, mark_seen: bool) -> anyhow::Result<Value> {
    let a = account::account(app, account).await?;
    let seen = super::cache::count(app, r"SELECT count(*) FROM msg WHERE account=?1 AND folder=?2 AND uid=?3 AND flags LIKE '%\Seen%'", args(&[&a.id, &folder, &(uid as i64)])).await? > 0;
    let mark = mark_seen && !seen;
    if let Some(v) = cached(app, &a.id, folder, uid).await {
        if mark {
            mark_seen_cached(app, &a.id, folder, uid).await;
            let (app, folder) = (app.clone(), folder.to_string());
            tokio::spawn(async move {
                let r = async {
                    let mut s = imap::connect(&app, &a).await?;
                    let r = async { imap::select(&mut s, &folder).await?; imap::uid_store(&mut s, &uid.to_string(), "+FLAGS (\\Seen)").await }.await;
                    imap::logout(s).await;
                    r
                }.await;
                if let Err(e) = r { tracing::warn!("mark read on the mail server: {e:#}"); }
            });
        }
        return Ok(v);
    }
    let mut s = imap::connect(app, &a).await?;
    let r = async {
        if mark { imap::select(&mut s, folder).await?; } else { imap::examine(&mut s, folder).await?; }
        full(&mut s, &a.id, folder, uid).await
    }.await;
    if let Ok(v) = &r {
        remember(app, &a.id, folder, uid, v).await;
        if mark { mark_seen_cached(app, &a.id, folder, uid).await; } // the mail server's \Seen follows the answer, below
    }
    // Opening answers at once; marking it read on the mail server and logging out happen after.
    let store = mark && r.is_ok();
    tokio::spawn(async move {
        if store { if let Err(e) = imap::uid_store(&mut s, &uid.to_string(), "+FLAGS (\\Seen)").await { tracing::warn!("mark read on the mail server: {e:#}"); } }
        imap::logout(s).await;
    });
    r
}

/// Fetch the newest inbox messages that aren't cached yet (during the sync,
/// on its connection), so opening them needs no trip to the mail server.
pub async fn prefetch(app: &App, s: &mut Session, a: &Account) -> anyhow::Result<usize> {
    let aid = a.id.clone();
    let uids: Vec<u32> = app.db.call(move |c| c.prepare(
        "SELECT uid FROM (SELECT uid FROM msg WHERE account=?1 AND folder='INBOX' ORDER BY ts DESC LIMIT ?2) m \
         WHERE NOT EXISTS (SELECT 1 FROM mail_body b WHERE b.account=?1 AND b.folder='INBOX' AND b.uid=m.uid)")?
        .query_map(rusqlite::params![aid, PREFETCH], |r| r.get(0))?.collect()).await?;
    if uids.is_empty() { return Ok(0); }
    imap::examine(s, "INBOX").await?;
    for &uid in &uids {
        let v = full(s, &a.id, "INBOX", uid).await?;
        remember(app, &a.id, "INBOX", uid, &v).await;
    }
    Ok(uids.len())
}

async fn mark_seen_cached(app: &App, aid: &str, folder: &str, uid: u32) {
    let (aid, folder) = (aid.to_string(), folder.to_string());
    let _ = app.db.call(move |c| c.execute(r"UPDATE msg SET flags=trim(flags || ' \Seen') WHERE account=?1 AND folder=?2 AND uid=?3 AND flags NOT LIKE '%\Seen%'", rusqlite::params![aid, folder, uid])).await;
    super::changed(app);
}

/// One attachment: (filename, content type, bytes).
pub async fn attachment(app: &App, account: &str, folder: &str, uid: u32, index: usize) -> anyhow::Result<(String, String, Vec<u8>)> {
    let a = account::account(app, account).await?;
    let mut s = imap::connect(app, &a).await?;
    let r = async {
        imap::examine(&mut s, folder).await?;
        let f = head(&mut s, uid).await?;
        if big(&f) {
            let parts = f.structure.clone().unwrap_or_default();
            let p = *structure_atts(&parts).get(index).ok_or_else(|| anyhow!("attachment {index} not found"))?;
            let meta = att(index, &p.filename, &p.ctype, 0, false);
            let bytes = one_part(&mut s, uid, &p.section).await?;
            return anyhow::Ok((meta["filename"].as_str().unwrap_or("").to_string(), p.ctype.clone(), bytes));
        }
        let raw = whole(&mut s, uid).await?;
        let m = MessageParser::default().parse(&raw).ok_or_else(|| anyhow!("unreadable message"))?;
        let (i, meta) = parsed_atts(&m).into_iter().nth(index).ok_or_else(|| anyhow!("attachment {index} not found"))?;
        Ok((meta["filename"].as_str().unwrap_or("").to_string(), meta["content_type"].as_str().unwrap_or("").to_string(), part_bytes(&m.parts[i])))
    }.await;
    imap::logout(s).await;
    r
}

fn uploads_dir(app: &App) -> std::path::PathBuf { std::path::Path::new(&app.cfg.data_dir).join("uploads") }

/// Store a compose upload; returns its token.
pub async fn upload(app: &App, filename: &str, ctype: &str, mut field: axum::extract::multipart::Field<'_>) -> anyhow::Result<Value> {
    use tokio::io::AsyncWriteExt;
    let dir = uploads_dir(app);
    tokio::fs::create_dir_all(&dir).await?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let path = dir.join(&token);
    let mut file = tokio::fs::File::create(&path).await?;
    let mut size = 0usize;
    while let Some(chunk) = field.chunk().await? {
        size += chunk.len();
        if size > UPLOAD_MAX { drop(file); let _ = tokio::fs::remove_file(&path).await; bail!("files can be up to 25 MB"); }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    let name: String = std::path::Path::new(filename).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into()).chars().take(200).collect();
    let (t, n, ct, p) = (token.clone(), name.clone(), ctype.to_string(), path.to_string_lossy().into_owned());
    app.db.call(move |c| c.execute("INSERT INTO uploads (token,filename,ctype,size,path,ts) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![t, n, ct, size as i64, p, now_ts()])).await?;
    Ok(json!({"token": token, "filename": name, "size": size}))
}

pub async fn delete_upload(app: &App, token: &str) -> anyhow::Result<()> {
    let t = token.to_string();
    let path: Option<String> = app.db.call(move |c| {
        use rusqlite::OptionalExtension;
        let p = c.query_row("SELECT path FROM uploads WHERE token=?1", [&t], |r| r.get(0)).optional()?;
        c.execute("DELETE FROM uploads WHERE token=?1", [&t])?;
        Ok(p)
    }).await?;
    if let Some(p) = path { let _ = tokio::fs::remove_file(p).await; }
    Ok(())
}

/// Uploads older than a day go.
pub async fn expire_uploads(app: &App) {
    let old = now_ts() - 86400;
    let tokens: Vec<String> = app.db.call(move |c| c.prepare("SELECT token FROM uploads WHERE ts<?1")?.query_map([old], |r| r.get(0))?.collect()).await.unwrap_or_default();
    for t in tokens { let _ = delete_upload(app, &t).await; }
}

async fn smtp(app: &App, a: &Account) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
    let user = if a.smtp_user.is_empty() { a.imap_user.clone() } else { a.smtp_user.clone() };
    let b = if a.smtp_security == "ssl" || a.smtp_port == 465 { AsyncSmtpTransport::<Tokio1Executor>::relay(&a.smtp_host)? } else { AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&a.smtp_host)? };
    let b = b.port(a.smtp_port).timeout(Some(Duration::from_secs(30)));
    let b = if a.oauth.is_empty() {
        let pw = crate::secrets::decrypt(if a.smtp_pw.is_empty() { &a.imap_pw } else { &a.smtp_pw });
        b.credentials(Credentials::new(user, pw)).authentication(vec![Mechanism::Plain, Mechanism::Login])
    } else {
        b.credentials(Credentials::new(user, account::token(app, a).await?)).authentication(vec![Mechanism::Xoauth2])
    };
    Ok(b.build())
}

/// Send one message through the account's SMTP.
pub async fn smtp_send(app: &App, a: &Account, m: lettre::Message) -> anyhow::Result<()> {
    smtp(app, a).await?.send(m).await.map_err(|e| anyhow!("the mail server refused it: {e}"))?;
    Ok(())
}

pub fn from_box(a: &Account) -> anyhow::Result<Mailbox> {
    Ok(Mailbox::new(if a.display_name.is_empty() { None } else { Some(a.display_name.clone()) }, a.from.parse()?))
}

fn mailboxes(s: &str) -> anyhow::Result<Option<Mailboxes>> {
    let s = s.trim().trim_matches(',');
    if s.is_empty() { return Ok(None); }
    Ok(Some(s.parse::<Mailboxes>().map_err(|e| anyhow!("bad address in {s:?}: {e}"))?))
}

/// Send: {account_id, to, cc, bcc, subject, body, attachments:[tokens], in_reply_to?, references?, source?:{account,folder,uid}}.
pub async fn send(app: &App, req: &Value) -> anyhow::Result<Value> {
    let str_of = |k: &str| req[k].as_str().unwrap_or("").to_string();
    let accts = account::accounts(app).await?;
    let id = str_of("account_id");
    let a = accts.iter().find(|a| if id.is_empty() { a.default } else { a.id == id }).filter(|a| a.usable() && !a.smtp_host.is_empty())
        .ok_or_else(|| anyhow!("no account to send from"))?.clone();
    let mut b = lettre::Message::builder().from(from_box(&a)?).subject(str_of("subject"))
        .message_id(Some(format!("<{}@{}>", uuid::Uuid::new_v4().simple(), a.from.rsplit('@').next().unwrap_or("cerq.local"))));
    let mut any = false;
    for (k, add) in [("to", 0), ("cc", 1), ("bcc", 2)] {
        if let Some(list) = mailboxes(&str_of(k))? {
            for m in list { any = true; b = match add { 0 => b.to(m), 1 => b.cc(m), _ => b.bcc(m) }; }
        }
    }
    if !any { bail!("who is it for?"); }
    let irt = str_of("in_reply_to");
    if !irt.is_empty() { b = b.in_reply_to(irt.clone()); }
    let refs = str_of("references");
    if !refs.is_empty() { b = b.references(refs); } else if !irt.is_empty() { b = b.references(irt); }
    let tokens: Vec<String> = req["attachments"].as_array().map(|v| v.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default();
    let body = SinglePart::plain(str_of("body"));
    let msg = if tokens.is_empty() { b.singlepart(body)? } else {
        let mut mp = MultiPart::mixed().singlepart(body);
        for t in &tokens {
            let t2 = t.clone();
            let row: Option<(String, String, String)> = app.db.call(move |c| { use rusqlite::OptionalExtension; c.query_row("SELECT filename, ctype, path FROM uploads WHERE token=?1", [t2], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional() }).await?;
            let (name, ct, path) = row.ok_or_else(|| anyhow!("an attachment expired; add it again"))?;
            let ct = ContentType::parse(&ct).unwrap_or(ContentType::parse("application/octet-stream")?);
            mp = mp.singlepart(Attachment::new(name).body(tokio::fs::read(&path).await?, ct));
        }
        b.multipart(mp)?
    };
    let raw = msg.formatted();
    smtp_send(app, &a, msg).await?;
    for t in &tokens { let _ = delete_upload(app, t).await; }
    // After a successful send, filing a copy and marking the source are best effort.
    if let Err(e) = after_send(app, &a, &raw, &req["source"]).await { tracing::warn!("after send: {e:#}"); }
    super::sync::KICK.notify_one();
    Ok(json!({"ok": true}))
}

/// Append to Sent where the server doesn't keep a copy itself, and mark the
/// message answered as \Answered.
async fn after_send(app: &App, a: &Account, raw: &[u8], source: &Value) -> anyhow::Result<()> {
    let src_uid = source["uid"].as_i64().or_else(|| source["uid"].as_str().and_then(|u| u.parse().ok()));
    let src = match (source["account"].as_str(), source["folder"].as_str(), src_uid) { (Some(acc), Some(f), Some(u)) if acc == a.id => Some((f.to_string(), u)), _ => None };
    let sent = if a.saves_sent() { None } else { rules::role(app, &a.id, "sent").await };
    if src.is_none() && sent.is_none() { return Ok(()); }
    let mut s = imap::connect(app, a).await?;
    let r = async {
        if let Some(folder) = &sent { imap::t(s.append(folder, Some("(\\Seen)"), None, raw)).await?; }
        if let Some((folder, uid)) = &src {
            imap::select(&mut s, folder).await?;
            imap::uid_store(&mut s, &uid.to_string(), "+FLAGS (\\Answered)").await?;
            let (aid, f, u) = (a.id.clone(), folder.clone(), *uid);
            app.db.call(move |c| c.execute(r"UPDATE msg SET flags=trim(flags || ' \Answered') WHERE account=?1 AND folder=?2 AND uid=?3 AND flags NOT LIKE '%\Answered%'", rusqlite::params![aid, f, u])).await?;
            super::changed(app);
        }
        anyhow::Ok(())
    }.await;
    imap::logout(s).await;
    r
}

const REPLY_PROMPT: &str = "You are drafting an email reply. Write only the reply body, no subject line, and no extra commentary. \
For English replies, default to 'Hi [Name]' rather than 'Hey'. Be direct and concise. Match the tone and the language of the original email.\n\n\
MECHANICAL STYLE RULES - CRITICAL: Never use an em dash or en dash; use -- instead. Never use curly apostrophes; write I'm, don't, we'll with straight '.\n\n\
IDENTITY RULE - CRITICAL: write as the user/mailbox owner only. NEVER sign as, speak as, or imply you are the recipient, original sender, quoted sender, \
assistant, company, or any third party. Do not copy a name from the quoted thread into the sign-off.\n\n\
CRITICAL RULE: NEVER invent facts, names, dates, phone numbers, emails, addresses, or any specifics not present in the original email. \
If the sender asks for information you don't have, say plainly that you don't have it on hand; do not guess. \
If you have no real information to offer, write a short honest reply (2-4 sentences max).\n\n\
OUTPUT FORMAT: Put ONLY the final email reply between these exact markers, each on its own line:\n<<<REPLY>>>\n(the reply body goes here)\n<<<END>>>";

/// The reply between the markers, with the mechanical style rules applied.
pub fn clean_reply(out: &str) -> String {
    let t = llm::strip_think(out);
    let t = match t.split_once("<<<REPLY>>>") { Some((_, r)) => r.split("<<<END>>>").next().unwrap_or(r).to_string(), None => t.replace("<<<END>>>", "") };
    t.replace(['\u{2014}', '\u{2013}'], "--").replace(['\u{2019}', '\u{2018}'], "'").trim().to_string()
}

/// A reply draft from your model for {account, folder, uid, hint?}.
pub async fn ai_reply(app: &App, req: &Value) -> anyhow::Result<Value> {
    let (acc, folder) = (req["account"].as_str().unwrap_or(""), req["folder"].as_str().unwrap_or("INBOX"));
    let uid = req["uid"].as_u64().or_else(|| req["uid"].as_str().and_then(|u| u.parse().ok())).ok_or_else(|| anyhow!("uid missing"))? as u32;
    let m = read(app, acc, folder, uid, false).await?;
    let me = app.setting("CERQ_MY_NAME");
    let mut sys = REPLY_PROMPT.to_string();
    if !me.trim().is_empty() { sys += &format!("\n\nSign off as {}.", me.trim()); } else { sys += "\n\nOmit the sign-off name."; }
    let mut user = format!("Recipient: {} <{}>\nSubject: {}\n\nOriginal email:\n{}\n\n", m["from_name"].as_str().unwrap_or(""), m["from_address"].as_str().unwrap_or(""),
        m["subject"].as_str().unwrap_or(""), m["body"].as_str().unwrap_or("").chars().take(6000).collect::<String>());
    let hint = req["hint"].as_str().unwrap_or("").trim();
    if !hint.is_empty() {
        user += &format!("User guidance for THIS reply. Fold it into a normal polished reply; don't answer with only this guidance:\n{}\n\n", hint.chars().take(2000).collect::<String>());
    }
    user += "Draft a reply. Return only the reply body text.";
    let out = llm::complete(app, vec![json!({"role": "system", "content": sys}), json!({"role": "user", "content": user})], 900, llm::Prio::Fg).await
        .ok_or_else(|| anyhow!("Your model didn't answer. Try again in a moment."))?;
    let draft = clean_reply(&out);
    if draft.is_empty() { bail!("Your model didn't answer. Try again in a moment."); }
    Ok(json!({"draft": draft}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_replies() {
        assert_eq!(decode_name("utf-8''Factura%20%C3%B1.pdf"), "Factura ñ.pdf");
        assert_eq!(decode_name("=?utf-8?b?w7Fvw7FvLnBkZg==?="), "ñoño.pdf");
        assert_eq!(decode_name("plain.pdf"), "plain.pdf");
        assert_eq!(clean_reply("<think>x</think>notes\n<<<REPLY>>>\nHi Ana — I’m in.\n<<<END>>>\n"), "Hi Ana -- I'm in.");
        assert_eq!(clean_reply("Just text"), "Just text");
    }

    #[test]
    fn cid_pictures() {
        let mut html = r#"<p>hi</p><img src="cid:logo@x"><img src='cid:logo@x'>"#.to_string();
        let mut budget = 100;
        assert!(embed(&mut html, "<logo@x>", "image/png", b"PNG", &mut budget));
        assert_eq!(html, r#"<p>hi</p><img src="data:image/png;base64,UE5H"><img src='data:image/png;base64,UE5H'>"#);
        assert_eq!(budget, 97);
        assert!(!embed(&mut html, "<other@x>", "image/png", b"PNG", &mut budget));
        assert!(!embed(&mut html, "<logo@x>", "application/pdf", b"PDF", &mut budget));
        let mut tiny = 2;
        let mut h2 = "cid:a".to_string();
        assert!(!embed(&mut h2, "a", "image/gif", b"GIF", &mut tiny));
    }

    #[test]
    fn recipients() {
        let m = mailboxes("\"Doe, John\" <j@x.com>, a@b.c,").unwrap().unwrap();
        let v: Vec<String> = m.into_iter().map(|b| b.email.to_string()).collect();
        assert_eq!(v, vec!["j@x.com", "a@b.c"]);
        assert!(mailboxes("  ").unwrap().is_none());
        assert!(mailboxes("not an address").is_err());
    }

    #[test]
    fn attachments_of_a_parsed_message() {
        let raw = b"Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nhi\r\n--b\r\nContent-Type: application/pdf; name=a.pdf\r\nContent-Disposition: attachment; filename=a.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERg==\r\n--b\r\nContent-Type: image/png\r\nContent-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\niVBO\r\n--b--\r\n";
        let m = MessageParser::default().parse(raw).unwrap();
        let a = parsed_atts(&m);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].1["filename"], "a.pdf");
        assert_eq!(a[0].1["size"], 4);
        assert_eq!(part_bytes(&m.parts[a[0].0]), b"%PDF");
        assert_eq!(a[1].1["filename"], "attachment_1.png");
        assert_eq!(a[1].1["is_inline"], true);
    }
}
