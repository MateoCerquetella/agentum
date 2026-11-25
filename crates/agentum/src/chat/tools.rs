//! The tools Ask CERQ can use. Reads run at once; writes (calendar changes,
//! sending mail) wait for your approval in the chat.
use crate::{state::App, util::s};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access { Private, Workspace, Web }

impl Access {
    pub fn allows(self, name: &str) -> bool {
        match self {
            Self::Private => false,
            Self::Workspace => !matches!(name, "web_search" | "web_fetch"),
            Self::Web => true,
        }
    }
}

pub fn defs(access: Access) -> Vec<Value> {
    if access == Access::Private { return vec![]; }
    let f = |name: &str, desc: &str, props: Value, req: &[&str]| json!({"type": "function", "function": {"name": name, "description": desc,
        "parameters": {"type": "object", "properties": props, "required": req}}});
    [
      f("calendar_list", "List calendar events between two dates (inclusive). Use for any question about the schedule.",
        json!({"start": {"type": "string", "description": "YYYY-MM-DD"}, "end": {"type": "string", "description": "YYYY-MM-DD"}}), &["start", "end"]),
      f("calendar_create", "Create a calendar event or reminder. Times are local (no offset). The user must approve.",
        json!({"title": {"type": "string"}, "start": {"type": "string", "description": "YYYY-MM-DDTHH:MM, or YYYY-MM-DD for all day"},
               "end": {"type": "string", "description": "optional, same format"}, "all_day": {"type": "boolean"}, "location": {"type": "string"},
               "description": {"type": "string"}, "calendar_id": {"type": "string", "description": "optional; default calendar if empty"}}), &["title", "start"]),
      f("calendar_update", "Change an event (id from calendar_list). Only pass the fields to change. The user must approve.",
        json!({"id": {"type": "string"}, "title": {"type": "string"}, "start": {"type": "string"}, "end": {"type": "string"}, "location": {"type": "string"}, "description": {"type": "string"}}), &["id"]),
      f("calendar_delete", "Delete an event (id from calendar_list). The user must approve.", json!({"id": {"type": "string"}}), &["id"]),
      f("mail_search", "Search the user's email (subject, sender, snippet). Returns ids for mail_read.",
        json!({"query": {"type": "string"}, "limit": {"type": "integer"}}), &["query"]),
      f("mail_waiting", "Who is waiting for a reply from the user (mail they haven't answered) and whom the user is waiting on (mail they sent with no answer yet), from the last 30 days. Use for 'who is waiting for me', 'what do I owe a reply', 'who hasn't answered me'.", json!({}), &[]),
      f("mail_read", "Read one email in full (id from mail_search).", json!({"id": {"type": "string", "description": "account:folder:uid"}}), &["id"]),
      f("mail_send", "Send a new email, or reply when reply_to is an id from mail_search. The user reviews and approves it.",
        json!({"to": {"type": "string"}, "cc": {"type": "string"}, "subject": {"type": "string"}, "body": {"type": "string"},
               "reply_to": {"type": "string", "description": "optional id of the email being answered"}, "account_id": {"type": "string"}}), &["body"]),
      f("todo_list", "List open to-dos.", json!({}), &[]),
      f("todo_add", "Add a to-do (optionally with a due date YYYY-MM-DD or YYYY-MM-DDTHH:MM).", json!({"title": {"type": "string"}, "due": {"type": "string"}}), &["title"]),
      f("todo_done", "Mark a to-do done (id from todo_list).", json!({"id": {"type": "string"}}), &["id"]),
      f("notes_search", "Search the user's notes (their floatnote Markdown notes) by words. Returns ids, titles, dates and the matching line; read one with note_read. Use for anything the user may have written down.",
        json!({"query": {"type": "string"}}), &["query"]),
      f("note_read", "Read one note in full (id from notes_search).", json!({"id": {"type": "string"}}), &["id"]),
      f("note_add", "Save a new note (Markdown; the title is its first line). Not for reminders: use calendar_create or todo_add for those.", json!({"title": {"type": "string"}, "content": {"type": "string"}}), &["title"]),
      f("memory_add", "Remember a lasting fact about the user (preferences, people, places) for future chats. One short fact per call. Not for events or to-dos.", json!({"text": {"type": "string"}}), &["text"]),
      f("memory_forget", "Forget a memory that is wrong or that the user asks you to forget (its [id] from your instructions).", json!({"id": {"type": "string"}}), &["id"]),
      f("file_read", "Read a file the user attached: one part of it, or the lines matching a query. To find something specific in a long file, try a query first.",
        json!({"id": {"type": "string", "description": "file id from the attachment"}, "part": {"type": "integer", "description": "1-based part number"}, "query": {"type": "string", "description": "optional words to find"}}), &["id"]),
      f("code_status", "What the user's coding agents (Agentum threads on their machines) are doing: each open thread's project, title, machine and state (working, needs the user, error, idle). Use for 'what are my agents doing', 'is the PR ready', 'what needs me in code'.", json!({}), &[]),
      f("code_task", "Hand a coding task to the Dispatcher: it starts an Agentum thread (Claude Code) in that project on the user's main machine. Use when the user asks to code, fix, build or change something in one of their projects, or writes @dispatcher. Pass the project's name as the user said it and the task as a clear brief in the user's words (goal, where, done when). It sends the task to Claude, so never put private mail, calendar or memory details in it. The user must approve.",
        json!({"project": {"type": "string", "description": "project name, e.g. agentum, website"}, "task": {"type": "string"}}), &["project", "task"]),
      f("web_search", "Search the public web for current facts or when the user asks for an online lookup. Use a short public query without private details. Returns titles, links and snippets.", json!({"query": {"type": "string"}}), &["query"]),
      f("web_fetch", "Read a web page as text.", json!({"url": {"type": "string"}}), &["url"]),
    ].into_iter().filter(|tool| access.allows(tool["function"]["name"].as_str().unwrap_or_default())).collect()
}

pub fn needs_approval(name: &str) -> bool { matches!(name, "calendar_create" | "calendar_update" | "calendar_delete" | "mail_send" | "code_task") }

fn local_iso(v: &str) -> String { v.trim().to_string() }

/// A one-line description for approval cards and receipts.
pub async fn summary(app: &App, name: &str, a: &Value) -> String {
    match name {
        "calendar_create" => {
            let cal = calendar_name(app, &s(a, "calendar_id")).await;
            format!("Create “{}” · {}{}{} · {}", s(a, "title"), s(a, "start").replace('T', " "), if s(a, "end").is_empty() { String::new() } else { format!(" – {}", s(a, "end").replace('T', " ")) },
                if s(a, "location").is_empty() { String::new() } else { format!(" · {}", s(a, "location")) }, cal)
        }
        "calendar_update" => format!("Change event {}: {}", s(a, "id"), a.as_object().map(|o| o.iter().filter(|(k, _)| *k != "id").map(|(k, v)| format!("{k} → {}", v.as_str().unwrap_or(&v.to_string()))).collect::<Vec<_>>().join(", ")).unwrap_or_default()),
        "calendar_delete" => format!("Delete event {}", s(a, "id")),
        "mail_send" => format!("Send “{}” to {}", s(a, "subject"), s(a, "to")),
        "code_task" => format!("Start a coding task on {} (Claude, Opus 5.5): {}", s(a, "project"), s(a, "task")),
        _ => name.to_string(),
    }
}

async fn calendar_name(app: &App, id: &str) -> String {
    let cals = crate::calendar::calendars(app).await.unwrap_or_default();
    cals.iter().find(|c| if id.is_empty() { c["default"].as_bool().unwrap_or(false) } else { s(c, "id") == id })
        .map(|c| s(c, "name")).unwrap_or_else(|| "your default calendar".into())
}

/// Fill in what the approval card must show (mail: exactly what the recipient gets).
pub async fn preview(app: &App, name: &str, a: &Value) -> Value {
    let mut a = a.clone();
    if name == "mail_send" {
        let accts = crate::mail::accounts_brief(app).await.unwrap_or_default();
        let mut acct = s(&a, "account_id");
        if !s(&a, "reply_to").is_empty() {
            if let Some((ac, folder, uid)) = split_mail_id(&s(&a, "reply_to")) {
                if let Ok(m) = crate::mail::read(app, &ac, &folder, uid).await {
                    if acct.is_empty() { acct = ac.clone(); }
                    if s(&a, "to").is_empty() { a["to"] = json!(s(&m, "from_address")); }
                    if s(&a, "subject").is_empty() { let sub = s(&m, "subject"); a["subject"] = json!(if sub.to_lowercase().starts_with("re:") { sub } else { format!("Re: {sub}") }); }
                    a["in_reply_to"] = m["message_id"].clone();
                    a["references"] = json!(format!("{} {}", s(&m, "references"), s(&m, "message_id")).trim().to_string());
                }
            }
        }
        let pick = accts.iter().find(|x| s(x, "id") == acct).or_else(|| accts.iter().find(|x| x["default"].as_bool().unwrap_or(false))).or(accts.first());
        if let Some(p) = pick { a["account_id"] = p["id"].clone(); a["from"] = p["from"].clone(); }
    }
    if name == "calendar_create" { a["calendar"] = json!(calendar_name(app, &s(&a, "calendar_id")).await); }
    a
}

pub fn split_mail_id(id: &str) -> Option<(String, String, i64)> {
    let mut p = id.rsplitn(2, ':');
    let uid = p.next()?.parse().ok()?;
    let rest = p.next()?;
    let (acct, folder) = rest.split_once(':')?;
    Some((acct.to_string(), folder.to_string(), uid))
}

/// Run a tool. Ok(text for the model), Err(reason).
pub async fn run(app: &App, name: &str, a: &Value) -> Result<String, String> {
    let r: anyhow::Result<String> = async {
        match name {
            "calendar_list" => {
                let d = |k: &str| chrono::NaiveDate::parse_from_str(&s(a, k), "%Y-%m-%d");
                let (st, en) = (d("start")?, d("end")?);
                let occ = crate::calendar::occurrences(app, st, en + chrono::Duration::days(1)).await?;
                if occ.is_empty() { return Ok("No events.".into()); }
                Ok(occ.iter().take(80).map(|o| format!("- id={} | {} | {} | {}{}", o.id, if o.all_day { o.start.format("%a %Y-%m-%d (all day)").to_string() } else { format!("{}–{}", o.start.format("%a %Y-%m-%d %H:%M"), o.end.format("%H:%M")) },
                    o.title, o.calendar, if o.location.is_empty() { String::new() } else { format!(" | at {}", o.location) })).collect::<Vec<_>>().join("\n"))
            }
            "calendar_create" => {
                let all_day = a["all_day"].as_bool().unwrap_or(false) || s(a, "start").len() == 10;
                let ev = crate::calendar::create_event(app, json!({"summary": s(a, "title"), "dtstart": local_iso(&s(a, "start")), "dtend": if s(a, "end").is_empty() { Value::Null } else { json!(local_iso(&s(a, "end"))) },
                    "all_day": all_day, "location": s(a, "location"), "description": s(a, "description"), "calendar_id": if s(a, "calendar_id").is_empty() { Value::Null } else { json!(s(a, "calendar_id")) }})).await?;
                Ok(format!("Created: {}", ev))
            }
            "calendar_update" => {
                let mut p = json!({});
                for (k, to) in [("title", "summary"), ("start", "dtstart"), ("end", "dtend"), ("location", "location"), ("description", "description")] { if !s(a, k).is_empty() { p[to] = json!(s(a, k)); } }
                let ev = crate::calendar::update_event(app, &s(a, "id"), p).await?;
                Ok(format!("Updated: {}", ev))
            }
            "calendar_delete" => { crate::calendar::delete_event(app, &s(a, "id")).await?; Ok("Deleted.".into()) }
            "code_status" => Ok(crate::code::status_text(app).await),
            "code_task" => crate::code::start_task(app, &s(a, "project"), &s(a, "task")).await,
            "mail_waiting" => {
                let rz = crate::mail::reply_zero(app, 30).await?;
                // No ids: the model copies them into the answer. To read one, it finds it with mail_search.
                let day = |m: &Value| chrono::DateTime::from_timestamp(m["ts"].as_i64().unwrap_or(0), 0).map(|d| d.with_timezone(&app.tz()).format("%a %d %b").to_string()).unwrap_or_default();
                let name = |m: &Value| if s(m, "from_name").is_empty() { s(m, "from_addr") } else { s(m, "from_name") };
                let mine: Vec<String> = rz["to_reply"].as_array().cloned().unwrap_or_default().iter().take(15)
                    .map(|m| format!("- {} wrote \"{}\" on {}", name(m), s(m, "subject"), day(m))).collect();
                let theirs: Vec<String> = rz["awaiting"].as_array().cloned().unwrap_or_default().iter().take(15).map(|m| {
                    let to = m["to"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                    format!("- you wrote to {} \"{}\" on {} ({} days without an answer)", if to.is_empty() { "someone".into() } else { to }, s(m, "subject"), day(m), m["days"])
                }).collect();
                Ok(format!("Waiting for a reply from the user ({}):\n{}\n\nThe user is waiting on ({}):\n{}",
                    mine.len(), if mine.is_empty() { "nobody".into() } else { mine.join("\n") }, theirs.len(), if theirs.is_empty() { "nobody".into() } else { theirs.join("\n") }))
            }
            "mail_search" => {
                let n = a["limit"].as_u64().unwrap_or(10).clamp(1, 25) as usize;
                let rows = crate::mail::search(app, &s(a, "query"), n).await?;
                if rows.is_empty() { return Ok("No emails found.".into()); }
                Ok(rows.iter().map(|m| format!("- id={}:{}:{} | {} | from {} <{}> | {} | {}", s(m, "account"), s(m, "folder"), m["uid"], chrono::DateTime::from_timestamp(m["ts"].as_i64().unwrap_or(0), 0).map(|d| d.with_timezone(&app.tz()).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default(),
                    s(m, "from_name"), s(m, "from_addr"), s(m, "subject"), m["extra"]["snippet"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n"))
            }
            "mail_read" => {
                let (ac, f, uid) = split_mail_id(&s(a, "id")).ok_or_else(|| anyhow::anyhow!("bad id; use one from mail_search"))?;
                let m = crate::mail::read(app, &ac, &f, uid).await?;
                let body: String = s(&m, "body").chars().take(8000).collect();
                Ok(format!("From: {} <{}>\nTo: {}\nDate: {}\nSubject: {}\n\n{}", s(&m, "from_name"), s(&m, "from_address"), s(&m, "to"), s(&m, "date"), s(&m, "subject"), body))
            }
            "mail_send" => {
                let r = crate::mail::send(app, json!({"account_id": a["account_id"], "to": s(a, "to"), "cc": s(a, "cc"), "bcc": "", "subject": s(a, "subject"), "body": s(a, "body"),
                    "attachments": [], "in_reply_to": a["in_reply_to"], "references": a["references"],
                    "source": split_mail_id(&s(a, "reply_to")).map(|(ac, f, u)| json!({"account": ac, "folder": f, "uid": u}))})).await?;
                Ok(format!("Sent. {r}"))
            }
            "todo_list" => {
                let t = crate::notes::open_todos(app).await?;
                if t.is_empty() { return Ok("No open to-dos.".into()); }
                Ok(t.iter().map(|n| format!("- id={} | {}{}", s(n, "id"), s(n, "title"), n["due"].as_str().map(|d| format!(" | due {d}")).unwrap_or_default())).collect::<Vec<_>>().join("\n"))
            }
            "todo_add" => { let n = crate::notes::insert(app, &json!({"title": s(a, "title"), "note_type": "todo", "due": s(a, "due")})).await?; Ok(format!("Added to-do id={}", s(&n, "id"))) }
            "todo_done" => { crate::notes::patch(app, &s(a, "id"), &json!({"done": true})).await?; Ok("Marked done.".into()) }
            "notes_search" => {
                let (d, q) = (crate::floatnote::dir(app), s(a, "query"));
                let hits = tokio::task::spawn_blocking(move || crate::floatnote::search_in(&d, &q, 10)).await?;
                if hits.is_empty() { return Ok("No notes match.".into()); }
                Ok(hits.iter().map(|h| format!("- id={} | {} | {} | {}", s(h, "id"), s(h, "title"),
                    chrono::DateTime::from_timestamp(h["mtime"].as_f64().unwrap_or(0.0) as i64, 0).map(|t| t.with_timezone(&app.tz()).format("%Y-%m-%d").to_string()).unwrap_or_default(), s(h, "line"))).collect::<Vec<_>>().join("\n"))
            }
            "note_read" => {
                let (d, id) = (crate::floatnote::dir(app), s(a, "id"));
                let n = tokio::task::spawn_blocking(move || crate::floatnote::read_in(&d, &id)).await?.ok_or_else(|| anyhow::anyhow!("bad id; use one from notes_search"))?;
                let md: String = s(&n, "md").chars().take(8000).collect();
                if md.is_empty() { anyhow::bail!("no such note"); }
                Ok(md)
            }
            "note_add" => {
                let (title, content) = (s(a, "title").trim().to_string(), s(a, "content"));
                if title.is_empty() { anyhow::bail!("a note needs a title"); }
                let md = if content.trim().is_empty() { title } else { format!("{title}\n\n{}", content.trim()) };
                let (d, stamp) = (crate::floatnote::dir(app), crate::today::now(app).format("%Y-%m-%d %H.%M.%S").to_string());
                let id = tokio::task::spawn_blocking(move || crate::floatnote::create_in(&d, &stamp, &md)).await??;
                app.emit(json!({"kind": "notes"}));
                Ok(format!("Saved note id={id}"))
            }
            "memory_add" => {
                let new = super::memory::add(app, &s(a, "text")).await?;
                app.emit(json!({"kind": "memories"}));
                Ok(if new { "Remembered.".into() } else { "Already remembered.".into() })
            }
            "memory_forget" => super::memory::forget(app, &s(a, "id")).await,
            "file_read" => super::files::read(app, a).await,
            "web_search" => web_search(app, &s(a, "query")).await,
            "web_fetch" => web_fetch(app, &s(a, "url")).await,
            _ => anyhow::bail!("unknown tool {name}"),
        }
    }.await;
    r.map_err(|e| format!("{e:#}"))
}

async fn web_search(app: &App, q: &str) -> anyhow::Result<String> {
    if q.trim().is_empty() { anyhow::bail!("empty query"); }
    if !app.cfg.searxng.is_empty() {
        let v: Value = app.http.get(format!("{}/search", app.cfg.searxng.trim_end_matches('/'))).query(&[("q", q), ("format", "json")]).timeout(Duration::from_secs(15)).send().await?.error_for_status()?.json().await?;
        let rs: Vec<String> = v["results"].as_array().cloned().unwrap_or_default().iter().take(8).map(|r| format!("- {} | {} | {}", s(r, "title"), s(r, "url"), s(r, "content"))).collect();
        return Ok(if rs.is_empty() { "No results.".into() } else { rs.join("\n") });
    }
    if let Ok(found) = duckduckgo(app, q).await { if !found.is_empty() { return Ok(found.join("\n")); } }
    let body = app.http.get("https://www.bing.com/search").query(&[("q", q), ("format", "rss")])
        .timeout(Duration::from_secs(15)).send().await?.error_for_status()?.bytes().await?;
    let feed = feed_rs::parser::parse(&body[..])?;
    let out: Vec<String> = feed.entries.into_iter().filter_map(|entry| {
        let link = entry.links.first()?;
        let title = entry.title?.content;
        let summary = entry.summary.map(|s| crate::widgets::strip_tags(&s.content)).unwrap_or_default();
        Some(format!("- {} | {} | {}", title, link.href, summary))
    }).take(8).collect();
    Ok(if out.is_empty() { "No results.".into() } else { out.join("\n") })
}

/// DuckDuckGo's plain HTML results: each is a result__a link (title) and a result__snippet.
async fn duckduckgo(app: &App, q: &str) -> anyhow::Result<Vec<String>> {
    static RESULT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(
        r#"(?s)class="result__a" href="([^"]+)"[^>]*>(.*?)</a>.*?class="result__snippet"[^>]*>(.*?)</a>"#).unwrap());
    let html = app.http.post("https://html.duckduckgo.com/html/").header("User-Agent", "Mozilla/5.0 (X11; Linux aarch64)").form(&[("q", q)])
        .timeout(Duration::from_secs(15)).send().await?.error_for_status()?.text().await?;
    let clean = |t: &str| html_escape::decode_html_entities(&crate::widgets::strip_tags(t)).trim().to_string();
    Ok(RESULT.captures_iter(&html).filter(|c| !c[1].contains("duckduckgo.com/y.js")).take(8)
        .map(|c| format!("- {} | {} | {}", clean(&c[2]), &c[1], clean(&c[3]))).collect())
}

/// Fetch a public web page as text. Refuses private and local addresses
/// (a page could ask the model to poke at your LAN).
async fn web_fetch(app: &App, u: &str) -> anyhow::Result<String> {
    let url = url::Url::parse(u)?;
    if !matches!(url.scheme(), "http" | "https") { anyhow::bail!("only http(s)"); }
    let host = url.host_str().ok_or_else(|| anyhow::anyhow!("no host"))?.to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| is_private(a.ip())) { anyhow::bail!("refusing a private or local address"); }
    let r = app.http.get(url).header("User-Agent", "Mozilla/5.0 (X11; Linux aarch64) cerq/1.0").timeout(Duration::from_secs(20)).send().await?;
    let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut body = Vec::new();
    let mut st = r.bytes_stream();
    use futures::StreamExt;
    while let Some(c) = st.next().await { body.extend_from_slice(&c?); if body.len() > 1_500_000 { break; } }
    let text = String::from_utf8_lossy(&body);
    let t = if ct.contains("html") {
        let re = regex::Regex::new(r"(?is)<(script|style|noscript|svg|head)[^>]*>.*?</(script|style|noscript|svg|head)>").unwrap();
        crate::widgets::strip_tags(&re.replace_all(&text, " "))
    } else { text.to_string() };
    Ok(t.chars().take(6000).collect())
}

fn is_private(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.octets()[0] == 100 && (64..128).contains(&v.octets()[1]),
        std::net::IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80 || v.to_ipv4_mapped().map(|m| is_private(m.into())).unwrap_or(false),
    }
}
