//! Shared CDP-over-HTTP discovery helpers. Both the screencast bridge
//! (`cdp_screencast`) and the request-response op driver (`cdp_driver`) need to
//! fetch a browser's `/json` listing and resolve its page-target WebSocket URL.
//! These lived in `cdp_screencast` with `cdp_driver` importing across — an
//! asymmetric coupling. Co-locating them here lets each CDP surface depend on a
//! neutral helper module instead of on the other.

use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

/// Pick the first inspectable **page** target's `webSocketDebuggerUrl` from a
/// CDP `GET /json` listing. Pure so the parse is unit-tested off a fixture.
pub(crate) fn pick_page_ws_url(listing: &Value) -> Option<String> {
    listing.as_array()?.iter().find_map(|t| {
        let is_page = t.get("type").and_then(Value::as_str) == Some("page");
        let url = t.get("webSocketDebuggerUrl").and_then(Value::as_str);
        match (is_page, url) {
            (true, Some(u)) => Some(u.to_string()),
            _ => None,
        }
    })
}

/// Resolve the CDP page-target WebSocket URL for a browser exposing CDP at
/// `cdp_http_base` (e.g. `http://127.0.0.1:9300`). Fetches `/json` and returns
/// the first `type:"page"` target — the tab the agent drives and the user watches.
pub(crate) async fn discover_page_ws_url(cdp_http_base: &str) -> Result<String> {
    let url = format!("{}/json", cdp_http_base.trim_end_matches('/'));
    let body: Value = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .with_context(|| format!("query CDP target list at {url}"))?
        .json()
        .await
        .context("parse CDP /json listing")?;
    pick_page_ws_url(&body)
        .with_context(|| format!("no inspectable page target at {url} (browser has no open tab?)"))
}

/// Fetch + parse a CDP HTTP endpoint (`/json`, `/json/version`, `/json/list`).
pub(crate) async fn cdp_http_json(url: &str) -> Result<Value> {
    reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .json()
        .await
        .with_context(|| format!("parse JSON from {url}"))
}
