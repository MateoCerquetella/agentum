//! `TauriBridge` — the desktop-side implementation of
//! `agentum_server::bridge::DesktopBridge`. It lets the embedded server reach
//! the two things only this process can do: drive the browser-pane webviews
//! (it owns them) and run the macOS computer-use engine (it holds the
//! Accessibility grant). Installed via `serve_embedded_loopback_with_bridge`.

use agentum_server::bridge::{BridgeFuture, DesktopBridge};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

/// Browser webviews created by `commands::browser_native` use this label prefix.
const BROWSER_LABEL_PREFIX: &str = "browser-page-";

pub struct TauriBridge {
    app: AppHandle,
}

impl TauriBridge {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn op_name(args: &Value) -> String {
        args.get("op").and_then(Value::as_str).unwrap_or("").to_string()
    }

    /// The browser webviews, as (label, current-url) pairs.
    fn browser_tabs(&self) -> Vec<(String, String)> {
        self.app
            .webviews()
            .into_iter()
            .filter(|(label, _)| label.starts_with(BROWSER_LABEL_PREFIX))
            .map(|(label, wv)| {
                let url = wv.url().map(|u| u.to_string()).unwrap_or_default();
                (label, url)
            })
            .collect()
    }

    /// Resolve which browser webview to act on: an explicit `tab` label (exact
    /// or prefix-suffix match), else the only/first browser webview.
    fn pick_webview(&self, args: &Value) -> Option<tauri::Webview> {
        let want = args.get("tab").and_then(Value::as_str);
        let mut candidates: Vec<(String, tauri::Webview)> = self
            .app
            .webviews()
            .into_iter()
            .filter(|(label, _)| label.starts_with(BROWSER_LABEL_PREFIX))
            .collect();
        if let Some(w) = want {
            return candidates
                .into_iter()
                .find(|(label, _)| label == w || label.ends_with(w))
                .map(|(_, wv)| wv);
        }
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        candidates.into_iter().next().map(|(_, wv)| wv)
    }

    fn browser_sync(&self, op: &str, args: &Value) -> anyhow::Result<Value> {
        match op {
            "tabs" => {
                let tabs: Vec<Value> = self
                    .browser_tabs()
                    .into_iter()
                    .map(|(label, url)| json!({ "tab": label, "url": url }))
                    .collect();
                Ok(json!({ "tabs": tabs }))
            }
            "navigate" => {
                let url = args
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("missing `url`"))?;
                let wv = self
                    .pick_webview(args)
                    .ok_or_else(|| anyhow::anyhow!("no browser tab open"))?;
                let parsed: tauri::Url = url
                    .parse()
                    .map_err(|e| anyhow::anyhow!("bad url `{url}`: {e}"))?;
                wv.navigate(parsed).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                Ok(json!({ "ok": true }))
            }
            "click" => {
                let sel = args
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("missing `selector`"))?;
                let wv = self
                    .pick_webview(args)
                    .ok_or_else(|| anyhow::anyhow!("no browser tab open"))?;
                // Fire-and-forget JS click — no result channel needed.
                let js = format!(
                    "(function(){{var e=document.querySelector({sel}); if(e){{e.click();}}}})()",
                    sel = js_string(sel)
                );
                wv.eval(&js).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                Ok(json!({ "ok": true, "selector": sel }))
            }
            "fill" => {
                let sel = args
                    .get("selector")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("missing `selector`"))?;
                let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                let wv = self
                    .pick_webview(args)
                    .ok_or_else(|| anyhow::anyhow!("no browser tab open"))?;
                let js = format!(
                    "(function(){{var e=document.querySelector({sel}); if(e){{e.focus(); e.value={val}; \
                     e.dispatchEvent(new Event('input',{{bubbles:true}})); \
                     e.dispatchEvent(new Event('change',{{bubbles:true}}));}}}})()",
                    sel = js_string(sel),
                    val = js_string(text)
                );
                wv.eval(&js).map_err(|e| anyhow::anyhow!(e.to_string()))?;
                Ok(json!({ "ok": true }))
            }
            "snapshot" => {
                // A DOM snapshot needs a value back from page JS; Tauri's eval is
                // fire-and-forget, so a full DOM read needs an injected page
                // bridge (future work). Return what we can read natively so the
                // op is honest rather than silently empty.
                let wv = self
                    .pick_webview(args)
                    .ok_or_else(|| anyhow::anyhow!("no browser tab open"))?;
                Ok(json!({
                    "url": wv.url().map(|u| u.to_string()).unwrap_or_default(),
                    "note": "DOM snapshot not available in this build; navigate/click/fill by selector work",
                }))
            }
            "screenshot" => Ok(json!({
                "error": "browser screenshot not implemented in this build",
            })),
            other => Ok(json!({ "error": format!("unsupported browser op: {other}") })),
        }
    }
}

/// JSON-encode a string as a JS string literal (safe for embedding in eval'd JS).
fn js_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

impl DesktopBridge for TauriBridge {
    fn browser(&self, op: Value) -> BridgeFuture<'_> {
        Box::pin(async move {
            let name = Self::op_name(&op);
            self.browser_sync(&name, &op)
        })
    }

    fn computer(&self, op: Value) -> BridgeFuture<'_> {
        Box::pin(async move {
            let name = Self::op_name(&op);
            // The AX/CGEvent calls are blocking FFI — keep them off the async
            // runtime's worker threads.
            tokio::task::spawn_blocking(move || crate::computer::handle(&name, &op))
                .await
                .map_err(|e| anyhow::anyhow!("computer task join error: {e}"))?
        })
    }
}
