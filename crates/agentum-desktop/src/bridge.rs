//! `TauriBridge` — the desktop-side implementation of
//! `agentum_server::bridge::DesktopBridge`. It lets the embedded server reach
//! the two things only this process can do: drive the browser-pane webviews
//! (it owns them) and run the macOS computer-use engine (it holds the
//! Accessibility grant). Installed via `serve_embedded_loopback_with_bridge`.

use std::collections::HashMap;
use std::sync::Mutex;

use agentum_server::bridge::{BridgeFuture, DesktopBridge};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

/// Browser webviews created by `commands::browser_native` use this label prefix.
const BROWSER_LABEL_PREFIX: &str = "browser-page-";

/// How long the `open` op waits for the renderer to create the tab and reply
/// before giving up — a renderer that never answers (no active worktree, a
/// remote runtime active) must not hang the MCP/HTTP caller forever.
const OPEN_TAB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Pending main-initiated browser-tab-create requests, keyed by request id.
/// The bridge emits `ui-request-tab-create` to the renderer and parks a oneshot
/// here; the renderer answers via the `ui_reply_tab_create` command (see
/// `commands::ui`), which resolves the matching oneshot with the new page id (or
/// an error). Tauri-managed so both the bridge and the command can reach it.
/// The React half of this protocol already existed (`useIpcEvents`'s
/// `onRequestTabCreate`/`replyTabCreate`); only the Rust side was unported, which
/// is why agents could drive existing tabs but never open one.
#[derive(Default)]
pub struct TabCreateRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<String, String>>>>,
}

impl TabCreateRegistry {
    fn register(&self, id: String) -> oneshot::Receiver<Result<String, String>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        rx
    }

    /// Resolve a pending request (called from the `ui_reply_tab_create` command).
    /// A missing id is a no-op: the request may have already timed out.
    pub fn resolve(&self, id: &str, result: Result<String, String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(result);
        }
    }

    fn cancel(&self, id: &str) {
        self.pending.lock().unwrap().remove(id);
    }
}

/// Pending renderer round-trips for the browser annotation/grab ops
/// (`annotations` / `grab` / `annotate`), keyed by request id. Same shape as
/// [`TabCreateRegistry`] but the reply carries an arbitrary JSON value (the
/// annotation list, a grabbed element payload, …) rather than just a page id.
/// Kept separate from tab-create so the verified `open` path is untouched.
#[derive(Default)]
pub struct BrowserOpRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
}

impl BrowserOpRegistry {
    fn register(&self, id: String) -> oneshot::Receiver<Result<Value, String>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        rx
    }

    /// Resolve a pending op (called from the `ui_reply_browser_op` command).
    pub fn resolve(&self, id: &str, result: Result<Value, String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(result);
        }
    }

    fn cancel(&self, id: &str) {
        self.pending.lock().unwrap().remove(id);
    }
}

/// Pending `grab` requests, keyed by request id. Tauri webview `eval` is
/// fire-and-forget and Tauri injects no IPC into external pages, so the bridge
/// can't read a value back from the guest directly. Instead the injected
/// extractor script POSTs its result to the `agentumgrab://` custom scheme
/// (an app scheme — not mixed content, unlike a loopback `http://` fetch from
/// an HTTPS page), whose handler (registered in `lib.rs`) resolves the matching
/// oneshot here. Shared `Arc` between the scheme handler and the bridge.
#[derive(Default)]
pub struct GrabRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
}

impl GrabRegistry {
    fn register(&self, id: String) -> oneshot::Receiver<Result<Value, String>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        rx
    }

    /// Resolve a pending grab (called from the `agentumgrab://` scheme handler).
    pub fn resolve(&self, id: &str, result: Result<Value, String>) {
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(result);
        }
    }

    fn cancel(&self, id: &str) {
        self.pending.lock().unwrap().remove(id);
    }
}

/// Build the JS injected (fire-and-forget) into a guest page to extract the
/// element matching `selector` and POST it back via the `agentumgrab://` scheme.
/// `selector` and `request_id` are embedded as JSON string literals so arbitrary
/// values can't break out of the script.
fn grab_extractor_script(selector: &str, request_id: &str) -> String {
    let sel = serde_json::Value::String(selector.to_string());
    let rid = serde_json::Value::String(request_id.to_string());
    format!(
        r#"(function(){{
  var SEL={sel}, RID={rid};
  function send(o){{o.requestId=RID;try{{fetch('agentumgrab://grab/result',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify(o)}}).catch(function(){{}});}}catch(e){{}}}}
  try{{
    var el=document.querySelector(SEL);
    if(!el){{send({{error:'no element matches selector '+SEL}});return;}}
    var r=el.getBoundingClientRect(), cs=getComputedStyle(el);
    var attrs={{}};for(var i=0;i<el.attributes.length;i++){{attrs[el.attributes[i].name]=el.attributes[i].value;}}
    var pick=function(k){{return cs.getPropertyValue(k)||'';}};
    send({{payload:{{
      page:{{url:location.href,title:document.title,viewport:{{width:innerWidth,height:innerHeight}},scrollX:scrollX,scrollY:scrollY,devicePixelRatio:devicePixelRatio}},
      target:{{
        tagName:el.tagName.toLowerCase(),selector:SEL,
        textSnippet:(el.innerText||el.textContent||'').trim().slice(0,600),
        htmlSnippet:el.outerHTML.slice(0,4000),
        cssClasses:el.className&&el.className.toString?el.className.toString():'',
        attributes:attrs,
        accessibility:{{role:el.getAttribute('role')||'',accessibleName:el.getAttribute('aria-label')||el.getAttribute('alt')||el.title||(el.innerText||'').trim().slice(0,120)}},
        rectViewport:{{x:r.x,y:r.y,width:r.width,height:r.height}},
        rectPage:{{x:r.x+scrollX,y:r.y+scrollY,width:r.width,height:r.height}},
        computedStyles:{{display:pick('display'),position:pick('position'),color:pick('color'),backgroundColor:pick('background-color'),borderRadius:pick('border-radius'),fontFamily:pick('font-family'),fontSize:pick('font-size'),fontWeight:pick('font-weight'),lineHeight:pick('line-height'),textAlign:pick('text-align'),zIndex:pick('z-index')}}
      }},
      nearbyText:[],ancestorPath:[],screenshot:null
    }}}});
  }}catch(e){{send({{error:String(e)}});}}
}})();"#,
        sel = sel,
        rid = rid
    )
}

pub struct TauriBridge {
    app: AppHandle,
}

impl TauriBridge {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn op_name(args: &Value) -> String {
        args.get("op")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
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

    /// Open a NEW browser tab navigated to `url`, by asking the renderer (which
    /// owns the tab list + webview lifecycle) to create it and report back the
    /// new page id. Unlike the other ops this can't be done from the bridge
    /// alone: a webview created here would be an orphan the React UI never tracks
    /// (no chrome, no tab entry). So we drive the renderer's existing
    /// tab-create protocol and await its reply. The renderer mounts the webview
    /// for automation via its bootstrap lease without yanking the user's view.
    async fn open_tab(&self, args: &Value) -> anyhow::Result<Value> {
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `url`"))?;
        // Reject a bad url here rather than letting it fail silently in the
        // renderer, where the caller would only see a timeout.
        let _: tauri::Url = url
            .parse()
            .map_err(|e| anyhow::anyhow!("bad url `{url}`: {e}"))?;

        let registry = self
            .app
            .try_state::<TabCreateRegistry>()
            .ok_or_else(|| anyhow::anyhow!("tab-create registry unavailable"))?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let rx = registry.register(request_id.clone());

        // `worktreeId` is optional — the renderer defaults to the active worktree,
        // which is what an agent that doesn't track worktree ids wants.
        let mut payload = json!({ "requestId": request_id, "url": url });
        if let Some(wt) = args.get("worktree_id").and_then(Value::as_str) {
            payload["worktreeId"] = json!(wt);
        }
        self.app
            .emit_to("main", "ui-request-tab-create", payload)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        match tokio::time::timeout(OPEN_TAB_TIMEOUT, rx).await {
            Ok(Ok(Ok(page_id))) => Ok(json!({ "ok": true, "tab": page_id, "url": url })),
            Ok(Ok(Err(msg))) => Err(anyhow::anyhow!(msg)),
            Ok(Err(_dropped)) => Err(anyhow::anyhow!("tab-create reply channel dropped")),
            Err(_elapsed) => {
                registry.cancel(&request_id);
                Err(anyhow::anyhow!(
                    "timed out waiting for the renderer to open a browser tab"
                ))
            }
        }
    }

    /// Round-trip a browser op to the renderer (which owns the annotation store
    /// and the webview guest) and await its reply. The renderer listens for
    /// `event`, does the work, and answers via the `ui_reply_browser_op` command
    /// keyed by the `requestId` we inject here. Used by the annotate/grab ops
    /// that can't be served from Rust alone.
    async fn renderer_op(&self, event: &str, mut payload: Value) -> anyhow::Result<Value> {
        let registry = self
            .app
            .try_state::<BrowserOpRegistry>()
            .ok_or_else(|| anyhow::anyhow!("browser-op registry unavailable"))?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let rx = registry.register(request_id.clone());
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("requestId".into(), json!(request_id));
        }
        self.app
            .emit_to("main", event, payload)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        match tokio::time::timeout(OPEN_TAB_TIMEOUT, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(msg))) => Err(anyhow::anyhow!(msg)),
            Ok(Err(_dropped)) => Err(anyhow::anyhow!("browser-op reply channel dropped")),
            Err(_elapsed) => {
                registry.cancel(&request_id);
                Err(anyhow::anyhow!("timed out waiting for the renderer (op: {event})"))
            }
        }
    }

    /// Grab the element matching `selector` on a tab: eval an extractor into the
    /// guest (fire-and-forget) that POSTs the element payload back via the
    /// `agentumgrab://` scheme, and await that callback keyed by request id.
    async fn grab_element(&self, args: &Value) -> anyhow::Result<Value> {
        let selector = args
            .get("selector")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `selector`"))?;
        let registry = self
            .app
            .try_state::<std::sync::Arc<GrabRegistry>>()
            .ok_or_else(|| anyhow::anyhow!("grab registry unavailable"))?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let rx = registry.register(request_id.clone());

        let wv = self
            .pick_webview(args)
            .ok_or_else(|| anyhow::anyhow!("no browser tab open"))?;
        wv.eval(&grab_extractor_script(selector, &request_id))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        match tokio::time::timeout(OPEN_TAB_TIMEOUT, rx).await {
            Ok(Ok(Ok(payload))) => Ok(payload),
            Ok(Ok(Err(msg))) => Err(anyhow::anyhow!(msg)),
            Ok(Err(_dropped)) => Err(anyhow::anyhow!("grab reply channel dropped")),
            Err(_elapsed) => {
                registry.cancel(&request_id);
                Err(anyhow::anyhow!(
                    "timed out waiting for the grab result (page may block the agentumgrab scheme)"
                ))
            }
        }
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
                wv.navigate(parsed)
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
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

/// Handle a request to the `agentumgrab://` scheme — the channel the injected
/// grab extractor uses to return its result. Parses the JSON body
/// (`{requestId, payload | error}`) and resolves the matching pending grab.
/// Always answers with permissive CORS so the guest `fetch` (cross-origin from
/// the page's perspective) isn't blocked.
pub fn handle_grab_scheme(
    registry: &GrabRegistry,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let ack = |status: u16| -> tauri::http::Response<std::borrow::Cow<'static, [u8]>> {
        tauri::http::Response::builder()
            .status(status)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "POST, OPTIONS")
            .header("Access-Control-Allow-Headers", "content-type")
            .body(std::borrow::Cow::Owned(Vec::new()))
            .unwrap()
    };
    // CORS preflight for the guest's POST.
    if request.method() == tauri::http::Method::OPTIONS {
        responder.respond(ack(204));
        return;
    }
    if let Ok(v) = serde_json::from_slice::<Value>(request.body()) {
        if let Some(id) = v.get("requestId").and_then(Value::as_str) {
            if let Some(err) = v.get("error").and_then(Value::as_str) {
                registry.resolve(id, Err(err.to_string()));
            } else if let Some(payload) = v.get("payload") {
                registry.resolve(id, Ok(payload.clone()));
            }
        }
    }
    responder.respond(ack(200));
}

impl DesktopBridge for TauriBridge {
    fn browser(&self, op: Value) -> BridgeFuture<'_> {
        Box::pin(async move {
            let name = Self::op_name(&op);
            // `open` creates a tab and must round-trip through the renderer
            // (async); the rest act on existing webviews synchronously.
            if name == "open" {
                return self.open_tab(&op).await;
            }
            // The annotation/grab ops live in the renderer's store + webview
            // guest, so they round-trip there too. `op` is forwarded verbatim;
            // the renderer reads `tab`/`selector`/`comment`/`intent` as needed.
            match name.as_str() {
                "annotations" => return self.renderer_op("ui-request-browser-annotations", op).await,
                "grab" => return self.grab_element(&op).await,
                "annotate" => return self.renderer_op("ui-request-browser-annotate", op).await,
                _ => {}
            }
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

#[cfg(test)]
mod tests {
    use super::TabCreateRegistry;

    // The end-to-end `open` op needs a live webview (covered manually); here we
    // pin the registry plumbing the renderer reply rides on.
    #[tokio::test]
    async fn resolve_delivers_the_reply_to_the_waiter() {
        let reg = TabCreateRegistry::default();
        let rx = reg.register("req-1".into());
        reg.resolve("req-1", Ok("browser-page-abc".into()));
        assert_eq!(rx.await.unwrap(), Ok("browser-page-abc".to_string()));
    }

    #[tokio::test]
    async fn resolve_propagates_an_error_reply() {
        let reg = TabCreateRegistry::default();
        let rx = reg.register("req-2".into());
        reg.resolve("req-2", Err("no active worktree".into()));
        assert_eq!(rx.await.unwrap(), Err("no active worktree".to_string()));
    }

    #[tokio::test]
    async fn cancel_drops_the_waiter_so_a_late_reply_is_a_noop() {
        let reg = TabCreateRegistry::default();
        let rx = reg.register("req-3".into());
        reg.cancel("req-3");
        // The sender was dropped on cancel → the receiver errors rather than hangs.
        assert!(rx.await.is_err());
        // A reply that lands after cancel (e.g. post-timeout) must not panic.
        reg.resolve("req-3", Ok("late".into()));
    }
}
