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

use agentum_core::{Event, Session};
use anyhow::{Context, Result, bail};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client as HttpClient;
use rustls::ClientConfig;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::{Connector, connect_async_tls_with_config, tungstenite::Message as WsMsg};
use url::Url;
use uuid::Uuid;

use super::trust;

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

    /// `POST /api/sessions` — create a new session row. The server records
    /// it as `Idle`; call `start_session` to actually spawn the tmux process.
    pub async fn create_session(
        &self,
        name: &str,
        workdir: &str,
        tool: &str,
        model: Option<&str>,
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
                flags: Vec::new(),
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

    pub async fn start_session(&self, id: Uuid) -> Result<()> {
        self.post_session_action(id, "start").await
    }

    pub async fn stop_session(&self, id: Uuid) -> Result<()> {
        self.post_session_action(id, "stop").await
    }

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

    pub async fn send_text(&self, id: Uuid, text: &str, append_enter: bool) -> Result<()> {
        #[derive(Serialize)]
        struct Body<'a> {
            text: &'a str,
            append_enter: bool,
        }
        let url = self.base.join(&format!("/api/sessions/{id}/send"))?;
        self.http
            .post(url)
            .bearer_auth(&self.token)
            .json(&Body { text, append_enter })
            .send()
            .await?
            .error_for_status()?;
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
        mut key_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> JoinHandle<()> {
        let base = self.base.clone();
        let token = self.token.clone();
        let trust = self.trust.clone();
        tokio::spawn(async move {
            let url = ws_url(&base, &format!("/api/sessions/{id}/stream"), &token);
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

            // Pump outbound keystrokes onto the WS until the channel closes.
            // Detached so a chatty pane never starves keystrokes.
            let writer = tokio::spawn(async move {
                while let Some(bytes) = key_rx.recv().await {
                    if sink.send(WsMsg::Binary(bytes)).await.is_err() {
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
            let url = ws_url(&base, "/api/events", &token);
            let connector = ws_connector(&url, &trust);
            let result = connect_async_tls_with_config(url.as_str(), None, false, connector).await;
            let mut stream = match result {
                Ok((s, _)) => {
                    let _ = tx.send(EventMsg::Connected);
                    s
                }
                Err(e) => {
                    let _ = tx.send(EventMsg::Error(format!("ws connect: {e}")));
                    let _ = tx.send(EventMsg::Closed);
                    return;
                }
            };
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(WsMsg::Text(t)) => match serde_json::from_str::<Event>(&t) {
                        Ok(ev) => {
                            if tx.send(EventMsg::Event(ev)).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            // bus.lagged uses a slim shape (no `ts`); pass kind through.
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                                let kind = v
                                    .get("kind")
                                    .and_then(|k| k.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                if tx.send(EventMsg::Raw(kind)).is_err() {
                                    break;
                                }
                            }
                        }
                    },
                    Ok(WsMsg::Close(_)) => break,
                    Ok(_) => {}
                    Err(e) => {
                        let _ = tx.send(EventMsg::Error(format!("ws: {e}")));
                        break;
                    }
                }
            }
            let _ = tx.send(EventMsg::Closed);
        })
    }
}

#[derive(Debug)]
pub enum TerminalMsg {
    Bytes(Bytes),
    Error(String),
    Closed,
}

#[derive(Debug)]
pub enum EventMsg {
    Connected,
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

fn ws_url(base: &Url, path: &str, token: &str) -> Url {
    let mut url = base.clone();
    let target = if base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    url.set_scheme(target).expect("valid ws scheme");
    url.set_path(path);
    url.query_pairs_mut().clear().append_pair("token", token);
    url
}

fn ws_connector(url: &Url, trust: &TlsTrust) -> Option<Connector> {
    if url.scheme() != "wss" {
        return None;
    }
    trust.rustls_config().map(Connector::Rustls)
}
