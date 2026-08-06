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

use agentum_core::{
    Event, Host, HostReadiness, NewHost, Session, WorktreeSpec, transcript::AgentTaskState,
};
use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client as HttpClient;
use rustls::ClientConfig;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::{Connector, connect_async_tls_with_config, tungstenite::Message as WsMsg};
use url::Url;
use uuid::Uuid;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
// SSH readiness is a serialized, multi-stage operation: it may first wait
// behind another lifecycle task for the same host, then run separate SSH
// probes for required tools and agent CLIs. Keep a finite client deadline,
// but do not cancel a healthy readiness check at the ordinary read timeout.
const HOST_READINESS_TIMEOUT: Duration = Duration::from_secs(60);
// Creating an SSH-hosted session performs a remote launch preflight before the
// row is persisted (HOME/workdir/tool/shell/transcript checks). That is the
// same class of bounded multi-stage operation as start, so it must not inherit
// the ordinary 15-second read deadline.
const REMOTE_SESSION_CREATE_TIMEOUT: Duration = Duration::from_secs(90);
// SSH-hosted lifecycle mutations are not ordinary API reads: start may need to
// warm ControlMasters and the reverse MCP tunnel, while stop/force-delete may
// need the bounded graceful-shutdown + ownership-check path. Each SSH stage is
// independently bounded server-side, so the generic 15-second client deadline
// can cancel a healthy mutation halfway through. Keep a finite outer bound,
// but leave enough room for the complete lifecycle transaction.
const SESSION_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(90);
/// `connect_async_tls_with_config`'s third argument controls TCP_NODELAY.
/// Terminal frames are latency-sensitive, unlike bulk HTTP transfers.
const TERMINAL_WS_DISABLE_NAGLE: bool = true;

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

/// Mirrors the server's `/api/health` response, trimmed to the fields
/// the TUI actually consumes. `#[serde(default)]` on the optional
/// fields lets older daemons (pre-v0.6.x) parse cleanly with empty
/// values rather than failing the probe outright.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct Health {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Minimal board-item shape returned by `GET /api/board/{id}`.
/// The TUI only needs `id` and `title` for the hint strip (Phase 2,
/// plan 05); extra server fields are captured via `#[serde(default)]`
/// for forward-compat.
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct BoardItemSummary {
    pub id: i64,
    #[serde(default)]
    pub title: Option<String>,
}

/// Response from `POST /api/board/goals`. The server returns the newly
/// created board item; the TUI currently only needs the `id` so the
/// planner can be told which goal to expand — extra fields are captured
/// for forward-compat and future "jump to card" flows.
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct SubmitGoalResponse {
    pub id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Mirrors `agentum_server::routes::agents::AgentInfo`. Returned by
/// `/api/agents`; the TUI gates the New Session form's tool picker on
/// `available` so users can't pick an agent whose CLI isn't installed.
/// `binary`, `yolo_flag`, and `path` round-trip the full server shape so
/// future surfaces (status overlay, doctor view) can render them
/// without re-extending this struct.
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct AgentInfo {
    pub name: String,
    pub binary: String,
    pub available: bool,
    #[serde(default)]
    pub yolo_flag: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

/// CLI-side mirror of the server's `ClaudeUsageSnapshot` (spec 001),
/// trimmed to the fields the TUI's bottom-left readout consumes. EVERY
/// field is `#[serde(default)]` + `Option`, so a snapshot from an older
/// daemon (which omits the spec-001 fields entirely) still deserializes —
/// the readout just shows "usage unavailable" for the missing parts.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ClaudeUsage {
    /// Sum of input+output+cache-create tokens in the 5h window.
    #[serde(default)]
    pub window_tokens: u64,
    /// `true` when Claude Code has ever run on the daemon host.
    #[serde(default)]
    pub claude_installed: bool,
    /// Headline plan-limit utilization, `max(5h, 7d)`, 0..=100.
    #[serde(default)]
    pub limit_pct: Option<f64>,
    /// Unix-ms reset time of the binding window.
    #[serde(default)]
    pub resets_at_ms: Option<i64>,
    /// Estimated USD spend for the window (labeled "est." in the UI).
    #[serde(default)]
    pub est_cost_usd: Option<f64>,
    /// `"oauth"` when `limit_pct` is a real plan number, `"scan"` when we
    /// only have the local transcript scan (no real % — degraded).
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HostProbe {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub uname: Option<String>,
    pub tmux: bool,
    pub git: bool,
}

/// Mirrors `agentum_server::routes::uploads::UploadResponse`. Returned by
/// `POST /api/sessions/{id}/uploads` after the daemon has written the
/// image bytes to disk and typed the relative path into the tmux pane.
/// The TUI only needs `relative_path` + `size_bytes` for the success
/// toast; `path` (absolute on the daemon host) is captured for
/// forward-compat debugging surfaces.
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct UploadResponse {
    pub path: String,
    pub relative_path: String,
    pub size_bytes: u64,
}

/// Typed result from `Client::request_clipboard`. The 503 `kind`
/// discriminant from the broker is what drives the TUI's fallback
/// decision — only `AgentNotConnected` triggers the local arboard
/// fallback (single-host users who never installed clip-agent should
/// keep working). `NoImage` and `Timeout` get targeted toasts but no
/// fallback because the user already had a chance to provide an
/// image and falling back would either yield the same "no image"
/// error from arboard or write whatever stale image had been copied
/// before the user's intended one.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardRequestError {
    #[error("no clipboard agent connected")]
    AgentNotConnected,
    #[error("no image in clipboard")]
    NoImage,
    #[error("clipboard agent did not respond in time")]
    Timeout,
    #[error("{0}")]
    Other(#[from] anyhow::Error),
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
    let mut b = HttpClient::builder().timeout(DEFAULT_HTTP_TIMEOUT);
    if let Some(cfg) = trust.rustls_config() {
        // We pass an owned ClientConfig (reqwest expects the unwrapped form
        // because it stuffs it into a `dyn Any`).
        let owned = (*cfg).clone();
        b = b.use_preconfigured_tls(owned);
    }
    b.build().context("build reqwest client")
}

/// Turn the daemon's standard `{"error":"..."}` envelope into a message
/// suitable for a toast or the errors overlay. Keeping this at the HTTP
/// boundary prevents escaped JSON (and escaped newlines in SSH stderr) from
/// leaking into every caller that records the error.
fn format_api_error(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct ErrorEnvelope {
        error: String,
    }

    let body = body.trim();
    let detail = serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .map(|envelope| envelope.error)
        .filter(|error| !error.trim().is_empty())
        .unwrap_or_else(|| body.to_string());

    if detail.is_empty() {
        status.to_string()
    } else {
        format!("{status} — {detail}")
    }
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
        bail!("{}", format_api_error(status, &body));
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        token: String,
    }
    let r: Resp = resp.json().await?;
    Ok(r.token)
}

/// `GET /api/auth/status`. `needs_setup = true` means zero users
/// exist on this daemon and `register` is open anonymously — the
/// signal we use to decide whether to auto-bootstrap a local account
/// instead of prompting the user for credentials.
pub async fn auth_needs_setup(base: &Url, trust: &TlsTrust) -> Result<bool> {
    let url = base.join("/api/auth/status")?;
    let http = build_http(trust)?;
    let resp = http
        .get(url)
        .send()
        .await
        .context("auth status request failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("{}", format_api_error(status, &body));
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        needs_setup: bool,
    }
    let r: Resp = resp.json().await?;
    Ok(r.needs_setup)
}

/// Returns `true` when the server was started with `--no-auth`. The TUI
/// uses this to skip the credential flow and send a dummy bearer token
/// (the middleware accepts all requests regardless of token value).
pub async fn auth_is_disabled(base: &Url, trust: &TlsTrust) -> Result<bool> {
    let url = base.join("/api/auth/status")?;
    let http = build_http(trust)?;
    let resp = http
        .get(url)
        .send()
        .await
        .context("auth status request failed")?;
    if !resp.status().is_success() {
        return Ok(false);
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        #[serde(default)]
        no_auth: bool,
    }
    let r: Resp = resp.json().await?;
    Ok(r.no_auth)
}

/// POST /api/auth/register. Same payload as login. Only succeeds
/// when the daemon reports `needs_setup = true` (zero users yet);
/// after that the route is closed for the rest of the daemon's
/// lifetime. Returns the freshly-issued bearer token.
pub async fn register(
    base: &Url,
    trust: &TlsTrust,
    username: &str,
    password: &str,
) -> Result<String> {
    let url = base.join("/api/auth/register")?;
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
        .context("register request failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("{}", format_api_error(status, &body));
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

    pub async fn health(&self) -> Result<Health> {
        let url = self.base.join("/api/health")?;
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("health returned {}", resp.status())
        }
        // Older daemons may omit optional fields; serde defaults handle
        // that path. A malformed body bubbles up as an error and the
        // caller falls back to "version unknown" rather than crashing.
        let body: Health = resp.json().await?;
        Ok(body)
    }

    /// `GET /api/agents` — runtime probe of which first-class agent
    /// binaries resolve on the daemon's PATH. Older daemons return 404;
    /// the TUI treats that as "fail open" and skips installation gating.
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        self.list_agents_on(None).await
    }

    pub async fn list_agents_on(&self, host_id: Option<Uuid>) -> Result<Vec<AgentInfo>> {
        let mut url = self.base.join("/api/agents")?;
        if let Some(host_id) = host_id {
            url.query_pairs_mut()
                .append_pair("host_id", &host_id.to_string());
        }
        let resp = self.http.get(url).bearer_auth(&self.token).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !resp.status().is_success() {
            bail!("agents returned {}", resp.status());
        }
        Ok(resp.json::<Vec<AgentInfo>>().await.unwrap_or_default())
    }

    /// `GET /api/usage/claude` — the daemon's Claude account usage snapshot
    /// (spec 001): plan-limit %, estimated $, window tokens. Daemons
    /// predating the route return 404; we surface that as a clean error so
    /// the caller can render "usage unavailable" rather than crash. The
    /// daemon caches the upstream OAuth fetch, so polling this cheaply is
    /// fine.
    pub async fn claude_usage(&self) -> Result<ClaudeUsage> {
        let url = self.base.join("/api/usage/claude")?;
        let resp = self.http.get(url).bearer_auth(&self.token).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            bail!("daemon does not expose /api/usage/claude — update `agentum serve`");
        }
        if !resp.status().is_success() {
            bail!("usage returned {}", resp.status());
        }
        Ok(resp.json::<ClaudeUsage>().await?)
    }

    pub async fn list_hosts(&self) -> Result<Vec<Host>> {
        let url = self.base.join("/api/hosts")?;
        let resp = self.http.get(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            bail!("hosts returned {}", resp.status());
        }
        Ok(resp.json::<Vec<Host>>().await.unwrap_or_default())
    }

    pub async fn create_host(&self, new: &NewHost) -> Result<Host> {
        let url = self.base.join("/api/hosts")?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(new)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<Host>().await?)
    }

    /// `PUT /api/hosts/{id}` — edit an existing SSH host's connection
    /// settings. Sends the same `NewHost` body as create; the daemon
    /// rewrites the row in place (same id, so attached sessions are
    /// preserved) and returns the refreshed host.
    pub async fn update_host(&self, id: Uuid, new: &NewHost) -> Result<Host> {
        let url = self.base.join(&format!("/api/hosts/{id}"))?;
        let resp = self
            .http
            .put(url)
            .bearer_auth(&self.token)
            .json(new)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<Host>().await?)
    }

    pub async fn test_host(&self, id: Uuid) -> Result<HostProbe> {
        let url = self.base.join(&format!("/api/hosts/{id}/test"))?;
        let resp = self.http.post(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<HostProbe>().await?)
    }

    /// `GET /api/hosts/{id}/readiness` — full structured readiness report
    /// (required deps + agent CLIs + package manager + install hints).
    /// Daemons predating this route return 404; we surface a clear
    /// "update the daemon" message rather than a bare error so the user
    /// knows the gap is the server, not their host.
    pub async fn host_readiness(&self, id: Uuid) -> Result<HostReadiness> {
        let resp = self.host_readiness_request(id)?.send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            // Ambiguous between "old daemon, no route" and "unknown host
            // id". An empty body means the route itself is missing; a
            // non-empty ApiError body means the host id wasn't found.
            let body = resp.text().await.unwrap_or_default();
            if body.trim().is_empty() {
                bail!("daemon does not support host readiness — update `agentum serve`");
            }
            bail!("host not found");
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<HostReadiness>().await?)
    }

    fn host_readiness_request(&self, id: Uuid) -> Result<reqwest::RequestBuilder> {
        let url = self.base.join(&format!("/api/hosts/{id}/readiness"))?;
        Ok(self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .timeout(HOST_READINESS_TIMEOUT))
    }

    /// `POST /api/hosts/{id}/bootstrap` — install `tmux`/`git` on the host
    /// after explicit confirmation. `confirm: true` is sent unconditionally
    /// because the TUI already gated this behind a y/N Confirm overlay.
    /// Returns the re-probed readiness so the caller can refresh its dots.
    pub async fn bootstrap_host(&self, id: Uuid, items: &[&str]) -> Result<HostReadiness> {
        #[derive(Serialize)]
        struct BootstrapBody<'a> {
            items: &'a [&'a str],
            confirm: bool,
        }
        let url = self.base.join(&format!("/api/hosts/{id}/bootstrap"))?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&BootstrapBody {
                items,
                confirm: true,
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<HostReadiness>().await?)
    }

    /// `POST /api/hosts/{id}/install-agent` — install agent CLIs on the
    /// host over SSH (phase 3). Confirmed at the call site (TUI confirm
    /// overlay / CLI prompt), so `confirm: true` is sent here. Returns the
    /// re-probed readiness.
    pub async fn install_agents(&self, id: Uuid, tools: &[&str]) -> Result<HostReadiness> {
        #[derive(Serialize)]
        struct InstallBody<'a> {
            tools: &'a [&'a str],
            confirm: bool,
        }
        let url = self.base.join(&format!("/api/hosts/{id}/install-agent"))?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&InstallBody {
                tools,
                confirm: true,
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<HostReadiness>().await?)
    }

    pub async fn delete_host(&self, id: Uuid) -> Result<()> {
        let url = self.base.join(&format!("/api/hosts/{id}"))?;
        let resp = self
            .http
            .delete(url)
            .bearer_auth(&self.token)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(())
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

    /// PUT the shared user preferences blob — keeps the dashboard's
    /// theme picker (and any future shared knob) in sync with the TUI.
    /// Best-effort: failures are logged by the caller as a status hint
    /// and the local file write is what actually persists the change.
    pub async fn put_preferences(
        &self,
        theme: Option<&str>,
        tui_theme: Option<&str>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct Body<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            theme: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            tui_theme: Option<&'a str>,
        }
        let url = self.base.join("/api/preferences")?;
        let resp = self
            .http
            .put(url)
            .bearer_auth(&self.token)
            .json(&Body { theme, tui_theme })
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("preferences returned {}", resp.status());
        }
        Ok(())
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

    /// `POST /api/sessions/{id}/agent-tasks/reset` — wipe the cached
    /// plan/todos/tasks for this session on the daemon and fast-forward
    /// the transcript cursor past anything already on disk. Used by the
    /// TUI when the user runs `/clear` (or `\clear`) inside the agent
    /// pane so the right-side panel mirrors the agent's own context
    /// wipe instead of leaving stale entries behind. 204 No Content on
    /// success.
    pub async fn reset_agent_tasks(&self, id: Uuid) -> Result<()> {
        let url = self
            .base
            .join(&format!("/api/sessions/{id}/agent-tasks/reset"))?;
        let resp = self.http.post(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(())
    }

    /// POST raw image bytes to `/api/sessions/{id}/uploads`. The daemon
    /// writes the bytes under `<session.workdir>/.agentum-uploads/` and
    /// types the relative path into the tmux pane (no Enter — the
    /// agent's prompt commits when the user hits return themselves).
    /// Returns the daemon's response so the TUI can echo
    /// `relative_path` + `size_bytes` in a success toast.
    pub async fn upload_image(
        &self,
        id: Uuid,
        bytes: Vec<u8>,
        mime: &str,
    ) -> Result<UploadResponse> {
        let url = build_upload_url(&self.base, id)?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, mime)
            .body(bytes)
            .send()
            .await
            .context("upload_image request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<UploadResponse>().await?)
    }

    /// POST `/api/clipboard/request` — ask the daemon's clipboard
    /// broker to fetch an image from the user's local clipboard via
    /// the long-running `agentum clip-agent` on whichever host owns
    /// the OS clipboard, and write it as a session upload.
    ///
    /// Returns the same `UploadResponse` shape as a direct upload so
    /// the TUI's success-toast code path stays one branch. The 503
    /// `kind` discriminant is decoded into the typed error so the
    /// caller can decide whether to fall back to the local arboard
    /// path (only on `AgentNotConnected`).
    pub async fn request_clipboard(
        &self,
        session_id: Uuid,
        timeout_ms: u64,
    ) -> Result<UploadResponse, ClipboardRequestError> {
        let url = self
            .base
            .join("/api/clipboard/request")
            .map_err(|e| ClipboardRequestError::Other(anyhow!("build url: {e}")))?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&serde_json::json!({
                "session_id": session_id,
                "timeout_ms": timeout_ms,
            }))
            .send()
            .await
            .map_err(|e| ClipboardRequestError::Other(anyhow!("request failed: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return resp
                .json::<UploadResponse>()
                .await
                .map_err(|e| ClipboardRequestError::Other(anyhow!("decode 200 body: {e}")));
        }
        if status.as_u16() == 503 {
            // Decode the 503 envelope: `{ "error": "...", "kind": "..." }`.
            // The `kind` discriminant drives the caller's fallback decision.
            #[derive(serde::Deserialize)]
            struct Body {
                #[serde(default)]
                kind: String,
            }
            let body = resp.json::<Body>().await.unwrap_or(Body {
                kind: String::new(),
            });
            return Err(match body.kind.as_str() {
                "agent_not_connected" => ClipboardRequestError::AgentNotConnected,
                "no_image" => ClipboardRequestError::NoImage,
                "timeout" => ClipboardRequestError::Timeout,
                other => {
                    ClipboardRequestError::Other(anyhow!("unknown clipboard 503 kind: {other}"))
                }
            });
        }
        let body = resp.text().await.unwrap_or_default();
        Err(ClipboardRequestError::Other(anyhow!(
            "clipboard request: {}",
            format_api_error(status, &body)
        )))
    }

    /// `GET /api/fs/list` — enumerate directories under `path` (or `$HOME`
    /// if `path` is `None`). Mirrors the web `DirPicker`'s feed for the
    /// TUI's workdir picker overlay.
    pub async fn list_dir(&self, path: Option<&str>) -> Result<DirListing> {
        self.list_dir_on(path, None).await
    }

    pub async fn list_dir_on(
        &self,
        path: Option<&str>,
        host_id: Option<Uuid>,
    ) -> Result<DirListing> {
        let mut url = self.base.join("/api/fs/list")?;
        {
            let mut qp = url.query_pairs_mut();
            if let Some(p) = path {
                qp.append_pair("path", p);
            }
            if let Some(id) = host_id {
                qp.append_pair("host_id", &id.to_string());
            }
        }
        let resp = self.http.get(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
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
        self.create_session_on(name, workdir, tool, model, flags, None, false)
            .await
    }

    /// `worktree`: when true, ask the server to `git worktree add` a
    /// dedicated branch + checkout for this session (sibling at
    /// `<repo>-worktrees/agentum-<name>`, forked from `HEAD`). We send an
    /// empty `WorktreeSpec` — branch is derived from the session name and
    /// the base ref defaults to `HEAD` server-side, matching the
    /// dashboard's "off what" knob being the only thing worth exposing.
    /// The key is omitted entirely when false so pre-worktree daemons
    /// (no `CreateBody.worktree`) still accept the request.
    // Thin request builder mirroring the `/api/sessions` body field-for-
    // field — splitting it into a params struct would just shuffle the
    // same fields one level up at the single call site.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_session_on(
        &self,
        name: &str,
        workdir: &str,
        tool: &str,
        model: Option<&str>,
        flags: Vec<String>,
        host_id: Option<Uuid>,
        worktree: bool,
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
            #[serde(skip_serializing_if = "Option::is_none")]
            host_id: Option<Uuid>,
            #[serde(skip_serializing_if = "Option::is_none")]
            worktree: Option<WorktreeSpec>,
        }
        let url = self.base.join("/api/sessions")?;
        let mut request = self.http.post(url).bearer_auth(&self.token).json(&Body {
            name,
            workdir,
            tool,
            model,
            flags,
            host_id,
            worktree: worktree.then_some(WorktreeSpec {
                branch: None,
                base_ref: None,
            }),
        });
        if let Some(timeout) = session_create_timeout(host_id) {
            // RequestBuilder::timeout overrides the client's default for this
            // remote lifecycle request only. Local creates remain fast-fail.
            request = request.timeout(timeout);
        }
        let resp = request
            .send()
            .await
            .context("create session request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<Session>().await?)
    }

    /// `POST /api/board/goals` — submit a goal to the planner.
    ///
    /// The server creates a new board item of type `goal` and hands it to
    /// the autonomous planner, which creates 3–7 child cards. Returns the
    /// newly created goal item.
    pub async fn submit_goal(&self, text: &str) -> Result<SubmitGoalResponse> {
        let url = self.base.join("/api/board/goals")?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "title": text }))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<SubmitGoalResponse>().await?)
    }

    /// `GET /api/board/{id}` — fetch a single board item by its integer
    /// primary key. Returns a minimal `BoardItemSummary` (id + title).
    /// Used by the `c`-key hint strip in `Focus::Tree` to display the
    /// bound card's title without a full board fetch (Phase 2, plan 05).
    pub async fn get_board_item(&self, id: i64) -> Result<BoardItemSummary> {
        let url = self.base.join(&format!("/api/board/{id}"))?;
        let resp = self.http.get(url).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<BoardItemSummary>().await?)
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
            bail!("{}", format_api_error(status, &body));
        }
        Ok(resp.json::<Session>().await?)
    }

    pub async fn start_session(&self, id: Uuid) -> Result<()> {
        self.post_session_action(id, "start").await
    }

    pub async fn stop_session(&self, id: Uuid) -> Result<()> {
        self.post_session_action(id, "stop").await
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
        let mut request = self.http.delete(url).bearer_auth(&self.token);
        if let Some(timeout) = session_delete_timeout(force) {
            request = request.timeout(timeout);
        }
        let resp = request.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(())
    }

    async fn post_session_action(&self, id: Uuid, action: &str) -> Result<()> {
        let url = self.base.join(&format!("/api/sessions/{id}/{action}"))?;
        let mut request = self.http.post(url).bearer_auth(&self.token);
        if let Some(timeout) = session_action_timeout(action) {
            // RequestBuilder::timeout overrides the client's default for this
            // lifecycle request only; ordinary reads/mutations retain 15 s.
            request = request.timeout(timeout);
        }
        let resp = request.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("{}", format_api_error(status, &body));
        }
        Ok(())
    }

    /// Open a bidirectional terminal stream with automatic reconnect.
    ///
    /// Server → client: tmux pane bytes arrive on `tx` as `TerminalMsg`s.
    /// Client → server: each entry pulled off `key_rx` is sent as either
    /// a binary WS frame (raw keystrokes) or a JSON text frame (resize),
    /// which the server forwards to the tmux pane via `send-keys -H` /
    /// `resize-window`. This is what makes the terminal pane interactive
    /// (typing into claude code, sending Ctrl-C, arrow keys, etc.).
    ///
    /// On any connection drop the task transparently reconnects with
    /// exponential backoff. After the first successful connect, every
    /// retry passes `resume=true` so the server replays only the bytes
    /// the client missed instead of clobbering the cached parser with a
    /// fresh snapshot. `Closed` is only emitted when the caller drops
    /// the receiver side of `tx` — there is no other terminal-loop exit.
    pub fn open_terminal_stream(
        &self,
        id: Uuid,
        tx: mpsc::UnboundedSender<TerminalMsg>,
        mut key_rx: mpsc::UnboundedReceiver<TermOut>,
        initial_resume: bool,
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
            let mut want_resume = initial_resume;
            // One-shot per reconnect: ask the server to force a full repaint
            // (`?redraw=true`, a SIGWINCH nudge) before snapshotting. A
            // reconnect is the suspend/resume path — while the WS was down an
            // OS `wall` broadcast (systemd's "system will suspend now!") may
            // have been written straight into the pane grid, and the resume
            // delta just re-feeds it; the agent won't overpaint cells it never
            // drew. Same URL-query rationale as `resume`: old daemons drop it.
            // Never set on the first connect (nothing to heal yet).
            let mut want_redraw = false;
            let mut attempt: u32 = 0;
            loop {
                if tx.is_closed() {
                    return;
                }
                let mut extra: Vec<(&str, &str)> = Vec::new();
                if want_resume {
                    extra.push(("resume", "true"));
                }
                if want_redraw {
                    extra.push(("redraw", "true"));
                }
                let url = ws_url(&base, &path, &token, &extra);
                let connector = ws_connector(&url, &trust);
                // Terminal frames are latency-sensitive and often only a few
                // bytes. Disable Nagle so a key or short pane update is not
                // held behind a delayed ACK on remote daemon profiles.
                let result = connect_async_tls_with_config(
                    url.as_str(),
                    None,
                    TERMINAL_WS_DISABLE_NAGLE,
                    connector,
                )
                .await;
                let stream = match result {
                    Ok((s, _)) => {
                        attempt = 0;
                        // want_redraw is intentionally NOT reset here: it starts
                        // false (first connect never heals) and every reconnect
                        // path below re-arms it, so the redraw fires once per
                        // reconnect — exactly when a suspend could have corrupted
                        // the grid.
                        let _ = tx.send(TerminalMsg::Connected);
                        s
                    }
                    Err(e) => {
                        // Terminal-state HTTP responses: 404 means the daemon
                        // we're asking has no record of this session (we're
                        // pointing at the wrong daemon, or the session was
                        // deleted). 401/403 means our token isn't valid for
                        // this endpoint. Retrying any of these forever just
                        // spams the errors overlay with the same line — bail
                        // and let the user act (re-select, log back in,
                        // restart). Other errors (TCP, TLS, transient
                        // upgrade failures) keep the existing backoff loop.
                        if let WsError::Http(ref resp) = e {
                            let status = resp.status().as_u16();
                            if matches!(status, 401 | 403 | 404) {
                                let _ = tx.send(TerminalMsg::Error(format!(
                                    "ws connect: HTTP {status} — session unavailable on this daemon (give up)"
                                )));
                                return;
                            }
                        }
                        attempt = attempt.saturating_add(1);
                        let delay = backoff_delay(attempt);
                        // Surface the underlying error once per backoff cycle so
                        // the user can see "ws connect: tcp connect: ..." in the
                        // errors overlay. The `Reconnecting` chip carries the
                        // attempt count + delay separately.
                        let _ = tx.send(TerminalMsg::Error(format!("ws connect: {e}")));
                        let _ = tx.send(TerminalMsg::Reconnecting {
                            attempt,
                            delay_ms: delay.as_millis() as u64,
                        });
                        tokio::time::sleep(delay).await;
                        // Subsequent retries always resume — the parser still
                        // holds state from the last good connect, so a fresh
                        // capture-pane snapshot would clobber visible chat
                        // history with whatever the agent's UI happens to look
                        // like *now* (often near-empty after a task finishes).
                        want_resume = true;
                        // ...and force a repaint once we reconnect, in case the
                        // outage was a suspend that left broadcast garbage in
                        // the grid (see want_redraw's declaration).
                        want_redraw = true;
                        continue;
                    }
                };
                let (mut sink, mut src) = stream.split();

                // Run the bidi pump on a single task. We deliberately do NOT
                // split the writer onto its own `tokio::spawn(...)` like the
                // pre-reconnect implementation did: that pattern moves
                // `key_rx` into the writer, so when the WS dies, key_rx
                // drops with the writer task and the caller's `term_in`
                // sender starts erroring on every keystroke. Keeping the
                // outer loop in possession of `key_rx` lets keystrokes
                // queue across reconnects and flush the moment the next
                // attempt succeeds.
                let drop_reason: DropReason = loop {
                    // Fair selection matters when the user types while an agent
                    // is streaming output. Prioritising key_rx indefinitely can
                    // starve ready pane frames and make rendering appear frozen.
                    tokio::select! {
                        out = key_rx.recv() => {
                            let Some(out) = out else {
                                // Caller dropped the keystroke sender. No more
                                // input is coming; close the WS politely.
                                break DropReason::CallerClosed;
                            };
                            let msg = term_out_to_ws_message(out);
                            if sink.send(msg).await.is_err() {
                                break DropReason::SinkErr;
                            }
                        }
                        msg = src.next() => match msg {
                            Some(Ok(WsMsg::Binary(b))) => {
                                if tx.send(TerminalMsg::Bytes(b.into())).is_err() {
                                    return;
                                }
                            }
                            Some(Ok(WsMsg::Text(t))) => {
                                if tx.send(TerminalMsg::Error(t)).is_err() {
                                    return;
                                }
                            }
                            Some(Ok(WsMsg::Close(_))) | None => break DropReason::Eof,
                            Some(Ok(_)) => {} // ping/pong
                            Some(Err(e)) => {
                                let _ = tx.send(TerminalMsg::Error(format!("ws: {e}")));
                                break DropReason::SrcErr;
                            }
                        }
                    }
                };
                let _ = sink.close().await;
                if matches!(drop_reason, DropReason::CallerClosed) {
                    let _ = tx.send(TerminalMsg::Closed);
                    return;
                }
                // Server-side close, network error, or sink failure → roll
                // into a backoff and reconnect with `resume=true`. We don't
                // emit `Closed` here: the caller treats `Closed` as
                // terminal, and we're still trying to recover.
                attempt = attempt.saturating_add(1);
                let delay = backoff_delay(attempt);
                let _ = tx.send(TerminalMsg::Reconnecting {
                    attempt,
                    delay_ms: delay.as_millis() as u64,
                });
                tokio::time::sleep(delay).await;
                want_resume = true;
                // Heal on reconnect: a dropped established connection is the
                // suspend/resume path too (see want_redraw's declaration).
                want_redraw = true;
            }
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

fn session_action_timeout(action: &str) -> Option<Duration> {
    matches!(action, "start" | "stop" | "kill").then_some(SESSION_LIFECYCLE_TIMEOUT)
}

fn session_delete_timeout(force: bool) -> Option<Duration> {
    force.then_some(SESSION_LIFECYCLE_TIMEOUT)
}

fn session_create_timeout(host_id: Option<Uuid>) -> Option<Duration> {
    host_id
        .filter(|id| *id != agentum_core::LOCAL_HOST_ID)
        .map(|_| REMOTE_SESSION_CREATE_TIMEOUT)
}

#[derive(Debug)]
pub enum TerminalMsg {
    /// A successful WS upgrade completed. After a `Reconnecting` cycle
    /// this signals the gap closed and bytes are flowing again — the
    /// TUI uses it to clear the reconnect overlay and snap any active
    /// scrollback back to the live tail so the user sees fresh state.
    Connected,
    Bytes(Bytes),
    Reconnecting {
        attempt: u32,
        delay_ms: u64,
    },
    Error(String),
    /// Caller dropped the keystroke sender — the stream task is
    /// shutting down for good. Auto-reconnect attempts do NOT emit
    /// this; they emit `Reconnecting` and silently retry.
    Closed,
}

/// Reason the inner WS read/write loop exited. Distinguishes "caller is
/// done with this stream" (terminal — emit `Closed`, return) from
/// "connection died" (transient — backoff + reconnect).
enum DropReason {
    CallerClosed,
    SinkErr,
    SrcErr,
    Eof,
}

/// Outbound terminal stream messages: keystrokes (binary frames) and
/// resize notifications (JSON text frames). Sent from the TUI to the
/// daemon over the same WebSocket as input.
#[derive(Debug, Clone)]
pub enum TermOut {
    Bytes(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

/// Preserve terminal input as an opaque byte stream on the wire. In
/// particular, never format `TermOut::Bytes` with `Debug`: doing so would send
/// the enum/type representation instead of the user's UTF-8 and control bytes.
fn term_out_to_ws_message(out: TermOut) -> WsMsg {
    match out {
        TermOut::Bytes(bytes) => WsMsg::Binary(bytes),
        TermOut::Resize { cols, rows } => WsMsg::Text(format!(
            "{{\"resize\":{{\"cols\":{cols},\"rows\":{rows}}}}}"
        )),
    }
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

/// Build the upload URL for a session. Extracted so the Ctrl-V
/// handler can pin the path shape in a unit test without spinning up
/// a real `Client`.
pub fn build_upload_url(base: &Url, id: Uuid) -> Result<Url> {
    Ok(base.join(&format!("/api/sessions/{id}/uploads"))?)
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
    use super::{
        Client, DEFAULT_HTTP_TIMEOUT, HOST_READINESS_TIMEOUT, REMOTE_SESSION_CREATE_TIMEOUT,
        SESSION_LIFECYCLE_TIMEOUT, TERMINAL_WS_DISABLE_NAGLE, TermOut, TlsTrust, build_upload_url,
        format_api_error, session_action_timeout, session_create_timeout, session_delete_timeout,
        term_out_to_ws_message, ws_url,
    };
    use reqwest::StatusCode;
    use tokio_tungstenite::tungstenite::Message as WsMsg;
    use url::Url;
    use uuid::Uuid;

    fn base(scheme: &str) -> Url {
        Url::parse(&format!("{scheme}://127.0.0.1:8822/")).unwrap()
    }

    #[test]
    fn terminal_input_is_an_exact_binary_websocket_payload() {
        let input = "empirical-λ-🛠-BYTES-check\n".as_bytes().to_vec();

        let message = term_out_to_ws_message(TermOut::Bytes(input.clone()));

        assert_eq!(message, WsMsg::Binary(input));
    }

    #[test]
    fn terminal_websocket_disables_nagle() {
        assert!(TERMINAL_WS_DISABLE_NAGLE);
    }

    #[test]
    fn terminal_control_envelopes_cannot_capture_input_that_looks_like_json() {
        let input = br#"{"resize":{"cols":1,"rows":1}}"#.to_vec();

        let message = term_out_to_ws_message(TermOut::Bytes(input.clone()));

        assert_eq!(message, WsMsg::Binary(input));
        assert_eq!(
            term_out_to_ws_message(TermOut::Resize {
                cols: 120,
                rows: 40
            }),
            WsMsg::Text(r#"{"resize":{"cols":120,"rows":40}}"#.into())
        );
    }

    #[test]
    fn upload_url_is_session_scoped() {
        // Pin the wire path so the daemon side (routes/uploads.rs) and
        // the TUI side never drift on the route shape. Any change
        // here is a coordinated breaking change.
        let b = base("https");
        let id = Uuid::nil();
        let u = build_upload_url(&b, id).unwrap();
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.path(), format!("/api/sessions/{id}/uploads"));
    }

    #[test]
    fn host_readiness_overrides_the_ordinary_http_timeout() {
        let client = Client::new(base("http"), "token".into(), TlsTrust::Plain).unwrap();
        let id = Uuid::nil();
        let request = client.host_readiness_request(id).unwrap().build().unwrap();

        assert_eq!(request.url().path(), format!("/api/hosts/{id}/readiness"));
        assert_eq!(request.timeout(), Some(&HOST_READINESS_TIMEOUT));
        assert!(
            HOST_READINESS_TIMEOUT > DEFAULT_HTTP_TIMEOUT,
            "SSH readiness must outlive the generic API deadline"
        );
    }

    #[test]
    fn remote_lifecycle_actions_override_the_ordinary_http_timeout() {
        assert_eq!(
            session_action_timeout("start"),
            Some(SESSION_LIFECYCLE_TIMEOUT)
        );
        assert_eq!(
            session_action_timeout("stop"),
            Some(SESSION_LIFECYCLE_TIMEOUT)
        );
        assert_eq!(
            session_action_timeout("kill"),
            Some(SESSION_LIFECYCLE_TIMEOUT)
        );
        assert!(
            SESSION_LIFECYCLE_TIMEOUT > DEFAULT_HTTP_TIMEOUT,
            "remote lifecycle mutations must outlive the generic API deadline"
        );
        assert_eq!(
            session_action_timeout("rename"),
            None,
            "ordinary mutations retain the client default"
        );
        assert_eq!(
            session_delete_timeout(true),
            Some(SESSION_LIFECYCLE_TIMEOUT),
            "force-delete includes remote tmux teardown"
        );
        assert_eq!(session_delete_timeout(false), None);
    }

    #[test]
    fn remote_session_create_uses_a_lifecycle_timeout() {
        assert_eq!(
            session_create_timeout(Some(Uuid::new_v4())),
            Some(REMOTE_SESSION_CREATE_TIMEOUT)
        );
        assert!(
            REMOTE_SESSION_CREATE_TIMEOUT > DEFAULT_HTTP_TIMEOUT,
            "SSH preflight must outlive the generic API deadline"
        );
        assert_eq!(
            session_create_timeout(None),
            None,
            "local creates retain the client default"
        );
        assert_eq!(
            session_create_timeout(Some(agentum_core::LOCAL_HOST_ID)),
            None,
            "an explicit local-host id is still a local create"
        );
    }

    #[test]
    fn api_error_decodes_standard_json_envelope() {
        let body = r#"{"error":"ssh/tmux exited with status 1\nstderr: permission denied"}"#;
        assert_eq!(
            format_api_error(StatusCode::INTERNAL_SERVER_ERROR, body),
            "500 Internal Server Error — ssh/tmux exited with status 1\nstderr: permission denied"
        );
    }

    #[test]
    fn api_error_preserves_plain_text_and_handles_empty_bodies() {
        assert_eq!(
            format_api_error(StatusCode::BAD_GATEWAY, "  upstream unavailable\n  "),
            "502 Bad Gateway — upstream unavailable"
        );
        assert_eq!(
            format_api_error(StatusCode::NO_CONTENT, "  "),
            "204 No Content"
        );
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
    fn ws_url_carries_resume_and_redraw_together_on_reconnect() {
        // A reconnect after a suspend sends both: resume (replay the missed
        // delta) and redraw (force the agent to repaint over any broadcast
        // garbage the resume delta would otherwise re-feed). Both must be real
        // query pairs so old daemons can drop the unknown one and still route.
        let u = ws_url(
            &base("https"),
            "/api/sessions/abc/stream",
            "tok",
            &[("resume", "true"), ("redraw", "true")],
        );
        assert_eq!(u.path(), "/api/sessions/abc/stream");
        assert_eq!(u.query(), Some("token=tok&resume=true&redraw=true"));
        assert!(!u.as_str().contains("%3F"));
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
