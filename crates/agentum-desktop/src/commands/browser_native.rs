use serde::{Deserialize, Serialize};
use tauri::webview::PageLoadEvent;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Rect, Size, Url,
    WebviewUrl,
};

// Native in-window browser: each Agentum browser page is one Tauri child
// webview overlaid on the main window at the bounds the React pane reports.
// This replaces the Electron `<webview>` tag the UI was originally written
// against (Tauri/WKWebView has no such element, which left the pane blank) and
// the remote screencast runtime (which has no backend in this port).

const LABEL_PREFIX: &str = "browser-page-";

/// User-agent for the browser-pane webviews. Without an explicit UA, macOS
/// WKWebView reports a bare WebKit build with no `Version/…Safari` tokens, which
/// Google (and other UA sniffers) misidentify as the old Mail.app webview
/// ("Apple Mail 13") and then serve a degraded page. The engine really is
/// WebKit, so we present an honest, current Safari UA rather than spoofing
/// Chrome (which would invite Chrome-only code paths WebKit can't run). A true
/// Chromium engine would require the host-resident-browser route, not a UA swap.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

fn webview_label(browser_page_id: &str) -> String {
    // Tauri labels only allow [a-zA-Z0-9-/:_]; page ids are uuid-ish but coerce
    // anything else rather than erroring at create time.
    let safe: String = browser_page_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{LABEL_PREFIX}{safe}")
}

/// Stable 16-byte WKWebView data-store id for a worktree, so each worktree's
/// native browser keeps its OWN cookies / logins / storage instead of sharing
/// one global session (the root cause of "worktree B shows worktree A's
/// browser"). Derived from the worktree id via SHA-256 (first 16 bytes) so it's
/// deterministic across launches. All browser tabs in the same worktree share
/// this store; a different worktree gets a different one.
fn worktree_data_store_id(worktree_id: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(worktree_id.as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct BrowserWebviewBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BrowserWebviewBounds {
    fn rect(&self) -> Rect {
        Rect {
            position: Position::Logical(LogicalPosition::new(self.x, self.y)),
            size: Size::Logical(LogicalSize::new(self.width.max(1.0), self.height.max(1.0))),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserPageLoadEvent {
    browser_page_id: String,
    event: &'static str,
    url: String,
}

fn parse_url(url: &str) -> Result<Url, String> {
    url.parse::<Url>().map_err(|e| format!("invalid url: {e}"))
}

fn get_browser_webview(app: &AppHandle, browser_page_id: &str) -> Option<tauri::Webview> {
    app.get_webview(&webview_label(browser_page_id))
}

/// Pin the native browser webview to the display's true backing scale so it
/// renders at device resolution (sharp) on Retina. A WKWebView whose
/// configuration carries a custom URL-scheme handler — which ours do (the
/// app-wide `agentumgrab` scheme, alongside the shipped `tauri://` main webview)
/// — can report `deviceScaleFactor = 1` on a HiDPI display, so WebKit rasterizes
/// the WHOLE page (text + graphics) at 1× and macOS upscales it 2× → blurry. The
/// stable `_setOverrideDeviceScaleFactor:` SPI (used by Electron, Playwright, and
/// WebKit's own test harness since macOS 10.11) sets WebKit's internal device
/// scale — the authoritative source of `window.devicePixelRatio` — which the
/// public `layer.contentsScale` cannot. A no-op when the scale is already
/// correct, so it is safe to apply unconditionally.
///
/// The scale is read from the webview's OWN `NSWindow.backingScaleFactor` (the
/// authoritative, per-display value) rather than trusting the passed
/// `fallback_scale` — which comes from Tauri's `window.scale_factor()` and can
/// report 1 on the shipped build (the same unreliable source the screencast pane
/// already had to abandon). `fallback_scale` is used only before the view is in a
/// window (early create); the on-load re-pin then reads the real value.
pub(crate) fn force_device_scale(webview: &tauri::Webview, fallback_scale: f64) {
    // NO-OP (native rendering). WKWebView handles HiDPI natively: its layer's
    // contentsScale follows the window's backing scale, so it rasterizes at device
    // resolution AND lays out at the frame's point size — sharp and reflowing to the
    // pane, "like any browser". The `_setOverrideDeviceScaleFactor:` override that
    // v0.32.0 added (to chase a reported dpr=1) instead made the shipped child
    // webview render compressed/chunky. Removing the override is the fix the user
    // asked for ("resolución nativa, que se adapte al size"). The call sites are
    // kept so the override can be reinstated behind a flag if a real dpr=1 case
    // resurfaces, but by default we let WebKit do what it already does correctly.
    let _ = (webview, fallback_scale);
}

/// The main window's backing scale factor (2.0 on Retina; 1.0 on a standard
/// display), or 2.0 if it can't be read — the scale we pin the browser webview to.
fn main_window_scale(app: &AppHandle) -> f64 {
    app.get_window("main")
        .and_then(|w| w.scale_factor().ok())
        .filter(|s| *s > 0.0)
        .unwrap_or(2.0)
}

/// Create (or reveal) the native webview for a browser page at the given
/// window-relative logical bounds, navigated to `url`.
#[tauri::command]
pub fn browser_webview_open(
    app: AppHandle,
    browser_page_id: String,
    worktree_id: String,
    url: String,
    bounds: BrowserWebviewBounds,
) -> Result<(), String> {
    let parsed = parse_url(&url)?;
    if let Some(webview) = get_browser_webview(&app, &browser_page_id) {
        webview
            .set_bounds(bounds.rect())
            .map_err(|e| e.to_string())?;
        let _ = webview.show();
        // Re-pin device scale on reveal (e.g. the window moved to a display with
        // a different backing scale while this tab was hidden).
        force_device_scale(&webview, main_window_scale(&app));
        return Ok(());
    }

    let window = app
        .get_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let label = webview_label(&browser_page_id);
    let event_page_id = browser_page_id.clone();
    // Per-worktree session: this worktree's own WKWebView data store, so its
    // browser doesn't share cookies / logins / storage with other worktrees.
    let data_store_id = worktree_data_store_id(&worktree_id);

    // Webview creation must run on the main thread on macOS; commands execute
    // on the async runtime, so hop over and relay the result back.
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    app.run_on_main_thread(move || {
        let builder = tauri::webview::WebviewBuilder::new(&label, WebviewUrl::External(parsed))
            .user_agent(BROWSER_USER_AGENT)
            // Each worktree gets its OWN data store (macOS/iOS >= 14/17; a no-op
            // on other OSes) so two worktrees never share one browser session.
            .data_store_identifier(data_store_id)
            .on_page_load(move |webview, payload| {
                let event = match payload.event() {
                    PageLoadEvent::Started => "started",
                    PageLoadEvent::Finished => "finished",
                };
                // Re-pin the device scale once the page has loaded. A WKWebView with
                // a custom URL-scheme handler resets to deviceScaleFactor=1 on each
                // navigation, so the create-time pin is undone by the time the page
                // paints → blurry/chunky on Retina, every load, on every display.
                // Re-applying on Finished (the view is in its window by now, so
                // force_device_scale reads the real backing scale) is what actually
                // makes the in-app browser render at device resolution.
                if matches!(payload.event(), PageLoadEvent::Finished) {
                    force_device_scale(&webview, 2.0);
                }
                let _ = webview.app_handle().emit_to(
                    "main",
                    "browser-page-load",
                    BrowserPageLoadEvent {
                        browser_page_id: event_page_id.clone(),
                        event,
                        url: payload.url().to_string(),
                    },
                );
            });
        let result = window
            .add_child(
                builder,
                LogicalPosition::new(bounds.x, bounds.y),
                LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0)),
            )
            .map(|_| ())
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())??;

    // Pin the freshly-created webview to the display's true backing scale so it
    // renders sharp on Retina (see `force_device_scale`).
    if let Some(child) = get_browser_webview(&app, &browser_page_id) {
        force_device_scale(&child, main_window_scale(&app));
    }

    // macOS WKWebView child webviews added via `add_child` can stay black until
    // a relayout pass forces them to composite — on first paint the view exists,
    // loads, and is scriptable, but the window shows only the dark pane beneath.
    // Force one bounds nudge shortly after create (a brief off-thread delay so
    // the view is in the hierarchy, then hop back to the main thread): bump the
    // height by 1px and set it straight back. The React pane's bounds poll keeps
    // it glued afterwards; this is purely the kick that makes the surface paint.
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        let page_id = browser_page_id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(60));
            let app_main = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(webview) = get_browser_webview(&app_main, &page_id) {
                    let nudged = BrowserWebviewBounds {
                        height: bounds.height + 1.0,
                        ..bounds
                    };
                    let _ = webview.set_bounds(nudged.rect());
                    let _ = webview.set_bounds(bounds.rect());
                    // Re-pin device scale after the relayout settles — covers the
                    // case where the webview was created before the window's
                    // backing scale was established.
                    force_device_scale(&webview, main_window_scale(&app_main));
                }
            });
        });
    }

    Ok(())
}

#[tauri::command]
pub fn browser_webview_navigate(
    app: AppHandle,
    browser_page_id: String,
    url: String,
) -> Result<(), String> {
    let parsed = parse_url(&url)?;
    let webview = get_browser_webview(&app, &browser_page_id)
        .ok_or_else(|| "browser webview not found".to_string())?;
    webview.navigate(parsed).map_err(|e| e.to_string())
}

/// Back/forward/reload ride the page's own session history via JS; Tauri does
/// not expose native history controls on child webviews.
#[tauri::command]
pub fn browser_webview_history(
    app: AppHandle,
    browser_page_id: String,
    action: String,
) -> Result<(), String> {
    let webview = get_browser_webview(&app, &browser_page_id)
        .ok_or_else(|| "browser webview not found".to_string())?;
    let script = match action.as_str() {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        other => return Err(format!("unknown history action: {other}")),
    };
    webview.eval(script).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn browser_webview_set_bounds(
    app: AppHandle,
    browser_page_id: String,
    bounds: BrowserWebviewBounds,
) -> Result<(), String> {
    let webview = get_browser_webview(&app, &browser_page_id)
        .ok_or_else(|| "browser webview not found".to_string())?;
    webview.set_bounds(bounds.rect()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn browser_webview_set_visible(
    app: AppHandle,
    browser_page_id: String,
    visible: bool,
) -> Result<(), String> {
    let Some(webview) = get_browser_webview(&app, &browser_page_id) else {
        // Hiding a never-created or already-closed page is a no-op, not an error:
        // tab switches fire this for every page in the workspace.
        return Ok(());
    };
    let result = if visible {
        webview.show()
    } else {
        webview.hide()
    };
    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn browser_webview_close(app: AppHandle, browser_page_id: String) -> Result<(), String> {
    let Some(webview) = get_browser_webview(&app, &browser_page_id) else {
        return Ok(());
    };
    webview.close().map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserWebviewState {
    pub url: String,
}

#[tauri::command]
pub fn browser_webview_state(
    app: AppHandle,
    browser_page_id: String,
) -> Option<BrowserWebviewState> {
    let webview = get_browser_webview(&app, &browser_page_id)?;
    let url = webview.url().ok()?;
    Some(BrowserWebviewState {
        url: url.to_string(),
    })
}

/// In-page annotate mode (orca-style). Because the browser content is a native
/// child webview that paints over all React UI, the annotation picker + comment
/// box are injected INTO the guest page (so they render on top of the content).
/// On submit, the element payload + comment + intent are sent to the host over
/// the `agentumgrab://annotation/add` scheme, which forwards to the renderer's
/// annotation store. `enabled=false` tears the UI down.
#[tauri::command]
pub fn browser_inpage_annotate(
    app: AppHandle,
    browser_page_id: String,
    enabled: bool,
) -> Result<(), String> {
    let webview = get_browser_webview(&app, &browser_page_id)
        .ok_or_else(|| "browser webview not found".to_string())?;
    let js = if enabled {
        INPAGE_ANNOTATE_JS.replace("__PAGE_ID__", &browser_page_id.replace(['\\', '\''], ""))
    } else {
        "(function(){if(window.__agentumAnnotate){window.__agentumAnnotate.teardown();}})();"
            .to_string()
    };
    webview.eval(&js).map_err(|e| e.to_string())
}

/// The injected in-page annotate UI. Self-contained vanilla JS, idempotent.
/// `__PAGE_ID__` is replaced with the browser page id so submissions are
/// attributed to the right tab. Delivers via Image src (WebKit blocks fetch to
/// custom schemes) to `agentumgrab://annotation/add`.
const INPAGE_ANNOTATE_JS: &str = r#"(function(){
  if (window.__agentumAnnotate) { window.__agentumAnnotate.on(); return; }
  var PAGE_ID='__PAGE_ID__';
  var hl, box, current, intent='change', state={on:false};
  function css(el){
    if(!el||el.nodeType!==1) return '';
    if(el.id) return '#'+CSS.escape(el.id);
    var parts=[],node=el,depth=0;
    while(node&&node.nodeType===1&&depth<5){
      var sel=node.tagName.toLowerCase();
      if(node.classList&&node.classList.length){sel+='.'+Array.from(node.classList).slice(0,2).map(function(c){return CSS.escape(c);}).join('.');}
      var p=node.parentNode;
      if(p){var sib=Array.from(p.children).filter(function(c){return c.tagName===node.tagName;});if(sib.length>1){sel+=':nth-of-type('+(sib.indexOf(node)+1)+')';}}
      parts.unshift(sel); node=node.parentNode; depth++;
      if(node&&node.id){parts.unshift('#'+CSS.escape(node.id));break;}
    }
    return parts.join(' > ');
  }
  function extract(el){
    var r=el.getBoundingClientRect(), cs=getComputedStyle(el), attrs={};
    for(var i=0;i<el.attributes.length;i++){attrs[el.attributes[i].name]=el.attributes[i].value;}
    var pick=function(k){return cs.getPropertyValue(k)||'';};
    return {page:{url:location.href,title:document.title,viewport:{width:innerWidth,height:innerHeight},scrollX:scrollX,scrollY:scrollY,devicePixelRatio:devicePixelRatio},
      target:{tagName:el.tagName.toLowerCase(),selector:css(el),
        textSnippet:(el.innerText||el.textContent||'').trim().slice(0,300),
        htmlSnippet:el.outerHTML.slice(0,1200),
        cssClasses:el.className&&el.className.toString?el.className.toString():'',attributes:attrs,
        accessibility:{role:el.getAttribute('role')||'',accessibleName:el.getAttribute('aria-label')||el.getAttribute('alt')||el.title||(el.innerText||'').trim().slice(0,120)},
        rectViewport:{x:r.x,y:r.y,width:r.width,height:r.height},rectPage:{x:r.x+scrollX,y:r.y+scrollY,width:r.width,height:r.height},
        computedStyles:{display:pick('display'),position:pick('position'),color:pick('color'),backgroundColor:pick('background-color'),borderRadius:pick('border-radius'),fontFamily:pick('font-family'),fontSize:pick('font-size'),fontWeight:pick('font-weight'),lineHeight:pick('line-height'),textAlign:pick('text-align'),zIndex:pick('z-index')}},
      nearbyText:[],ancestorPath:[],screenshot:null};
  }
  function isOurs(el){return el&&el.closest&&el.closest('#__agentum_annotate_root');}
  function ensureHl(){ if(hl) return; hl=document.createElement('div'); hl.style.cssText='position:fixed;z-index:2147483646;pointer-events:none;border:2px solid #2563eb;background:rgba(37,99,235,.12);border-radius:3px;display:none;transition:all .03s;'; root.appendChild(hl); }
  function onMove(e){ if(!state.on||box) return; var el=document.elementFromPoint(e.clientX,e.clientY); if(!el||isOurs(el)){hl.style.display='none';return;} current=el; var r=el.getBoundingClientRect(); ensureHl(); hl.style.display='block'; hl.style.left=r.left+'px'; hl.style.top=r.top+'px'; hl.style.width=r.width+'px'; hl.style.height=r.height+'px'; }
  function onClick(e){ if(!state.on||box) return; var el=document.elementFromPoint(e.clientX,e.clientY); if(!el||isOurs(el)) return; e.preventDefault(); e.stopPropagation(); current=el; showBox(el); }
  function showBox(el){
    var r=el.getBoundingClientRect();
    box=document.createElement('div');
    box.style.cssText='position:fixed;z-index:2147483647;width:300px;background:#0b0b0d;color:#fff;border:1px solid rgba(255,255,255,.16);border-radius:12px;padding:12px;box-shadow:0 16px 40px rgba(0,0,0,.5);font:13px -apple-system,system-ui,sans-serif;';
    var top=Math.min(r.bottom+8,innerHeight-200), left=Math.min(Math.max(8,r.left),innerWidth-312);
    box.style.top=top+'px'; box.style.left=left+'px';
    var label=(el.innerText||el.tagName).trim().slice(0,40);
    box.innerHTML='<div style="font-weight:600;margin-bottom:2px">'+el.tagName.toLowerCase()+'</div><div style="opacity:.6;font-size:11px;margin-bottom:8px">'+label.replace(/</g,'&lt;')+'</div>'+
      '<textarea id="__aa_t" placeholder="Describe what the agent should change here…" style="width:100%;height:70px;resize:none;background:#000;color:#fff;border:1px solid rgba(255,255,255,.18);border-radius:8px;padding:8px;font:13px inherit;box-sizing:border-box"></textarea>'+
      '<div style="display:flex;gap:6px;margin:8px 0"><button id="__aa_change" style="flex:1;padding:6px;border-radius:7px;border:1px solid rgba(255,255,255,.18);background:#1d4ed8;color:#fff;cursor:pointer">Change</button><button id="__aa_q" style="flex:1;padding:6px;border-radius:7px;border:1px solid rgba(255,255,255,.18);background:transparent;color:#fff;cursor:pointer">Question</button></div>'+
      '<div style="display:flex;gap:6px;justify-content:flex-end"><button id="__aa_cancel" style="padding:6px 10px;border-radius:7px;border:none;background:transparent;color:#fff;cursor:pointer">Cancel</button><button id="__aa_add" style="padding:6px 12px;border-radius:7px;border:none;background:#2563eb;color:#fff;cursor:pointer">Add</button></div>';
    root.appendChild(box); hl.style.display='none'; intent='change';
    var ta=box.querySelector('#__aa_t'); ta.focus();
    box.querySelector('#__aa_change').onclick=function(){intent='change';this.style.background='#1d4ed8';box.querySelector('#__aa_q').style.background='transparent';};
    box.querySelector('#__aa_q').onclick=function(){intent='question';this.style.background='#1d4ed8';box.querySelector('#__aa_change').style.background='transparent';};
    box.querySelector('#__aa_cancel').onclick=closeBox;
    box.querySelector('#__aa_add').onclick=function(){ submit(el,ta.value); };
    ta.addEventListener('keydown',function(ev){if((ev.metaKey||ev.ctrlKey)&&ev.key==='Enter'){submit(el,ta.value);}if(ev.key==='Escape'){closeBox();}});
  }
  function closeBox(){ if(box){box.remove();box=null;} }
  function marker(el){ var r=el.getBoundingClientRect(),m=document.createElement('div'); m.className='__aa_marker'; m.style.cssText='position:fixed;z-index:2147483645;width:22px;height:22px;border-radius:11px;background:#2563eb;color:#fff;display:flex;align-items:center;justify-content:center;font:600 11px sans-serif;border:1px solid #fff;pointer-events:none;'; m.textContent=String(root.querySelectorAll('.__aa_marker').length+1); m.style.left=(r.left)+'px'; m.style.top=(r.top)+'px'; root.appendChild(m); }
  function submit(el,comment){ comment=(comment||'').trim(); if(!comment){return;} var payload=extract(el); try{var img=new Image();img.src='agentumgrab://annotation/add?p='+encodeURIComponent(JSON.stringify({pageId:PAGE_ID,comment:comment,intent:intent,payload:payload}));}catch(e){} marker(el); closeBox(); }
  function onKey(e){ if(e.key==='Escape'){ if(box){closeBox();} else {api_off();} } }
  function api_off(){ state.on=false; if(hl)hl.style.display='none'; if(toolbar)toolbar.style.display='none'; }
  var root=document.createElement('div'); root.id='__agentum_annotate_root'; document.documentElement.appendChild(root);
  var toolbar=document.createElement('div'); toolbar.style.cssText='position:fixed;z-index:2147483647;top:10px;left:50%;transform:translateX(-50%);background:#0b0b0d;color:#fff;border:1px solid rgba(255,255,255,.18);border-radius:9999px;padding:6px 14px;font:12px -apple-system,system-ui,sans-serif;box-shadow:0 10px 30px rgba(0,0,0,.4)'; toolbar.textContent='Annotate: click an element  ·  Esc to finish'; root.appendChild(toolbar);
  ensureHl();
  document.addEventListener('mousemove',onMove,true);
  document.addEventListener('click',onClick,true);
  document.addEventListener('keydown',onKey,true);
  window.__agentumAnnotate={ on:function(){state.on=true;toolbar.style.display='block';}, teardown:function(){state.on=false;document.removeEventListener('mousemove',onMove,true);document.removeEventListener('click',onClick,true);document.removeEventListener('keydown',onKey,true);if(root)root.remove();window.__agentumAnnotate=null;} };
  state.on=true;
})();"#;
