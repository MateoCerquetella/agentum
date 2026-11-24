//! Calendar sync: pull every account (90 days back, a year ahead) into the
//! tables, and push local edits back. Port of Odysseus' caldav_sync +
//! caldav_writeback + cerq_calendar (Google bearer tokens, Outlook via Graph
//! with its rotating refresh-token chain in kv "calendar.tokens").
//!
//! Dev switches: CERQ_SKIP_OUTLOOK=1 leaves Outlook accounts alone (their
//! refresh tokens rotate); CERQ_CALENDAR_READONLY=1 never writes to a server.
use super::{dav::{self, Auth, RemoteCal}, graph, ics, store};
use crate::state::App;
use anyhow::{anyhow, bail, Context};
use chrono::{Duration, NaiveDateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{HashMap, HashSet}, sync::{LazyLock, Mutex}};

const LOOKBACK_DAYS: i64 = 90;
const LOOKAHEAD_DAYS: i64 = 365;

fn env_on(k: &str) -> bool { matches!(std::env::var(k).as_deref(), Ok("1") | Ok("true")) }

// ---- accounts and tokens
#[derive(Debug, Clone)]
pub struct Account { pub id: String, pub label: String, pub kind: String, pub url: String, pub username: String, pub password: String }

async fn accounts(app: &App) -> anyhow::Result<Vec<Account>> {
    app.db.call(|c| {
        let mut st = c.prepare("SELECT id, coalesce(label,''), coalesce(kind,'caldav'), coalesce(url,''), coalesce(username,''), coalesce(password,'') FROM calendar_accounts")?;
        st.query_map([], |r| Ok(Account { id: r.get(0)?, label: r.get(1)?, kind: r.get(2)?, url: r.get(3)?, username: r.get(4)?, password: r.get(5)? }))?.collect()
    }).await
}

fn hash16(s: &str) -> String { hex::encode(Sha256::digest(s.as_bytes()))[..16].to_string() }

/// Access tokens by "g:<hash>" / "m:<hash>": (token, expires at).
static TOKENS: LazyLock<Mutex<HashMap<String, (String, i64)>>> = LazyLock::new(Default::default);
static MS_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

fn cached(k: &str) -> Option<String> {
    TOKENS.lock().unwrap().get(k).filter(|(_, exp)| exp - 120 > crate::util::now_ts()).map(|(t, _)| t.clone())
}
async fn token_post(app: &App, url: &str, form: &[(&str, &str)]) -> anyhow::Result<Value> {
    let r = app.http.post(url).form(form).send().await?;
    let st = r.status();
    let v: Value = r.json().await.unwrap_or(json!({}));
    if !st.is_success() { bail!("token refresh failed ({st}): {}", v["error_description"].as_str().or(v["error"].as_str()).unwrap_or("")); }
    Ok(v)
}

async fn google_token(app: &App, refresh: &str) -> anyhow::Result<String> {
    let k = format!("g:{}", hash16(refresh));
    if let Some(t) = cached(&k) { return Ok(t); }
    let (id, secret) = (std::env::var("GOOGLE_OAUTH_CLIENT_ID").unwrap_or_default(), std::env::var("GOOGLE_OAUTH_CLIENT_SECRET").unwrap_or_default());
    let v = token_post(app, "https://oauth2.googleapis.com/token", &[("client_id", &id), ("client_secret", &secret), ("refresh_token", refresh), ("grant_type", "refresh_token")]).await?;
    let t = v["access_token"].as_str().ok_or_else(|| anyhow!("no access token from Google"))?.to_string();
    TOKENS.lock().unwrap().insert(k, (t.clone(), crate::util::now_ts() + v["expires_in"].as_i64().unwrap_or(3600)));
    Ok(t)
}

/// Microsoft rotates refresh tokens: the newest one of the chain started by
/// the signed-in token (seed "m:<sha256[:16]>") lives in kv "calendar.tokens".
async fn microsoft_token(app: &App, refresh: &str) -> anyhow::Result<String> {
    let seed = format!("m:{}", hash16(refresh));
    if let Some(t) = cached(&seed) { return Ok(t); }
    let _g = MS_LOCK.lock().await;
    if let Some(t) = cached(&seed) { return Ok(t); }
    let mut chain = app.db.kv_get("calendar.tokens").await?.unwrap_or(json!({}));
    let current = chain[&seed].as_str().map(crate::secrets::decrypt).filter(|t| !t.is_empty()).unwrap_or_else(|| refresh.to_string());
    let id = std::env::var("MICROSOFT_OAUTH_CLIENT_ID").unwrap_or_default();
    let v = token_post(app, "https://login.microsoftonline.com/consumers/oauth2/v2.0/token", &[("client_id", &id), ("refresh_token", &current),
        ("grant_type", "refresh_token"), ("scope", "https://graph.microsoft.com/Calendars.ReadWrite offline_access")]).await?;
    if let Some(next) = v["refresh_token"].as_str() {
        chain[&seed] = json!(crate::secrets::encrypt(next));
        app.db.kv_set("calendar.tokens", &chain).await?;
    }
    let t = v["access_token"].as_str().ok_or_else(|| anyhow!("no access token from Microsoft"))?.to_string();
    TOKENS.lock().unwrap().insert(seed, (t.clone(), crate::util::now_ts() + v["expires_in"].as_i64().unwrap_or(3600)));
    Ok(t)
}

enum Conn { Dav(Auth), Graph(String) }

async fn connect(app: &App, a: &Account) -> anyhow::Result<Conn> {
    let secret = crate::secrets::decrypt(&a.password);
    if a.url.is_empty() || a.username.is_empty() || secret.is_empty() { bail!("missing URL, username, or password"); }
    Ok(match a.kind.as_str() {
        "google" => Conn::Dav(Auth::Bearer(google_token(app, &secret).await?)),
        "outlook" => Conn::Graph(microsoft_token(app, &secret).await?),
        _ => Conn::Dav(Auth::Basic(a.username.clone(), secret)),
    })
}

/// Whose calendar it is, for "Calendario · Gmail": the mail account with
/// that address, else the address' domain (cerq_calendar._account_label).
async fn account_label(app: &App, username: &str) -> String {
    let u = username.to_string();
    let name: Option<String> = app.db.call(move |c| c.query_row("SELECT name FROM email_accounts WHERE lower(imap_user)=lower(?1) LIMIT 1", [u], |r| r.get(0)).optional()).await.ok().flatten().flatten();
    if let Some(n) = name.map(|n: String| n.trim().to_string()).filter(|n| !n.is_empty()) { return n; }
    let domain = username.rsplit_once('@').map(|(_, d)| d.to_lowercase()).unwrap_or_default();
    match domain.as_str() {
        "gmail.com" | "googlemail.com" => "Gmail".into(),
        "icloud.com" | "me.com" | "mac.com" => "iCloud".into(),
        "hotmail.com" | "outlook.com" | "live.com" => "Outlook".into(),
        "" => String::new(),
        d => { let f = d.split('.').next().unwrap_or(""); f[..1].to_uppercase() + &f[1..] }
    }
}

// ---- pull
fn url_path(u: &str) -> String { url::Url::parse(u).map(|x| x.path().trim_end_matches('/').to_string()).unwrap_or_else(|_| u.to_string()) }

/// The local row for a remote calendar (matched by account and URL path, so
/// ids stay what Odysseus made them: "caldav-" + sha256(owner, account, url)).
async fn upsert_calendar(app: &App, acct: &Account, rc: &RemoteCal, label: &str) -> anyhow::Result<(String, String)> {
    let base = if rc.name.trim().is_empty() { "Calendar".to_string() } else { rc.name.trim().to_string() };
    let name = if label.is_empty() || base.ends_with(&format!("· {label}")) { base } else { format!("{base} · {label}") };
    let (aid, url, ro, color) = (acct.id.clone(), rc.url.clone(), rc.read_only, rc.color.clone());
    app.db.call(move |c| {
        let mut st = c.prepare("SELECT id, coalesce(remote_url,''), coalesce(ctag,'') FROM calendars WHERE account_id=?1")?;
        let mine: Vec<(String, String, String)> = st.query_map([&aid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        let now = store::now_str();
        if let Some((id, _, ctag)) = mine.into_iter().find(|(_, u, _)| url_path(u) == url_path(&url)) {
            c.execute("UPDATE calendars SET name=?2, remote_url=?3, read_only=?4, source='caldav', updated_at=?5 WHERE id=?1 AND (name IS NOT ?2 OR remote_url IS NOT ?3 OR read_only IS NOT ?4)",
                params![id, name, url, ro as i64, now])?;
            return Ok((id, ctag));
        }
        let id = format!("caldav-{}", &hex::encode(Sha256::digest(format!("admin\n{aid}\n{url}").as_bytes()))[..24]);
        c.execute("INSERT OR IGNORE INTO calendars(id,account_id,name,color,source,remote_url,read_only,created_at,updated_at) VALUES(?1,?2,?3,?4,'caldav',?5,?6,?7,?7)",
            params![id, aid, name, if color.is_empty() { "#5b8abf".to_string() } else { color }, url, ro as i64, now])?;
        Ok((id, String::new()))
    }).await
}

#[derive(Default, Debug)]
struct Stats { calendars: usize, events: usize, deleted: usize, changed: usize }

/// Write one calendar's pull. Events edited here and not yet pushed, or
/// deleted here (tombstone), are left alone; an event already in another of
/// your synced calendars (one invite in two accounts) stays where it is.
/// With a clean read, rows that vanished upstream in the window go.
fn store_events(c: &mut rusqlite::Connection, cal_id: &str, evs: Vec<(ics::Event, String, String)>, win: (NaiveDateTime, NaiveDateTime), clean: bool) -> rusqlite::Result<Stats> {
    let tx = c.transaction()?;
    let mut st = Stats::default();
    let mut seen: HashSet<String> = HashSet::new();
    let now = store::now_str();
    for (e, href, etag) in evs {
        st.events += 1;
        seen.insert(e.uid.clone());
        if tx.query_row("SELECT 1 FROM calendar_tombstones WHERE uid=?1", [&e.uid], |_| Ok(())).optional()?.is_some() { continue; }
        let cur = tx.query_row(&format!("{} WHERE e.uid=?1", store::SELECT), [&e.uid], store::row).optional()?;
        if e.people.is_empty() { tx.execute("DELETE FROM calendar_people WHERE uid=?1", [&e.uid])?; }
        else { tx.execute("INSERT OR REPLACE INTO calendar_people(uid, people) VALUES(?1, ?2)", params![e.uid, serde_json::to_string(&e.people).unwrap_or_default()])?; }
        let (href, etag) = (Some(href).filter(|h| !h.is_empty()), Some(etag).filter(|t| !t.is_empty()));
        match cur {
            Some(r) => {
                if matches!(r.sync_pending.as_deref(), Some("create") | Some("update")) { continue; }
                if r.calendar_id != cal_id && r.cal_source != "local" { continue; }
                let mut old = r.ev.clone();
                old.people = e.people.clone();
                old.exdates = old.exdates.filter(|x| x != "[]");
                if old != e || r.calendar_id != cal_id {
                    tx.execute("UPDATE calendar_events SET calendar_id=?2,summary=?3,description=?4,location=?5,dtstart=?6,dtend=?7,all_day=?8,is_utc=?9,rrule=?10,recurrence_exdates=?11,
                                status=?12,origin='caldav',remote_href=?13,remote_etag=?14,sync_pending=NULL,updated_at=?15 WHERE uid=?1",
                        params![e.uid, cal_id, e.summary, e.description, e.location, store::fmt(e.dtstart), store::fmt(e.dtend), e.all_day as i64, e.is_utc as i64,
                                e.rrule, e.exdates, e.status, href, etag, now])?;
                    st.changed += 1;
                } else {
                    // Same event: keep its updated_at (the brief's signature), refresh the server's handles only.
                    tx.execute("UPDATE calendar_events SET remote_href=?2, remote_etag=?3 WHERE uid=?1 AND (remote_href IS NOT ?2 OR remote_etag IS NOT ?3)", params![e.uid, href, etag])?;
                }
            }
            None => {
                tx.execute("INSERT INTO calendar_events(uid,calendar_id,summary,description,location,dtstart,dtend,all_day,is_utc,rrule,recurrence_exdates,status,origin,remote_href,remote_etag,created_at,updated_at)
                            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'caldav',?13,?14,?15,?15)",
                    params![e.uid, cal_id, e.summary, e.description, e.location, store::fmt(e.dtstart), store::fmt(e.dtend), e.all_day as i64, e.is_utc as i64,
                            e.rrule, e.exdates, e.status, href, etag, now])?;
                st.changed += 1;
            }
        }
    }
    if clean {
        let stale: Vec<String> = {
            let mut q = tx.prepare("SELECT uid FROM calendar_events WHERE calendar_id=?1 AND origin='caldav' AND dtstart>=?2 AND dtstart<=?3 AND remote_href IS NOT NULL AND sync_pending IS NULL")?;
            q.query_map(params![cal_id, store::fmt(win.0), store::fmt(win.1)], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter().filter(|u| !seen.contains(u)).collect()
        };
        for u in &stale { tx.execute("DELETE FROM calendar_events WHERE uid=?1", [u])?; tx.execute("DELETE FROM calendar_people WHERE uid=?1", [u])?; }
        st.deleted = stale.len();
    }
    tx.commit()?;
    Ok(st)
}

async fn pull_account(app: &App, a: &Account, force: bool) -> anyhow::Result<Stats> {
    let conn = connect(app, a).await?;
    let label = account_label(app, &a.username).await;
    let remote = match &conn { Conn::Dav(auth) => dav::discover(&a.url, auth).await.context("discovery")?, Conn::Graph(t) => graph::calendars(t).await? };
    let now = Utc::now().naive_utc();
    let win = (now - Duration::days(LOOKBACK_DAYS), now + Duration::days(LOOKAHEAD_DAYS));
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let tz = app.tz();
    let mut total = Stats::default();
    for rc in remote {
        let (cal_id, old_ctag) = upsert_calendar(app, a, &rc, &label).await?;
        total.calendars += 1;
        // Unchanged since today's pull (CalDAV getctag): nothing to read.
        let stamp = format!("{}|{today}", rc.ctag);
        if !force && !rc.ctag.is_empty() && old_ctag == stamp { continue; }
        let (evs, clean) = match &conn {
            Conn::Dav(auth) => {
                let fmt = |d: NaiveDateTime| d.format("%Y%m%dT%H%M%SZ").to_string();
                let objs = match dav::report(&rc.url, auth, &fmt(win.0), &fmt(win.1)).await {
                    Ok(o) => o,
                    Err(e) => { tracing::warn!("calendar {}: {e:#}", rc.name); continue; }
                };
                let mut evs = vec![];
                let mut clean = true;
                for o in objs {
                    let got = ics::parse_events(&o.data, tz);
                    if got.is_empty() && o.data.contains("BEGIN:VEVENT") { clean = false; }
                    evs.extend(got.into_iter().map(|e| (e, o.href.clone(), o.etag.clone())));
                }
                (evs, clean)
            }
            Conn::Graph(t) => match graph::events(t, &rc.url, win.0, win.1).await {
                Ok(v) => (v, true),
                Err(e) => { tracing::warn!("calendar {}: {e:#}", rc.name); continue; }
            },
        };
        let id2 = cal_id.clone();
        let st = app.db.call(move |c| store_events(c, &id2, evs, win, clean)).await?;
        if clean && !rc.ctag.is_empty() {
            app.db.call(move |c| c.execute("UPDATE calendars SET ctag=?2 WHERE id=?1", params![cal_id, stamp])).await?;
        }
        total.events += st.events; total.deleted += st.deleted; total.changed += st.changed;
    }
    Ok(total)
}

/// Pull every account. {calendars, events, deleted, errors}
pub async fn pull_all(app: &App, force: bool) -> Value {
    let accts = match accounts(app).await { Ok(a) => a, Err(e) => return json!({"calendars": 0, "events": 0, "deleted": 0, "errors": [e.to_string()]}) };
    let (mut cals, mut evs, mut del, mut changed, mut errors) = (0, 0, 0, 0, vec![]);
    for a in accts {
        if a.kind == "outlook" && env_on("CERQ_SKIP_OUTLOOK") { continue; }
        match pull_account(app, &a, force).await {
            Ok(s) => { cals += s.calendars; evs += s.events; del += s.deleted; changed += s.changed + s.deleted; }
            Err(e) => { tracing::warn!("calendar sync {}: {e:#}", a.label); errors.push(format!("{}: {}", a.label, format!("{e:#}").chars().take(200).collect::<String>())); }
        }
    }
    if changed > 0 { app.emit(json!({"kind": "calendar"})); }
    json!({"calendars": cals, "events": evs, "deleted": del, "changed": changed, "errors": errors})
}

// ---- push (write-back)
static PUSH_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

struct Target { acct: Account, cal_url: String }
async fn target(app: &App, cal_id: &str) -> anyhow::Result<Target> {
    let c = store::cals(app).await?.into_iter().find(|c| c.id == cal_id).ok_or_else(|| anyhow!("calendar {cal_id} is gone"))?;
    let acct = accounts(app).await?.into_iter().find(|a| a.id == c.account_id).ok_or_else(|| anyhow!("no account for {}", c.name))?;
    Ok(Target { acct, cal_url: c.remote_url })
}

/// Split a server copy into what we keep around our master: its time zones
/// and the other edited occurrences. `own` = the override this row is, if any.
fn keep_parts(comps: &[ics::Comp], uid: &str) -> (Option<usize>, Vec<usize>) {
    let mut own = None;
    let mut keep = vec![];
    for (i, c) in comps.iter().enumerate() {
        match (c.kind.as_str(), c.get("RECURRENCE-ID")) {
            ("VTIMEZONE", _) => keep.push(i),
            ("VEVENT", Some(rid)) => {
                if format!("{}_{}", c.text("UID"), rid.value.trim().trim_end_matches(['Z', 'z'])) == uid { own = Some(i); } else { keep.push(i); }
            }
            ("VEVENT", None) if c.text("UID") != uid => keep.push(i),
            _ => {}
        }
    }
    (own, keep)
}

/// Create or update one event on its server. An edited occurrence that came
/// from a series object is written back inside that object.
async fn push_row(app: &App, r: &store::EvRow) -> anyhow::Result<(String, String)> {
    let t = target(app, &r.calendar_id).await?;
    let tz = app.tz();
    match connect(app, &t.acct).await? {
        Conn::Graph(tok) => match &r.remote_href {
            Some(h) => Ok((h.clone(), graph::update(&tok, h, &r.ev).await?)),
            None => graph::create(&tok, &t.cal_url, &r.ev).await,
        },
        Conn::Dav(auth) => {
            let href = r.remote_href.clone().unwrap_or_else(|| dav::new_href(&t.cal_url, &r.ev.uid));
            let server = if r.remote_href.is_some() { dav::get(&href, &auth).await? } else { None };
            let comps = server.as_deref().map(ics::components).unwrap_or_default();
            let (own, keep) = keep_parts(&comps, &r.ev.uid);
            let parts: Vec<&ics::Comp> = keep.iter().map(|k| &comps[*k]).collect();
            let body = match own {
                // This row is one edited date of a series: replace that VEVENT, keep the rest.
                Some(i) => {
                    let mut e = r.ev.clone();
                    e.uid = comps[i].text("UID");
                    let rid = comps[i].raw.lines().find(|l| l.to_ascii_uppercase().starts_with("RECURRENCE-ID")).unwrap_or("").to_string();
                    let ve = ics::build_vevent(&e).replacen("END:VEVENT\r\n", &format!("{rid}\r\nEND:VEVENT\r\n"), 1);
                    ics::build_calendar(&ve, &parts)
                }
                None => {
                    // The series' own edited dates stay on the server; they're not skipped dates.
                    let mut e = r.ev.clone();
                    let kept: HashSet<String> = parts.iter().filter_map(|c| c.get("RECURRENCE-ID").and_then(|p| ics::when(p, tz))).map(|w| ics::exdate_key(w, e.all_day)).collect();
                    let ex: Vec<String> = r.exdates().into_iter().filter(|k| !kept.contains(k)).collect();
                    e.exdates = if ex.is_empty() { None } else { Some(ics::ex_json(&ex)) };
                    ics::build_calendar(&ics::build_vevent(&e), &parts)
                }
            };
            let etag = dav::put(&href, &auth, body).await?;
            Ok((href, etag))
        }
    }
}

/// Delete one event from its server (from its tombstone). An edited date of
/// a series is taken out of the series object instead (and skipped there).
async fn push_tombstone(app: &App, uid: &str, cal_id: &str, href: &str) -> anyhow::Result<()> {
    let t = target(app, cal_id).await?;
    match connect(app, &t.acct).await? {
        Conn::Graph(tok) => graph::delete(&tok, href).await,
        Conn::Dav(auth) => {
            let Some(server) = dav::get(href, &auth).await? else { return Ok(()) };
            let comps = ics::components(&server);
            let (own, keep) = keep_parts(&comps, uid);
            let Some(i) = own else {
                // Our own object: delete it, unless the server copy is somebody else's master (then leave it).
                if comps.iter().any(|c| c.kind == "VEVENT" && c.text("UID") == uid) || comps.iter().all(|c| c.kind != "VEVENT") { return dav::delete(href, &auth).await; }
                return Ok(());
            };
            let rid = comps[i].get("RECURRENCE-ID").unwrap();
            let ps: String = rid.params.iter().map(|(k, v)| format!(";{k}={v}")).collect();
            let ex = format!("EXDATE{ps}:{}\r\nEND:VEVENT\r\n", rid.value);
            let mut parts: Vec<ics::Comp> = keep.iter().map(|k| comps[*k].clone()).collect();
            for c in parts.iter_mut().filter(|c| c.kind == "VEVENT" && c.get("RECURRENCE-ID").is_none() && c.text("UID") == comps[i].text("UID")) {
                c.raw = c.raw.replacen("END:VEVENT\r\n", &ex, 1);
            }
            let refs: Vec<&ics::Comp> = parts.iter().collect();
            // build_calendar wants a master first; the kept parts already hold it.
            dav::put(href, &auth, ics::build_calendar("", &refs)).await.map(|_| ())
        }
    }
}

/// Write one local change out now. Never fatal: the row stays pending and the
/// sync loop retries.
pub async fn push(app: &App, uid: &str) -> anyhow::Result<()> {
    if env_on("CERQ_CALENDAR_READONLY") { tracing::info!("calendar write-back of {uid} skipped (CERQ_CALENDAR_READONLY)"); return Ok(()); }
    let _g = PUSH_LOCK.lock().await;
    if let Some(r) = store::get_row(app, uid).await? {
        if !r.remote() || r.sync_pending.is_none() { return Ok(()); }
        let (href, etag) = push_row(app, &r).await?;
        let u = uid.to_string();
        app.db.call(move |c| c.execute("UPDATE calendar_events SET remote_href=?2, remote_etag=coalesce(nullif(?3,''), remote_etag), sync_pending=NULL, origin=coalesce(origin,'caldav') WHERE uid=?1", params![u, href, etag])).await?;
        return Ok(());
    }
    let u = uid.to_string();
    let tomb: Option<(String, String)> = app.db.call(move |c| c.query_row("SELECT calendar_id, coalesce(remote_href,'') FROM calendar_tombstones WHERE uid=?1", [u], |r| Ok((r.get(0)?, r.get(1)?))).optional()).await?;
    if let Some((cal, href)) = tomb {
        if !href.is_empty() { push_tombstone(app, uid, &cal, &href).await?; }
        let u = uid.to_string();
        app.db.call(move |c| c.execute("DELETE FROM calendar_tombstones WHERE uid=?1", [u])).await?;
    }
    Ok(())
}

/// Push in the background, right after the local change.
pub fn push_soon(app: &App, uid: &str) {
    let (app, uid) = (app.clone(), uid.to_string());
    tokio::spawn(async move { if let Err(e) = push(&app, &uid).await { tracing::warn!("calendar write-back {uid}: {e:#}"); } });
}

/// Everything still waiting to go out.
async fn push_pending(app: &App) {
    let uids: Vec<String> = app.db.call(|c| {
        let mut st = c.prepare("SELECT e.uid FROM calendar_events e JOIN calendars c ON c.id=e.calendar_id WHERE c.source != 'local' AND e.sync_pending IS NOT NULL
                                UNION SELECT uid FROM calendar_tombstones")?;
        st.query_map([], |r| r.get(0))?.collect()
    }).await.unwrap_or_default();
    for u in uids { if let Err(e) = push(app, &u).await { tracing::warn!("calendar write-back {u}: {e:#}"); } }
}

// ---- one pull at a time (brand.py's throttle)
struct SyncState { running: bool, at: i64, result: Value }
static STATE: LazyLock<Mutex<SyncState>> = LazyLock::new(|| Mutex::new(SyncState { running: false, at: 0, result: Value::Null }));
static DONE: LazyLock<tokio::sync::Notify> = LazyLock::new(Default::default);

/// The last pull's result ({errors:[…], events, …}), for account status.
pub fn last_result() -> Value { STATE.lock().unwrap().result.clone() }

/// Push what's pending, then pull. A pull under a minute old is reused
/// unless `force`; the caller waits at most 40 s, then gets in_progress.
pub async fn sync(app: &App, force: bool) -> Value {
    let waiter = DONE.notified();
    tokio::pin!(waiter);
    waiter.as_mut().enable();
    {
        let mut st = STATE.lock().unwrap();
        if !st.running {
            if !force && !st.result.is_null() && crate::util::now_ts() - st.at < 60 {
                return json!({"ok": true, "cached": true, "synced_at": st.at, "result": st.result});
            }
            st.running = true;
            let app = app.clone();
            tokio::spawn(async move {
                push_pending(&app).await;
                let res = pull_all(&app, force).await;
                let mut st = STATE.lock().unwrap();
                st.running = false; st.at = crate::util::now_ts(); st.result = res;
                drop(st);
                DONE.notify_waiters();
            });
        }
    }
    match tokio::time::timeout(std::time::Duration::from_secs(40), waiter).await {
        Ok(()) => { let st = STATE.lock().unwrap(); json!({"ok": true, "synced_at": st.at, "result": st.result}) }
        Err(_) => json!({"ok": true, "in_progress": true, "synced_at": STATE.lock().unwrap().at}),
    }
}

/// Every 10 minutes (first after a minute), and drop done marks older than a year.
pub fn start(app: App) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        loop {
            let r = sync(&app, false).await;
            if let Some(e) = r["result"]["errors"].as_array().filter(|e| !e.is_empty()) { tracing::info!("calendar sync: {} errors", e.len()); }
            let old = crate::util::now_ts() - 366 * 86400;
            let _ = app.db.call(move |c| c.execute("DELETE FROM calendar_done WHERE ts < ?1", [old])).await;
            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        }
    });
}
