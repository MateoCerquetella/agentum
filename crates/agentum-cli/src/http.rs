//! Minimal HTTP client for `agentum` subcommands that drive a running server —
//! the desktop's embedded loopback server, or a standalone `agentum serve`.
//!
//! The base URL comes from [`crate::api_base::resolve_api_base`], so a command
//! run INSIDE a desktop pane (where the embedded server injected
//! `AGENTUM_API_URL`) reaches that exact desktop's control plane; outside a
//! pane it uses the active profile or `127.0.0.1:8822`.
//!
//! Auth: the embedded desktop server is no-auth, so the common case needs no
//! token. For a standalone daemon, set `AGENTUM_TOKEN` (or run it `--no-auth`).

use anyhow::{Context, Result};
use serde_json::Value;

pub struct ApiClient {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl ApiClient {
    /// Build from the resolved base URL + optional `AGENTUM_TOKEN`.
    pub fn from_env() -> Self {
        Self::with_base(
            crate::api_base::resolve_api_base(),
            std::env::var("AGENTUM_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
        )
    }

    pub fn with_base(base: String, token: Option<String>) -> Self {
        // Remote profiles may be https (rustls verifies normally); loopback is
        // plain http. We do NOT disable cert verification.
        let http = reqwest::Client::builder()
            .build()
            .expect("build reqwest client");
        Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            http,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn with_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// `GET {base}{path}` → parsed JSON. Errors carry the URL + body so a
    /// connection refused (no server) or a 401 (needs a token) is legible.
    pub async fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .with_auth(self.http.get(&url))
            .send()
            .await
            .with_context(|| format!("GET {url} (is a server running there?)"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("{status} from {url} — {body}");
        }
        serde_json::from_str(&body).with_context(|| format!("parse JSON from {url}"))
    }

    /// `POST {base}{path}` with a JSON body → parsed JSON response.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .with_auth(self.http.post(&url).json(body))
            .send()
            .await
            .with_context(|| format!("POST {url} (is a server running there?)"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("{status} from {url} — {text}");
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).with_context(|| format!("parse JSON from {url}"))
    }
}
