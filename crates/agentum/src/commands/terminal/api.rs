//! HTTP + WebSocket client wrapping the agentum daemon API.
//!
//! Bearer token is injected as `Authorization: Bearer <token>` for HTTP and
//! as a `?token=…` query parameter for WS upgrades (browsers can't set
//! custom headers on WS, the server accepts both).
//!
//! TLS verification: SSH-style trust-on-first-use. For `https://` URLs we
//! pin to the SHA-256 fingerprint the user accepted on first contact (see
//! [`crate::commands::terminal::trust`]). Plain `http://` is accepted as-is
//! and assumed to live on a trusted network.

use std::sync::Arc;
use std::time::Duration;

use agentum_core::{Event, Session, transcript::AgentTaskState};
use anyhow::{Context, Result, bail};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client as HttpClient;
use rustls::ClientConfig;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::{Connector, connect_async_tls_with_config, tungstenite::Message as WsMsg};
use url::Url;
use uuid::Uuid;

use super::trust;

/// Mirrors the server's `/api/fs/list` response. Used by the TUI's
/// directory picker overlay.
#[derive(Debug, Deserialize, Clone)]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub dirs: Vec<DirEntry>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
}

/// Optional pinned fingerprint, plus an "insecure" escape hatch. The
/// escape hatch is *only* exposed via an explicit CLI flag and only
/// covers the user's own machine in throwaway test setups.
#[derive(Clone, Debug)]
pub enum TlsTrust {
    /// `http://` URL — no TLS at all.
    Plain,
    /// `https://` URL — pin to this SHA-256 fingerprint.
    Pinned(String),
    /// `https://` URL — accept any cert. NOT enabled by default; user must
    /// pass `--insecure`.
    AcceptAny,
}

impl TlsTrust {
    fn rustls_config(&self) -> Option<Arc<ClientConfig>> {
        match self {
            TlsTrust::Plain => None,
            TlsTrust::Pinned(fp) => Some(trust::pinned_tls_config(fp.clone())),
            TlsTrust::AcceptAny => {
                Some(trust::pinned_tls_config(String::new())).map(|_| accept_any_config())
            }
        }
    }
}

fn accept_any_config() -> Arc<ClientConfig> {
    // We deliberately don't expose this anywhere except via --insecure.
    // It's the same shape as before but only reached on explicit opt-in.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let verifier = Arc::new(NoVerify);
    let cfg = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(cfg)
}

#[derive(Debug)]
struct NoVerify;
impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme::*;
        vec![
            RSA_PSS_SHA256,
            RSA_PSS_SHA384,
            RSA_PSS_SHA512,
            RSA_PKCS1_SHA256,
            RSA_PKCS1_SHA384,
            RSA_PKCS1_SHA512,
            ECDSA_NISTP256_SHA256,
            ECDSA_NISTP384_SHA384,
            ED25519,
        ]
    }
}

#[derive(Clone)]
pub struct Client {
    http: HttpClient,
    base: Url,
    token: String,
    trust: TlsTrust,
}

fn build_http(trust: &TlsTrust) -> Result<HttpClient> {
    let mut b = HttpClient::builder().timeout(Duration::from_secs(15));
    if let Some(cfg) = trust.rustls_config() {
        // We pass an owned ClientConfig (reqwest expects the unwrapped form
        // because it stuffs it into a `dyn Any`).
        let owned = (*cfg).clone();
        b = b.use_preconfigured_tls(owned);
    }
    b.build().context("build reqwest client")
}

/// Standalone POST /api/auth/login. Returns the bearer token.
pub async fn login(base: &Url, trust: &TlsTrust, username: &str, password: &str) -> Result<String> {
    let url = base.join("/api/auth/login")?;
    let http = build_http(trust)?;
    #[derive(Serialize)]
    struct Body<'a> {
        username: &'a str,
        password: &'a str,
    }
    let resp = http
        .post(url)
        .json(&Body { username, password })
        .send()
        .await
        .context("login request failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("{} — {}", status, body);
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        token: String,
    }
    let r: Resp = resp.json().await?;
    Ok(r.token)
}

impl Client {
    pub fn new(base: Url, token: String, trust: TlsTrust) -> Result<Self> {
        let http = build_http(&trust)?;
        Ok(Self {
            http,
            base,
            token,
            trust,
        })
    }

    pub async fn health(&self) -> Result<()> {
        let url = self.base.join("/api/health")?;
        let resp = self.http.get(url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            bail!("health returned {}", resp.status())
        }
    }

    /// Probe the server's `/api/health` for advertised capabilities.
    /// Daemons before v0.6.7 don't return a `capabilities` field — those
    /// land here as an empty `Vec` and the caller treats every feature
    /// as unsupported. Doing this once at startup keeps the WS hot path
    /// free of per-frame version checks.
    pub async fn capabilities(&self) -> Result<Vec<String>> {
        #[derive(serde::Deserialize, Default)]
        struct Health {
            #[serde(default)]
            capabilities: Vec<String>,
        }
        let url = self.base.join("/api/health")?;
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("health returned {}", resp.status())
        }
        let h: Health = resp.json().await.unwrap_or_default();
        Ok(h.capabilities)
    }

    pub async fn me(&self) -> Result<String> {
        let url = self.base.join("/api/auth/me")?;
        let resp = self.http.get(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            bail!("me returned {}", resp.status());
        }
        #[derive(serde::Deserialize)]
        struct Me {
            username: String,
        }
        let me: Me = resp.json().await?;
        Ok(me.username)
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        let url = self.base.join("/api/sessions")?;
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json::<Vec<Session>>().await?)
    }

    /// `GET /api/sessions/{id}/agent-tasks` — current plan / todos /
    /// background tasks for one agent. Backed by the daemon's
    /// transcript-tail watcher, so the data refreshes within a beat of
    /// every TodoWrite / ExitPlanMode / Task tool call the agent makes.
    pub async fn agent_tasks(&self, id: Uuid) -> Result<AgentTaskState> {
        let url = self.base.join(&format!("/api/sessions/{id}/agent-tasks"))?;
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json::<AgentTaskState>().await?)
    }

    /// `GET /api/fs/list` — enumerate directories under `path` (or `$HOME`
    /// if `path` is `None`). Mirrors the web `DirPicker`'s feed for the
    /// TUI's workdir picker overlay.
    pub async fn list_dir(&self, path: Option<&str>) -> Result<DirListing> {
        let mut url = self.base.join("/api/fs/list")?;
        if let Some(p) = path {
            url.query_pairs_mut().append_pair("path", p);
        }
        let resp = self.http.get(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{status} — {body}");
        }
        Ok(resp.json::<DirListing>().await?)
    }

    /// `POST /api/sessions` — create a new session row. The server records
    /// it as `Idle`; call `start_session` to actually spawn the tmux process.
    pub async fn create_session(
        &self,
        name: &str,
        workdir: &str,
        tool: &str,
        model: Option<&str>,
        flags: Vec<String>,
    ) -> Result<Session> {
        #[derive(Serialize)]
        struct Body<'a> {
            name: &'a str,
            workdir: &'a str,
            tool: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            model: Option<&'a str>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            flags: Vec<String>,
        }
        let url = self.base.join("/api/sessions")?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&Body {
                name,
                workdir,
                tool,
                model,
                flags,
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{status} — {body}");
        }
        Ok(resp.json::<Session>().await?)
    }

    /// `PATCH /api/sessions/{id}` with `{name: ...}`. Server validates
    /// (trimmed, non-empty, ≤ 64 chars) and emits `session.renamed` on
    /// the bus so other clients pick the new label up automatically.
    /// Allowed even on a running session — pure metadata.
    pub async fn rename_session(&self, id: Uuid, new_name: &str) -> Result<Session> {
        #[derive(Serialize)]
        struct Body<'a> {
            name: &'a str,
        }
        let url = self.base.join(&format!("/api/sessions/{id}"))?;
        let resp = self
            .http
            .patch(url)
            .bearer_auth(&self.token)
            .json(&Body { name: new_name })
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{status} — {body}");
        }
        Ok(resp.json::<Session>().await?)
    }

    pub async fn start_session(&self, id: Uuid) -> Result<()> {
        self.post_session_action(id, "start").await
    }

    pub async fn stop_session(&self, id: Uuid) -> Result<()> {
        self.post_session_action(id, "stop").await
    }

    /// Kept as a thin wrapper around `/api/sessions/{id}/kill`. The TUI's
    /// "Kill" verb now routes through `delete_session(id, force=true)`
    /// (which kills *and* removes the record so the entry disappears
    /// from the tree). The bare-kill endpoint is still useful for any
    /// future caller that wants to stop the process while keeping the
    /// session listed for restart — leave it on the client surface.
    #[allow(dead_code)]
    pub async fn kill_session(&self, id: Uuid) -> Result<()> {
        self.post_session_action(id, "kill").await
    }

    /// DELETE /api/sessions/{id}. Pass `force=true` to also kill a running
    /// session as part of the delete (server does the SIGTERM dance).
    pub async fn delete_session(&self, id: Uuid, force: bool) -> Result<()> {
        let path = if force {
            format!("/api/sessions/{id}?force=true")
        } else {
            format!("/api/sessions/{id}")
        };
        let url = self.base.join(&path)?;
        let resp = self
            .http
            .delete(url)
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{status} — {body}");
        }
        Ok(())
    }

    async fn post_session_action(&self, id: Uuid, action: &str) -> Result<()> {
        let url = self.base.join(&format!("/api/sessions/{id}/{action}"))?;
        let resp = self.http.post(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{status} — {body}");
        }
        Ok(())
    }

    /// Open a bidirectional terminal stream.
    ///
    /// Server → client: tmux pane bytes arrive on `tx` as `TerminalMsg`s.
    /// Client → server: each `Vec<u8>` pulled off `key_rx` is sent as a
    /// binary WS frame, which the server forwards to the tmux pane via
    /// `send-keys -H`. This is what makes the terminal pane interactive
    /// (typing into claude code, sending Ctrl-C, arrow keys, etc.).
    pub fn open_terminal_stream(
        &self,
        id: Uuid,
        tx: mpsc::UnboundedSender<TerminalMsg>,
        mut key_rx: mpsc::UnboundedReceiver<TermOut>,
        resume: bool,
    ) -> JoinHandle<()> {
        let base = self.base.clone();
        let token = self.token.clone();
        let trust = self.trust.clone();
        tokio::spawn(async move {
            // Resume signal travels in the URL query, not as a wire frame:
            // axum strips unknown query params silently, so old daemons
            // (no resume support) do the right thing — they upgrade the
            // WS and proceed with the existing snapshot path. A wire
            // frame would risk being typed into the agent's prompt by
            // any old daemon that doesn't recognise it.
            //
            // CRITICAL: the resume bit must be appended as a structured
            // query pair, NOT baked into `path`. v0.6.21..=v0.6.24 had
            // `format!("/api/sessions/{id}/stream?resume=true")` and
            // handed it to `ws_url`, whose `url.set_path(...)` percent-
            // encodes the `?` (it treats the whole string as a path
            // component). The daemon then saw a literal path of
            // `/api/sessions/{id}/stream?resume=true` (after URL
            // decoding), which doesn't match the registered
            // `/api/sessions/{id}/stream` route and falls through to
            // the SPA fallback (`embed::static_handler`) — 200 OK with
            // index.html. tungstenite reports `HTTP error: 200 OK` and
            // the user sees "stream closed" + cannot reconnect on
            // session-switch. Only first-connects (resume=false)
            // worked, which is why the bug only bit reconnects.
            let path = format!("/api/sessions/{id}/stream");
            let extra: &[(&str, &str)] = if resume { &[("resume", "true")] } else { &[] };
            let url = ws_url(&base, &path, &token, extra);
            let connector = ws_connector(&url, &trust);
            let result = connect_async_tls_with_config(url.as_str(), None, false, connector).await;
            let stream = match result {
                Ok((s, _)) => s,
                Err(e) => {
                    let _ = tx.send(TerminalMsg::Error(format!("ws connect: {e}")));
                    let _ = tx.send(TerminalMsg::Closed);
                    return;
                }
            };
            let (mut sink, mut src) = stream.split();

            // Pump outbound keystrokes / resize messages onto the WS until
            // the channel closes. Detached so a chatty pane never starves
            // keystrokes.
            let writer = tokio::spawn(async move {
                while let Some(out) = key_rx.recv().await {
                    let msg = match out {
                        TermOut::Bytes(b) => WsMsg::Binary(b.into()),
                        TermOut::Resize { cols, rows } => WsMsg::Text(
                            format!("{{\"resize\":{{\"cols\":{cols},\"rows\":{rows}}}}}").into(),
                        ),
                    };
                    if sink.send(msg).await.is_err() {
                        break;
                    }
                }
                let _ = sink.close().await;
            });

            while let Some(msg) = src.next().await {
                match msg {
                    Ok(WsMsg::Binary(b)) => {
                        if tx.send(TerminalMsg::Bytes(b.into())).is_err() {
                            break;
                        }
                    }
                    Ok(WsMsg::Text(t)) => {
                        if tx.send(TerminalMsg::Error(t.to_string())).is_err() {
                            break;
                        }
                    }
                    Ok(WsMsg::Close(_)) => break,
                    Ok(_) => {}
                    Err(e) => {
                        let _ = tx.send(TerminalMsg::Error(format!("ws: {e}")));
                        break;
                    }
                }
            }
            writer.abort();
            let _ = tx.send(TerminalMsg::Closed);
        })
    }

    pub fn open_event_stream(&self, tx: mpsc::UnboundedSender<EventMsg>) -> JoinHandle<()> {
        let base = self.base.clone();
        let token = self.token.clone();
        let trust = self.trust.clone();
        tokio::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                if tx.is_closed() {
                    return;
                }
                let url = ws_url(&base, "/api/events", &token, &[]);
                let connector = ws_connector(&url, &trust);
                let result =
                    connect_async_tls_with_config(url.as_str(), None, false, connector).await;
                let mut stream = match result {
                    Ok((s, _)) => {
                        attempt = 0;
                        let _ = tx.send(EventMsg::Connected);
                        s
                    }
                    Err(_) => {
                        attempt = attempt.saturating_add(1);
                        let delay = backoff_delay(attempt);
                        let _ = tx.send(EventMsg::Reconnecting {
                            attempt,
                            delay_ms: delay.as_millis() as u64,
                        });
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                };
                while let Some(msg) = stream.next().await {
                    if tx.is_closed() {
                        return;
                    }
                    match msg {
                        Ok(WsMsg::Text(t)) => match serde_json::from_str::<Event>(&t) {
                            Ok(ev) => {
                                if tx.send(EventMsg::Event(ev)).is_err() {
                                    return;
                                }
                            }
                            Err(_) => {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                                    let kind = v
                                        .get("kind")
                                        .and_then(|k| k.as_str())
                                        .unwrap_or("unknown")
                                        .to_string();
                                    if tx.send(EventMsg::Raw(kind)).is_err() {
                                        return;
                                    }
                                }
                            }
                        },
                        Ok(WsMsg::Close(_)) => break,
                        Ok(_) => {}
                    Err(err) => {
                        let _ = tx.send(EventMsg::Error(format!("ws: {err}")));
                        break;
                        }
                    }
                }
                let _ = tx.send(EventMsg::Closed);
                attempt = attempt.saturating_add(1);
            }
        })
    }
}

#[derive(Debug)]
pub enum TerminalMsg {
    Bytes(Bytes),
    Reconnecting { attempt: u32, delay_ms: u64 },
    Error(String),
    Closed,
}

/// Outbound terminal stream messages: keystrokes (binary frames) and
/// resize notifications (JSON text frames). Sent from the TUI to the
/// daemon over the same WebSocket as input.
#[derive(Debug, Clone)]
pub enum TermOut {
    Bytes(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

#[derive(Debug)]
pub enum EventMsg {
    Connected,
    Reconnecting { attempt: u32, delay_ms: u64 },
    Event(Event),
    Raw(String),
    Error(String),
    Closed,
}

pub async fn probe_health(base: &Url, trust: &TlsTrust, timeout: Duration) -> Result<()> {
    let mut b = HttpClient::builder().timeout(timeout);
    if let Some(cfg) = trust.rustls_config() {
        b = b.use_preconfigured_tls((*cfg).clone());
    }
    let http = b.build()?;
    let url = base.join("/api/health")?;
    let resp = http.get(url).send().await?;
    if resp.status().is_success() {
        Ok(())
    } else {
        bail!("status {}", resp.status())
    }
}

/// Build a `wss://` (or `ws://`) URL for a daemon WebSocket endpoint.
///
/// `path` MUST be a path-only string (no `?`). Embedding a query in
/// `path` will percent-encode the `?` and produce a broken URL — see
/// the call-site comment in `open_terminal_stream` for the v0.6.25
/// regression details. Pass extra query parameters via `extra_query`.
fn ws_url(base: &Url, path: &str, token: &str, extra_query: &[(&str, &str)]) -> Url {
    debug_assert!(
        !path.contains('?'),
        "ws_url: `path` must not contain a query string; use `extra_query` instead (got {path:?})"
    );
    let mut url = base.clone();
    let target = if base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    url.set_scheme(target).expect("valid ws scheme");
    url.set_path(path);
    {
        let mut q = url.query_pairs_mut();
        q.clear();
        q.append_pair("token", token);
        for (k, v) in extra_query {
            q.append_pair(k, v);
        }
    }
    url
}

fn ws_connector(url: &Url, trust: &TlsTrust) -> Option<Connector> {
    if url.scheme() != "wss" {
        return None;
    }
    trust.rustls_config().map(Connector::Rustls)
}

fn backoff_delay(attempt: u32) -> Duration {
    let base = Duration::from_secs(1);
    let cap = Duration::from_secs(30);
    let shift = attempt.saturating_sub(1).min(5);
    let ms = base.as_millis() as u64 * (1u64 << shift);
    Duration::from_millis(ms.min(cap.as_millis() as u64))
}

#[cfg(test)]
mod tests {
    use super::ws_url;
    use url::Url;

    fn base(scheme: &str) -> Url {
        Url::parse(&format!("{scheme}://127.0.0.1:8822/")).unwrap()
    }

    #[test]
    fn ws_url_promotes_https_to_wss_and_appends_token() {
        let u = ws_url(&base("https"), "/api/events", "tok", &[]);
        assert_eq!(u.scheme(), "wss");
        assert_eq!(u.path(), "/api/events");
        assert_eq!(u.query(), Some("token=tok"));
    }

    #[test]
    fn ws_url_promotes_http_to_ws() {
        let u = ws_url(&base("http"), "/api/events", "tok", &[]);
        assert_eq!(u.scheme(), "ws");
    }

    #[test]
    fn ws_url_appends_extra_query_pairs_after_token() {
        // Regression for v0.6.21..=v0.6.24: caller embedded
        // `?resume=true` in `path`, set_path percent-encoded the `?`,
        // and the daemon couldn't route the request → 200 OK from
        // SPA fallback, "ws connect: HTTP error: 200 OK" client-side.
        // Resume must be a real query pair, not part of the path.
        let u = ws_url(
            &base("https"),
            "/api/sessions/abc/stream",
            "tok",
            &[("resume", "true")],
        );
        assert_eq!(u.path(), "/api/sessions/abc/stream");
        assert_eq!(u.query(), Some("token=tok&resume=true"));
        // Critical: serialized form has no `%3F` in the path.
        assert!(
            !u.as_str().contains("%3F"),
            "URL must not contain a percent-encoded `?`: {u}"
        );
    }

    #[test]
    #[should_panic(expected = "must not contain a query string")]
    fn ws_url_rejects_query_in_path_under_debug() {
        // The debug_assert! catches the v0.6.21..=v0.6.24 mistake at
        // call time in dev builds. Release builds skip this and rely
        // on the regression tests above for protection.
        let _ = ws_url(
            &base("https"),
            "/api/sessions/abc/stream?resume=true",
            "tok",
            &[],
        );
    }
}
