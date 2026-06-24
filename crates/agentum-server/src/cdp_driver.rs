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

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
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
    matches!(
        op,
        "navigate" | "snapshot" | "screenshot" | "click" | "fill" | "get_console" | "wait"
    )
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
    // Start the console/network listener (idempotent) so diagnostics are captured
    // continuously from the first browser op, not only when `get_console` is called.
    ensure_console_listener(&base);
    match op {
        "navigate" => cdp_navigate(&base, args).await,
        "snapshot" => cdp_snapshot(&base, args).await,
        "screenshot" => cdp_screenshot(&base, args).await,
        "click" => cdp_click(&base, args).await,
        "fill" => cdp_fill(&base, args).await,
        "get_console" => cdp_get_console(args).await,
        "wait" => cdp_wait(&base, args).await,
        other => anyhow::bail!("cdp_driver does not handle op `{other}`"),
    }
}

// --- ops --------------------------------------------------------------------

/// Navigate the active page, wait per `wait_until` (load|domcontentloaded|
/// network_idle, default load), and return the final url + title (+ http_status
/// of the main document when known).
pub(crate) async fn cdp_navigate(base: &str, args: &Value) -> Result<Value> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `url`"))?;
    let wait_until = args
        .get("wait_until")
        .and_then(Value::as_str)
        .unwrap_or("load");
    let mut conn = connect_active_page(base).await?;
    let nav = conn.call("Page.navigate", json!({ "url": url })).await?;
    // A hard navigation error (bad scheme, DNS) comes back inline.
    if let Some(err) = nav.get("errorText").and_then(Value::as_str) {
        if !err.is_empty() {
            return Ok(json!({ "ok": false, "url": url, "error": err }));
        }
    }
    wait_for_load(&mut conn, wait_until, 15_000).await;
    let info = conn
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "JSON.stringify({u:location.href,t:document.title})",
                "returnByValue": true
            }),
        )
        .await?;
    let (final_url, title) = parse_url_title(&info);
    let mut out = json!({ "ok": true, "url": url, "final_url": final_url, "title": title });
    if let Some(st) = last_document_status() {
        out["http_status"] = json!(st);
    }
    Ok(out)
}

/// `wait`: block until a condition holds or `timeout_ms` (default 5000) elapses.
/// `condition` ∈ selector | text | url | network_idle; `arg` is the css/text/url
/// substring. Always returns `ok:true` with `timed_out` telling the agent which.
pub(crate) async fn cdp_wait(base: &str, args: &Value) -> Result<Value> {
    let condition = args.get("condition").and_then(Value::as_str).unwrap_or("");
    let arg = args.get("arg").and_then(Value::as_str).unwrap_or("");
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5000);
    let mut conn = connect_active_page(base).await?;
    let pred = wait_predicate_expr(condition, arg);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut idle_polls = 0u32;
    loop {
        let satisfied = if condition == "network_idle" {
            // ~2 in-flight sustained over 3 polls (≈500ms) — the Playwright heuristic.
            if in_flight_requests() <= 2 {
                idle_polls += 1;
                idle_polls >= 3
            } else {
                idle_polls = 0;
                false
            }
        } else {
            match &pred {
                Some(expr) => conn.eval_bool(expr).await.unwrap_or(false),
                None => true, // unknown condition → don't hang the agent
            }
        };
        if satisfied {
            return Ok(json!({ "ok": true, "timed_out": false }));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(json!({ "ok": true, "timed_out": true }));
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Real page snapshot: url + title + visible text, read out of the live DOM via
/// `Runtime.evaluate` (returnByValue). Not the old stub. Optional viewport args
/// (`width`/`height`/`mobile`/`deviceScaleFactor`) snapshot the page at a chosen
/// breakpoint for responsive testing.
pub(crate) async fn cdp_snapshot(base: &str, args: &Value) -> Result<Value> {
    let interactive_only = args
        .get("interactive_only")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut conn = connect_active_page(base).await?;
    apply_viewport(&mut conn, args).await?;
    // url + title + visible text.
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
    let mut out: Value = serde_json::from_str(value).context("snapshot: parse page JSON")?;
    // Accessibility refs the agent can act on — opaque, generation-stamped so a
    // stale ref (acted on after a re-snapshot or navigation) is rejected.
    let _ = conn.call("Accessibility.enable", json!({})).await;
    let tree = conn
        .call("Accessibility.getFullAXTree", json!({}))
        .await
        .context("snapshot: Accessibility.getFullAXTree")?;
    let generation = next_generation();
    let (refs, truncated) = parse_ax_refs(&tree, generation, interactive_only);
    store_refs(generation, &refs);
    out["generation"] = json!(generation);
    out["refs"] = Value::Array(refs.iter().map(ax_ref_public).collect());
    if truncated {
        out["truncated"] = json!(true);
    }
    Ok(out)
}

/// Real screenshot via `Page.captureScreenshot`: decode the base64 JPEG, write it
/// to the browser profile dir, and return the path + byte count. Empty bytes fail
/// loudly rather than reporting a hollow success.
pub(crate) async fn cdp_screenshot(base: &str, args: &Value) -> Result<Value> {
    let mut conn = connect_active_page(base).await?;
    // Optional viewport override for responsive capture (e.g. `width:375`). Set on
    // THIS short-lived connection, so it auto-clears on disconnect and never
    // disturbs the live screencast's own viewport.
    apply_viewport(&mut conn, args).await?;
    let mut params = json!({ "format": "png" });
    // `full_page:true` captures the whole scrollable page, not just the viewport.
    if args.get("full_page").and_then(Value::as_bool).unwrap_or(false) {
        params["captureBeyondViewport"] = json!(true);
    }
    // Optional element clip by snapshot `ref` (capture just that element).
    if let Some(ref_id) = args.get("ref").and_then(Value::as_str) {
        let Some(backend) = resolve_ref(ref_id) else {
            return Ok(stale_ref(ref_id));
        };
        if let Some(object_id) = resolve_node_object(&mut conn, backend).await? {
            if let Some(clip) = node_clip(&mut conn, &object_id).await? {
                params["clip"] = clip;
            }
        }
    }
    let result = conn.call("Page.captureScreenshot", params).await?;
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
    let (width, height) = png_dimensions(&bytes).unwrap_or((0, 0));
    // `image_b64` is lifted into an MCP image content block by routes::mcp so the
    // agent can SEE the render; the path is kept for reference.
    Ok(json!({
        "ok": true,
        "format": "png",
        "bytes": bytes.len(),
        "width": width,
        "height": height,
        "path": path.to_string_lossy(),
        "image_b64": data_b64,
    }))
}

/// Click the element matching `selector` (scroll into view first). Returns whether
/// the selector matched, so the agent can tell a no-op from a real click.
pub(crate) async fn cdp_click(base: &str, args: &Value) -> Result<Value> {
    let mut conn = connect_active_page(base).await?;
    // Prefer a snapshot `ref` (trusted input at the element's center); fall back to
    // a CSS `selector` (JS `.click()`) for back-compat.
    if let Some(ref_id) = args.get("ref").and_then(Value::as_str) {
        return click_ref(&mut conn, ref_id).await;
    }
    let sel = args
        .get("selector")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `selector` or `ref`"))?;
    let found = conn.eval_bool(&click_expr(sel)).await?;
    Ok(json!({ "ok": true, "selector": sel, "found": found }))
}

/// Type into an element. With a snapshot `ref`, uses TRUSTED key input
/// (`Input.insertText`, optional `submit`=Enter). With a CSS `selector`, sets the
/// value and fires `input`+`change` so framework listeners react.
pub(crate) async fn cdp_fill(base: &str, args: &Value) -> Result<Value> {
    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
    let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
    let mut conn = connect_active_page(base).await?;
    if let Some(ref_id) = args.get("ref").and_then(Value::as_str) {
        return type_ref(&mut conn, ref_id, text, submit).await;
    }
    let sel = args
        .get("selector")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `selector` or `ref`"))?;
    let found = conn.eval_bool(&fill_expr(sel, text)).await?;
    Ok(json!({ "ok": true, "selector": sel, "found": found }))
}

// --- CDP connection ----------------------------------------------------------

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

/// Apply a viewport override on `conn` when the op carries viewport args, so the
/// page lays out at the requested breakpoint before the snapshot/screenshot. A
/// no-op when no viewport was requested. The override is scoped to this
/// short-lived connection, so it clears on disconnect.
async fn apply_viewport(conn: &mut CdpConn, args: &Value) -> Result<()> {
    if let Some(metrics) = device_metrics_params(args) {
        conn.call("Emulation.setDeviceMetricsOverride", metrics)
            .await?;
    }
    Ok(())
}

/// Poll the page until it reaches `wait_until` (load|domcontentloaded|
/// network_idle) or `timeout_ms` elapses. Best-effort: a timeout doesn't fail the
/// navigation, it just means the page was still busy.
async fn wait_for_load(conn: &mut CdpConn, wait_until: &str, timeout_ms: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut idle_polls = 0u32;
    loop {
        let ready = conn
            .call(
                "Runtime.evaluate",
                json!({ "expression": "document.readyState", "returnByValue": true }),
            )
            .await
            .ok()
            .and_then(|r| {
                r.get("result")
                    .and_then(|x| x.get("value"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let interactive = ready == "interactive" || ready == "complete";
        let done = match wait_until {
            "domcontentloaded" => interactive,
            "network_idle" => {
                if interactive && in_flight_requests() <= 2 {
                    idle_polls += 1;
                    idle_polls >= 3
                } else {
                    idle_polls = 0;
                    false
                }
            }
            _ => ready == "complete",
        };
        if done || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Resolve a `backendDOMNodeId` to a JS RemoteObject `objectId` so we can call
/// element methods on it. Enables the DOM domain first (idempotent). `None` means
/// the node is gone (the page navigated) — the caller treats that as a stale ref.
async fn resolve_node_object(conn: &mut CdpConn, backend_node_id: i64) -> Result<Option<String>> {
    let _ = conn.call("DOM.enable", json!({})).await;
    let Ok(resolved) = conn
        .call("DOM.resolveNode", json!({ "backendNodeId": backend_node_id }))
        .await
    else {
        return Ok(None);
    };
    Ok(resolved
        .get("object")
        .and_then(|o| o.get("objectId"))
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// Scroll the resolved element into view and return its viewport-center (CSS px),
/// or `None` when it has no layout box (display:none / zero size). Coordinates are
/// in the same CSS-px space `Input.dispatchMouseEvent` expects.
async fn node_center(conn: &mut CdpConn, object_id: &str) -> Result<Option<(f64, f64)>> {
    let func = "function(){this.scrollIntoView({block:'center',inline:'center'});\
        var r=this.getBoundingClientRect();\
        return JSON.stringify({x:r.left+r.width/2,y:r.top+r.height/2,w:r.width,h:r.height});}";
    let res = conn
        .call(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": func, "returnByValue": true }),
        )
        .await?;
    let Some(raw) = res
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let rect: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let w = rect.get("w").and_then(Value::as_f64).unwrap_or(0.0);
    let h = rect.get("h").and_then(Value::as_f64).unwrap_or(0.0);
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    Ok(Some((
        rect.get("x").and_then(Value::as_f64).unwrap_or(0.0),
        rect.get("y").and_then(Value::as_f64).unwrap_or(0.0),
    )))
}

/// A `Page.captureScreenshot` `clip` for the resolved element (its viewport rect),
/// or `None` when it has no layout box. Scrolls it into view first.
async fn node_clip(conn: &mut CdpConn, object_id: &str) -> Result<Option<Value>> {
    let func = "function(){this.scrollIntoView({block:'center',inline:'center'});\
        var r=this.getBoundingClientRect();\
        return JSON.stringify({x:r.left,y:r.top,w:r.width,h:r.height});}";
    let res = conn
        .call(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": func, "returnByValue": true }),
        )
        .await?;
    let Some(raw) = res
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let rect: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let w = rect.get("w").and_then(Value::as_f64).unwrap_or(0.0);
    let h = rect.get("h").and_then(Value::as_f64).unwrap_or(0.0);
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    Ok(Some(json!({
        "x": rect.get("x").and_then(Value::as_f64).unwrap_or(0.0),
        "y": rect.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        "width": w,
        "height": h,
        "scale": 1,
    })))
}

/// Click an element by snapshot ref with a TRUSTED mouse event at its center
/// (falls back to a synthetic `.click()` when it has no layout box). A stale ref
/// returns `stale_ref` so the agent re-snapshots.
async fn click_ref(conn: &mut CdpConn, ref_id: &str) -> Result<Value> {
    let Some(backend) = resolve_ref(ref_id) else {
        return Ok(stale_ref(ref_id));
    };
    let Some(object_id) = resolve_node_object(conn, backend).await? else {
        return Ok(stale_ref(ref_id));
    };
    match node_center(conn, &object_id).await? {
        Some((x, y)) => {
            for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
                conn.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": "left",
                            "buttons": buttons, "clickCount": 1 }),
                )
                .await?;
            }
            Ok(json!({ "ok": true, "ref": ref_id }))
        }
        None => {
            // No layout box (off-screen/hidden) — synthetic click is the best effort.
            conn.call(
                "Runtime.callFunctionOn",
                json!({ "objectId": object_id, "functionDeclaration": "function(){this.click();}" }),
            )
            .await?;
            Ok(json!({ "ok": true, "ref": ref_id, "synthetic": true }))
        }
    }
}

/// Type into an element by snapshot ref using TRUSTED key input: focus, then
/// `Input.insertText` (fires the input/change events frameworks listen for — unlike
/// a raw `el.value=`). `submit` presses Enter after. Stale ref → `stale_ref`.
async fn type_ref(conn: &mut CdpConn, ref_id: &str, text: &str, submit: bool) -> Result<Value> {
    let Some(backend) = resolve_ref(ref_id) else {
        return Ok(stale_ref(ref_id));
    };
    let Some(object_id) = resolve_node_object(conn, backend).await? else {
        return Ok(stale_ref(ref_id));
    };
    conn.call(
        "Runtime.callFunctionOn",
        json!({ "objectId": object_id, "functionDeclaration": "function(){this.focus();}" }),
    )
    .await?;
    if !text.is_empty() {
        conn.call("Input.insertText", json!({ "text": text })).await?;
    }
    if submit {
        for kind in ["keyDown", "keyUp"] {
            conn.call(
                "Input.dispatchKeyEvent",
                json!({ "type": kind, "key": "Enter", "code": "Enter",
                        "windowsVirtualKeyCode": 13, "text": "\r" }),
            )
            .await?;
        }
    }
    Ok(json!({ "ok": true, "ref": ref_id, "submitted": submit }))
}

/// The standard stale-ref response — the ref's generation is gone, so the agent
/// must call `snapshot` again to get fresh refs.
fn stale_ref(ref_id: &str) -> Value {
    json!({ "ok": false, "error": "stale_ref", "ref": ref_id })
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

/// Build `Emulation.setDeviceMetricsOverride` params from optional viewport args,
/// or `None` when the op requested no override. Both `width` and `height` are
/// required to override; dimensions floor at 1 (Chrome rejects 0), `mobile`
/// defaults false (desktop layout), `deviceScaleFactor` defaults 1.0. Pure so the
/// responsive-capture mapping is unit-tested without a browser.
fn device_metrics_params(args: &Value) -> Option<Value> {
    let width = args.get("width").and_then(Value::as_u64)?;
    let height = args.get("height").and_then(Value::as_u64)?;
    let dsf = args
        .get("deviceScaleFactor")
        .and_then(Value::as_f64)
        .filter(|d| *d > 0.0)
        .unwrap_or(1.0);
    let mobile = args.get("mobile").and_then(Value::as_bool).unwrap_or(false);
    Some(json!({
        "width": width.max(1),
        "height": height.max(1),
        "deviceScaleFactor": dsf,
        "mobile": mobile,
    }))
}

/// The JS predicate (returns bool) for a `wait` condition, or `None` for
/// `network_idle`/unknown (handled out of band). Pure so it's unit-tested.
fn wait_predicate_expr(condition: &str, arg: &str) -> Option<String> {
    match condition {
        "selector" => Some(format!("!!document.querySelector({})", js_string(arg))),
        "text" => Some(format!(
            "!!(document.body&&document.body.innerText.indexOf({})>=0)",
            js_string(arg)
        )),
        "url" => Some(format!("location.href.indexOf({})>=0", js_string(arg))),
        _ => None,
    }
}

/// Parse the `{u,t}` JSON string a navigate's `Runtime.evaluate` returns into
/// (final_url, title). Tolerant of a missing/garbled result.
fn parse_url_title(eval_result: &Value) -> (String, String) {
    let raw = eval_result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    (
        parsed
            .get("u")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        parsed
            .get("t")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    )
}

/// Read (width, height) from a PNG's IHDR header (big-endian u32s at byte 16/20),
/// or `None` if it isn't a PNG. Avoids pulling in an image-decode dependency.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[0..8] != SIG {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

// --- accessibility refs (snapshot → opaque refs the agent acts on) -----------

/// Interactive AX roles surfaced as actionable refs.
const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "listbox",
    "checkbox",
    "radio",
    "switch",
    "slider",
    "spinbutton",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "option",
    "textarea",
];

/// Roles useful even without an accessible name (a bare input an agent types into).
const NAMELESS_OK_ROLES: &[&str] = &["textbox", "searchbox", "textarea", "combobox"];

/// Cap on refs returned by one snapshot, so a huge page can't blow up the response.
const MAX_REFS: usize = 250;

/// One interactive element from a snapshot. `ref_id` is opaque
/// (`e{generation}_{idx}`) and resolves to a `backendDOMNodeId` via [`ref_registry`].
#[derive(Debug, Clone, PartialEq)]
struct AxRef {
    ref_id: String,
    role: String,
    name: String,
    value: Option<String>,
    disabled: Option<bool>,
    checked: Option<String>,
    backend_node_id: i64,
}

/// Server-side ref→backendNodeId map for the latest snapshot. The CDP browser is a
/// per-machine singleton, so one global registry mirrors it. A new snapshot bumps
/// `generation` and replaces `map`; a ref carrying a stale generation isn't in the
/// current map, so it resolves to `None` → the action returns `stale_ref`.
#[derive(Default)]
struct RefRegistry {
    generation: u64,
    map: HashMap<String, i64>,
}

impl RefRegistry {
    /// Store the ref→backendNodeId map for `generation`, unless a newer snapshot
    /// has already superseded it (then its map wins and these refs read as stale).
    fn store(&mut self, generation: u64, refs: &[AxRef]) {
        if self.generation == generation {
            self.map = refs
                .iter()
                .map(|r| (r.ref_id.clone(), r.backend_node_id))
                .collect();
        }
    }

    /// Resolve a ref to its backendNodeId, or `None` when stale (wrong generation,
    /// or the page moved on so the ref isn't in the current map).
    fn resolve(&self, ref_id: &str) -> Option<i64> {
        self.map.get(ref_id).copied()
    }
}

fn ref_registry() -> &'static Mutex<RefRegistry> {
    static REG: OnceLock<Mutex<RefRegistry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(RefRegistry::default()))
}

/// Allocate the next snapshot generation.
fn next_generation() -> u64 {
    let mut reg = ref_registry().lock().expect("ref registry poisoned");
    reg.generation += 1;
    reg.generation
}

/// The current snapshot generation without bumping it — used to stamp console /
/// network entries so `get_console(since_generation)` can return "what happened
/// since my last snapshot".
fn current_generation() -> u64 {
    ref_registry()
        .lock()
        .expect("ref registry poisoned")
        .generation
}

fn store_refs(generation: u64, refs: &[AxRef]) {
    ref_registry()
        .lock()
        .expect("ref registry poisoned")
        .store(generation, refs);
}

fn resolve_ref(ref_id: &str) -> Option<i64> {
    ref_registry()
        .lock()
        .expect("ref registry poisoned")
        .resolve(ref_id)
}

/// Read an AX node `properties[].value.value` for `key`.
fn ax_property(node: &Value, key: &str) -> Option<Value> {
    node.get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(key))?
        .get("value")?
        .get("value")
        .cloned()
}

/// Parse a CDP `Accessibility.getFullAXTree` result into ref entries for
/// `generation`. `interactive_only` keeps only actionable roles (the default).
/// Capped at [`MAX_REFS`]; the returned `bool` is `true` when truncated.
fn parse_ax_refs(tree: &Value, generation: u64, interactive_only: bool) -> (Vec<AxRef>, bool) {
    let mut out = Vec::new();
    let Some(nodes) = tree.get("nodes").and_then(Value::as_array) else {
        return (out, false);
    };
    for node in nodes {
        if node.get("ignored").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(backend_node_id) = node.get("backendDOMNodeId").and_then(Value::as_i64) else {
            continue;
        };
        let role = node
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if interactive_only && !INTERACTIVE_ROLES.contains(&role.as_str()) {
            continue;
        }
        let name = node
            .get("name")
            .and_then(|n| n.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // A nameless element is rarely actionable — except bare inputs, which an
        // agent still needs to type into.
        if interactive_only && name.is_empty() && !NAMELESS_OK_ROLES.contains(&role.as_str()) {
            continue;
        }
        if out.len() >= MAX_REFS {
            return (out, true);
        }
        let value = node
            .get("value")
            .and_then(|v| v.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let disabled = ax_property(node, "disabled").and_then(|v| v.as_bool());
        let checked = ax_property(node, "checked").map(|v| match v {
            Value::Bool(b) => b.to_string(),
            Value::String(s) => s,
            other => other.to_string(),
        });
        out.push(AxRef {
            ref_id: format!("e{generation}_{}", out.len() + 1),
            role,
            name,
            value,
            disabled,
            checked,
            backend_node_id,
        });
    }
    (out, false)
}

/// Public JSON for a ref (drops the internal backendNodeId).
fn ax_ref_public(r: &AxRef) -> Value {
    let mut o = json!({ "ref": r.ref_id, "role": r.role, "name": r.name });
    if let Some(v) = &r.value {
        o["value"] = json!(v);
    }
    if let Some(d) = r.disabled {
        o["disabled"] = json!(d);
    }
    if let Some(c) = &r.checked {
        o["checked"] = json!(c);
    }
    o
}

// --- console + network diagnostics (long-lived listener) ---------------------

/// One buffered console/log entry, stamped with the snapshot generation current
/// when it arrived (so `get_console(since_generation)` can scope to a snapshot).
#[derive(Debug, Clone, PartialEq)]
struct ConsoleEntry {
    level: String,
    text: String,
    source: String,
    url: Option<String>,
    line: Option<i64>,
    generation: u64,
}

/// One buffered network failure (HTTP >=400 or a transport error).
#[derive(Debug, Clone, PartialEq)]
struct NetFailure {
    url: String,
    status: i64,
    error: String,
    generation: u64,
}

#[derive(Default)]
struct ConsoleState {
    console: VecDeque<ConsoleEntry>,
    network: VecDeque<NetFailure>,
    /// requestId → url, so `Network.loadingFailed` (which lacks a url) can report one.
    request_urls: HashMap<String, String>,
    /// In-flight request count (req sent − finished/failed) for `network_idle` waits.
    in_flight: i64,
    /// Status of the most recent main-document response, for `navigate`'s http_status.
    last_doc_status: Option<i64>,
}

const MAX_CONSOLE: usize = 1000;
const MAX_NETFAIL: usize = 500;
const MAX_REQ_MAP: usize = 4000;

fn console_state() -> &'static Mutex<ConsoleState> {
    static STATE: OnceLock<Mutex<ConsoleState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ConsoleState::default()))
}

/// Rank a level for `min_level` filtering. error > warning > info (unknown → info).
fn level_rank(level: &str) -> u8 {
    match level {
        "error" => 3,
        "warning" => 2,
        _ => 1,
    }
}

/// Normalize a `consoleAPICalled` type / `Log` level to error|warning|info.
fn normalize_level(raw: &str) -> &'static str {
    match raw {
        "error" | "assert" => "error",
        "warning" => "warning",
        _ => "info",
    }
}

fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Join `Runtime.consoleAPICalled` args into a readable line.
fn console_args_text(args: Option<&Value>) -> String {
    let Some(args) = args.and_then(Value::as_array) else {
        return String::new();
    };
    args.iter()
        .map(|a| {
            a.get("value")
                .map(value_to_text)
                .or_else(|| {
                    a.get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a [`ConsoleEntry`] from a CDP event, or `None` if it isn't one we capture.
fn console_entry_from_event(method: &str, params: &Value, generation: u64) -> Option<ConsoleEntry> {
    match method {
        "Runtime.consoleAPICalled" => {
            let raw = params.get("type").and_then(Value::as_str).unwrap_or("log");
            let frame = params
                .get("stackTrace")
                .and_then(|s| s.get("callFrames"))
                .and_then(Value::as_array)
                .and_then(|f| f.first());
            Some(ConsoleEntry {
                level: normalize_level(raw).to_string(),
                text: console_args_text(params.get("args")),
                source: "console".into(),
                url: frame
                    .and_then(|f| f.get("url"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                line: frame.and_then(|f| f.get("lineNumber")).and_then(Value::as_i64),
                generation,
            })
        }
        "Runtime.exceptionThrown" => {
            let d = params.get("exceptionDetails")?;
            let text = d
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(Value::as_str)
                .or_else(|| d.get("text").and_then(Value::as_str))
                .unwrap_or("uncaught exception")
                .to_string();
            Some(ConsoleEntry {
                level: "error".into(),
                text,
                source: "exception".into(),
                url: d.get("url").and_then(Value::as_str).map(str::to_string),
                line: d.get("lineNumber").and_then(Value::as_i64),
                generation,
            })
        }
        "Log.entryAdded" => {
            let e = params.get("entry")?;
            Some(ConsoleEntry {
                level: normalize_level(e.get("level").and_then(Value::as_str).unwrap_or("info"))
                    .to_string(),
                text: e.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                source: e
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("log")
                    .to_string(),
                url: e.get("url").and_then(Value::as_str).map(str::to_string),
                line: e.get("lineNumber").and_then(Value::as_i64),
                generation,
            })
        }
        _ => None,
    }
}

/// A failed HTTP response (status >= 400) → a [`NetFailure`], else `None`.
fn net_failure_from_response(params: &Value, generation: u64) -> Option<NetFailure> {
    let resp = params.get("response")?;
    let status = resp.get("status").and_then(Value::as_i64).unwrap_or(0);
    if status < 400 {
        return None;
    }
    Some(NetFailure {
        url: resp
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        status,
        error: resp
            .get("statusText")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        generation,
    })
}

fn push_console(entry: ConsoleEntry) {
    let mut s = console_state().lock().expect("console state poisoned");
    s.console.push_back(entry);
    while s.console.len() > MAX_CONSOLE {
        s.console.pop_front();
    }
}

fn push_netfailure(f: NetFailure) {
    let mut s = console_state().lock().expect("console state poisoned");
    s.network.push_back(f);
    while s.network.len() > MAX_NETFAIL {
        s.network.pop_front();
    }
}

/// Dispatch one CDP event into the diagnostics buffers. Returns whether it matched
/// (handy for unit tests).
fn ingest_event(method: &str, params: &Value) -> bool {
    let generation = current_generation();
    if let Some(entry) = console_entry_from_event(method, params, generation) {
        push_console(entry);
        return true;
    }
    match method {
        "Network.requestWillBeSent" => {
            let mut s = console_state().lock().expect("console state poisoned");
            s.in_flight += 1;
            if let (Some(id), Some(url)) = (
                params.get("requestId").and_then(Value::as_str),
                params
                    .get("request")
                    .and_then(|r| r.get("url"))
                    .and_then(Value::as_str),
            ) {
                if s.request_urls.len() > MAX_REQ_MAP {
                    s.request_urls.clear();
                }
                s.request_urls.insert(id.to_string(), url.to_string());
            }
            true
        }
        "Network.responseReceived" => {
            // Capture the main-document status for `navigate`'s http_status.
            if params.get("type").and_then(Value::as_str) == Some("Document") {
                if let Some(st) = params
                    .get("response")
                    .and_then(|r| r.get("status"))
                    .and_then(Value::as_i64)
                {
                    console_state().lock().expect("console state poisoned").last_doc_status = Some(st);
                }
            }
            if let Some(f) = net_failure_from_response(params, generation) {
                push_netfailure(f);
            }
            true
        }
        "Network.loadingFinished" => {
            decrement_in_flight();
            true
        }
        "Network.loadingFailed" => {
            decrement_in_flight();
            let url = params
                .get("requestId")
                .and_then(Value::as_str)
                .and_then(|id| {
                    console_state()
                        .lock()
                        .expect("console state poisoned")
                        .request_urls
                        .get(id)
                        .cloned()
                })
                .unwrap_or_default();
            push_netfailure(NetFailure {
                url,
                status: 0,
                error: params
                    .get("errorText")
                    .and_then(Value::as_str)
                    .unwrap_or("loading failed")
                    .to_string(),
                generation,
            });
            true
        }
        _ => false,
    }
}

fn decrement_in_flight() {
    let mut s = console_state().lock().expect("console state poisoned");
    if s.in_flight > 0 {
        s.in_flight -= 1;
    }
}

/// Current in-flight request count (for `network_idle` waits).
fn in_flight_requests() -> i64 {
    console_state().lock().expect("console state poisoned").in_flight
}

/// Status of the most recent main-document response (for `navigate` http_status).
fn last_document_status() -> Option<i64> {
    console_state()
        .lock()
        .expect("console state poisoned")
        .last_doc_status
}

/// Start the diagnostics listener for the local browser once (idempotent). Runs
/// for the process lifetime, reconnecting if the CDP socket drops, so console /
/// network events are captured continuously rather than only during an op.
fn ensure_console_listener(base: &str) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let base = base.to_string();
    tokio::spawn(async move {
        loop {
            let _ = run_console_listener(&base).await;
            // socket dropped (navigation/close) — back off and reconnect.
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    });
}

/// One connect→listen pass: enable the diagnostics domains and feed every event
/// into [`ingest_event`] until the socket closes.
async fn run_console_listener(base: &str) -> Result<()> {
    let ws_url = discover_page_ws_url(base).await?;
    let (ws, _) = tokio_tungstenite::connect_async(&ws_url).await?;
    let (mut write, mut read) = ws.split();
    for (id, method) in [(1, "Runtime.enable"), (2, "Log.enable"), (3, "Network.enable")] {
        write
            .send(Message::Text(json!({ "id": id, "method": method }).to_string()))
            .await?;
    }
    while let Some(frame) = read.next().await {
        let Message::Text(txt) = frame? else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else {
            continue;
        };
        if let Some(method) = v.get("method").and_then(Value::as_str) {
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            ingest_event(method, &params);
        }
    }
    Ok(())
}

/// `get_console`: buffered console entries + network failures, filtered by
/// `min_level` (default "warning") and `since_generation` (default 0 = all).
pub(crate) async fn cdp_get_console(args: &Value) -> Result<Value> {
    let min_rank = level_rank(
        args.get("min_level")
            .and_then(Value::as_str)
            .unwrap_or("warning"),
    );
    let since = args
        .get("since_generation")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let s = console_state().lock().expect("console state poisoned");
    let entries: Vec<Value> = s
        .console
        .iter()
        .filter(|e| e.generation >= since && level_rank(&e.level) >= min_rank)
        .map(|e| {
            let mut o = json!({ "level": e.level, "text": e.text, "source": e.source });
            if let Some(u) = &e.url {
                o["url"] = json!(u);
            }
            if let Some(l) = e.line {
                o["line"] = json!(l);
            }
            o
        })
        .collect();
    let network_failures: Vec<Value> = s
        .network
        .iter()
        .filter(|f| f.generation >= since)
        .map(|f| json!({ "url": f.url, "status": f.status, "error": f.error }))
        .collect();
    Ok(json!({ "entries": entries, "network_failures": network_failures }))
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
    Ok(dir.join("last-screenshot.png"))
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

    #[test]
    fn device_metrics_none_without_both_dimensions() {
        // No viewport args → no override (the common case).
        assert!(device_metrics_params(&json!({})).is_none());
        // width without height (and vice-versa) is not enough to override.
        assert!(device_metrics_params(&json!({ "width": 375 })).is_none());
        assert!(device_metrics_params(&json!({ "height": 812 })).is_none());
    }

    #[test]
    fn device_metrics_builds_override_with_defaults_and_floors() {
        // Full args flow through; mobile honored.
        let m = device_metrics_params(&json!({
            "width": 375, "height": 812, "deviceScaleFactor": 2, "mobile": true
        }))
        .expect("override");
        assert_eq!(m["width"], 375);
        assert_eq!(m["height"], 812);
        assert_eq!(m["deviceScaleFactor"], 2.0);
        assert_eq!(m["mobile"], true);

        // Defaults: deviceScaleFactor→1.0, mobile→false. 0 dims floor to 1, and a
        // non-positive scale falls back to 1.0 (Chrome rejects 0).
        let d = device_metrics_params(&json!({ "width": 0, "height": 0, "deviceScaleFactor": 0 }))
            .expect("override");
        assert_eq!(d["width"], 1);
        assert_eq!(d["height"], 1);
        assert_eq!(d["deviceScaleFactor"], 1.0);
        assert_eq!(d["mobile"], false);
    }

    fn sample_ax_tree() -> Value {
        json!({ "nodes": [
            { "ignored": false, "role": {"value":"button"}, "name": {"value":"Submit"},
              "backendDOMNodeId": 10, "properties": [{"name":"disabled","value":{"value":false}}] },
            { "ignored": false, "role": {"value":"textbox"}, "name": {"value":""},
              "value": {"value":"hi"}, "backendDOMNodeId": 11 },
            { "ignored": false, "role": {"value":"checkbox"}, "name": {"value":"Agree"},
              "backendDOMNodeId": 12, "properties": [{"name":"checked","value":{"value":"true"}}] },
            { "ignored": true, "role": {"value":"button"}, "name": {"value":"Hidden"},
              "backendDOMNodeId": 13 },
            { "ignored": false, "role": {"value":"StaticText"}, "name": {"value":"label"},
              "backendDOMNodeId": 14 },
            { "ignored": false, "role": {"value":"generic"}, "name": {"value":""},
              "backendDOMNodeId": 15 }
        ]})
    }

    #[test]
    fn parse_ax_refs_filters_to_interactive_and_extracts_fields() {
        let (refs, truncated) = parse_ax_refs(&sample_ax_tree(), 7, true);
        assert!(!truncated);
        // button + nameless textbox (kept) + checkbox; ignored/StaticText/generic dropped.
        assert_eq!(refs.len(), 3);

        assert_eq!(refs[0].ref_id, "e7_1");
        assert_eq!(refs[0].role, "button");
        assert_eq!(refs[0].name, "Submit");
        assert_eq!(refs[0].disabled, Some(false));
        assert_eq!(refs[0].backend_node_id, 10);

        assert_eq!(refs[1].ref_id, "e7_2");
        assert_eq!(refs[1].role, "textbox");
        assert_eq!(refs[1].value.as_deref(), Some("hi"));

        assert_eq!(refs[2].ref_id, "e7_3");
        assert_eq!(refs[2].role, "checkbox");
        assert_eq!(refs[2].checked.as_deref(), Some("true"));
    }

    #[test]
    fn parse_ax_refs_full_mode_includes_noninteractive() {
        let (interactive, _) = parse_ax_refs(&sample_ax_tree(), 1, true);
        let (full, _) = parse_ax_refs(&sample_ax_tree(), 1, false);
        assert!(full.len() > interactive.len(), "full mode surfaces more nodes");
    }

    #[test]
    fn ax_ref_public_omits_backend_id_and_absent_optionals() {
        let r = AxRef {
            ref_id: "e1_1".into(),
            role: "link".into(),
            name: "Home".into(),
            value: None,
            disabled: None,
            checked: None,
            backend_node_id: 99,
        };
        let v = ax_ref_public(&r);
        assert_eq!(v["ref"], "e1_1");
        assert_eq!(v["role"], "link");
        assert_eq!(v["name"], "Home");
        assert!(v.get("value").is_none());
        assert!(v.get("disabled").is_none());
        // The internal backendNodeId is never exposed to the agent.
        assert!(v.get("backend_node_id").is_none());
        assert!(v.get("backendNodeId").is_none());
    }

    #[test]
    fn ref_registry_resolves_current_generation_and_rejects_stale() {
        let refs = vec![AxRef {
            ref_id: "e5_1".into(),
            role: "button".into(),
            name: "Go".into(),
            value: None,
            disabled: None,
            checked: None,
            backend_node_id: 42,
        }];
        let mut reg = RefRegistry {
            generation: 5,
            map: HashMap::new(),
        };
        reg.store(5, &refs);
        assert_eq!(reg.resolve("e5_1"), Some(42));
        // A ref from another generation isn't in the map → stale.
        assert_eq!(reg.resolve("e4_1"), None);

        // A store for a superseded generation is ignored (newer snapshot wins).
        reg.generation = 6;
        let other = vec![AxRef {
            ref_id: "e5_1".into(),
            role: "button".into(),
            name: "Go".into(),
            value: None,
            disabled: None,
            checked: None,
            backend_node_id: 999,
        }];
        reg.store(5, &other);
        assert_eq!(
            reg.resolve("e5_1"),
            Some(42),
            "a stale-generation store must not overwrite the current map"
        );
    }

    #[test]
    fn stale_ref_response_shape() {
        let v = stale_ref("e3_2");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "stale_ref");
        assert_eq!(v["ref"], "e3_2");
    }

    #[test]
    fn normalize_level_and_rank() {
        assert_eq!(normalize_level("error"), "error");
        assert_eq!(normalize_level("assert"), "error");
        assert_eq!(normalize_level("warning"), "warning");
        assert_eq!(normalize_level("log"), "info");
        assert_eq!(normalize_level("debug"), "info");
        assert!(level_rank("error") > level_rank("warning"));
        assert!(level_rank("warning") > level_rank("info"));
        // default min_level "warning" excludes info.
        assert!(level_rank("info") < level_rank("warning"));
    }

    #[test]
    fn console_event_parsing_covers_console_exception_and_log() {
        let e = console_entry_from_event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "error", "args": [{"type":"string","value":"boom"}],
                     "stackTrace": {"callFrames":[{"url":"http://x/app.js","lineNumber":12}]} }),
            5,
        )
        .expect("console entry");
        assert_eq!(e.level, "error");
        assert_eq!(e.text, "boom");
        assert_eq!(e.source, "console");
        assert_eq!(e.url.as_deref(), Some("http://x/app.js"));
        assert_eq!(e.line, Some(12));
        assert_eq!(e.generation, 5);

        let l = console_entry_from_event(
            "Runtime.consoleAPICalled",
            &json!({ "type": "log", "args": [{"type":"string","value":"hi"}] }),
            1,
        )
        .unwrap();
        assert_eq!(l.level, "info");

        let x = console_entry_from_event(
            "Runtime.exceptionThrown",
            &json!({ "exceptionDetails": { "exception": {"description":"TypeError: x"},
                     "url":"u", "lineNumber": 3 } }),
            2,
        )
        .unwrap();
        assert_eq!(x.level, "error");
        assert!(x.text.contains("TypeError"));
        assert_eq!(x.source, "exception");

        let log = console_entry_from_event(
            "Log.entryAdded",
            &json!({ "entry": { "source":"network", "level":"warning", "text":"slow", "url":"u" } }),
            3,
        )
        .unwrap();
        assert_eq!(log.level, "warning");
        assert_eq!(log.source, "network");

        assert!(console_entry_from_event("Page.loadEventFired", &json!({}), 1).is_none());
    }

    #[test]
    fn console_args_join_strings_and_objects() {
        let t = console_args_text(Some(&json!([
            {"type":"string","value":"count"},
            {"type":"number","value":42},
            {"type":"object","description":"[object Object]"}
        ])));
        assert!(t.contains("count"));
        assert!(t.contains("42"));
        assert!(t.contains("[object Object]"));
    }

    #[test]
    fn wait_predicate_expr_builds_safe_js() {
        assert!(
            wait_predicate_expr("selector", "#go")
                .unwrap()
                .contains("querySelector(\"#go\")")
        );
        assert!(
            wait_predicate_expr("text", "Done")
                .unwrap()
                .contains("innerText.indexOf(\"Done\")")
        );
        assert!(
            wait_predicate_expr("url", "/dash")
                .unwrap()
                .contains("location.href.indexOf(\"/dash\")")
        );
        // network_idle / unknown are handled out of band.
        assert!(wait_predicate_expr("network_idle", "").is_none());
        assert!(wait_predicate_expr("bogus", "x").is_none());
        // a quote in the arg can't break out of the expression.
        assert!(
            wait_predicate_expr("selector", "a\"]")
                .unwrap()
                .contains("\\\"")
        );
    }

    #[test]
    fn parse_url_title_reads_eval_json() {
        let r = json!({ "result": { "value": "{\"u\":\"http://x/\",\"t\":\"Hi\"}" } });
        let (u, t) = parse_url_title(&r);
        assert_eq!(u, "http://x/");
        assert_eq!(t, "Hi");
        // garbled input → empty strings, never a panic.
        let (u2, t2) = parse_url_title(&json!({}));
        assert_eq!(u2, "");
        assert_eq!(t2, "");
    }

    #[test]
    fn net_failure_only_for_4xx_5xx() {
        let f = net_failure_from_response(
            &json!({ "response": { "url":"http://x/missing.js", "status":404, "statusText":"Not Found" } }),
            7,
        )
        .expect("404 is a failure");
        assert_eq!(f.status, 404);
        assert_eq!(f.url, "http://x/missing.js");
        assert_eq!(f.error, "Not Found");
        assert_eq!(f.generation, 7);
        assert!(
            net_failure_from_response(&json!({ "response": { "url":"u", "status":200 } }), 1)
                .is_none()
        );
    }

    #[test]
    fn png_dimensions_reads_ihdr_and_rejects_non_png() {
        let mut b = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        b.extend_from_slice(&[0, 0, 0, 13]); // IHDR chunk length
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&640u32.to_be_bytes());
        b.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(png_dimensions(&b), Some((640, 480)));
        // Not a PNG (jpeg magic / too short) → None, never a panic.
        assert_eq!(png_dimensions(&[0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0]), None);
        assert_eq!(png_dimensions(&[0u8; 10]), None);
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
            let s = cdp_snapshot(&base, &json!({})).await.expect("snapshot");
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
            let s = cdp_snapshot(&base, &json!({})).await.expect("snapshot after click");
            if s.get("title").and_then(Value::as_str) == Some("clicked:Ada") {
                break; // fill + click verifiably mutated the DOM
            }
            assert!(
                Instant::now() < deadline,
                "click/fill did not mutate the DOM: {s}"
            );
            sleep(Duration::from_millis(150)).await;
        }

        // screenshot returns non-empty bytes.
        let shot = cdp_screenshot(&base, &json!({}))
            .await
            .expect("screenshot");
        assert!(
            shot.get("bytes").and_then(Value::as_u64).unwrap_or(0) > 0,
            "screenshot must be non-empty: {shot}"
        );

        // F1 — a viewport-overridden capture (responsive testing) drives
        // `Emulation.setDeviceMetricsOverride` then captures, still non-empty.
        let mobile_shot = cdp_screenshot(
            &base,
            &json!({ "width": 375, "height": 812, "deviceScaleFactor": 2 }),
        )
        .await
        .expect("viewport screenshot");
        assert!(
            mobile_shot.get("bytes").and_then(Value::as_u64).unwrap_or(0) > 0,
            "viewport screenshot must be non-empty: {mobile_shot}"
        );

        // F3/F4 — snapshot returns interactive refs; act by ref with TRUSTED input;
        // a ref from a superseded snapshot is rejected as stale.
        cdp_navigate(&base, &json!({ "url": data_url }))
            .await
            .expect("re-navigate for refs");
        let deadline = Instant::now() + Duration::from_secs(5);
        let snap = loop {
            let s = cdp_snapshot(&base, &json!({})).await.expect("ref snapshot");
            let has_refs = s
                .get("refs")
                .and_then(Value::as_array)
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            if s.get("title").and_then(Value::as_str) == Some("qa-start") && has_refs {
                break s;
            }
            assert!(Instant::now() < deadline, "snapshot never returned refs: {s}");
            sleep(Duration::from_millis(150)).await;
        };
        let refs = snap.get("refs").and_then(Value::as_array).expect("refs array");
        let ref_by = |role: Option<&str>, name: Option<&str>| -> Option<String> {
            refs.iter()
                .find(|r| {
                    role.map_or(true, |x| r.get("role").and_then(Value::as_str) == Some(x))
                        && name.map_or(true, |x| r.get("name").and_then(Value::as_str) == Some(x))
                })
                .and_then(|r| r.get("ref"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let input_ref = ref_by(Some("textbox"), None).expect("a textbox ref");
        let go_ref = ref_by(None, Some("GO")).expect("the GO button ref");

        // type by ref (trusted insertText) + click by ref (trusted mouse).
        let typed = cdp_fill(&base, &json!({ "ref": input_ref, "text": "Grace" }))
            .await
            .expect("type by ref");
        assert_eq!(
            typed.get("ok").and_then(Value::as_bool),
            Some(true),
            "type by ref ok: {typed}"
        );
        let clicked = cdp_click(&base, &json!({ "ref": go_ref }))
            .await
            .expect("click by ref");
        assert_eq!(
            clicked.get("ok").and_then(Value::as_bool),
            Some(true),
            "click by ref ok: {clicked}"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = cdp_snapshot(&base, &json!({}))
                .await
                .expect("snapshot after ref click");
            if s.get("title").and_then(Value::as_str) == Some("clicked:Grace") {
                break; // trusted ref type+click verifiably mutated the DOM
            }
            assert!(
                Instant::now() < deadline,
                "ref type/click did not mutate the DOM: {s}"
            );
            sleep(Duration::from_millis(150)).await;
        }

        // The snapshots above bumped the generation, so the original `go_ref` is
        // now stale and must be rejected (not silently acted on).
        let stale = cdp_click(&base, &json!({ "ref": go_ref }))
            .await
            .expect("stale ref click call");
        assert_eq!(
            stale.get("error").and_then(Value::as_str),
            Some("stale_ref"),
            "a superseded ref must be rejected: {stale}"
        );

        // F5 — the diagnostics listener captures a console error AND an HTTP 404.
        // Serve the page SAME-ORIGIN from a tiny HTTP server (200 for "/", 404 for
        // "/missing.js"): a `data:` page fetching loopback is CORS/PNA-blocked, so
        // the request must originate from the same loopback origin to yield a real
        // 404 rather than a transport error.
        let http = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind diag server");
        let http_port = http.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            while let Ok((mut sock, _)) = http.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let resp: Vec<u8> = if req.contains("missing") {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    } else {
                        let body = "<!doctype html><html><head><title>diag</title></head>\
                            <body><script>console.error('boom-marker');\
                            fetch('/missing.js').catch(function(){});</script></body></html>";
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .into_bytes()
                    };
                    let _ = sock.write_all(&resp).await;
                });
            }
        });
        // Start the listener and let it connect BEFORE the page (whose inline script
        // fires the console error + 404 fetch on load) is navigated to.
        ensure_console_listener(&base);
        sleep(Duration::from_millis(700)).await;
        cdp_navigate(&base, &json!({ "url": format!("http://127.0.0.1:{http_port}/") }))
            .await
            .expect("navigate to diag page");
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let diag = cdp_get_console(&json!({ "min_level": "error" }))
                .await
                .expect("get_console");
            let has_error = diag
                .get("entries")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter().any(|e| {
                        e.get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|t| t.contains("boom-marker"))
                    })
                })
                .unwrap_or(false);
            let has_404 = diag
                .get("network_failures")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .any(|f| f.get("status").and_then(Value::as_i64) == Some(404))
                })
                .unwrap_or(false);
            if has_error && has_404 {
                break; // criterion #4: JS error + 404 both captured
            }
            assert!(
                Instant::now() < deadline,
                "console error / 404 not captured: {diag}"
            );
            sleep(Duration::from_millis(200)).await;
        }

        // F6 — navigate returns final_url + title; `wait` resolves an existing
        // selector and times out on a missing one.
        let nav = cdp_navigate(&base, &json!({ "url": data_url, "wait_until": "load" }))
            .await
            .expect("navigate enriched");
        assert_eq!(nav.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            nav.get("title").and_then(Value::as_str),
            Some("qa-start"),
            "navigate returns the page title: {nav}"
        );
        assert!(
            nav.get("final_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .starts_with("data:"),
            "navigate returns the final url: {nav}"
        );
        let hit = cdp_wait(
            &base,
            &json!({ "condition": "selector", "arg": "#go", "timeout_ms": 3000 }),
        )
        .await
        .expect("wait selector");
        assert_eq!(
            hit.get("timed_out").and_then(Value::as_bool),
            Some(false),
            "an existing selector resolves: {hit}"
        );
        let miss = cdp_wait(
            &base,
            &json!({ "condition": "selector", "arg": "#nope-xyz", "timeout_ms": 600 }),
        )
        .await
        .expect("wait missing");
        assert_eq!(
            miss.get("timed_out").and_then(Value::as_bool),
            Some(true),
            "a missing selector times out: {miss}"
        );

        // F7 — screenshot is PNG with real dimensions; an element-clip by ref is no
        // wider than the full page.
        let full = cdp_screenshot(&base, &json!({}))
            .await
            .expect("full screenshot");
        assert_eq!(full.get("format").and_then(Value::as_str), Some("png"));
        let full_w = full.get("width").and_then(Value::as_u64).unwrap_or(0);
        assert!(full_w > 0, "screenshot reports a width: {full}");
        let snap2 = cdp_snapshot(&base, &json!({}))
            .await
            .expect("snapshot for clip");
        let clip_ref = snap2
            .get("refs")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|r| r.get("ref"))
            .and_then(Value::as_str)
            .expect("a ref to clip")
            .to_string();
        let clip = cdp_screenshot(&base, &json!({ "ref": clip_ref }))
            .await
            .expect("clip screenshot");
        assert_eq!(
            clip.get("ok").and_then(Value::as_bool),
            Some(true),
            "element-clip screenshot ok: {clip}"
        );
        let clip_w = clip.get("width").and_then(Value::as_u64).unwrap_or(0);
        assert!(
            clip_w > 0 && clip_w <= full_w,
            "clip width {clip_w} should be >0 and ≤ full width {full_w}: {clip}"
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
