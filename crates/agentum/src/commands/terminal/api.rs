//! HTTP + WebSocket client wrapping the agentum daemon API.
//!
//! Bearer token is injected as `Authorization: Bearer <token>` for HTTP and
//! as a `?token=…` query parameter for WS upgrades (browsers can't set
//! custom headers on WS, the server accepts both).

use std::sync::Arc;
use std::time::Duration;

use agentum_core::{Event, Session};
use anyhow::{Context, Result, bail};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client as HttpClient;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::{Connector, connect_async_tls_with_config, tungstenite::Message as WsMsg};
use url::Url;
use uuid::Uuid;

use rustls::ClientConfig;
use rustls::DigitallySignedStruct;
use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

#[derive(Clone)]
pub struct Client {
    http: HttpClient,
    base: Url,
    token: String,
}

/// Standalone POST /api/auth/login. Returns the bearer token.
pub async fn login(base: &Url, username: &str, password: &str) -> Result<String> {
    let url = base.join("/api/auth/login")?;
    let http = HttpClient::builder()
        .timeout(Duration::from_secs(15))
        .danger_accept_invalid_certs(is_localhost(base))
        .build()
        .context("build reqwest client")?;
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
    pub fn new(base: Url, token: String) -> Result<Self> {
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(15))
            .danger_accept_invalid_certs(is_localhost(&base))
            .build()
            .context("build reqwest client")?;
        Ok(Self { http, base, token })
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
        tokio::spawn(async move {
            let url = ws_url(&base, &format!("/api/sessions/{id}/stream"), &token);
            let connector = ws_connector(&url);
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
        tokio::spawn(async move {
            let url = ws_url(&base, "/api/events", &token);
            let connector = ws_connector(&url);
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

pub async fn probe_health(base: &Url, timeout: Duration) -> Result<()> {
    let http = HttpClient::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(is_localhost(base))
        .build()?;
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

fn ws_connector(url: &Url) -> Option<Connector> {
    if url.scheme() == "wss" {
        Some(Connector::Rustls(accept_any_rustls_config()))
    } else {
        None
    }
}

fn is_localhost(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some("127.0.0.1") | Some("::1") | Some("localhost")
    )
}

fn accept_any_rustls_config() -> Arc<ClientConfig> {
    // Install a default crypto provider once; ignore "already installed".
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny))
        .with_no_client_auth();
    Arc::new(config)
}

#[derive(Debug)]
struct AcceptAny;

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}
