//! CalDAV over HTTP (iCloud with an app password, Google with a bearer
//! token): discovery, the time-range REPORT, and PUT/DELETE for write-back.
//! Redirects are never followed (as Odysseus: a URL that passed the check
//! can't be bounced somewhere else).
use anyhow::{anyhow, bail, Context};
use quick_xml::events::Event as X;
use reqwest::{Method, StatusCode};
use std::{sync::OnceLock, time::Duration};

#[derive(Clone)]
pub enum Auth { Basic(String, String), Bearer(String) }

/// Its own client: no redirects, a longer timeout for big REPORTs.
pub fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder()
        .tls_backend_preconfigured(crate::rustls_config())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(1)
        .build().expect("http client"))
}

fn req(method: &str, url: &str, auth: &Auth) -> reqwest::RequestBuilder {
    let r = client().request(Method::from_bytes(method.as_bytes()).unwrap(), url).header("User-Agent", "CERQ/1.0");
    match auth { Auth::Basic(u, p) => r.basic_auth(u, Some(p)), Auth::Bearer(t) => r.bearer_auth(t) }
}

// ---- a tiny XML tree (namespaces dropped: DAV and CalDAV names don't clash)
#[derive(Debug, Default)]
pub struct El { pub name: String, pub attrs: Vec<(String, String)>, pub text: String, pub kids: Vec<El> }
impl El {
    pub fn child(&self, n: &str) -> Option<&El> { self.kids.iter().find(|k| k.name == n) }
    pub fn path(&self, p: &[&str]) -> Option<&El> { p.iter().try_fold(self, |e, n| e.child(n)) }
    pub fn attr(&self, n: &str) -> Option<&str> { self.attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str()) }
    /// Every descendant with this name.
    pub fn all<'a>(&'a self, n: &str, out: &mut Vec<&'a El>) {
        for k in &self.kids { if k.name == n { out.push(k); } k.all(n, out); }
    }
}

fn local(n: &[u8]) -> String {
    let s = String::from_utf8_lossy(n);
    s.rsplit(':').next().unwrap_or("").to_string()
}

pub fn parse_xml(s: &str) -> anyhow::Result<El> {
    let mut r = quick_xml::Reader::from_str(s);
    let mut stack: Vec<El> = vec![El::default()];
    let open = |e: &quick_xml::events::BytesStart| El {
        name: local(e.name().as_ref()),
        attrs: e.attributes().flatten().map(|a| (local(a.key.as_ref()), a.unescape_value().map(|v| v.to_string()).unwrap_or_default())).collect(),
        ..Default::default()
    };
    loop {
        match r.read_event()? {
            X::Start(e) => stack.push(open(&e)),
            X::Empty(e) => { let el = open(&e); stack.last_mut().unwrap().kids.push(el); }
            X::End(_) => {
                let el = stack.pop().ok_or_else(|| anyhow!("bad xml"))?;
                stack.last_mut().ok_or_else(|| anyhow!("bad xml"))?.kids.push(el);
            }
            X::Text(t) => stack.last_mut().unwrap().text.push_str(&t.decode()?),
            X::CData(t) => stack.last_mut().unwrap().text.push_str(&t.decode()?),
            X::GeneralRef(g) => {
                let txt = match g.resolve_char_ref()? {
                    Some(c) => c.to_string(),
                    None => quick_xml::escape::resolve_predefined_entity(&g.decode()?).unwrap_or("").to_string(),
                };
                stack.last_mut().unwrap().text.push_str(&txt);
            }
            X::Eof => break,
            _ => {}
        }
    }
    stack.pop().ok_or_else(|| anyhow!("bad xml"))
}

/// (href, the prop element of its 200 propstat) for each multistatus response.
fn responses(root: &El) -> Vec<(String, &El)> {
    let mut rs = vec![];
    root.all("response", &mut rs);
    rs.into_iter().filter_map(|r| {
        let href = r.child("href")?.text.trim().to_string();
        let ok = r.kids.iter().filter(|k| k.name == "propstat")
            .find(|p| p.child("status").map(|s| s.text.contains(" 200")).unwrap_or(true))?;
        Some((href, ok.child("prop")?))
    }).collect()
}

async fn dav(method: &str, url: &str, auth: &Auth, depth: &str, body: String) -> anyhow::Result<El> {
    let r = req(method, url, auth).header("Depth", depth).header("Content-Type", "application/xml; charset=utf-8").body(body).send().await?;
    let st = r.status();
    if st == StatusCode::UNAUTHORIZED || st == StatusCode::FORBIDDEN { bail!("sign-in refused ({st})"); }
    if !st.is_success() { bail!("{method} {} answered {st}", url.split('?').next().unwrap_or(url)); }
    parse_xml(&r.text().await?)
}

fn join(base: &str, href: &str) -> anyhow::Result<String> { Ok(url::Url::parse(base)?.join(href)?.to_string()) }

const NS: &str = r#"xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/""#;

#[derive(Debug, Clone)]
pub struct RemoteCal { pub url: String, pub name: String, pub ctag: String, pub color: String, pub read_only: bool }

/// principal → calendar-home-set → calendars; the URL itself as one
/// calendar when the server has no discovery (Google's …/user → …/events).
pub async fn discover(base: &str, auth: &Auth) -> anyhow::Result<Vec<RemoteCal>> {
    let principal = dav("PROPFIND", base, auth, "0", format!(r#"<?xml version="1.0" encoding="utf-8"?><d:propfind {NS}><d:prop><d:current-user-principal/></d:prop></d:propfind>"#)).await?;
    let mut cals = vec![];
    if let Some(p) = responses(&principal).iter().find_map(|(_, prop)| prop.path(&["current-user-principal", "href"])) {
        let purl = join(base, p.text.trim())?;
        let home = dav("PROPFIND", &purl, auth, "0", format!(r#"<?xml version="1.0" encoding="utf-8"?><d:propfind {NS}><d:prop><c:calendar-home-set/></d:prop></d:propfind>"#)).await?;
        if let Some(h) = responses(&home).iter().find_map(|(_, prop)| prop.path(&["calendar-home-set", "href"])) {
            let hurl = join(&purl, h.text.trim())?;
            let list = dav("PROPFIND", &hurl, auth, "1", format!(r#"<?xml version="1.0" encoding="utf-8"?><d:propfind {NS}><d:prop><d:resourcetype/><d:displayname/><cs:getctag/><a:calendar-color/><c:supported-calendar-component-set/><d:current-user-privilege-set/></d:prop></d:propfind>"#)).await?;
            for (href, prop) in responses(&list) {
                if prop.path(&["resourcetype", "calendar"]).is_none() { continue; }
                let comps: Vec<String> = prop.child("supported-calendar-component-set").map(|s| s.kids.iter().filter_map(|c| c.attr("name").map(String::from)).collect()).unwrap_or_default();
                let mut privs = vec![];
                if let Some(p) = prop.child("current-user-privilege-set") { p.all("privilege", &mut privs); }
                let writable = privs.is_empty() || privs.iter().any(|p| p.kids.iter().any(|k| matches!(k.name.as_str(), "write" | "write-content" | "all" | "bind")));
                let color = prop.child("calendar-color").map(|c| c.text.trim().to_string()).unwrap_or_default();
                cals.push(RemoteCal {
                    url: join(&hurl, &href)?,
                    name: prop.child("displayname").map(|d| d.text.trim().to_string()).unwrap_or_default(),
                    ctag: prop.child("getctag").map(|c| c.text.trim().to_string()).unwrap_or_default(),
                    color: if color.len() >= 7 && color.starts_with('#') { color[..7].to_string() } else { String::new() },
                    read_only: !writable || (!comps.is_empty() && !comps.iter().any(|c| c.eq_ignore_ascii_case("VEVENT"))),
                });
            }
        }
    }
    if cals.is_empty() {
        let mut u = url::Url::parse(base)?;
        let google = u.host_str().map(|h| h.ends_with("googleusercontent.com")).unwrap_or(false);
        if google && u.path().trim_end_matches('/').ends_with("/user") {
            let p = format!("{}/events/", u.path().trim_end_matches('/').trim_end_matches("/user"));
            u.set_path(&p);
        }
        cals.push(RemoteCal { url: u.to_string(), name: String::new(), ctag: String::new(), color: String::new(), read_only: false });
    }
    Ok(cals)
}

/// A calendar object from the server: (href, etag, iCalendar text).
pub struct Obj { pub href: String, pub etag: String, pub data: String }

fn objects(root: &El, base: &str) -> anyhow::Result<(Vec<Obj>, Vec<String>)> {
    let (mut got, mut missing) = (vec![], vec![]);
    for (href, prop) in responses(root) {
        let etag = prop.child("getetag").map(|e| e.text.trim().to_string()).unwrap_or_default();
        let url = join(base, &href)?;
        match prop.child("calendar-data").map(|d| d.text.clone()).filter(|d| d.contains("BEGIN:")) {
            Some(data) => got.push(Obj { href: url, etag, data }),
            None => if !href.ends_with('/') { missing.push(href) },
        }
    }
    Ok((got, missing))
}

/// Every object with a VEVENT in [start, end) (UTC "YYYYMMDDTHHMMSSZ").
/// Servers that answer with hrefs only get a calendar-multiget for the data.
pub async fn report(cal_url: &str, auth: &Auth, start: &str, end: &str) -> anyhow::Result<Vec<Obj>> {
    let q = format!(r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query {NS}><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{start}" end="{end}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#);
    let root = dav("REPORT", cal_url, auth, "1", q).await.context("calendar query")?;
    let (mut got, missing) = objects(&root, cal_url)?;
    drop(root);
    for chunk in missing.chunks(50) {
        let hrefs: String = chunk.iter().map(|h| format!("<d:href>{}</d:href>", quick_xml::escape::escape(h.as_str()))).collect();
        let q = format!(r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-multiget {NS}><d:prop><d:getetag/><c:calendar-data/></d:prop>{hrefs}</c:calendar-multiget>"#);
        let root = dav("REPORT", cal_url, auth, "1", q).await.context("calendar multiget")?;
        got.extend(objects(&root, cal_url)?.0);
    }
    Ok(got)
}

/// GET one object; None when it's gone.
pub async fn get(href: &str, auth: &Auth) -> anyhow::Result<Option<String>> {
    let r = req("GET", href, auth).send().await?;
    if r.status() == StatusCode::NOT_FOUND || r.status() == StatusCode::GONE { return Ok(None); }
    if !r.status().is_success() { bail!("GET answered {}", r.status()); }
    Ok(Some(r.text().await?))
}

/// PUT an object; returns its new ETag when the server says.
pub async fn put(href: &str, auth: &Auth, ics: String) -> anyhow::Result<String> {
    let r = req("PUT", href, auth).header("Content-Type", "text/calendar; charset=utf-8").body(ics).send().await?;
    if !r.status().is_success() { bail!("PUT answered {}: {}", r.status(), r.text().await.unwrap_or_default().chars().take(200).collect::<String>()); }
    Ok(r.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or("").to_string())
}

pub async fn delete(href: &str, auth: &Auth) -> anyhow::Result<()> {
    let r = req("DELETE", href, auth).send().await?;
    if r.status().is_success() || r.status() == StatusCode::NOT_FOUND || r.status() == StatusCode::GONE { return Ok(()); }
    bail!("DELETE answered {}", r.status())
}

/// Where a new event goes on the server: <calendar>/<uid>.ics.
pub fn new_href(cal_url: &str, uid: &str) -> String {
    let safe: String = uid.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.@".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("{}/{}.ics", cal_url.trim_end_matches('/'), safe)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multistatus() {
        let x = r#"<?xml version="1.0"?><multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><response><href>/cal/a.ics</href><propstat><prop><getetag>"1"</getetag><C:calendar-data><![CDATA[BEGIN:VCALENDAR
END:VCALENDAR]]></C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/cal/b.ics</href><propstat><prop><getetag>"2"</getetag><C:calendar-data>BEGIN:VCALENDAR&#13;
SUMMARY:A &amp; B
END:VCALENDAR</C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/cal/c.ics</href><propstat><prop><getetag>"3"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        let (got, missing) = objects(&parse_xml(x).unwrap(), "https://h.example/cal/").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].href, "https://h.example/cal/a.ics");
        assert_eq!(got[1].etag, "\"2\"");
        assert!(got[1].data.contains("SUMMARY:A & B"));
        assert_eq!(missing, ["/cal/c.ics"]);
        assert_eq!(new_href("https://h/cal/", "a b@x"), "https://h/cal/a%20b@x.ics");
    }
}
