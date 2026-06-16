//! Thin HTTP client for the board API endpoints used by the planner CLI.
//!
//! This is the planner agent's only output surface (D-05). It authenticates
//! by reading `credentials.toml` via the trust layer — no token in argv, no
//! token in env vars (T-05-01, T-05-02).

use anyhow::{Context, Result, bail};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde_json::Value;

use crate::commands::terminal::{profiles, trust};

/// HTTP wrapper around the agentum board API.
///
/// One `BoardClient` per subcommand invocation: built in `new()`, consumed
/// through the `post_*` / `resolve_*` helpers. The bearer token is held
/// only in the reqwest client's default `Authorization` header with the
/// `sensitive` flag set so reqwest's tracing layer redacts it from logs.
pub struct BoardClient {
    client: Client,
    base_url: String,
}

impl BoardClient {
    /// Resolve `profile_name` from `profiles.toml` + `credentials.toml` and
    /// build a reqwest client whose default headers carry `Authorization:
    /// Bearer <token>` (sensitive-marked).
    ///
    /// Exits 4 if credentials are missing so the planner agent gets a
    /// parseable, deterministic signal rather than a Rust backtrace.
    pub fn new(profile_name: &str) -> Result<Self> {
        let profiles = profiles::load().context("load profiles.toml")?;
        let profile = profiles.get(profile_name).ok_or_else(|| {
            anyhow::anyhow!(
                "no profile named '{profile_name}'; run `agentum profiles add {profile_name} <url>` first"
            )
        })?;

        let token = match trust::token_for_url(&profile.url)? {
            Some(t) => t,
            None => {
                eprintln!(
                    "no credentials for profile '{profile_name}' ({}); \
                     run `agentum auth login --profile {profile_name}` first",
                    profile.url
                );
                std::process::exit(4);
            }
        };

        // Build the Authorization header with the sensitive flag so reqwest's
        // tracing layer replaces the value with REDACTED in any log output.
        let mut auth_value =
            HeaderValue::from_str(&format!("Bearer {token}")).context("build auth header")?;
        auth_value.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth_value);

        // Mirror the TUI's api.rs build_http() pattern: use_preconfigured_tls
        // for pinned fingerprints, plain platform TLS otherwise. Never call
        // danger_accept_invalid_certs — that bypasses the PinningVerifier and
        // defeats T-05-07.
        let mut builder = Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30));

        if let Some(ref fp) = profile.fingerprint {
            // The pinned TLS config installs a PinningVerifier (trust.rs) that
            // checks the cert fingerprint on every connection. An MITM
            // presenting a different self-signed cert fails verification.
            // Never call danger_accept_invalid_certs — that bypasses the
            // verifier and defeats T-05-07.
            let cfg = trust::pinned_tls_config(fp.clone());
            let owned = (*cfg).clone();
            builder = builder.use_preconfigured_tls(owned);
        } else if profile.insecure {
            // Matches the TUI's AcceptAny branch in api.rs:97-98: only
            // reachable via an explicit per-profile `insecure = true` flag,
            // never the default. Uses use_preconfigured_tls (not
            // danger_accept_invalid_certs) so the code path mirrors the
            // fingerprint case.
            let cfg = trust::accept_any_tls_config();
            let owned = (*cfg).clone();
            builder = builder.use_preconfigured_tls(owned);
        }
        // When neither fingerprint nor insecure is set, the platform's default
        // TLS trust store is used (reqwest default). No extra builder call needed.

        let client = builder.build().context("build reqwest client")?;

        Ok(Self {
            client,
            base_url: profile.url.clone(),
        })
    }

    /// POST /api/board/goals — create a new goal card.
    pub async fn post_goal(
        &self,
        title: &str,
        body: Option<&str>,
        workdir: Option<&str>,
    ) -> Result<Value> {
        let url = format!("{}/api/board/goals", self.base_url);
        let mut payload = serde_json::json!({ "title": title });
        if let Some(b) = body {
            payload["body"] = Value::String(b.to_owned());
        }
        if let Some(w) = workdir {
            payload["workdir"] = Value::String(w.to_owned());
        }
        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .context("POST /api/board/goals")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("daemon returned {status}: {body}");
        }
        resp.json::<Value>().await.context("parse goal response")
    }

    /// POST /api/board — create a new board item (card under a goal).
    pub async fn post_board_item(&self, payload: Value) -> Result<Value> {
        let url = format!("{}/api/board", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .context("POST /api/board")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("daemon returned {status}: {body}");
        }
        resp.json::<Value>()
            .await
            .context("parse board item response")
    }

    /// POST /api/board/links — create a symbolic blocking-link between two
    /// cards. If the server responds 400 with an "unknown sibling key:"
    /// prefix, exit 5 so the planner agent gets the deterministic signal
    /// described in CONTEXT D-06.
    pub async fn post_link_symbolic(
        &self,
        parent_goal_id: i64,
        from_key: &str,
        to_key: &str,
        kind: &str,
    ) -> Result<Value> {
        let url = format!("{}/api/board/links", self.base_url);
        let payload = serde_json::json!({
            "parent_goal_id": parent_goal_id,
            "from_key": from_key,
            "to_key": to_key,
            "kind": kind,
        });
        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .context("POST /api/board/links")?;
        let status = resp.status();
        if status == reqwest::StatusCode::BAD_REQUEST {
            let body = resp.text().await.unwrap_or_default();
            // The server embeds the key that was unresolvable in the error field.
            // The planner agent reads this from stderr and can retry after
            // creating the missing target (forward-reference pattern, D-06).
            if body.contains("unknown sibling key:") || body.contains("\"unknown sibling key:") {
                let msg = extract_error_field(&body).unwrap_or(body);
                eprintln!("{msg}");
                std::process::exit(5);
            }
            bail!("daemon returned {status}: {body}");
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("daemon returned {status}: {body}");
        }
        resp.json::<Value>().await.context("parse link response")
    }

    /// GET /api/board — scan for the card whose `key` field equals `ag_key`
    /// and return its numeric `id`. Emits a stderr warning when `lbl != "goal"`
    /// because --parent-goal should always reference a goal card.
    pub async fn resolve_parent_goal_id(&self, ag_key: &str) -> Result<i64> {
        let url = format!("{}/api/board", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("GET /api/board")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("daemon returned {status}: {body}");
        }
        let items: Value = resp.json().await.context("parse board list")?;
        let arr = items
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("expected JSON array from GET /api/board"))?;
        for item in arr {
            if item.get("key").and_then(|v| v.as_str()) == Some(ag_key) {
                let lbl = item.get("lbl").and_then(|v| v.as_str()).unwrap_or("");
                if lbl != "goal" {
                    // Warn but proceed — user may have a card with the same key.
                    eprintln!("warning: card '{ag_key}' has lbl='{lbl}', expected 'goal'");
                }
                return item
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| anyhow::anyhow!("card '{ag_key}' has no numeric id"));
            }
        }
        bail!("no board card with key '{ag_key}' found")
    }
}

/// Validate a symbolic key: `[a-zA-Z0-9_-]{1,64}`.
///
/// Mirrors the server-side validate_symbolic_key in routes/board_links.rs.
/// Running this check client-side catches injection attempts before any HTTP
/// call is made (T-05-04).
pub fn validate_symbolic_key(key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("symbolic key must not be empty");
    }
    if key.len() > 64 {
        bail!(
            "symbolic key must be at most 64 characters, got {}",
            key.len()
        );
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!(
            "symbolic key '{}' contains invalid characters (only [a-zA-Z0-9_-] allowed)",
            key
        );
    }
    Ok(())
}

/// Extract the `error` field from a JSON body string, falling back to the
/// raw string. Avoids leaking internal JSON structure in the user-facing
/// error message for the unknown-sibling-key case.
fn extract_error_field(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.get("error")
            .and_then(|e| e.as_str())
            .map(|s| s.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_symbolic_key_accepts_valid_chars() {
        assert!(validate_symbolic_key("foo").is_ok());
        assert!(validate_symbolic_key("foo_bar").is_ok());
        assert!(validate_symbolic_key("auth-2").is_ok());
        assert!(validate_symbolic_key("a1b2c3").is_ok());
        assert!(validate_symbolic_key("A-Z_09").is_ok());
    }

    #[test]
    fn validate_symbolic_key_rejects_invalid_chars() {
        assert!(validate_symbolic_key("..").is_err());
        assert!(validate_symbolic_key("foo/bar").is_err());
        assert!(validate_symbolic_key("foo bar").is_err());
        assert!(validate_symbolic_key("").is_err());
        assert!(validate_symbolic_key(&"x".repeat(65)).is_err());
        // dot is not in the allowed set
        assert!(validate_symbolic_key("foo.bar").is_err());
    }
}
