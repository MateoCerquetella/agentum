//! The bulk cleaner: archive everything from a sender, and unsubscribe
//! (RFC 8058 one-click first, then mailto through your own SMTP).
use super::{account, cache::{self, args}, imap, message, rules::{self, Action}};
use crate::state::App;
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};

pub async fn archive_sender(app: &App, aid: &str, sender: &str) -> anyhow::Result<Value> {
    let a = account::account(app, aid).await?;
    let sender = sender.to_lowercase();
    let targets = cache::msgs(app, "WHERE account=?1 AND folder='INBOX' AND from_addr=?2", args(&[&aid, &sender])).await?;
    let mut s = imap::connect(app, &a).await?;
    let act = Action { kind: "archive".into(), ..Default::default() };
    let mut done = vec![];
    for m in &targets { done.push(rules::apply_action(app, &mut s, &a, "INBOX", m, &act, "Bulk archive").await); }
    imap::logout(s).await;
    let uids: Vec<i64> = targets.iter().map(|m| m.uid).collect();
    let aid2 = aid.to_string();
    app.db.call(move |c| { for u in uids { c.execute("DELETE FROM msg WHERE account=?1 AND folder='INBOX' AND uid=?2", rusqlite::params![aid2, u])?; } Ok(()) }).await?;
    super::changed(app);
    let errors: Vec<&Value> = done.iter().filter(|d| d["status"] == "error").map(|d| &d["detail"]).take(3).collect();
    Ok(json!({"ok": true, "archived": done.iter().filter(|d| d["status"] == "applied").count(), "errors": errors}))
}

/// The links in a List-Unsubscribe header: (https, mailto).
pub fn unsub_links(header: &str) -> (Vec<String>, Vec<String>) {
    let links: Vec<String> = header.split('<').skip(1).filter_map(|x| x.split_once('>').map(|(l, _)| l.trim().to_string())).collect();
    let pick = |p: &str| links.iter().filter(|l| l.to_lowercase().starts_with(p)).cloned().collect::<Vec<_>>();
    (pick("https://"), pick("mailto:"))
}

/// An address on the public internet (not private, loopback, link-local, CGNAT, reserved…).
pub fn is_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_private() || v.is_loopback() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.is_documentation() || v.is_multicast()
                || o[0] == 0 || (o[0] == 100 && (o[1] & 0xc0) == 64) || (o[0] == 198 && (o[1] & 0xfe) == 18) || o[0] >= 240 || (o[0] == 192 && o[1] == 0 && o[2] == 0))
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() { return is_global(IpAddr::V4(v4)); }
            let s = v.segments();
            !(v.is_loopback() || v.is_unspecified() || v.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || s[0] == 0x2001 && s[1] == 0x0db8 || s[0] == 0x64 && s[1] == 0xff9b)
        }
    }
}

/// RFC 8058 POST to a public address only, pinned to the address checked (no
/// second DNS lookup); redirects are not followed. Returns the HTTP status.
async fn one_click(url: &str) -> anyhow::Result<u16> {
    let u = url::Url::parse(url)?;
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() { anyhow::bail!("not a plain https URL"); }
    let host = u.host_str().ok_or_else(|| anyhow::anyhow!("no host"))?.to_string();
    let port = u.port().unwrap_or(443);
    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await?.collect();
    if addrs.is_empty() || !addrs.iter().all(|a| is_global(a.ip())) { anyhow::bail!("unsubscribe host is not a public address"); }
    addrs.sort();
    let client = reqwest::Client::builder().tls_backend_preconfigured(crate::rustls_config()).redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addrs[0]).timeout(std::time::Duration::from_secs(20)).build()?;
    let r = client.post(u).header("Content-Type", "application/x-www-form-urlencoded").header("User-Agent", "Agentum Mail").body("List-Unsubscribe=One-Click").send().await?;
    Ok(r.status().as_u16())
}

pub async fn unsubscribe(app: &App, aid: &str, sender: &str) -> anyhow::Result<Value> {
    let a = account::account(app, aid).await?;
    let Some(m) = cache::msgs(app, "WHERE account=?1 AND from_addr=?2 AND list_unsub != '' ORDER BY ts DESC LIMIT 1", args(&[&aid, &sender.to_lowercase()])).await?.pop() else {
        return Ok(json!({"ok": false, "error": "no unsubscribe header from this sender"}));
    };
    let (https, mailto) = unsub_links(&m.list_unsub);
    if !https.is_empty() && m.list_unsub_post.to_lowercase().contains("one-click") {
        match one_click(&https[0]).await {
            Ok(st) if (200..300).contains(&st) || mailto.is_empty() => return Ok(json!({"ok": (200..300).contains(&st), "method": "one-click", "status": st})),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("one-click unsubscribe: {e:#}");
                if mailto.is_empty() { return Ok(json!({"ok": false, "method": "link", "url": https[0], "error": "one-click failed; open the sender's page to confirm"})); }
            }
        }
    }
    if let Some(link) = mailto.first() {
        let u = url::Url::parse(link)?;
        let to = u.path().to_string();
        let subject = u.query_pairs().find(|(k, _)| k.eq_ignore_ascii_case("subject")).map(|(_, v)| v.into_owned()).unwrap_or_else(|| "unsubscribe".into());
        let msg = lettre::Message::builder().from(message::from_box(&a)?).to(to.parse()?).subject(subject).body("unsubscribe".to_string())?;
        message::smtp_send(app, &a, msg).await?;
        return Ok(json!({"ok": true, "method": "mailto", "to": to}));
    }
    if let Some(h) = https.first() {
        return Ok(json!({"ok": false, "method": "link", "url": h, "error": "this sender only offers a web page; open it to confirm"}));
    }
    Ok(json!({"ok": false, "error": "no usable unsubscribe method"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links() {
        let (h, m) = unsub_links("<mailto:unsub@x.com?subject=bye>, <https://x.com/u?id=1> , <HTTP://insecure>");
        assert_eq!(h, vec!["https://x.com/u?id=1"]);
        assert_eq!(m, vec!["mailto:unsub@x.com?subject=bye"]);
        assert_eq!(unsub_links("no links here"), (vec![], vec![]));
    }
    #[test]
    fn public_addresses() {
        for ip in ["10.1.2.3", "192.168.1.1", "127.0.0.1", "100.64.0.1", "169.254.1.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:10.0.0.1"] {
            assert!(!is_global(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "142.250.1.1", "2606:4700::1111"] { assert!(is_global(ip.parse().unwrap()), "{ip}"); }
    }
}
