//! IMAP plumbing on async-imap: connect (implicit TLS or STARTTLS, LOGIN or
//! XOAUTH2), a streaming UID FETCH that hands each message over as it
//! arrives (nothing is held per mailbox), and the few commands the agent uses.
//! Every server round trip has a timeout so a stuck server can't stall a sync.
use super::account::{self, Account};
use crate::{secrets, state::App};
use anyhow::{anyhow, bail, Context};
use async_imap::{imap_proto::{AttributeValue, BodyContentCommon, BodyContentSinglePart, BodyParams, BodyStructure, ContentEncoding, MessageSection, NameAttribute, Response, SectionPath, Status}, types::Mailbox};
use futures::TryStreamExt;
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio_rustls::{client::TlsStream, TlsConnector};

pub type Session = async_imap::Session<TlsStream<TcpStream>>;

const STEP: Duration = Duration::from_secs(60);

/// One server round trip, with a timeout.
pub async fn t<T, E: Into<anyhow::Error>>(f: impl Future<Output = Result<T, E>>) -> anyhow::Result<T> {
    match tokio::time::timeout(STEP, f).await {
        Ok(r) => r.map_err(Into::into),
        Err(_) => bail!("the mail server stopped answering"),
    }
}

async fn tls(host: &str, tcp: TcpStream) -> anyhow::Result<TlsStream<TcpStream>> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).context("bad host name")?;
    Ok(TlsConnector::from(Arc::new(crate::rustls_config())).connect(name, tcp).await?)
}

struct XOAuth2(Option<String>);
impl async_imap::Authenticator for XOAuth2 {
    type Response = String;
    // The first answer is the token; after a failure the server sends an
    // error challenge that wants an empty answer.
    fn process(&mut self, _challenge: &[u8]) -> String { self.0.take().unwrap_or_default() }
}

pub fn xoauth2(user: &str, token: &str) -> String { format!("user={user}\x01auth=Bearer {token}\x01\x01") }

pub async fn connect(app: &App, a: &Account) -> anyhow::Result<Session> {
    let addr = (a.imap_host.as_str(), a.imap_port);
    let tcp = t(TcpStream::connect(addr)).await.with_context(|| format!("can't reach {}", a.imap_host))?;
    let client = if a.imap_port == 993 || !a.imap_starttls {
        let mut c = async_imap::Client::new(tls(&a.imap_host, tcp).await?);
        t(c.read_response()).await?.ok_or_else(|| anyhow!("no greeting from the server"))?;
        c
    } else {
        let mut c = async_imap::Client::new(tcp);
        t(c.read_response()).await?.ok_or_else(|| anyhow!("no greeting from the server"))?;
        t(c.run_command_and_check_ok("STARTTLS", None)).await?;
        async_imap::Client::new(tls(&a.imap_host, c.into_inner()).await?)
    };
    let s = if a.oauth.is_empty() {
        t(async { client.login(&a.imap_user, secrets::decrypt(&a.imap_pw)).await.map_err(|(e, _)| e) }).await.context("sign-in failed")?
    } else {
        let tok = account::token(app, a).await?;
        t(async { client.authenticate("XOAUTH2", XOAuth2(Some(xoauth2(&a.imap_user, &tok)))).await.map_err(|(e, _)| e) }).await.context("sign-in failed")?
    };
    Ok(s)
}

pub async fn logout(mut s: Session) { let _ = tokio::time::timeout(Duration::from_secs(5), s.logout()).await; }

/// A quoted IMAP string; CR/LF/NUL would end the command and start a new one.
pub fn q(name: &str) -> String {
    let n: String = name.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).collect();
    format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Folder names and their special-use flags (lowercase, e.g. "\\sent").
pub async fn list(s: &mut Session) -> anyhow::Result<Vec<(String, Vec<String>)>> {
    let names: Vec<_> = t(async { s.list(Some(""), Some("*")).await?.try_collect::<Vec<_>>().await }).await?;
    Ok(names.iter().map(|n| {
        let flags = n.attributes().iter().map(|f| match f {
            NameAttribute::Sent => "\\sent".to_string(),
            NameAttribute::Junk => "\\junk".into(),
            NameAttribute::Archive => "\\archive".into(),
            NameAttribute::Trash => "\\trash".into(),
            NameAttribute::All => "\\all".into(),
            NameAttribute::Extension(x) => x.to_lowercase(),
            other => format!("{other:?}").to_lowercase(),
        }).collect();
        (n.name().to_string(), flags)
    }).collect())
}

pub async fn examine(s: &mut Session, folder: &str) -> anyhow::Result<Mailbox> { t(s.examine(folder)).await }
pub async fn select(s: &mut Session, folder: &str) -> anyhow::Result<Mailbox> { t(s.select(folder)).await }

/// UID SEARCH, sorted ascending.
pub async fn uid_search(s: &mut Session, query: &str) -> anyhow::Result<Vec<u32>> {
    let mut v: Vec<u32> = t(s.uid_search(query)).await?.into_iter().collect();
    v.sort_unstable();
    Ok(v)
}

pub async fn uid_store(s: &mut Session, set: &str, query: &str) -> anyhow::Result<()> {
    t(async { s.uid_store(set, query).await?.try_collect::<Vec<_>>().await }).await.map(|_| ())
}

/// UID MOVE, or COPY + \Deleted + EXPUNGE on servers without MOVE.
pub async fn mv(s: &mut Session, uid: u32, dst: &str) -> anyhow::Result<()> {
    if t(s.uid_mv(uid.to_string(), dst)).await.is_ok() { return Ok(()); }
    t(s.uid_copy(uid.to_string(), dst)).await?;
    uid_store(s, &uid.to_string(), "+FLAGS (\\Deleted)").await?;
    t(async { s.expunge().await?.try_collect::<Vec<_>>().await }).await.map(|_| ())
}

/// CREATE, ignoring "already exists".
pub async fn ensure_folder(s: &mut Session, name: &str) { let _ = t(s.create(name)).await; }

/// One message out of a FETCH response.
#[derive(Default, Debug)]
pub struct Fetched {
    pub uid: u32,
    /// Raw flags joined by spaces, as cached: "\\Seen \\Flagged".
    pub flags: String,
    pub internal_date: String,
    pub thrid: String,
    pub labels: String,
    pub size: u32,
    /// BODY[HEADER…] contents.
    pub header: Option<Vec<u8>>,
    /// BODY[] contents (whole or partial).
    pub body: Option<Vec<u8>>,
    /// BODY[<part>] and BODY[<part>.MIME] contents by section ("1.2", "1.2.MIME").
    pub parts: HashMap<String, Vec<u8>>,
    /// BODYSTRUCTURE, flattened to its leaf parts.
    pub structure: Option<Vec<Part>>,
}

/// A leaf of BODYSTRUCTURE: enough to show it and to fetch it alone.
#[derive(Debug, Clone, Default)]
pub struct Part {
    /// IMAP section ("1", "2.1"); "TEXT" for a message that isn't multipart.
    pub section: String,
    /// "text/plain", lowercase.
    pub ctype: String,
    /// "attachment", "inline" or "".
    pub disposition: String,
    /// As sent (may still be RFC 2047/2231 encoded).
    pub filename: String,
    /// Size as sent; base64 parts (76-char lines + CRLF) decode to about 57/78 of it.
    pub octets: u32,
    pub base64: bool,
    /// Content-ID as sent ("<image001.png@01D…>"), for cid: pictures.
    pub cid: String,
}

fn flatten(b: &BodyStructure<'_>, prefix: &str, out: &mut Vec<Part>) {
    let leaf = |common: &BodyContentCommon<'_>, other: &BodyContentSinglePart<'_>| {
        let param = |p: &BodyParams<'_>, k: &str| p.as_ref().and_then(|v| v.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.to_string()));
        let disp = common.disposition.as_ref();
        let filename = disp.and_then(|d| param(&d.params, "filename").or_else(|| param(&d.params, "filename*")))
            .or_else(|| param(&common.ty.params, "name")).or_else(|| param(&common.ty.params, "name*")).unwrap_or_default();
        Part {
            section: if prefix.is_empty() { "TEXT".into() } else { prefix.to_string() },
            ctype: format!("{}/{}", common.ty.ty, common.ty.subtype).to_lowercase(),
            disposition: disp.map(|d| d.ty.to_lowercase()).unwrap_or_default(),
            filename,
            octets: other.octets,
            base64: matches!(other.transfer_encoding, ContentEncoding::Base64),
            cid: other.id.as_deref().unwrap_or("").to_string(),
        }
    };
    match b {
        BodyStructure::Multipart { bodies, .. } => {
            for (i, child) in bodies.iter().enumerate() {
                let sec = if prefix.is_empty() { (i + 1).to_string() } else { format!("{prefix}.{}", i + 1) };
                flatten(child, &sec, out);
            }
        }
        BodyStructure::Basic { common, other, .. } | BodyStructure::Text { common, other, .. } | BodyStructure::Message { common, other, .. } => out.push(leaf(common, other)),
    }
}

/// UID FETCH that calls `f` for each message as it arrives.
pub async fn uid_fetch(s: &mut Session, set: &str, query: &str, mut f: impl FnMut(Fetched)) -> anyhow::Result<()> {
    if set.is_empty() { return Ok(()); }
    let id = t(s.run_command(format!("UID FETCH {set} {query}"))).await?;
    loop {
        let r = t(s.read_response()).await?.ok_or_else(|| anyhow!("connection lost"))?;
        match r.parsed() {
            Response::Fetch(_, attrs) => {
                let mut m = Fetched::default();
                for a in attrs {
                    match a {
                        AttributeValue::Uid(u) => m.uid = *u,
                        AttributeValue::Flags(fl) => m.flags = fl.iter().map(|x| x.as_ref()).collect::<Vec<_>>().join(" "),
                        AttributeValue::InternalDate(d) => m.internal_date = d.to_string(),
                        AttributeValue::GmailThrId(x) => m.thrid = x.to_string(),
                        AttributeValue::GmailLabels(l) => m.labels = l.iter().map(|x| if x.contains(' ') { q(x) } else { x.to_string() }).collect::<Vec<_>>().join(" "),
                        AttributeValue::Rfc822Size(n) => m.size = *n,
                        AttributeValue::BodySection { section, data, .. } => {
                            let data = data.as_ref().map(|d| d.to_vec()).unwrap_or_default();
                            match section {
                                None => m.body = Some(data),
                                Some(SectionPath::Full(MessageSection::Header)) => m.header = Some(data),
                                Some(SectionPath::Part(p, sec)) => {
                                    let mut k = p.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".");
                                    if matches!(sec, Some(MessageSection::Mime)) { k.push_str(".MIME"); }
                                    m.parts.insert(k, data);
                                }
                                _ => {}
                            }
                        }
                        AttributeValue::BodyStructure(b) => { let mut v = vec![]; flatten(b, "", &mut v); m.structure = Some(v); }
                        _ => {}
                    }
                }
                // Unsolicited FETCHes (flag changes from elsewhere) carry no UID.
                if m.uid != 0 { f(m); }
            }
            Response::Done { tag, status, information, .. } if *tag == id => {
                if *status != Status::Ok { bail!("FETCH failed: {}", information.as_deref().unwrap_or("")); }
                return Ok(());
            }
            _ => {}
        }
    }
}

/// UID set text from uids ("1,2,3").
pub fn set(uids: &[u32]) -> String { uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",") }

/// The newest UID for a Message-ID in a folder (selected read-write).
pub async fn find_uid(s: &mut Session, folder: &str, msgid: &str) -> Option<u32> {
    let id: String = msgid.split_whitespace().collect();
    if id.is_empty() || select(s, folder).await.is_err() { return None; }
    uid_search(s, &format!("HEADER Message-ID {}", q(&id))).await.ok()?.last().copied()
}
