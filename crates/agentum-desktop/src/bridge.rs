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
    // Deliver via an Image src GET (WebKit routes subresource loads to the
    // custom scheme handler; it blocks fetch()/XHR to custom schemes). The
    // payload rides in the `p` query param, so snippets are capped to keep the
    // URL bounded. Chunking would lift the cap; not needed for element metadata.
    format!(
        r#"(function(){{
  var SEL={sel}, RID={rid};
  function send(o){{o.requestId=RID;try{{var img=new Image();img.src='agentumgrab://grab/result?p='+encodeURIComponent(JSON.stringify(o));}}catch(e){{}}}}
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
        textSnippet:(el.innerText||el.textContent||'').trim().slice(0,300),
        htmlSnippet:el.outerHTML.slice(0,1200),
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

/// Build the JS injected (fire-and-forget) into a guest page to walk its
/// **interactive** elements and POST a compact snapshot back via the same
/// `agentumgrab://` channel `grab` uses. Each entry carries a stable CSS `selector`
/// the agent then drives with `click`/`fill` (the bridge acts by selector), plus
/// tag/role/text/rect for the model to choose. `request_id` is embedded as a JSON
/// string literal so it can't break out of the script.
fn snapshot_extractor_script(request_id: &str) -> String {
    let rid = serde_json::Value::String(request_id.to_string());
    format!(
        r#"(function(){{
  var RID={rid};
  function send(o){{o.requestId=RID;try{{var img=new Image();img.src='agentumgrab://grab/result?p='+encodeURIComponent(JSON.stringify(o));}}catch(e){{}}}}
  function css(el){{
    if(el.id) return '#'+CSS.escape(el.id);
    var p=[],n=el,d=0;
    while(n&&n.nodeType===1&&d<4){{
      var s=n.tagName.toLowerCase();
      if(n.classList&&n.classList.length){{s+='.'+Array.from(n.classList).slice(0,2).map(function(c){{return CSS.escape(c);}}).join('.');}}
      var par=n.parentNode;
      if(par){{var sib=Array.from(par.children).filter(function(c){{return c.tagName===n.tagName;}});if(sib.length>1){{s+=':nth-of-type('+(sib.indexOf(n)+1)+')';}}}}
      p.unshift(s); n=n.parentNode; d++;
      if(n&&n.id){{p.unshift('#'+CSS.escape(n.id));break;}}
    }}
    return p.join(' > ');
  }}
  try{{
    var SEL='a[href],button,input:not([type=hidden]),select,textarea,[role=button],[role=link],[role=checkbox],[role=tab],[role=menuitem],[contenteditable=true],[onclick],[tabindex]';
    var nodes=Array.prototype.slice.call(document.querySelectorAll(SEL)).slice(0,250);
    var refs=[];
    nodes.forEach(function(el,i){{
      var r=el.getBoundingClientRect();
      if(r.width<=0||r.height<=0) return;
      var label=(el.getAttribute('aria-label')||el.value||el.innerText||el.textContent||el.getAttribute('placeholder')||el.title||'').replace(/\s+/g,' ').trim().slice(0,120);
      refs.push({{ref:'e'+i,tag:el.tagName.toLowerCase(),type:el.getAttribute('type')||'',role:el.getAttribute('role')||'',text:label,selector:css(el),rect:{{x:Math.round(r.x),y:Math.round(r.y),width:Math.round(r.width),height:Math.round(r.height)}}}});
    }});
    send({{payload:{{url:location.href,title:document.title,viewport:{{width:innerWidth,height:innerHeight}},refs:refs}}}});
  }}catch(e){{send({{error:String(e)}});}}
}})();"#,
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
        // which is what an agent that doesn't track worktree ids wants. Accept the
        // snake_case `worktree_id` and the MCP-injected camelCase `worktreeId`.
        let mut payload = json!({ "requestId": request_id, "url": url });
        if let Some(wt) = args
            .get("worktree_id")
            .or_else(|| args.get("worktreeId"))
            .and_then(Value::as_str)
        {
            payload["worktreeId"] = json!(wt);
        }
        // `split` (left|right|up|down) opens the browser BESIDE the agent as a layout
        // split instead of a stacked tab; optional `ratio` (0–1) sizes it. Forwarded
        // verbatim so the renderer can split the group before placing the tab.
        if let Some(split) = args.get("split").and_then(Value::as_str) {
            payload["split"] = json!(split);
        }
        if let Some(ratio) = args.get("ratio").and_then(Value::as_f64) {
            payload["ratio"] = json!(ratio);
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
                Err(anyhow::anyhow!(
                    "timed out waiting for the renderer (op: {event})"
                ))
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
        wv.eval(grab_extractor_script(selector, &request_id))
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

    /// Snapshot the active tab's INTERACTIVE elements: eval the extractor into the
    /// guest (fire-and-forget) that POSTs the refs back via `agentumgrab://`, and
    /// await that callback — the headed analogue of CDP `snapshot`, reusing the same
    /// verified channel as `grab`. Returns `{url,title,viewport,refs:[{ref,tag,role,
    /// text,selector,rect}]}`; the agent then `click`/`fill`s by the `selector`.
    async fn snapshot_page(&self, args: &Value) -> anyhow::Result<Value> {
        let registry = self
            .app
            .try_state::<std::sync::Arc<GrabRegistry>>()
            .ok_or_else(|| anyhow::anyhow!("grab registry unavailable"))?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let rx = registry.register(request_id.clone());

        let wv = self
            .pick_webview(args)
            .ok_or_else(|| anyhow::anyhow!("no browser tab open"))?;
        wv.eval(snapshot_extractor_script(&request_id))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        match tokio::time::timeout(OPEN_TAB_TIMEOUT, rx).await {
            Ok(Ok(Ok(payload))) => Ok(payload),
            Ok(Ok(Err(msg))) => Err(anyhow::anyhow!(msg)),
            Ok(Err(_dropped)) => Err(anyhow::anyhow!("snapshot reply channel dropped")),
            Err(_elapsed) => {
                registry.cancel(&request_id);
                Err(anyhow::anyhow!(
                    "timed out waiting for the snapshot (page may block the agentumgrab scheme)"
                ))
            }
        }
    }

    /// Add an annotation programmatically: grab the element by selector (reusing
    /// the verified extraction channel) and hand the payload + comment + intent
    /// to the renderer, which owns the annotation store, so it shows in the tray
    /// and the `annotations` read returns it.
    async fn annotate_element(&self, args: &Value) -> anyhow::Result<Value> {
        let comment = args
            .get("comment")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing `comment`"))?;
        // Extract the target first (errors here if the selector matches nothing).
        let payload = self.grab_element(args).await?;
        let intent = args
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or("change");
        let mut req = json!({
            "comment": comment,
            "intent": intent,
            "payload": payload,
        });
        if let Some(tab) = args.get("tab") {
            req["tab"] = tab.clone();
        }
        self.renderer_op("ui-request-browser-annotate", req).await
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
            // `snapshot` is async (round-trips the guest via `agentumgrab://`) so it's
            // handled in `browser()` before reaching here — see `snapshot_page`.
            "screenshot" => Ok(json!({
                "error": "A screenshot of the VISIBLE in-app browser isn't available yet — pass \
                          headless:true to screenshot a hidden CDP browser instead.",
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
    app: &AppHandle,
    registry: &GrabRegistry,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    // The extractor/in-page UI deliver via `new Image().src='agentumgrab://…?p=<json>'`
    // (WebKit routes subresource loads to the scheme handler but blocks fetch to
    // custom schemes), so the payload is in the URL query, not a body.
    let uri = request.uri().to_string();
    let query = request.uri().query().unwrap_or("");
    let raw = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("p="))
        .unwrap_or("");
    let decoded = percent_decode(raw);
    if let Ok(v) = serde_json::from_str::<Value>(&decoded) {
        if uri.contains("annotation/add") {
            // User-initiated in-page annotation → hand to the renderer to add to
            // its store (it shows in the tray and `annotations` returns it).
            let _ = app.emit_to("main", "browser-inpage-annotation", v);
        } else if let Some(id) = v.get("requestId").and_then(Value::as_str) {
            // Request/response for a bridge `grab`.
            if let Some(err) = v.get("error").and_then(Value::as_str) {
                registry.resolve(id, Err(err.to_string()));
            } else if let Some(payload) = v.get("payload") {
                registry.resolve(id, Ok(payload.clone()));
            }
        }
    }
    // A 1x1 transparent GIF so the Image load resolves cleanly (the data was
    // already captured from the URL).
    const GIF_1PX: &[u8] = &[
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    ];
    let resp = tauri::http::Response::builder()
        .status(200)
        .header("content-type", "image/gif")
        .body(std::borrow::Cow::Borrowed(GIF_1PX))
        .unwrap();
    responder.respond(resp);
}

/// Decode `%XX` percent-escapes (what `encodeURIComponent` emits). Sufficient
/// for the grab payload's JSON; unknown escapes pass through unchanged.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
                "annotations" => {
                    return self.renderer_op("ui-request-browser-annotations", op).await
                }
                "grab" => return self.grab_element(&op).await,
                "annotate" => return self.annotate_element(&op).await,
                "snapshot" => return self.snapshot_page(&op).await,
                // Resize the layout split holding the worktree's browser pane — a
                // renderer-owned layout op (`ratio` 0–1). Round-trips like the others.
                "set_split_ratio" => return self.renderer_op("ui-request-split-ratio", op).await,
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
    use super::{snapshot_extractor_script, TabCreateRegistry};

    #[test]
    fn snapshot_extractor_beacons_interactive_refs_with_the_request_id() {
        let js = snapshot_extractor_script("req-xyz");
        // Rides the same agentumgrab:// channel as grab, keyed by the request id.
        assert!(js.contains("agentumgrab://grab/result"));
        assert!(
            js.contains("\"req-xyz\""),
            "request id must be embedded: {js}"
        );
        // Collects interactive elements + a stable selector the agent drives by.
        assert!(js.contains("querySelectorAll"));
        assert!(js.contains("button"));
        assert!(js.contains("selector:css(el)"));
        assert!(js.contains("refs:refs"));
    }

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
