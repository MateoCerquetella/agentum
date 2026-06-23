//! Request/response CDP driver for the persistent local Chromium (009c — QA).
//!
//! The screencast ([`crate::cdp_screencast`]) *streams* the browser into agentum's
//! pane; this module *drives* it programmatically over the same CDP endpoint, so
//! the `agentum_browser` MCP ops (snapshot/screenshot/click/fill/navigate) act on
//! the **same persistent Chromium** the user watches — server-side, so they work
//! headless (GUI webview closed) and return REAL state instead of the desktop
//! webview's stub strings.
//!
//! Shape: ONE short-lived CDP WebSocket per op (connect → call → drop). The browser
//! itself is the long-lived singleton owned by [`crate::cdp_browser`] (a detached
//! tmux session that survives the desktop app), so there's nothing to keep alive
//! here. Each op resolves the browser's active page target and issues a single
//! `Runtime.evaluate` / `Page.*` command.
//!
//! **Localhost only.** `cdp_http_base` is a parameter (and an explicit `cdpPort`
//! arg targets an already-running browser without launching it), so an SSH host
//! (the 009a `ssh -L` tunnel) can be threaded later — but remote is **not**
//! implemented or verified here. That is a deliberate seam, not a feature.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::cdp_browser;
use crate::cdp_screencast::discover_page_ws_url;

/// CDP command timeout. A page op (evaluate / capture / navigate kickoff) should
/// answer well within this; a hang means the page or socket is wedged, and we'd
/// rather surface a clear error to the agent than block its tool call forever.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Ops the CDP driver owns — the ones that DRIVE the page and must hit the
/// persistent Chromium (not the desktop webview). `open`/`tabs`/`grab`/`annotate`/
/// `annotations` stay on the desktop bridge (tab lifecycle + annotation store).
pub fn handles_op(op: &str) -> bool {
    matches!(op, "navigate" | "snapshot" | "screenshot" | "click" | "fill")
}

/// Entry point for `routes::mcp`: run one driver-owned `agentum_browser` op against
/// the persistent local Chromium and return its result JSON.
///
/// With no `cdpPort` the shared LOCAL browser is launched on demand (idempotent —
/// reuses the persistent singleton). An explicit `cdpPort` targets an
/// already-running browser (e.g. an SSH-tunneled one) and is NEVER launched here.
pub async fn run_browser_op(op: &str, args: &Value) -> Result<Value> {
    let base = match args.get("cdpPort").and_then(Value::as_u64) {
        Some(port) => cdp_browser::cdp_endpoint_for(port as u16),
        None => cdp_browser::ensure_local_cdp_browser().await?,
    };
    match op {
        "navigate" => cdp_navigate(&base, args).await,
        "snapshot" => cdp_snapshot(&base).await,
        "screenshot" => cdp_screenshot(&base).await,
        "click" => cdp_click(&base, args).await,
        "fill" => cdp_fill(&base, args).await,
        other => anyhow::bail!("cdp_driver does not handle op `{other}`"),
    }
}

// --- ops --------------------------------------------------------------------

/// Navigate the active page. `Page.navigate` needs no `Page.enable`.
pub(crate) async fn cdp_navigate(base: &str, args: &Value) -> Result<Value> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `url`"))?;
    let mut conn = connect_active_page(base).await?;
    conn.call("Page.navigate", json!({ "url": url })).await?;
    Ok(json!({ "ok": true, "url": url }))
}

/// Real page snapshot: url + title + visible text, read out of the live DOM via
/// `Runtime.evaluate` (returnByValue). Not the old stub.
pub(crate) async fn cdp_snapshot(base: &str) -> Result<Value> {
    let mut conn = connect_active_page(base).await?;
    let result = conn
        .call(
            "Runtime.evaluate",
            json!({ "expression": SNAPSHOT_EXPR, "returnByValue": true }),
        )
        .await?;
    let value = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("snapshot: unexpected Runtime.evaluate result: {result}"))?;
    serde_json::from_str(value).context("snapshot: parse page JSON")
}

/// Real screenshot via `Page.captureScreenshot`: decode the base64 JPEG, write it
/// to the browser profile dir, and return the path + byte count. Empty bytes fail
/// loudly rather than reporting a hollow success.
pub(crate) async fn cdp_screenshot(base: &str) -> Result<Value> {
    let mut conn = connect_active_page(base).await?;
    let result = conn
        .call(
            "Page.captureScreenshot",
            json!({ "format": "jpeg", "quality": 80 }),
        )
        .await?;
    let data_b64 = result
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("screenshot: no `data` in Page.captureScreenshot result"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .context("screenshot: decode base64")?;
    if bytes.is_empty() {
        anyhow::bail!("screenshot: captured 0 bytes");
    }
    let path = screenshot_path()?;
    std::fs::write(&path, &bytes)
        .with_context(|| format!("screenshot: write {}", path.display()))?;
    Ok(json!({
        "ok": true,
        "format": "jpeg",
        "bytes": bytes.len(),
        "path": path.to_string_lossy(),
    }))
}

/// Click the element matching `selector` (scroll into view first). Returns whether
/// the selector matched, so the agent can tell a no-op from a real click.
pub(crate) async fn cdp_click(base: &str, args: &Value) -> Result<Value> {
    let sel = args
        .get("selector")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `selector`"))?;
    let mut conn = connect_active_page(base).await?;
    let found = conn.eval_bool(&click_expr(sel)).await?;
    Ok(json!({ "ok": true, "selector": sel, "found": found }))
}

/// Set the value of `selector` and fire `input`+`change` so framework listeners
/// react. Returns whether the selector matched.
pub(crate) async fn cdp_fill(base: &str, args: &Value) -> Result<Value> {
    let sel = args
        .get("selector")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `selector`"))?;
    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
    let mut conn = connect_active_page(base).await?;
    let found = conn.eval_bool(&fill_expr(sel, text)).await?;
    Ok(json!({ "ok": true, "selector": sel, "found": found }))
}

// --- CDP connection ----------------------------------------------------------

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// A short-lived CDP connection to one page target. Commands carry monotonic ids;
/// [`CdpConn::call`] awaits the response with the matching id, skipping the
/// interleaved events and other responses CDP multiplexes on the socket.
struct CdpConn {
    ws: Ws,
    next_id: u64,
}

impl CdpConn {
    async fn connect(ws_url: &str) -> Result<Self> {
        let (ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .with_context(|| format!("open CDP WebSocket {ws_url}"))?;
        Ok(Self { ws, next_id: 0 })
    }

    /// Send a CDP command and return its `result` (or surface its `error`).
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({ "id": id, "method": method, "params": params });
        self.ws
            .send(Message::Text(msg.to_string()))
            .await
            .with_context(|| format!("send CDP {method}"))?;

        let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("CDP {method} timed out after {CALL_TIMEOUT:?}");
            }
            let next = tokio::time::timeout(remaining, self.ws.next())
                .await
                .map_err(|_| anyhow::anyhow!("CDP {method} timed out after {CALL_TIMEOUT:?}"))?;
            let Some(frame) = next else {
                anyhow::bail!("CDP socket closed while awaiting {method}");
            };
            let frame = frame.with_context(|| format!("read CDP response for {method}"))?;
            let Message::Text(txt) = frame else {
                continue; // CDP speaks text; ignore ping/pong/binary
            };
            let Ok(v) = serde_json::from_str::<Value>(&txt) else {
                continue;
            };
            // Skip events (no id) and responses to other commands.
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                anyhow::bail!("CDP {method} error: {err}");
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// `Runtime.evaluate` an expression expected to yield a boolean.
    async fn eval_bool(&mut self, expr: &str) -> Result<bool> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": expr, "returnByValue": true }),
            )
            .await?;
        Ok(result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }
}

/// Resolve + connect to the persistent browser's active page target.
async fn connect_active_page(cdp_http_base: &str) -> Result<CdpConn> {
    let ws_url = discover_page_ws_url(cdp_http_base).await?;
    CdpConn::connect(&ws_url).await
}

// --- pure helpers (unit-tested without a browser) ----------------------------

/// JS read for [`cdp_snapshot`] — returns a JSON string the driver parses. Text is
/// capped so a huge page can't blow up the MCP response.
const SNAPSHOT_EXPR: &str = "JSON.stringify({url:location.href,title:document.title,\
text:((document.body&&document.body.innerText)||'').slice(0,20000)})";

/// JS-string-literal encode (safe to embed in an eval'd expression). Mirrors the
/// desktop bridge's `js_string`.
fn js_string(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

/// `querySelector(sel).click()` returning whether the element matched.
fn click_expr(selector: &str) -> String {
    format!(
        "(function(){{var e=document.querySelector({sel});if(!e)return false;\
         e.scrollIntoView({{block:'center'}});e.click();return true;}})()",
        sel = js_string(selector)
    )
}

/// Set `sel`'s value to `text` + fire input/change, returning whether it matched.
fn fill_expr(selector: &str, text: &str) -> String {
    format!(
        "(function(){{var e=document.querySelector({sel});if(!e)return false;e.focus();\
         e.value={val};e.dispatchEvent(new Event('input',{{bubbles:true}}));\
         e.dispatchEvent(new Event('change',{{bubbles:true}}));return true;}})()",
        sel = js_string(selector),
        val = js_string(text)
    )
}

/// Where a captured screenshot is written (inside the agent browser's profile dir).
fn screenshot_path() -> Result<PathBuf> {
    let dir = agentum_store::paths::state_dir()
        .context("resolve agentum state dir")?
        .join("cdp-browser");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create screenshot dir {}", dir.display()))?;
    Ok(dir.join("last-screenshot.jpg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_only_the_page_driving_ops() {
        for op in ["navigate", "snapshot", "screenshot", "click", "fill"] {
            assert!(handles_op(op), "{op} should be CDP-driven");
        }
        // These stay on the desktop bridge.
        for op in ["open", "tabs", "grab", "annotate", "annotations", "bogus"] {
            assert!(!handles_op(op), "{op} should NOT be CDP-driven");
        }
    }

    #[test]
    fn js_string_escapes_quotes_and_backslashes() {
        assert_eq!(js_string("a"), "\"a\"");
        assert_eq!(js_string("a\"b"), "\"a\\\"b\"");
        // A selector with a quote can't break out of the expression.
        assert!(click_expr("a\"]").contains("\\\""));
    }

    #[test]
    fn click_and_fill_exprs_guard_a_missing_element() {
        let c = click_expr("#go");
        assert!(c.contains("querySelector(\"#go\")"));
        assert!(c.contains("if(!e)return false"));
        assert!(c.contains(".click()"));

        let f = fill_expr("#name", "Ada");
        assert!(f.contains("querySelector(\"#name\")"));
        assert!(f.contains("e.value=\"Ada\""));
        // Frameworks listen on these — both must fire.
        assert!(f.contains("'input'"));
        assert!(f.contains("'change'"));
    }

    #[test]
    fn snapshot_expr_reads_url_title_and_text() {
        assert!(SNAPSHOT_EXPR.contains("location.href"));
        assert!(SNAPSHOT_EXPR.contains("document.title"));
        assert!(SNAPSHOT_EXPR.contains("innerText"));
    }

    // --- real-Chromium gate (manual) ----------------------------------------

    async fn http_version(base: &str) -> Option<Value> {
        reqwest::Client::new()
            .get(format!("{base}/json/version"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()
    }

    /// The browser's stable GUID (its `webSocketDebuggerUrl`) — same value across
    /// calls iff it's the same instance (i.e. a reconnect, not a respawn).
    async fn browser_guid(base: &str) -> Option<String> {
        http_version(base)
            .await?
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    async fn wait_http_ok(base: &str, max: Duration) -> bool {
        let deadline = std::time::Instant::now() + max;
        loop {
            if http_version(base).await.is_some() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Real-Chromium gate (manual; run with `-- --ignored`): launch a detached
    /// headless Chromium, prove it SURVIVES the launcher dropping its handle,
    /// RECONNECT to the SAME instance (no respawn), then DRIVE it — navigate,
    /// snapshot real text, fill+click to mutate the DOM, screenshot non-empty
    /// bytes. Covers acceptance A/B/C against a live browser.
    ///
    /// `#[ignore]` so CI's `cargo test --workspace --lib` (no `--ignored`, no
    /// Chrome) stays green; it FAILS LOUDLY if no Chrome/Chromium is installed —
    /// never a vacuous pass. Hermetic: its own port + temp profile, so it never
    /// disturbs the shared `:9300` browser the desktop app drives.
    #[tokio::test]
    #[ignore = "needs a real Chrome/Chromium; run with -- --ignored"]
    async fn persist_reconnect_and_drive_real_chromium() {
        use std::process::{Command, Stdio};
        use std::time::Instant;
        use tokio::time::sleep;

        let exe = cdp_browser::chromium_executable().expect(
            "no Chrome/Chromium found — install Google Chrome or run `npx playwright install chromium`",
        );

        // Hermetic port + profile so we never disturb the shared :9300 browser.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let profile = std::env::temp_dir().join(format!("agentum-cdp-test-{port}"));
        let base = cdp_browser::cdp_endpoint_for(port);

        let mut cmd = Command::new(&exe);
        cmd.args([
            "--headless=new".to_string(),
            "--remote-debugging-address=127.0.0.1".to_string(),
            format!("--remote-debugging-port={port}"),
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "about:blank".to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        // Own process group → not tied to the launcher's signals (the persistence
        // property: the browser is independent of whoever spawned it).
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let child = cmd.spawn().expect("spawn headless Chromium");
        #[cfg(unix)]
        let pid = child.id();

        // A — survives the launcher dropping its handle.
        drop(child);
        assert!(
            wait_http_ok(&base, Duration::from_secs(20)).await,
            "Chromium never exposed CDP on {base}"
        );

        // B — reconnect hits the SAME browser instance (stable GUID, no respawn).
        let guid1 = browser_guid(&base).await.expect("first /json/version");
        let guid2 = browser_guid(&base).await.expect("second /json/version");
        assert_eq!(
            guid1, guid2,
            "reconnect must reach the same browser, not a new one"
        );

        // C — drive it. A base64 data: URL sidesteps URL-encoding pitfalls.
        let html = "<!doctype html><html><head><title>qa-start</title></head><body>\
            <p>marker-hello</p><input id=\"name\">\
            <button id=\"go\" onclick=\"document.title='clicked:'+document.getElementById('name').value\">GO</button>\
            </body></html>";
        let data_url = format!(
            "data:text/html;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(html)
        );
        cdp_navigate(&base, &json!({ "url": data_url }))
            .await
            .expect("navigate");

        // Wait for load, then assert REAL snapshot text (not a stub).
        let deadline = Instant::now() + Duration::from_secs(5);
        let snap = loop {
            let s = cdp_snapshot(&base).await.expect("snapshot");
            if s.get("title").and_then(Value::as_str) == Some("qa-start") {
                break s;
            }
            assert!(Instant::now() < deadline, "page never loaded; snapshot={s}");
            sleep(Duration::from_millis(150)).await;
        };
        assert!(
            snap.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("marker-hello"),
            "snapshot text should contain the page marker: {snap}"
        );

        // fill + click mutate the DOM (the button's onclick rewrites the title).
        let filled = cdp_fill(&base, &json!({ "selector": "#name", "text": "Ada" }))
            .await
            .expect("fill");
        assert_eq!(filled.get("found").and_then(Value::as_bool), Some(true));
        let clicked = cdp_click(&base, &json!({ "selector": "#go" }))
            .await
            .expect("click");
        assert_eq!(clicked.get("found").and_then(Value::as_bool), Some(true));

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = cdp_snapshot(&base).await.expect("snapshot after click");
            if s.get("title").and_then(Value::as_str) == Some("clicked:Ada") {
                break; // fill + click verifiably mutated the DOM
            }
            assert!(Instant::now() < deadline, "click/fill did not mutate the DOM: {s}");
            sleep(Duration::from_millis(150)).await;
        }

        // screenshot returns non-empty bytes.
        let shot = cdp_screenshot(&base).await.expect("screenshot");
        assert!(
            shot.get("bytes").and_then(Value::as_u64).unwrap_or(0) > 0,
            "screenshot must be non-empty: {shot}"
        );

        // cleanup — kill the detached browser + drop its temp profile.
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status();
        }
        let _ = std::fs::remove_dir_all(&profile);
    }
}
