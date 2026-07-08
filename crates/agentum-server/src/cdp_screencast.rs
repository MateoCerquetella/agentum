//! CDP → in-agentum screencast bridge (009c-3).
//!
//! Renders the agent-driven headless CDP-Chromium (see [`crate::cdp_browser`])
//! **inside agentum's browser pane** instead of a separate OS window: connect to
//! the browser's CDP endpoint, run `Page.startScreencast`, and stream each frame
//! to the desktop in the **exact binary protocol the pane already decodes**
//! (`ui/src/shared/browser-screencast-protocol.ts`, kind `0x62`, version 1).
//! Pane input/navigation travels back the other way → CDP `Input.*` / `Page.*`.
//!
//! One bridge serves local AND host: only the CDP endpoint differs (a local port,
//! or a host port reached over the 009a `ssh -L` forward tunnel).
//!
//! This module is built bottom-up so each piece is verifiable on its own:
//!   1. the frame wire-format codec (this file's [`encode_frame`]) — unit-tested
//!      against the TS decoder's exact byte layout;  ← DONE
//!   2. the CDP client (tokio-tungstenite) that drives `Page.startScreencast`;
//!   3. the axum WS route bridging the pane socket ↔ the CDP client.

/// Wire constants — MUST match `browser-screencast-protocol.ts` exactly, or the
/// pane silently drops every frame.
const KIND: u8 = 0x62;
const VERSION: u8 = 1;
const OPCODE_FRAME: u8 = 0x01;
const HEADER_BYTES: usize = 16;

/// Image encoding of a screencast frame. Byte values match the TS `formatToByte`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    Jpeg,
    Png,
}

impl FrameFormat {
    fn to_byte(self) -> u8 {
        match self {
            FrameFormat::Jpeg => 1,
            FrameFormat::Png => 2,
        }
    }
}

/// Per-frame metadata. Serialized as a JSON **object** (the pane rejects a frame
/// whose metadata isn't a finite-number object), so every field is optional and
/// `None` fields are omitted — an empty `{}` is valid. Field names match the TS
/// `METADATA_KEYS` exactly.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct FrameMetadata {
    #[serde(rename = "offsetTop", skip_serializing_if = "Option::is_none")]
    pub offset_top: Option<f64>,
    #[serde(rename = "pageScaleFactor", skip_serializing_if = "Option::is_none")]
    pub page_scale_factor: Option<f64>,
    #[serde(rename = "deviceWidth", skip_serializing_if = "Option::is_none")]
    pub device_width: Option<f64>,
    #[serde(rename = "deviceHeight", skip_serializing_if = "Option::is_none")]
    pub device_height: Option<f64>,
    #[serde(rename = "imageWidth", skip_serializing_if = "Option::is_none")]
    pub image_width: Option<f64>,
    #[serde(rename = "imageHeight", skip_serializing_if = "Option::is_none")]
    pub image_height: Option<f64>,
    #[serde(rename = "scrollOffsetX", skip_serializing_if = "Option::is_none")]
    pub scroll_offset_x: Option<f64>,
    #[serde(rename = "scrollOffsetY", skip_serializing_if = "Option::is_none")]
    pub scroll_offset_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
}

/// Encode one screencast frame into the `0x62` binary wire format the pane
/// decodes. Layout (all u32 little-endian):
/// `[kind, ver, opcode, format, seq:u32, mdlen:u32, reserved:u32(=0)] + md_json + image`.
pub fn encode_frame(
    seq: u32,
    format: FrameFormat,
    metadata: &FrameMetadata,
    image: &[u8],
) -> Vec<u8> {
    let md = serde_json::to_vec(metadata).unwrap_or_else(|_| b"{}".to_vec());
    let mut out = Vec::with_capacity(HEADER_BYTES + md.len() + image.len());
    out.push(KIND);
    out.push(VERSION);
    out.push(OPCODE_FRAME);
    out.push(format.to_byte());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(md.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved — pane requires 0
    out.extend_from_slice(&md);
    out.extend_from_slice(image);
    out
}

// ============================================================================
// CDP client — drives `Page.startScreencast` over a CDP WebSocket and bridges
// the pane's input/navigation back to `Input.*` / `Page.*`. (009c-3 step 2.)
// ============================================================================

use anyhow::{Context, Result};
use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::cdp_http::discover_page_ws_url;

/// Screencast knobs passed straight to `Page.startScreencast`. `format=jpeg,
/// quality=90` match the pane's subscribe params (90 keeps text glyph edges
/// crisp; the canvas already captures at device DPR so this is the last lever).
#[derive(Debug, Clone, Copy)]
pub struct ScreencastOptions {
    pub format: FrameFormat,
    pub quality: u8,
    pub max_width: u32,
    pub max_height: u32,
    /// CDP `everyNthFrame` throttle. **Defaults to 1 (every frame)** — NOT the
    /// pane's nominal `2`, because `everyNthFrame:2` makes Chrome drop the *only*
    /// frame a static page emits (it sends the 2nd, 4th… compositor frame, and a
    /// loaded-but-still page produces just one). Dropping it leaves the pane blank
    /// until the next repaint. JPEG `quality` + `maxWidth/Height` already bound
    /// bandwidth; halving an idle page's already-zero rate buys nothing.
    pub every_nth_frame: u32,
}

impl Default for ScreencastOptions {
    fn default() -> Self {
        Self {
            format: FrameFormat::Jpeg,
            quality: 90,
            // 5K. We now capture at 2× (see cdp_browser `--force-device-scale-factor`),
            // so a 2× frame of a 2560×1440-CSS pane lands here. The old 4K cap capped
            // the pane at ~1920×1080 CSS before scaling the 2× frame back toward 1×.
            max_width: 5120,
            max_height: 2880,
            every_nth_frame: 1,
        }
    }
}

impl ScreencastOptions {
    fn format_str(&self) -> &'static str {
        match self.format {
            FrameFormat::Jpeg => "jpeg",
            FrameFormat::Png => "png",
        }
    }
}

/// A mouse button, as the pane names it (`left`/`middle`/`right`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl MouseButton {
    fn parse(s: &str) -> MouseButton {
        match s {
            "middle" => MouseButton::Middle,
            "right" => MouseButton::Right,
            _ => MouseButton::Left,
        }
    }
    /// CDP `button` name.
    fn cdp_name(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
        }
    }
    /// CDP `buttons` bitmask bit for this button (held-buttons set).
    fn mask_bit(self) -> u32 {
        match self {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
        }
    }
}

/// One pane→browser interaction, decoded from the pane's WS message. Mirrors the
/// `browser.*` RPCs the dormant `RemoteBrowserPagePane` already emits (see the
/// 009c-3 contract); the bridge maps each to one or more CDP commands.
#[derive(Debug, Clone, PartialEq)]
pub enum InputCommand {
    MouseMove {
        x: f64,
        y: f64,
    },
    MouseDown {
        button: MouseButton,
    },
    MouseUp {
        button: MouseButton,
    },
    MouseWheel {
        dx: f64,
        dy: f64,
    },
    /// A key press — either a printable char or a named key (Enter/Backspace/…).
    KeyPress {
        key: String,
    },
    /// Insert clipboard text (paste). The pane sends the OS clipboard text from an
    /// `onPaste` ClipboardEvent because a synthetic Cmd/Ctrl+V key event never
    /// triggers a real clipboard read in headless Chromium. Maps to
    /// `Input.insertText`, which inserts the text as trusted input.
    InsertText {
        text: String,
    },
    Goto {
        url: String,
    },
    Back,
    Forward,
    Reload,
    /// Resize the page's LAYOUT viewport to match the pane. Without this the
    /// headless page lays out at the launcher's fixed `--window-size=1280,800`, so
    /// the screencast frame is clipped (top/bottom cut off) when the pane's aspect
    /// ratio differs. Maps to `Emulation.setDeviceMetricsOverride`.
    SetViewport {
        width: u32,
        height: u32,
        device_scale_factor: f64,
    },
}

impl InputCommand {
    /// Whether this is an active human action that should grab the co-browse wheel
    /// (F12). Excludes passive `MouseMove` (hover) and automatic `SetViewport`
    /// (pane resize) so merely watching/resizing doesn't lock the agent out.
    pub fn is_human_action(&self) -> bool {
        matches!(
            self,
            InputCommand::MouseDown { .. }
                | InputCommand::MouseUp { .. }
                | InputCommand::MouseWheel { .. }
                | InputCommand::KeyPress { .. }
                | InputCommand::InsertText { .. }
                | InputCommand::Goto { .. }
                | InputCommand::Back
                | InputCommand::Forward
                | InputCommand::Reload
        )
    }
}

/// Parse a pane→server WS text message (`{"method":"browser.*","params":{…}}`)
/// into an [`InputCommand`]. Returns `None` for unknown/malformed methods so a
/// stray message never tears the bridge down. Kept pure for unit testing.
pub fn parse_input_message(raw: &str) -> Option<InputCommand> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let method = v.get("method")?.as_str()?;
    let p = v.get("params").cloned().unwrap_or(Value::Null);
    let num = |k: &str| p.get(k).and_then(Value::as_f64);
    let button = || {
        p.get("button")
            .and_then(Value::as_str)
            .map(MouseButton::parse)
            .unwrap_or(MouseButton::Left)
    };
    match method {
        "browser.mouseMove" => Some(InputCommand::MouseMove {
            x: num("x")?,
            y: num("y")?,
        }),
        "browser.mouseDown" => Some(InputCommand::MouseDown { button: button() }),
        "browser.mouseUp" => Some(InputCommand::MouseUp { button: button() }),
        "browser.mouseWheel" => Some(InputCommand::MouseWheel {
            dx: num("dx").unwrap_or(0.0),
            dy: num("dy").unwrap_or(0.0),
        }),
        "browser.keypress" => Some(InputCommand::KeyPress {
            key: p.get("key")?.as_str()?.to_string(),
        }),
        "browser.insertText" => Some(InputCommand::InsertText {
            text: p.get("text")?.as_str()?.to_string(),
        }),
        "browser.goto" => Some(InputCommand::Goto {
            url: p.get("url")?.as_str()?.to_string(),
        }),
        "browser.back" => Some(InputCommand::Back),
        "browser.forward" => Some(InputCommand::Forward),
        "browser.reload" => Some(InputCommand::Reload),
        "browser.setViewport" => Some(InputCommand::SetViewport {
            width: num("width")? as u32,
            height: num("height")? as u32,
            // Default to 1.0; the pane normally sends the (clamped) devicePixelRatio.
            device_scale_factor: num("deviceScaleFactor").unwrap_or(1.0),
        }),
        _ => None,
    }
}

/// Last-known pointer position. CDP `mousePressed`/`mouseReleased`/`mouseWheel`
/// need coordinates, but the pane only sends them on `mouseMove`; we remember the
/// most recent move so a down/up lands where the cursor is.
#[derive(Debug, Clone, Copy, Default)]
pub struct PointerState {
    pub x: f64,
    pub y: f64,
}

/// Translate one [`InputCommand`] into the CDP command(s) that realize it. Pure
/// (no I/O) so the mapping is unit-tested without a browser. `next_id` mints CDP
/// message ids; a keypress and the history nav helpers may emit two commands.
pub fn input_command_to_cdp(
    cmd: &InputCommand,
    pointer: &mut PointerState,
    next_id: &mut u64,
) -> Vec<Value> {
    let mut id = || {
        *next_id += 1;
        *next_id
    };
    match cmd {
        InputCommand::MouseMove { x, y } => {
            pointer.x = *x;
            pointer.y = *y;
            vec![json!({
                "id": id(), "method": "Input.dispatchMouseEvent",
                "params": { "type": "mouseMoved", "x": x, "y": y, "buttons": 0 }
            })]
        }
        InputCommand::MouseDown { button } => vec![json!({
            "id": id(), "method": "Input.dispatchMouseEvent",
            "params": {
                "type": "mousePressed", "x": pointer.x, "y": pointer.y,
                "button": button.cdp_name(), "buttons": button.mask_bit(), "clickCount": 1
            }
        })],
        InputCommand::MouseUp { button } => vec![json!({
            "id": id(), "method": "Input.dispatchMouseEvent",
            "params": {
                "type": "mouseReleased", "x": pointer.x, "y": pointer.y,
                "button": button.cdp_name(), "buttons": 0, "clickCount": 1
            }
        })],
        InputCommand::MouseWheel { dx, dy } => vec![json!({
            "id": id(), "method": "Input.dispatchMouseEvent",
            "params": {
                "type": "mouseWheel", "x": pointer.x, "y": pointer.y,
                "deltaX": dx, "deltaY": dy
            }
        })],
        InputCommand::KeyPress { key } => key_press_to_cdp(key, &mut id),
        // Paste: `Input.insertText` inserts the text as one trusted edit (the same
        // primitive the agent-driver fill path uses), which a synthetic Cmd/Ctrl+V
        // keystroke cannot achieve in headless Chromium.
        InputCommand::InsertText { text } => vec![json!({
            "id": id(), "method": "Input.insertText", "params": { "text": text }
        })],
        InputCommand::Goto { url } => vec![json!({
            "id": id(), "method": "Page.navigate", "params": { "url": url }
        })],
        // No first-class CDP back/forward; `history.*()` in the page is reliable
        // and avoids the Page.getNavigationHistory→navigateToHistoryEntry dance.
        InputCommand::Back => vec![eval_js(id(), "history.back()")],
        InputCommand::Forward => vec![eval_js(id(), "history.forward()")],
        InputCommand::Reload => vec![json!({
            "id": id(), "method": "Page.reload", "params": {}
        })],
        // Override the LAYOUT viewport so the page lays out at the pane size, not
        // the launcher's fixed `--window-size`. `mobile:false` keeps desktop
        // layout; floor dimensions at 1 because Chrome rejects a 0-sized override.
        InputCommand::SetViewport {
            width,
            height,
            device_scale_factor,
        } => vec![json!({
            "id": id(), "method": "Emulation.setDeviceMetricsOverride",
            "params": {
                "width": (*width).max(1),
                "height": (*height).max(1),
                "deviceScaleFactor": if *device_scale_factor > 0.0 { *device_scale_factor } else { 1.0 },
                "mobile": false
            }
        })],
    }
}

fn eval_js(id: u64, expr: &str) -> Value {
    json!({ "id": id, "method": "Runtime.evaluate", "params": { "expression": expr } })
}

/// Map a pane keypress to CDP `Input.dispatchKeyEvent`. A printable character is
/// inserted with a single `char` event; a named key (Enter/Backspace/…) becomes a
/// keyDown+keyUp pair carrying the Windows virtual-key code CDP expects.
fn key_press_to_cdp(key: &str, id: &mut impl FnMut() -> u64) -> Vec<Value> {
    if let Some((code, vk, text)) = named_key(key) {
        let mut down = json!({
            "id": id(), "method": "Input.dispatchKeyEvent",
            "params": { "type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk }
        });
        if let Some(t) = text {
            down["params"]["text"] = json!(t);
        }
        let up = json!({
            "id": id(), "method": "Input.dispatchKeyEvent",
            "params": { "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk }
        });
        return vec![down, up];
    }
    // Printable text — `char` inserts it directly into the focused field.
    vec![json!({
        "id": id(), "method": "Input.dispatchKeyEvent",
        "params": { "type": "char", "key": key, "text": key }
    })]
}

/// `(code, windowsVirtualKeyCode, optional text)` for the named keys the pane
/// emits, or `None` for a printable character.
fn named_key(key: &str) -> Option<(&'static str, u32, Option<&'static str>)> {
    match key {
        "Enter" => Some(("Enter", 13, Some("\r"))),
        "Backspace" => Some(("Backspace", 8, None)),
        "Delete" => Some(("Delete", 46, None)),
        "Tab" => Some(("Tab", 9, Some("\t"))),
        "Escape" => Some(("Escape", 27, None)),
        "ArrowUp" => Some(("ArrowUp", 38, None)),
        "ArrowDown" => Some(("ArrowDown", 40, None)),
        "ArrowLeft" => Some(("ArrowLeft", 37, None)),
        "ArrowRight" => Some(("ArrowRight", 39, None)),
        "Home" => Some(("Home", 36, None)),
        "End" => Some(("End", 35, None)),
        "PageUp" => Some(("PageUp", 33, None)),
        "PageDown" => Some(("PageDown", 34, None)),
        _ => None,
    }
}

/// Build [`FrameMetadata`] from CDP's `Page.screencastFrame` `metadata` object.
/// CDP carries no image pixel size, so `imageWidth/Height` are filled from the
/// device size (best-effort; the decoded `<img>` reports its own natural size).
pub fn metadata_from_cdp(md: &Value) -> FrameMetadata {
    let f = |k: &str| md.get(k).and_then(Value::as_f64);
    let device_width = f("deviceWidth");
    let device_height = f("deviceHeight");
    FrameMetadata {
        offset_top: f("offsetTop"),
        page_scale_factor: f("pageScaleFactor"),
        device_width,
        device_height,
        image_width: device_width,
        image_height: device_height,
        scroll_offset_x: f("scrollOffsetX"),
        scroll_offset_y: f("scrollOffsetY"),
        timestamp: f("timestamp"),
    }
}

/// Connect to the headless CDP browser at `cdp_http_base`, start a screencast on
/// its page target, and run the bridge until either side closes:
///   - each `Page.screencastFrame` → ack back to CDP **immediately** (required, or
///     CDP stops sending) → [`encode_frame`] → `frame_tx` (latest-wins `watch`, so
///     a slow pane never stalls Chrome's compositor);
///   - each [`InputCommand`] from `input_rx` → the CDP command(s) that realize it.
///
/// Returns `Ok(())` on a clean close (pane disconnected, frame sink dropped, or
/// CDP socket closed) and `Err` only on a connect/protocol failure the caller
/// should surface to the pane.
pub async fn run_screencast_bridge(
    cdp_http_base: &str,
    opts: ScreencastOptions,
    mut input_rx: mpsc::Receiver<InputCommand>,
    frame_tx: watch::Sender<Option<Vec<u8>>>,
) -> Result<()> {
    let ws_url = discover_page_ws_url(cdp_http_base).await?;
    let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .with_context(|| format!("open CDP WebSocket {ws_url}"))?;
    let (mut write, mut read) = ws.split();

    let mut next_id: u64 = 0;
    let mut pointer = PointerState::default();

    // Enable the Page domain, then start the screencast. ids 1,2.
    next_id += 1;
    write
        .send(Message::Text(
            json!({ "id": next_id, "method": "Page.enable" }).to_string(),
        ))
        .await
        .context("send Page.enable")?;
    next_id += 1;
    write
        .send(Message::Text(
            json!({
                "id": next_id, "method": "Page.startScreencast",
                "params": {
                    "format": opts.format_str(), "quality": opts.quality,
                    "maxWidth": opts.max_width, "maxHeight": opts.max_height,
                    "everyNthFrame": opts.every_nth_frame.max(1)
                }
            })
            .to_string(),
        ))
        .await
        .context("send Page.startScreencast")?;

    let mut seq: u32 = 0;
    loop {
        tokio::select! {
            msg = read.next() => match msg {
                Some(Ok(Message::Text(txt))) => {
                    if let Some((frame, ack)) = decode_cdp_frame(&txt, opts.format, &mut seq) {
                        // Ack IMMEDIATELY — before forwarding — so CDP keeps
                        // compositing. The old code gated the ack on the pane draining
                        // the frame, so a slow pane stalled Chrome's compositor; that
                        // is what made motion feel choppy / VPS-like. A missing ack
                        // stalls the stream after the first frame.
                        if write.send(Message::Text(ack)).await.is_err() {
                            break;
                        }
                        // Forward latest-wins: `watch` overwrites any frame the pane
                        // hasn't drained yet, so the pane always paints the freshest
                        // frame and never buffers a backlog. Err = receiver gone.
                        if frame_tx.send(Some(frame)).is_err() {
                            break; // pane gone
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {} // ping/pong/binary — CDP only speaks text
            },
            cmd = input_rx.recv() => match cmd {
                Some(cmd) => {
                    for c in input_command_to_cdp(&cmd, &mut pointer, &mut next_id) {
                        if write.send(Message::Text(c.to_string())).await.is_err() {
                            break;
                        }
                    }
                }
                None => break, // pane disconnected
            },
        }
    }
    Ok(())
}

/// Decode one CDP text message. On a `Page.screencastFrame`, return the encoded
/// `0x62` frame bytes and the `Page.screencastFrameAck` JSON to send back; `None`
/// for every other message (responses, other events). PURE (no I/O) so the caller
/// controls ordering: it acks IMMEDIATELY (so CDP keeps compositing) and then
/// forwards the frame latest-wins — decoupling Chrome from a slow pane.
fn decode_cdp_frame(txt: &str, format: FrameFormat, seq: &mut u32) -> Option<(Vec<u8>, String)> {
    let v: Value = serde_json::from_str(txt).ok()?;
    if v.get("method").and_then(Value::as_str)? != "Page.screencastFrame" {
        return None;
    }
    let params = v.get("params")?;
    let data_b64 = params.get("data")?.as_str()?;
    let session_id = params.get("sessionId")?.clone();
    let image = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .ok()?;
    let metadata = params
        .get("metadata")
        .map(metadata_from_cdp)
        .unwrap_or_default();
    let frame = encode_frame(*seq, format, &metadata, &image);
    *seq = seq.wrapping_add(1);
    let ack = json!({ "id": -1, "method": "Page.screencastFrameAck", "params": { "sessionId": session_id } }).to_string();
    Some((frame, ack))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp_http::pick_page_ws_url;

    #[test]
    fn header_matches_the_ts_decoder_layout() {
        let img = [0xAAu8, 0xBB, 0xCC];
        let bytes = encode_frame(7, FrameFormat::Jpeg, &FrameMetadata::default(), &img);

        // Fixed header bytes.
        assert_eq!(bytes[0], 0x62, "kind");
        assert_eq!(bytes[1], 1, "version");
        assert_eq!(bytes[2], 1, "opcode = Frame");
        assert_eq!(bytes[3], 1, "format = jpeg");
        // seq u32 LE at [4..8].
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 7);
        // metadata length u32 LE at [8..12] — default metadata serializes to "{}".
        let md_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        assert_eq!(&bytes[16..16 + md_len], b"{}");
        // reserved u32 LE at [12..16] MUST be 0 (pane rejects otherwise).
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 0);
        // image bytes follow the metadata.
        assert_eq!(&bytes[16 + md_len..], &img);
    }

    #[test]
    fn png_format_byte_and_total_length() {
        let img = vec![1u8; 100];
        let bytes = encode_frame(0, FrameFormat::Png, &FrameMetadata::default(), &img);
        assert_eq!(bytes[3], 2, "format = png");
        // 16 header + 2 ("{}") metadata + 100 image.
        assert_eq!(bytes.len(), HEADER_BYTES + 2 + 100);
    }

    #[test]
    fn metadata_uses_the_exact_ts_key_names() {
        let md = FrameMetadata {
            device_width: Some(1280.0),
            device_height: Some(800.0),
            image_width: Some(1280.0),
            image_height: Some(800.0),
            timestamp: Some(123.0),
            ..Default::default()
        };
        let bytes = encode_frame(1, FrameFormat::Jpeg, &md, &[]);
        let md_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&bytes[16..16 + md_len]).unwrap();
        // Keys must match METADATA_KEYS in browser-screencast-protocol.ts.
        assert!(json.contains("\"deviceWidth\":1280"));
        assert!(json.contains("\"deviceHeight\":800"));
        assert!(json.contains("\"imageWidth\":1280"));
        assert!(json.contains("\"timestamp\":123"));
        // Omitted (None) fields must not appear.
        assert!(!json.contains("pageScaleFactor"));
        assert!(!json.contains("scrollOffsetX"));
    }

    // --- CDP client: pure translation + parsing -----------------------------

    #[test]
    fn parses_each_browser_rpc() {
        assert_eq!(
            parse_input_message(r#"{"method":"browser.mouseMove","params":{"x":12,"y":34}}"#),
            Some(InputCommand::MouseMove { x: 12.0, y: 34.0 })
        );
        assert_eq!(
            parse_input_message(r#"{"method":"browser.mouseDown","params":{"button":"right"}}"#),
            Some(InputCommand::MouseDown {
                button: MouseButton::Right
            })
        );
        // Missing button defaults to left.
        assert_eq!(
            parse_input_message(r#"{"method":"browser.mouseUp","params":{}}"#),
            Some(InputCommand::MouseUp {
                button: MouseButton::Left
            })
        );
        assert_eq!(
            parse_input_message(r#"{"method":"browser.mouseWheel","params":{"dx":0,"dy":-120}}"#),
            Some(InputCommand::MouseWheel {
                dx: 0.0,
                dy: -120.0
            })
        );
        assert_eq!(
            parse_input_message(r#"{"method":"browser.keypress","params":{"key":"Enter"}}"#),
            Some(InputCommand::KeyPress {
                key: "Enter".into()
            })
        );
        assert_eq!(
            parse_input_message(r#"{"method":"browser.goto","params":{"url":"https://x.test"}}"#),
            Some(InputCommand::Goto {
                url: "https://x.test".into()
            })
        );
        assert_eq!(
            parse_input_message(r#"{"method":"browser.reload","params":{}}"#),
            Some(InputCommand::Reload)
        );
        // Unknown / malformed methods are ignored, never an error.
        assert_eq!(parse_input_message(r#"{"method":"browser.unknown"}"#), None);
        assert_eq!(parse_input_message("not json"), None);
    }

    #[test]
    fn insert_text_parses_and_maps_to_cdp_insert_text() {
        // Paste (F3): the pane sends browser.insertText with the clipboard text…
        assert_eq!(
            parse_input_message(
                r#"{"method":"browser.insertText","params":{"text":"hello world"}}"#
            ),
            Some(InputCommand::InsertText {
                text: "hello world".into()
            })
        );
        // Missing text → ignored, never an error (bridge stays up).
        assert_eq!(
            parse_input_message(r#"{"method":"browser.insertText","params":{}}"#),
            None
        );
        // …and maps to exactly one Input.insertText carrying the verbatim text —
        // NOT a synthetic keypress (which Chromium won't turn into a real paste).
        let mut p = PointerState::default();
        let mut id = 0u64;
        let out = input_command_to_cdp(
            &InputCommand::InsertText {
                text: "pasted".into(),
            },
            &mut p,
            &mut id,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["method"], "Input.insertText");
        assert_eq!(out[0]["params"]["text"], "pasted");
        // Paste is a human action (grabs the co-browse wheel like a keypress).
        assert!(InputCommand::InsertText {
            text: "x".into()
        }
        .is_human_action());
    }

    #[test]
    fn mouse_down_uses_the_last_moved_position() {
        let mut p = PointerState::default();
        let mut id = 0u64;
        // Move sets the remembered position…
        input_command_to_cdp(
            &InputCommand::MouseMove { x: 50.0, y: 60.0 },
            &mut p,
            &mut id,
        );
        // …which a button press (carrying no coords) then targets.
        let cmds = input_command_to_cdp(
            &InputCommand::MouseDown {
                button: MouseButton::Left,
            },
            &mut p,
            &mut id,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0]["method"], "Input.dispatchMouseEvent");
        assert_eq!(cmds[0]["params"]["type"], "mousePressed");
        assert_eq!(cmds[0]["params"]["x"], 50.0);
        assert_eq!(cmds[0]["params"]["y"], 60.0);
        assert_eq!(cmds[0]["params"]["button"], "left");
        assert_eq!(cmds[0]["params"]["buttons"], 1);
    }

    #[test]
    fn printable_key_is_one_char_event_named_key_is_a_down_up_pair() {
        let mut p = PointerState::default();
        let mut id = 0u64;
        // A printable character → a single `char` event with text.
        let ch = input_command_to_cdp(&InputCommand::KeyPress { key: "a".into() }, &mut p, &mut id);
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0]["params"]["type"], "char");
        assert_eq!(ch[0]["params"]["text"], "a");
        // A named key → keyDown + keyUp with the VK code; Enter carries "\r".
        let enter = input_command_to_cdp(
            &InputCommand::KeyPress {
                key: "Enter".into(),
            },
            &mut p,
            &mut id,
        );
        assert_eq!(enter.len(), 2);
        assert_eq!(enter[0]["params"]["type"], "keyDown");
        assert_eq!(enter[0]["params"]["windowsVirtualKeyCode"], 13);
        assert_eq!(enter[0]["params"]["text"], "\r");
        assert_eq!(enter[1]["params"]["type"], "keyUp");
    }

    #[test]
    fn nav_commands_map_to_cdp() {
        let mut p = PointerState::default();
        let mut id = 0u64;
        let goto = input_command_to_cdp(
            &InputCommand::Goto {
                url: "https://a.test".into(),
            },
            &mut p,
            &mut id,
        );
        assert_eq!(goto[0]["method"], "Page.navigate");
        assert_eq!(goto[0]["params"]["url"], "https://a.test");

        let back = input_command_to_cdp(&InputCommand::Back, &mut p, &mut id);
        assert_eq!(back[0]["method"], "Runtime.evaluate");
        assert_eq!(back[0]["params"]["expression"], "history.back()");

        let reload = input_command_to_cdp(&InputCommand::Reload, &mut p, &mut id);
        assert_eq!(reload[0]["method"], "Page.reload");
    }

    #[test]
    fn set_viewport_parses_and_maps_to_device_metrics_override() {
        let cmd = parse_input_message(
            r#"{"method":"browser.setViewport","params":{"width":375,"height":812,"deviceScaleFactor":2}}"#,
        )
        .expect("parse browser.setViewport");
        assert_eq!(
            cmd,
            InputCommand::SetViewport {
                width: 375,
                height: 812,
                device_scale_factor: 2.0,
            }
        );

        let mut p = PointerState::default();
        let mut id = 0u64;
        let out = input_command_to_cdp(&cmd, &mut p, &mut id);
        assert_eq!(out[0]["method"], "Emulation.setDeviceMetricsOverride");
        assert_eq!(out[0]["params"]["width"], 375);
        assert_eq!(out[0]["params"]["height"], 812);
        assert_eq!(out[0]["params"]["deviceScaleFactor"], 2.0);
        // mobile stays false — this drives desktop responsive layout, not device emulation.
        assert_eq!(out[0]["params"]["mobile"], false);
    }

    #[test]
    fn set_viewport_defaults_dsf_and_floors_zero_dimensions() {
        // deviceScaleFactor omitted → defaults to 1.0.
        let cmd = parse_input_message(
            r#"{"method":"browser.setViewport","params":{"width":1280,"height":720}}"#,
        )
        .expect("parse without deviceScaleFactor");
        assert_eq!(
            cmd,
            InputCommand::SetViewport {
                width: 1280,
                height: 720,
                device_scale_factor: 1.0,
            }
        );

        // A 0 dimension (and 0 scale) would make Chrome reject the override; floor them.
        let mut p = PointerState::default();
        let mut id = 0u64;
        let out = input_command_to_cdp(
            &InputCommand::SetViewport {
                width: 0,
                height: 0,
                device_scale_factor: 0.0,
            },
            &mut p,
            &mut id,
        );
        assert_eq!(out[0]["params"]["width"], 1);
        assert_eq!(out[0]["params"]["height"], 1);
        assert_eq!(out[0]["params"]["deviceScaleFactor"], 1.0);
    }

    #[test]
    fn cdp_ids_are_monotonic_across_commands() {
        let mut p = PointerState::default();
        let mut id = 0u64;
        let a = input_command_to_cdp(&InputCommand::Back, &mut p, &mut id);
        let b = input_command_to_cdp(&InputCommand::Reload, &mut p, &mut id);
        assert_eq!(a[0]["id"], 1);
        assert_eq!(b[0]["id"], 2);
    }

    #[test]
    fn is_human_action_excludes_passive_events() {
        assert!(
            InputCommand::MouseDown {
                button: MouseButton::Left
            }
            .is_human_action()
        );
        assert!(InputCommand::KeyPress { key: "a".into() }.is_human_action());
        assert!(InputCommand::Reload.is_human_action());
        // passive / automatic — must NOT grab the co-browse wheel.
        assert!(!InputCommand::MouseMove { x: 1.0, y: 2.0 }.is_human_action());
        assert!(
            !InputCommand::SetViewport {
                width: 800,
                height: 600,
                device_scale_factor: 1.0
            }
            .is_human_action()
        );
    }

    #[test]
    fn maps_cdp_metadata_to_frame_metadata() {
        let cdp = json!({
            "offsetTop": 0.0, "pageScaleFactor": 1.0,
            "deviceWidth": 1280.0, "deviceHeight": 800.0,
            "scrollOffsetX": 0.0, "scrollOffsetY": 120.0, "timestamp": 42.0
        });
        let md = metadata_from_cdp(&cdp);
        assert_eq!(md.device_width, Some(1280.0));
        assert_eq!(md.device_height, Some(800.0));
        // CDP has no image size; we fill it from the device size.
        assert_eq!(md.image_width, Some(1280.0));
        assert_eq!(md.image_height, Some(800.0));
        assert_eq!(md.scroll_offset_y, Some(120.0));
        assert_eq!(md.timestamp, Some(42.0));
    }

    #[test]
    fn picks_first_page_target_ws_url() {
        let listing = json!([
            {"type": "background_page", "webSocketDebuggerUrl": "ws://x/bg"},
            {"type": "page", "webSocketDebuggerUrl": "ws://127.0.0.1:9300/devtools/page/ABC"},
            {"type": "page", "webSocketDebuggerUrl": "ws://x/second"}
        ]);
        assert_eq!(
            pick_page_ws_url(&listing).as_deref(),
            Some("ws://127.0.0.1:9300/devtools/page/ABC")
        );
        // No page target → None (the bridge then fails loud).
        assert_eq!(pick_page_ws_url(&json!([{"type": "worker"}])), None);
    }

    #[test]
    fn screencast_frame_is_decoded_and_acked() {
        // A synthetic CDP `Page.screencastFrame` (base64 of [0xDE,0xAD]) must decode
        // to a valid 0x62 frame AND yield an ack — without a real browser. A
        // non-frame message yields neither. The decode is pure (the caller acks then
        // forwards latest-wins), so this needs no channel.
        let mut seq = 0u32;
        let data = base64::engine::general_purpose::STANDARD.encode([0xDE, 0xAD]);
        let frame_msg = json!({
            "method": "Page.screencastFrame",
            "params": { "data": data, "sessionId": 7, "metadata": { "deviceWidth": 800.0 } }
        })
        .to_string();

        let (frame, ack) =
            decode_cdp_frame(&frame_msg, FrameFormat::Jpeg, &mut seq).expect("a frame must decode");
        let ackv: Value = serde_json::from_str(&ack).unwrap();
        assert_eq!(ackv["method"], "Page.screencastFrameAck");
        assert_eq!(ackv["params"]["sessionId"], 7);

        let decoded = decode_for_test(&frame);
        assert_eq!(decoded, vec![0xDEu8, 0xAD]);
        assert_eq!(seq, 1, "seq advances per frame");

        // A CDP command response (no `method`) is not a frame.
        assert!(decode_cdp_frame(r#"{"id":2,"result":{}}"#, FrameFormat::Jpeg, &mut seq).is_none());
    }

    /// Strip the 16-byte header + metadata JSON, returning the image bytes — the
    /// inverse of [`encode_frame`] enough to assert the payload survived.
    fn decode_for_test(bytes: &[u8]) -> Vec<u8> {
        let md_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        bytes[16 + md_len..].to_vec()
    }
}
