//! Host-resident headless browser (spec 009a, Phase 1).
//!
//! Launches a headless Chromium **on the host** — in its own persistent tmux
//! session — with a CDP debugger bound to the host's loopback. The caller then
//! forward-tunnels that CDP port to the Mac (`host_runtime::ensure_forward_tunnel`)
//! and, in later phases, drives a screencast over it.
//!
//! Because the browser lives in a host tmux session it survives the Mac
//! sleeping / agentum closing; reconnect is a deterministic per-worktree lookup
//! (`agentum-hostbrowser-<wt>`), not a relaunch. Teardown is `kill_session` —
//! headless Chromium ignores `C-c`, so a graceful stop never reaps it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use agentum_core::Host;
use axum::extract::ws::{Message as DesktopMessage, WebSocket as DesktopWebSocket};
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Message as CdpMessage;

use crate::host_runtime::{self, HostRuntimeError, Result};

/// Browser binary launched on the host. Preflight / selection across
/// `chromium-browser` / `google-chrome` is a later phase; Phase 1 targets the
/// common `chromium` (present on the Arch/Omarchy test host at `/usr/bin/chromium`).
const CHROMIUM_BIN: &str = "chromium";

/// How long to wait for headless Chromium to bind its CDP port and write the
/// `DevToolsActivePort` file before giving up (a cold start + MCP-free boot is
/// a second or two; allow generous slack on a distant host).
const CDP_READY_TIMEOUT: Duration = Duration::from_secs(20);
const CDP_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A host-resident browser launched (or re-attached) for one worktree.
#[derive(Debug, Clone)]
pub struct HostBrowser {
    /// Sanitized worktree slug (the `<wt>` in the deterministic names).
    pub workdir_slug: String,
    /// tmux session name on the host (`agentum-hostbrowser-<wt>`).
    pub tmux_target: String,
    /// Chromium `--user-data-dir` on the host (`/tmp/agentum-hostbrowser-<wt>`).
    pub user_data_dir: String,
    /// The CDP port Chromium bound on the host's loopback (from DevToolsActivePort).
    pub cdp_port: u16,
    /// True when launch re-attached to an already-running session (reconnect).
    pub attached: bool,
}

/// Sanitized worktree slug from a workdir basename — `[A-Za-z0-9-]`, mirroring
/// the harness `sanitize`. Deterministic per worktree so reconnect is a lookup;
/// an empty/`/`-only path degrades to `default` rather than producing an empty
/// tmux/dir name.
fn workdir_slug(workdir: &Path) -> String {
    let base = workdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("default");
    let slug: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if slug.is_empty() {
        "default".to_string()
    } else {
        slug
    }
}

/// tmux session name for a worktree's host browser. `agentum-hostbrowser-<wt>`.
fn host_browser_target(slug: &str) -> String {
    agentum_tmux::target_for(&format!("hostbrowser-{slug}"))
}

/// Chromium `--user-data-dir` for a worktree. Deterministic so reconnect finds
/// the same DevToolsActivePort; `/tmp` keeps it isolated + auto-cleared on reboot.
fn host_browser_user_data_dir(slug: &str) -> String {
    format!("/tmp/agentum-hostbrowser-{slug}")
}

/// Path to Chromium's `DevToolsActivePort` file under a user-data dir.
fn devtools_active_port_path(user_data_dir: &str) -> String {
    format!("{user_data_dir}/DevToolsActivePort")
}

/// Parse the bound CDP port from a `DevToolsActivePort` file: its first line is
/// the port, its second the browser WS path. We only need the port.
fn parse_devtools_active_port(contents: &str) -> Option<u16> {
    contents.lines().next()?.trim().parse::<u16>().ok()
}

/// Chromium argv for a headless, **loopback-only** CDP browser. Port `0` →
/// Chromium picks a free port and records it in `<user_data_dir>/DevToolsActivePort`.
fn chromium_argv(bin: &str, user_data_dir: &str) -> Vec<String> {
    vec![
        bin.to_string(),
        "--headless=new".to_string(),
        // Security invariant: CDP reachable only via the SSH tunnel, never a
        // public interface.
        "--remote-debugging-address=127.0.0.1".to_string(),
        // 0 → Chromium binds a free port and writes it to DevToolsActivePort.
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={user_data_dir}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        // Most remote hosts are headless servers with no GPU.
        "--disable-gpu".to_string(),
        "about:blank".to_string(),
    ]
}

/// The full tmux pane command: clear any stale `DevToolsActivePort` (from a
/// prior crashed run, so the await loop can't read a dead port), then `exec`
/// Chromium as the pane's process. Wrapped in `sh -c` so the `rm`/`exec`
/// sequence runs regardless of the host's login shell.
fn launch_command(bin: &str, user_data_dir: &str, port_file: &str) -> Result<Vec<String>> {
    let argv = chromium_argv(bin, user_data_dir);
    let joined =
        shlex::try_join(argv.iter().map(String::as_str)).map_err(|_| HostRuntimeError::Quote)?;
    let pf = shlex::try_quote(port_file).map_err(|_| HostRuntimeError::Quote)?;
    let inner = format!("rm -f {pf}; exec {joined}");
    Ok(vec!["sh".to_string(), "-c".to_string(), inner])
}

// ── Screencast wire protocol (server → desktop) ───────────────────────────
// Byte-compatible with ui/src/shared/browser-screencast-protocol.ts so the
// dormant RemoteBrowserPagePane decodes our frames unchanged.

const SCREENCAST_KIND: u8 = 0x62;
const SCREENCAST_VERSION: u8 = 1;
const SCREENCAST_OPCODE_FRAME: u8 = 1;
const SCREENCAST_FORMAT_JPEG: u8 = 1;
const SCREENCAST_HEADER_BYTES: usize = 16;

/// Per-frame metadata mirrored from CDP `Page.screencastFrame.metadata` into the
/// keys the UI's `decodeFrameMetadata` reads (camelCase, all optional — absent
/// fields are omitted, never serialized as null).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreencastMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_top: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_scale_factor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_offset_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_offset_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
}

/// Encode one screencast frame to the wire format the desktop decodes: a 16-byte
/// little-endian header (kind, version, opcode, format, seq, metadata length,
/// reserved), the metadata JSON, then the raw JPEG bytes. Mirrors
/// `encodeBrowserScreencastFrame` in browser-screencast-protocol.ts.
fn encode_screencast_frame(seq: u32, jpeg: &[u8], metadata: &ScreencastMetadata) -> Vec<u8> {
    let meta_json = serde_json::to_vec(metadata).unwrap_or_else(|_| b"{}".to_vec());
    let mut out = Vec::with_capacity(SCREENCAST_HEADER_BYTES + meta_json.len() + jpeg.len());
    out.push(SCREENCAST_KIND);
    out.push(SCREENCAST_VERSION);
    out.push(SCREENCAST_OPCODE_FRAME);
    out.push(SCREENCAST_FORMAT_JPEG);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&(meta_json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved word (must be 0)
    out.extend_from_slice(&meta_json);
    out.extend_from_slice(jpeg);
    out
}

/// Re-point a Chromium-reported DevTools WS URL at the Mac-forwarded port:
/// Chromium reports its own bound (host) port, but we reach it through the SSH
/// `-L` tunnel, so keep the `/devtools/page/<id>` path and swap the authority to
/// `127.0.0.1:<mac_port>`. `None` if the input has no path component.
fn page_ws_url_for_mac(reported: &str, mac_port: u16) -> Option<String> {
    let after_scheme = reported.split_once("://")?.1;
    let (_authority, path) = after_scheme.split_once('/')?;
    Some(format!("ws://127.0.0.1:{mac_port}/{path}"))
}

/// Map a CDP `Page.screencastFrame.metadata` object to our wire metadata. Only
/// finite numbers are carried; absent fields stay `None` (omitted on the wire).
fn metadata_from_cdp(meta: &serde_json::Value) -> ScreencastMetadata {
    let f = |k: &str| meta.get(k).and_then(serde_json::Value::as_f64);
    ScreencastMetadata {
        offset_top: f("offsetTop"),
        page_scale_factor: f("pageScaleFactor"),
        device_width: f("deviceWidth"),
        device_height: f("deviceHeight"),
        image_width: f("imageWidth"),
        image_height: f("imageHeight"),
        scroll_offset_x: f("scrollOffsetX"),
        scroll_offset_y: f("scrollOffsetY"),
        timestamp: f("timestamp"),
    }
}

/// Build one CDP JSON-RPC command envelope: `{"id":N,"method":M,"params":P}`.
fn cdp_command(id: u64, method: &str, params: serde_json::Value) -> String {
    serde_json::json!({ "id": id, "method": method, "params": params }).to_string()
}

/// Map one inbound scratch-protocol input message (JSON text) to zero or more CDP
/// `(method, params)` calls (the caller assigns ids). Pure so the mapping is
/// unit-testable; unknown/garbage input yields no calls (never panics).
///
/// Scratch protocol (Phase 2 standalone WS; the desktop's runtime-environments
/// `browser.*` RPC is reconciled to this in Phase 3):
///   {"type":"mouse","action":"move|down|up","x":N,"y":N,"button":"left|middle|right"}
///   {"type":"wheel","x":N,"y":N,"dx":N,"dy":N}
///   {"type":"key","key":"a" | "Enter" | …}
fn input_to_cdp_calls(text: &str) -> Vec<(String, serde_json::Value)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let num = |k: &str| v.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    match v.get("type").and_then(serde_json::Value::as_str) {
        Some("mouse") => {
            let cdp_type = match v.get("action").and_then(serde_json::Value::as_str) {
                Some("move") => "mouseMoved",
                Some("down") => "mousePressed",
                Some("up") => "mouseReleased",
                _ => return Vec::new(),
            };
            let button = v
                .get("button")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("left");
            let mut params = serde_json::json!({ "type": cdp_type, "x": num("x"), "y": num("y") });
            // CDP wants a button + clickCount on press/release, not on a bare move.
            if cdp_type != "mouseMoved" {
                params["button"] = serde_json::json!(button);
                params["clickCount"] = serde_json::json!(1);
            }
            vec![("Input.dispatchMouseEvent".to_string(), params)]
        }
        Some("wheel") => {
            let params = serde_json::json!({
                "type": "mouseWheel", "x": num("x"), "y": num("y"),
                "deltaX": num("dx"), "deltaY": num("dy")
            });
            vec![("Input.dispatchMouseEvent".to_string(), params)]
        }
        // In-band navigation: routed through the bridge's single CDP connection,
        // so we never open a second client to the page target (Chromium can
        // reject concurrent attachers).
        Some("navigate") => match v.get("url").and_then(serde_json::Value::as_str) {
            Some(url) => vec![(
                "Page.navigate".to_string(),
                serde_json::json!({ "url": url }),
            )],
            None => Vec::new(),
        },
        Some("key") => {
            let Some(key) = v.get("key").and_then(serde_json::Value::as_str) else {
                return Vec::new();
            };
            // A single printable char types reliably via insertText; named keys
            // (Enter, Backspace, arrows, …) need a keyDown/keyUp pair.
            let chars: Vec<char> = key.chars().collect();
            if chars.len() == 1 && !chars[0].is_control() {
                return vec![(
                    "Input.insertText".to_string(),
                    serde_json::json!({ "text": key }),
                )];
            }
            vec![
                (
                    "Input.dispatchKeyEvent".to_string(),
                    serde_json::json!({ "type": "keyDown", "key": key }),
                ),
                (
                    "Input.dispatchKeyEvent".to_string(),
                    serde_json::json!({ "type": "keyUp", "key": key }),
                ),
            ]
        }
        _ => Vec::new(),
    }
}

/// Poll the host's `DevToolsActivePort` until Chromium has written a parseable
/// port, or time out (the bind is async to process start).
async fn await_cdp_port(host: &Host, port_file: &str) -> Result<u16> {
    let deadline = Instant::now() + CDP_READY_TIMEOUT;
    loop {
        if let Some(bytes) = host_runtime::read_file_bytes(host, port_file).await? {
            if let Some(port) = parse_devtools_active_port(&String::from_utf8_lossy(&bytes)) {
                return Ok(port);
            }
        }
        if Instant::now() >= deadline {
            return Err(HostRuntimeError::Bootstrap(format!(
                "headless Chromium did not write {port_file} within {}s",
                CDP_READY_TIMEOUT.as_secs()
            )));
        }
        sleep(CDP_POLL_INTERVAL).await;
    }
}

/// Launch (or **re-attach** to) the host browser for `workdir`. Returns once the
/// CDP port is known. The caller forward-tunnels `cdp_port` to reach it.
///
/// Re-attach is a lookup: an existing `agentum-hostbrowser-<wt>` session means
/// the browser is still running (it outlived a Mac sleep / agentum close), so we
/// read its current CDP port rather than relaunching.
pub async fn launch_host_browser(host: &Host, workdir: &Path) -> Result<HostBrowser> {
    let slug = workdir_slug(workdir);
    let target = host_browser_target(&slug);
    let user_data_dir = host_browser_user_data_dir(&slug);
    let port_file = devtools_active_port_path(&user_data_dir);

    let attached = host_runtime::has_session(host, &target).await?;
    if !attached {
        let cmd = launch_command(CHROMIUM_BIN, &user_data_dir, &port_file)?;
        host_runtime::new_session(host, &target, workdir, &cmd, &[]).await?;
    }

    let cdp_port = await_cdp_port(host, &port_file).await?;

    // Best-effort per-worktree marker so a later reconnect can find the port by
    // lookup. DevToolsActivePort is the live source of truth; this is a copy.
    let _ = host_runtime::write_home_relative_file(
        host,
        &format!(".agentum/hostbrowser/{slug}.port"),
        &format!("{cdp_port}\n"),
    )
    .await;

    Ok(HostBrowser {
        workdir_slug: slug,
        tmux_target: target,
        user_data_dir,
        cdp_port,
        attached,
    })
}

/// Kill the worktree's host browser (tmux session). `kill_session`, **not** a
/// graceful `C-c` — headless Chromium ignores SIGINT and would linger.
pub async fn teardown_host_browser(host: &Host, workdir: &Path) -> Result<()> {
    let target = host_browser_target(&workdir_slug(workdir));
    host_runtime::kill_session(host, &target).await
}

// ── Lifecycle registry (id → live bridge) ─────────────────────────────────
// One headless browser per worktree; the id is the worktree slug, so reconnect
// is a lookup. Kept module-local (not on AppState) so the route layer stays a
// thin view and AppState's many construction sites are untouched.

#[derive(Clone)]
struct BridgeEntry {
    host: Host,
    workdir: PathBuf,
    tmux_target: String,
    /// CDP port Chromium bound on the host loopback.
    cdp_host_port: u16,
    /// Mac loopback port the `-L` tunnel forwards to that CDP port.
    mac_port: u16,
}

fn registry() -> &'static Mutex<HashMap<String, BridgeEntry>> {
    static REG: OnceLock<Mutex<HashMap<String, BridgeEntry>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Result of starting (or re-attaching) a host browser for the route layer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StartedHostBrowser {
    pub id: String,
    pub attached: bool,
    pub mac_port: u16,
    pub cdp_host_port: u16,
}

/// Status snapshot for `GET /api/host-browser/{id}`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostBrowserStatus {
    pub id: String,
    pub mac_port: u16,
    pub cdp_host_port: u16,
    pub tmux_running: bool,
    pub cdp_reachable: bool,
}

/// Start (or re-attach to) the host browser for `workdir` and forward-tunnel its
/// CDP port, registering it for later screencast/navigate/status/stop by id.
pub async fn start_host_browser(host: &Host, workdir: &Path) -> Result<StartedHostBrowser> {
    let browser = launch_host_browser(host, workdir).await?;
    let mac_port = host_runtime::ensure_forward_tunnel(host, browser.cdp_port).await?;
    // Block "ready" on a reachable CDP port so a client never connects a dead WS.
    if !cdp_tcp_reachable(mac_port).await {
        return Err(HostRuntimeError::Bootstrap(format!(
            "CDP port not reachable through the tunnel at 127.0.0.1:{mac_port}"
        )));
    }
    let id = browser.workdir_slug.clone();
    registry().lock().await.insert(
        id.clone(),
        BridgeEntry {
            host: host.clone(),
            workdir: workdir.to_path_buf(),
            tmux_target: browser.tmux_target,
            cdp_host_port: browser.cdp_port,
            mac_port,
        },
    );
    Ok(StartedHostBrowser {
        id,
        attached: browser.attached,
        mac_port,
        cdp_host_port: browser.cdp_port,
    })
}

/// Drive the screencast for a registered browser onto `desktop_ws` (frames out,
/// input in). Returns when either side closes; a no-op if the id is unknown.
pub async fn run_screencast(id: &str, desktop_ws: DesktopWebSocket) {
    let Some(entry) = registry().lock().await.get(id).cloned() else {
        return; // unknown id → drop the socket (closes)
    };
    if let Err(e) = run_screencast_bridge(desktop_ws, entry.mac_port, None).await {
        tracing::warn!(error = ?e, id, "host-browser screencast bridge ended");
    }
}

/// One-shot navigate for a registered browser (the host app's `localhost:PORT`).
pub async fn navigate(id: &str, url: &str) -> Result<()> {
    let entry = lookup(id).await?;
    let page_ws = discover_page_ws(entry.mac_port).await?;
    let (cdp_ws, _) = tokio_tungstenite::connect_async(page_ws.as_str())
        .await
        .map_err(|e| HostRuntimeError::Bootstrap(format!("CDP connect for navigate: {e}")))?;
    let (mut sink, _src) = cdp_ws.split();
    sink.send(CdpMessage::Text(
        cdp_command(1, "Page.navigate", serde_json::json!({ "url": url })).into(),
    ))
    .await
    .map_err(|e| HostRuntimeError::Bootstrap(format!("CDP navigate send: {e}")))?;
    // Give Chromium a beat to accept the command before the socket drops.
    sleep(Duration::from_millis(150)).await;
    Ok(())
}

/// Status of a registered browser (or `None` when unknown).
pub async fn status(id: &str) -> Option<HostBrowserStatus> {
    let entry = registry().lock().await.get(id).cloned()?;
    let tmux_running = host_runtime::has_session(&entry.host, &entry.tmux_target)
        .await
        .unwrap_or(false);
    let cdp_reachable = cdp_tcp_reachable(entry.mac_port).await;
    Some(HostBrowserStatus {
        id: id.to_string(),
        mac_port: entry.mac_port,
        cdp_host_port: entry.cdp_host_port,
        tmux_running,
        cdp_reachable,
    })
}

/// Stop a registered browser: kill its tmux session and forget it. (The `-L`
/// tunnel drops with its SSH channel; the next start cancel-then-arms anyway.)
pub async fn stop(id: &str) -> Result<()> {
    let entry = registry().lock().await.remove(id);
    if let Some(e) = entry {
        teardown_host_browser(&e.host, &e.workdir).await?;
    }
    Ok(())
}

async fn lookup(id: &str) -> Result<BridgeEntry> {
    registry()
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| HostRuntimeError::Bootstrap(format!("unknown host browser id: {id}")))
}

/// True when the CDP port answers a TCP connect through the tunnel.
async fn cdp_tcp_reachable(mac_port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", mac_port))
        .await
        .is_ok()
}

/// Discover the page target's DevTools WS URL via `GET /json`, re-pointed at the
/// Mac-forwarded port (Chromium reports its own host-side port).
async fn discover_page_ws(mac_port: u16) -> Result<String> {
    let url = format!("http://127.0.0.1:{mac_port}/json");
    let targets: Vec<serde_json::Value> = reqwest::Client::new()
        .get(url.as_str())
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| HostRuntimeError::Bootstrap(format!("CDP /json request failed: {e}")))?
        .json()
        .await
        .map_err(|e| HostRuntimeError::Bootstrap(format!("CDP /json parse failed: {e}")))?;
    let reported = targets
        .iter()
        .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
        .and_then(|t| t.get("webSocketDebuggerUrl"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| HostRuntimeError::Bootstrap("no CDP page target found".into()))?;
    page_ws_url_for_mac(reported, mac_port)
        .ok_or_else(|| HostRuntimeError::Bootstrap(format!("unparseable CDP ws url: {reported}")))
}

/// The bidirectional bridge: CDP screencast frames → desktop (binary, 0x62
/// protocol); desktop input → CDP `Input.dispatch*`. A single writer task owns
/// the CDP sink (acks + input both feed it via an mpsc) so the two pumps never
/// race on it. Returns when either side closes.
async fn run_screencast_bridge(
    desktop_ws: DesktopWebSocket,
    mac_port: u16,
    navigate_url: Option<String>,
) -> Result<()> {
    let page_ws = discover_page_ws(mac_port).await?;
    let (cdp_ws, _) = tokio_tungstenite::connect_async(page_ws.as_str())
        .await
        .map_err(|e| HostRuntimeError::Bootstrap(format!("CDP connect: {e}")))?;
    let (mut cdp_sink, mut cdp_src) = cdp_ws.split();
    let (dt_sink, mut dt_src) = desktop_ws.split();
    let dt_sink = Arc::new(Mutex::new(dt_sink));

    // Single CDP writer fed by an mpsc: the ack path and the input path both push
    // commands here, so they never contend on the sink.
    let (cdp_tx, mut cdp_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(cmd) = cdp_rx.recv().await {
            if cdp_sink.send(CdpMessage::Text(cmd.into())).await.is_err() {
                break;
            }
        }
    });

    let ids = Arc::new(AtomicU64::new(1));
    let next_id = || ids.fetch_add(1, Ordering::Relaxed);

    // Setup: enable Page events, optionally navigate, then start the screencast.
    let _ = cdp_tx.send(cdp_command(next_id(), "Page.enable", serde_json::json!({})));
    if let Some(url) = navigate_url {
        let _ = cdp_tx.send(cdp_command(
            next_id(),
            "Page.navigate",
            serde_json::json!({ "url": url }),
        ));
    }
    let _ = cdp_tx.send(cdp_command(
        next_id(),
        "Page.startScreencast",
        serde_json::json!({ "format": "jpeg", "quality": 70, "everyNthFrame": 1 }),
    ));

    // Frame pump: CDP screencastFrame → desktop binary, then ACK (CDP pauses the
    // stream until each frame is acked).
    let frames = {
        let cdp_tx = cdp_tx.clone();
        let dt_sink = dt_sink.clone();
        let ids = ids.clone();
        tokio::spawn(async move {
            let mut seq: u32 = 0;
            while let Some(Ok(msg)) = cdp_src.next().await {
                let CdpMessage::Text(txt) = msg else { continue };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(txt.as_str()) else {
                    continue;
                };
                if v.get("method").and_then(|m| m.as_str()) != Some("Page.screencastFrame") {
                    continue;
                }
                let params = &v["params"];
                let session_id = params
                    .get("sessionId")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                if let Some(data_b64) = params.get("data").and_then(|d| d.as_str()) {
                    if let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data_b64) {
                        let meta = metadata_from_cdp(&params["metadata"]);
                        let frame = encode_screencast_frame(seq, &jpeg, &meta);
                        seq = seq.wrapping_add(1);
                        let mut s = dt_sink.lock().await;
                        if s.send(DesktopMessage::Binary(frame.into())).await.is_err() {
                            break;
                        }
                    }
                }
                let _ = cdp_tx.send(cdp_command(
                    ids.fetch_add(1, Ordering::Relaxed),
                    "Page.screencastFrameAck",
                    serde_json::json!({ "sessionId": session_id }),
                ));
            }
        })
    };

    // Input pump: desktop text → CDP Input.dispatch*.
    let input = {
        let cdp_tx = cdp_tx.clone();
        let ids = ids.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = dt_src.next().await {
                match msg {
                    DesktopMessage::Text(t) => {
                        for (method, params) in input_to_cdp_calls(t.as_str()) {
                            let _ = cdp_tx.send(cdp_command(
                                ids.fetch_add(1, Ordering::Relaxed),
                                &method,
                                params,
                            ));
                        }
                    }
                    DesktopMessage::Close(_) => break,
                    _ => {}
                }
            }
        })
    };

    // Run until either pump ends (CDP closed or desktop disconnected).
    tokio::select! {
        _ = frames => {},
        _ = input => {},
    }
    writer.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn workdir_slug_sanitizes_basename_and_defaults_empty() {
        assert_eq!(workdir_slug(Path::new("/home/malloc/My Repo")), "My-Repo");
        assert_eq!(workdir_slug(Path::new("/home/malloc/repo.git")), "repo-git");
        // No basename (root) → a stable fallback, never an empty name.
        assert_eq!(workdir_slug(Path::new("/")), "default");
    }

    #[test]
    fn names_are_deterministic_per_worktree() {
        let slug = "myrepo";
        assert_eq!(host_browser_target(slug), "agentum-hostbrowser-myrepo");
        assert_eq!(
            host_browser_user_data_dir(slug),
            "/tmp/agentum-hostbrowser-myrepo"
        );
        assert_eq!(
            devtools_active_port_path("/tmp/agentum-hostbrowser-myrepo"),
            "/tmp/agentum-hostbrowser-myrepo/DevToolsActivePort"
        );
    }

    #[test]
    fn parse_devtools_active_port_reads_first_line() {
        // The real file: port on line 1, browser WS path on line 2.
        assert_eq!(
            parse_devtools_active_port("45821\n/devtools/browser/abc-123\n"),
            Some(45821)
        );
        assert_eq!(parse_devtools_active_port("  45821  \n"), Some(45821));
        assert_eq!(parse_devtools_active_port(""), None);
        assert_eq!(parse_devtools_active_port("not-a-port\n"), None);
    }

    #[test]
    fn chromium_argv_is_headless_and_loopback_only() {
        let argv = chromium_argv("chromium", "/tmp/agentum-hostbrowser-myrepo");
        assert_eq!(argv[0], "chromium", "binary must lead the argv");
        assert!(argv.iter().any(|a| a == "--headless=new"), "{argv:?}");
        // Security invariant: the CDP debugger must bind loopback only, reached
        // solely via the SSH tunnel — never a public interface.
        assert!(
            argv.iter()
                .any(|a| a == "--remote-debugging-address=127.0.0.1"),
            "CDP not pinned to loopback: {argv:?}"
        );
        // Port 0 → Chromium auto-picks a free port (recorded in DevToolsActivePort).
        assert!(
            argv.iter().any(|a| a == "--remote-debugging-port=0"),
            "CDP port not auto-assigned: {argv:?}"
        );
        assert!(
            argv.iter()
                .any(|a| a == "--user-data-dir=/tmp/agentum-hostbrowser-myrepo"),
            "user-data-dir missing: {argv:?}"
        );
    }

    #[test]
    fn launch_command_clears_stale_port_then_execs_chromium() {
        let cmd = launch_command(
            "chromium",
            "/tmp/agentum-hostbrowser-myrepo",
            "/tmp/agentum-hostbrowser-myrepo/DevToolsActivePort",
        )
        .unwrap();
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[1], "-c");
        let inner = &cmd[2];
        // Stale DevToolsActivePort cleared so the await loop reads a fresh bind.
        assert!(inner.contains("rm -f"), "stale port not cleared: {inner}");
        assert!(inner.contains("DevToolsActivePort"), "{inner}");
        // Chromium exec'd as the pane process so a tmux kill reaps it cleanly.
        assert!(inner.contains("exec "), "chromium not exec'd: {inner}");
        assert!(
            inner.contains("--remote-debugging-port=0"),
            "chromium argv missing: {inner}"
        );
    }

    #[test]
    fn encode_screencast_frame_matches_wire_layout() {
        // The byte layout MUST match ui/src/shared/browser-screencast-protocol.ts
        // (`decodeBrowserScreencastFrame`) so the dormant UI consumes it verbatim:
        // 16-byte header, then metadata JSON, then the raw JPEG image.
        let meta = ScreencastMetadata {
            device_width: Some(800.0),
            device_height: Some(600.0),
            timestamp: Some(123.5),
            ..Default::default()
        };
        let jpeg = [0xff_u8, 0xd8, 0xff, 0xe0, 0x00, 0x10];
        let buf = encode_screencast_frame(7, &jpeg, &meta);

        assert_eq!(buf[0], 0x62, "kind");
        assert_eq!(buf[1], 1, "version");
        assert_eq!(buf[2], 1, "opcode Frame");
        assert_eq!(buf[3], 1, "format jpeg");
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            7,
            "seq LE"
        );
        let meta_len = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
        assert_eq!(
            u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            0,
            "reserved word must be zero"
        );

        // Metadata is camelCase JSON, omitting absent fields (matching the UI's
        // optional decode).
        let meta_bytes = &buf[16..16 + meta_len];
        let v: serde_json::Value = serde_json::from_slice(meta_bytes).unwrap();
        assert_eq!(v["deviceWidth"], 800.0);
        assert_eq!(v["deviceHeight"], 600.0);
        assert_eq!(v["timestamp"], 123.5);
        assert!(
            v.get("offsetTop").is_none(),
            "absent metadata fields must be omitted, not null"
        );

        // The raw image follows the metadata, unchanged.
        assert_eq!(&buf[16 + meta_len..], &jpeg, "image must follow metadata");
    }

    #[test]
    fn page_ws_url_repoints_authority_to_mac_port() {
        // Chromium reports the WS URL with the HOST's bound port; from the Mac we
        // reach it via the forwarded port, so the authority must be rewritten
        // while the /devtools/page/<id> path is preserved exactly.
        assert_eq!(
            page_ws_url_for_mac("ws://127.0.0.1:34917/devtools/page/ABC123", 9200).as_deref(),
            Some("ws://127.0.0.1:9200/devtools/page/ABC123")
        );
        // A trailing path segment is preserved verbatim.
        assert_eq!(
            page_ws_url_for_mac("ws://localhost:55001/devtools/page/X/Y", 9200).as_deref(),
            Some("ws://127.0.0.1:9200/devtools/page/X/Y")
        );
        // Malformed input (no path) → None rather than a bogus URL.
        assert_eq!(page_ws_url_for_mac("not-a-url", 9200), None);
    }

    #[test]
    fn metadata_from_cdp_maps_present_numeric_fields() {
        let cdp = serde_json::json!({
            "offsetTop": 0.0,
            "pageScaleFactor": 1.0,
            "deviceWidth": 800.0,
            "deviceHeight": 600.0,
            "scrollOffsetX": 0.0,
            "scrollOffsetY": 12.0,
            "timestamp": 123.5
        });
        let m = metadata_from_cdp(&cdp);
        assert_eq!(m.device_width, Some(800.0));
        assert_eq!(m.device_height, Some(600.0));
        assert_eq!(m.scroll_offset_y, Some(12.0));
        assert_eq!(m.timestamp, Some(123.5));
        // Absent fields stay None (omitted on the wire).
        let empty = metadata_from_cdp(&serde_json::json!({}));
        assert_eq!(empty.device_width, None);
        assert_eq!(empty.timestamp, None);
    }

    #[test]
    fn cdp_command_builds_jsonrpc_envelope() {
        let cmd = cdp_command(
            7,
            "Page.navigate",
            serde_json::json!({"url": "about:blank"}),
        );
        let v: serde_json::Value = serde_json::from_str(&cmd).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "Page.navigate");
        assert_eq!(v["params"]["url"], "about:blank");
    }

    #[test]
    fn input_to_cdp_calls_maps_mouse_wheel_and_text() {
        // Mouse move → mouseMoved.
        let calls = input_to_cdp_calls(r#"{"type":"mouse","action":"move","x":10,"y":20}"#);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Input.dispatchMouseEvent");
        assert_eq!(calls[0].1["type"], "mouseMoved");
        assert_eq!(calls[0].1["x"], 10.0);
        assert_eq!(calls[0].1["y"], 20.0);

        // Mouse down → mousePressed with button + clickCount.
        let down =
            input_to_cdp_calls(r#"{"type":"mouse","action":"down","x":5,"y":6,"button":"left"}"#);
        assert_eq!(down[0].1["type"], "mousePressed");
        assert_eq!(down[0].1["button"], "left");
        assert_eq!(down[0].1["clickCount"], 1);

        // Mouse up → mouseReleased.
        let up =
            input_to_cdp_calls(r#"{"type":"mouse","action":"up","x":5,"y":6,"button":"left"}"#);
        assert_eq!(up[0].1["type"], "mouseReleased");

        // Wheel → mouseWheel with deltaX/deltaY.
        let wheel = input_to_cdp_calls(r#"{"type":"wheel","x":1,"y":2,"dx":0,"dy":40}"#);
        assert_eq!(wheel[0].1["type"], "mouseWheel");
        assert_eq!(wheel[0].1["deltaY"], 40.0);

        // In-band navigate → Page.navigate (single CDP connection).
        let nav = input_to_cdp_calls(r#"{"type":"navigate","url":"https://example.test"}"#);
        assert_eq!(nav[0].0, "Page.navigate");
        assert_eq!(nav[0].1["url"], "https://example.test");

        // Printable key → insertText (reliable typing).
        let ch = input_to_cdp_calls(r#"{"type":"key","key":"a"}"#);
        assert_eq!(ch[0].0, "Input.insertText");
        assert_eq!(ch[0].1["text"], "a");

        // Named key → keyDown + keyUp pair.
        let enter = input_to_cdp_calls(r#"{"type":"key","key":"Enter"}"#);
        assert_eq!(enter.len(), 2, "named key expands to down+up");
        assert_eq!(enter[0].0, "Input.dispatchKeyEvent");
        assert_eq!(enter[0].1["type"], "keyDown");
        assert_eq!(enter[1].1["type"], "keyUp");
        assert_eq!(enter[0].1["key"], "Enter");

        // Garbage → no calls (dropped, never panics).
        assert!(input_to_cdp_calls("not json").is_empty());
        assert!(input_to_cdp_calls(r#"{"type":"bogus"}"#).is_empty());
    }
}
