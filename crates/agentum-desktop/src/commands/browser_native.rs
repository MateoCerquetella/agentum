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

/// Create (or reveal) the native webview for a browser page at the given
/// window-relative logical bounds, navigated to `url`.
#[tauri::command]
pub fn browser_webview_open(
    app: AppHandle,
    browser_page_id: String,
    url: String,
    bounds: BrowserWebviewBounds,
) -> Result<(), String> {
    let parsed = parse_url(&url)?;
    if let Some(webview) = get_browser_webview(&app, &browser_page_id) {
        webview
            .set_bounds(bounds.rect())
            .map_err(|e| e.to_string())?;
        let _ = webview.show();
        return Ok(());
    }

    let window = app
        .get_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let label = webview_label(&browser_page_id);
    let event_page_id = browser_page_id.clone();

    // Webview creation must run on the main thread on macOS; commands execute
    // on the async runtime, so hop over and relay the result back.
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    app.run_on_main_thread(move || {
        let builder = tauri::webview::WebviewBuilder::new(&label, WebviewUrl::External(parsed))
            .user_agent(BROWSER_USER_AGENT)
            .on_page_load(move |webview, payload| {
                let event = match payload.event() {
                    PageLoadEvent::Started => "started",
                    PageLoadEvent::Finished => "finished",
                };
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
    rx.recv().map_err(|e| e.to_string())?
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
