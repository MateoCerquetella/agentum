use std::path::PathBuf;

use base64::Engine as _;
use serde_json::{Map, Value};
use tauri::Manager;
use tauri_plugin_clipboard_manager::ClipboardExt;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// PersistedUIState is a renderer-owned preferences blob; this layer stores it
// opaquely (no per-field modeling) and shallow-merges partial updates.
fn ui_state_path() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(".agentum")
        .join("ui-state.json"))
}

fn read_ui_state() -> Map<String, Value> {
    let Ok(path) = ui_state_path() else {
        return Map::new();
    };
    if !path.exists() {
        return Map::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn write_ui_state(state: &Map<String, Value>) -> Result<(), String> {
    let path = ui_state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized =
        serde_json::to_string_pretty(&Value::Object(state.clone())).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

#[tauri::command]
pub fn ui_get() -> Result<Value, String> {
    Ok(Value::Object(read_ui_state()))
}

#[tauri::command]
pub fn ui_record_feature_interaction(value: String) -> Result<Value, String> {
    let mut state = read_ui_state();
    let interactions = state
        .entry("featureInteractions")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(object) = interactions.as_object_mut() {
        object.insert(value, Value::Bool(true));
    }
    write_ui_state(&state)?;
    Ok(Value::Object(state))
}

#[tauri::command]
pub fn ui_write_clipboard_text(app: tauri::AppHandle, value: String) -> Result<(), String> {
    app.clipboard().write_text(value).map_err(map_err)
}

#[tauri::command]
pub fn ui_read_clipboard_text(app: tauri::AppHandle) -> Result<String, String> {
    app.clipboard().read_text().map_err(map_err)
}

fn main_window(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window("main")
}

#[tauri::command]
pub fn ui_minimize(app: tauri::AppHandle) {
    if let Some(window) = main_window(&app) {
        let _ = window.minimize();
    }
}

#[tauri::command]
pub fn ui_maximize(app: tauri::AppHandle) {
    if let Some(window) = main_window(&app) {
        if window.is_maximized().unwrap_or(false) {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
    }
}

#[tauri::command]
pub fn ui_is_maximized(app: tauri::AppHandle) -> bool {
    main_window(&app)
        .and_then(|window| window.is_maximized().ok())
        .unwrap_or(false)
}

#[tauri::command]
pub fn ui_request_close(app: tauri::AppHandle) {
    if let Some(window) = main_window(&app) {
        let _ = window.close();
    }
}

// ui.set passes a Partial<PersistedUIState> as the WHOLE invoke payload (no wrapper
// key), so it reads the raw request body rather than a named arg, then shallow-merges.
#[tauri::command]
pub fn ui_set(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let updates = match request.body() {
        tauri::ipc::InvokeBody::Json(value) => value.as_object().cloned().unwrap_or_default(),
        tauri::ipc::InvokeBody::Raw(bytes) => serde_json::from_slice::<Value>(bytes)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default(),
    };
    let mut state = read_ui_state();
    for (key, value) in updates {
        state.insert(key, value);
    }
    write_ui_state(&state)
}

#[tauri::command]
pub fn ui_get_zoom_level() -> f64 {
    read_ui_state()
        .get("zoomLevel")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

#[tauri::command]
pub fn ui_set_zoom_level(app: tauri::AppHandle, value: f64) -> Result<(), String> {
    let mut state = read_ui_state();
    state.insert("zoomLevel".into(), Value::from(value));
    write_ui_state(&state)?;
    // Electron zoom levels are logarithmic (1.2^level); convert to a scale factor.
    if let Some(window) = main_window(&app) {
        let _ = window.set_zoom(1.2_f64.powf(value));
    }
    Ok(())
}

// Focus-state setters inform main for global-shortcut routing, which isn't ported
// yet — accept and no-op so callers don't error.
#[tauri::command]
pub fn ui_set_markdown_editor_focused(value: bool) {
    let _ = value;
}

#[tauri::command]
pub fn ui_set_terminal_input_focused(value: bool) {
    let _ = value;
}

#[tauri::command]
pub fn ui_set_shortcut_recorder_focused(value: bool) {
    let _ = value;
}

// The renderer's answer to a bridge-initiated `ui-request-tab-create`: it
// created (or failed to create) the browser tab and reports the new page id so
// the bridge's `open` op can return it to the MCP/HTTP/CLI caller. Resolves the
// matching oneshot parked in `TabCreateRegistry`; an unknown `request_id` is a
// no-op (the request likely timed out). Tauri maps the camelCase JS keys
// (`requestId`/`browserPageId`) onto these snake_case params.
#[tauri::command]
pub fn ui_reply_tab_create(
    registry: tauri::State<'_, crate::bridge::TabCreateRegistry>,
    request_id: String,
    browser_page_id: Option<String>,
    error: Option<String>,
) {
    let result = match (browser_page_id, error) {
        (Some(page_id), _) => Ok(page_id),
        (None, Some(err)) => Err(err),
        (None, None) => Err("renderer returned neither a page id nor an error".to_string()),
    };
    registry.resolve(&request_id, result);
}

// The renderer's answer to a bridge-initiated browser annotation/grab op
// (`ui-request-browser-annotations` / `-grab` / `-annotate`). `result` carries
// the op's payload (annotation list, grabbed element, the added annotation);
// `error` is set instead on failure. Resolves the matching oneshot in
// `BrowserOpRegistry`. Tauri maps the camelCase JS key `requestId`.
#[tauri::command]
pub fn ui_reply_browser_op(
    registry: tauri::State<'_, crate::bridge::BrowserOpRegistry>,
    request_id: String,
    result: Option<Value>,
    error: Option<String>,
) {
    let outcome = match (result, error) {
        (_, Some(err)) => Err(err),
        (Some(value), None) => Ok(value),
        (None, None) => Ok(Value::Null),
    };
    registry.resolve(&request_id, outcome);
}

// Request/reply responses (other main-initiated tab/terminal flows) and native
// chrome ops (traffic-light sync, context menu, close confirmation) aren't
// ported yet. These are fire-and-forget void methods, so accept and no-op.

#[tauri::command]
pub fn ui_reply_tab_set_profile() {}

#[tauri::command]
pub fn ui_reply_tab_close() {}

#[tauri::command]
pub fn ui_reply_terminal_create() {}

#[tauri::command]
pub fn ui_sync_traffic_lights() {}

#[tauri::command]
pub fn ui_popup_menu() {}

#[tauri::command]
pub fn ui_confirm_window_close() {}

// Clipboard TEXT lives in `commands/clipboard.rs`. This is the image *read*
// half, and it backs Cmd/Ctrl+V of a screenshot into an agent terminal: the
// renderer's `pasteTerminalClipboard` calls here when the clipboard holds no
// text, then bracketed-pastes the returned path so the agent (Claude Code et
// al.) attaches it as an image — the same path-handoff contract the web/mobile
// runtime already implements, just sourced from the desktop's OS clipboard.
//
// `async` is load-bearing: arboard's `get_image()` (under the plugin) must not
// run on the main thread — it can deadlock on Linux — and Tauri runs async
// commands off it. That's the same reason `clipboard_read` is async.
#[tauri::command]
pub async fn ui_save_clipboard_image_as_temp_file(
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    // A Cmd+V with nothing image-shaped on the clipboard is the common case,
    // and `read_image` errors on it. Treat that as `Ok(None)` so the renderer
    // silently no-ops; only a genuine encode/write failure on an image we did
    // read becomes `Err` (surfaced via the paste handler's error path).
    let image = match app.clipboard().read_image() {
        Ok(image) => image,
        Err(_) => return Ok(None),
    };
    let (rgba, width, height) = (image.rgba(), image.width(), image.height());
    if width == 0 || height == 0 || rgba.is_empty() {
        return Ok(None);
    }
    let png = encode_rgba_as_png(rgba, width, height).map_err(map_err)?;
    let path = write_clipboard_image_tempfile(&png).map_err(map_err)?;
    Ok(Some(path))
}

// Writing an image TO the system clipboard — browser-pane "Copy image" and the
// usage-card share button hand us a `data:image/png;base64,…` URL. Decode it to
// a `tauri::image::Image` (which needs the `image-png` feature, enabled in
// Cargo.toml) and set it. `async` keeps the arboard set off the main thread,
// mirroring `clipboard_write`.
#[tauri::command]
pub async fn ui_write_clipboard_image(app: tauri::AppHandle, value: String) -> Result<(), String> {
    let bytes = decode_image_data_url(&value)
        .ok_or_else(|| "clipboard image payload was not valid base64".to_string())?;
    let image = tauri::image::Image::from_bytes(&bytes).map_err(map_err)?;
    app.clipboard().write_image(&image).map_err(map_err)
}

/// Pull the bytes out of a `data:<mime>;base64,<data>` URL (the shape the
/// renderer sends), or treat the whole string as bare base64 when there's no
/// data-URL header. `None` on anything that isn't valid base64 — the caller
/// turns that into a surfaced error rather than writing garbage.
fn decode_image_data_url(value: &str) -> Option<Vec<u8>> {
    let b64 = match value.split_once(',') {
        Some((header, data)) if header.starts_with("data:") => data,
        _ => value,
    };
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()
}

/// PNG-encode raw RGBA8 (row-major, top-to-bottom — the layout the clipboard
/// plugin hands back). Split out so the encoder settings stay pinned in one
/// place and the command body reads as intent.
fn encode_rgba_as_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
    }
    Ok(out)
}

/// Write the PNG to a temp dir under a collision-proof name and return its
/// absolute path. Deliberately OUTSIDE any worktree — the path is consumed only
/// by the local agent (which shares this filesystem) as an image attachment,
/// never committed. The UUID name sidesteps the fixed-filename overwrite bug
/// the browser-annotate capture path has.
fn write_clipboard_image_tempfile(png: &[u8]) -> std::io::Result<String> {
    let dir = std::env::temp_dir().join("agentum-clipboard");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("clip-{}.png", uuid::Uuid::new_v4().simple()));
    std::fs::write(&path, png)?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_rgba_as_png_emits_a_valid_png() {
        // 2×2 opaque red, RGBA8.
        let rgba = [
            255, 0, 0, 255, 255, 0, 0, 255, // row 0
            255, 0, 0, 255, 255, 0, 0, 255, // row 1
        ];
        let png = encode_rgba_as_png(&rgba, 2, 2).expect("encode 2x2 rgba");
        // The 8-byte PNG signature — proves we wrote a real PNG, not raw bytes.
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(
            png.len() > 8,
            "encoded PNG should carry chunks past the header"
        );
    }

    #[test]
    fn write_clipboard_image_tempfile_writes_bytes_and_returns_abs_path() {
        let png = encode_rgba_as_png(&[0, 0, 0, 255], 1, 1).expect("encode 1x1");
        let path = write_clipboard_image_tempfile(&png).expect("write tempfile");
        let p = std::path::Path::new(&path);
        assert!(
            p.is_absolute(),
            "agent needs an absolute path it can open: {path}"
        );
        assert_eq!(std::fs::read(p).expect("read back"), png);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn decode_image_data_url_handles_data_url_and_bare_base64() {
        use base64::Engine as _;
        let raw: &[u8] = b"\x89PNG\r\n\x1a\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        // The data-URL header is stripped before decoding...
        assert_eq!(
            decode_image_data_url(&format!("data:image/png;base64,{b64}")).as_deref(),
            Some(raw)
        );
        // ...and a bare base64 string decodes just the same.
        assert_eq!(decode_image_data_url(&b64).as_deref(), Some(raw));
        // Garbage is rejected (None) so the command errors instead of writing junk.
        assert!(decode_image_data_url("data:image/png;base64,!! not base64 !!").is_none());
    }
}
