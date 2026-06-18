//! LIVE Phase-1 verification for spec 009a (host browser + forward tunnel).
//!
//! `#[ignore]` — it SSHes to a real host, launches a real headless Chromium in a
//! real tmux session, and opens an `ssh -L` tunnel, so it never runs in CI. Run:
//!
//!   cargo test -p agentum-server --test host_browser_live -- --ignored --nocapture
//!
//! Proves the Phase-1 acceptance: a headless Chromium runs ON the host, the Mac
//! forward-tunnels its CDP port, and `curl 127.0.0.1:<mac>/json/version` returns
//! the host Chromium's CDP banner — browser-on-host + forward tunnel end to end,
//! with zero UI.
//!
//! Credentials come from the environment — NEVER hardcode them (this file lives
//! under `crates/`, which is committable). The password is required; the test
//! soft-skips when it's unset. Host/user/port/workdir default to the Omarchy
//! test box but are overridable:
//!
//!   AGENTUM_LIVE_SSH_PASSWORD=… \
//!   [AGENTUM_LIVE_SSH_USER=malloc] [AGENTUM_LIVE_SSH_HOST=172.30.66.4] \
//!   [AGENTUM_LIVE_SSH_PORT=44444] [AGENTUM_LIVE_SSH_WORKDIR=/home/malloc] \
//!   cargo test -p agentum-server --test host_browser_live -- --ignored --nocapture
//!
//! The interactive ControlMaster is reused if already warm, so this needs no
//! fresh auth when the desktop/TUI already has the host open.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use agentum_core::{Host, HostKind, SshAuth};
use agentum_server::{host_browser, host_runtime};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Build the live host from env. `None` (→ soft-skip) when the password isn't
/// set, so the secret never has to live in source and CI never trips on it.
fn live_host() -> Option<Host> {
    let password = std::env::var("AGENTUM_LIVE_SSH_PASSWORD").ok()?;
    Some(Host {
        id: agentum_core::LOCAL_HOST_ID,
        name: "live".into(),
        kind: HostKind::Ssh {
            user: env_or("AGENTUM_LIVE_SSH_USER", "malloc"),
            hostname: env_or("AGENTUM_LIVE_SSH_HOST", "172.30.66.4"),
            port: env_or("AGENTUM_LIVE_SSH_PORT", "44444")
                .parse()
                .expect("AGENTUM_LIVE_SSH_PORT must be a u16"),
            auth: SshAuth::Password { password },
        },
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
        last_seen_at: None,
    })
}

#[tokio::test]
#[ignore = "live: needs an SSH host + chromium on it; set AGENTUM_LIVE_SSH_PASSWORD"]
async fn host_browser_launch_tunnel_and_cdp_banner() {
    let Some(host) = live_host() else {
        eprintln!("skipping: AGENTUM_LIVE_SSH_PASSWORD not set");
        return;
    };
    // The workdir's basename is the worktree slug; default exists on the host.
    let workdir = PathBuf::from(env_or("AGENTUM_LIVE_SSH_WORKDIR", "/home/malloc"));
    let workdir = workdir.as_path();

    // Start from a clean slate so we exercise the fresh-launch path (not attach).
    let _ = host_browser::teardown_host_browser(&host, workdir).await;

    let browser = host_browser::launch_host_browser(&host, workdir)
        .await
        .expect("launch host browser");
    println!(
        "launched: target={} cdp_port={} user_data_dir={} attached={}",
        browser.tmux_target, browser.cdp_port, browser.user_data_dir, browser.attached
    );
    assert!(browser.cdp_port > 0, "Chromium bound no CDP port");

    let mac_port = host_runtime::ensure_forward_tunnel(&host, browser.cdp_port)
        .await
        .expect("forward tunnel");
    println!(
        "forward tunnel up: mac 127.0.0.1:{mac_port} -> host 127.0.0.1:{}",
        browser.cdp_port
    );

    // The actual Phase-1 proof: the host Chromium's CDP banner over the tunnel.
    let out = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "10",
            &format!("http://127.0.0.1:{mac_port}/json/version"),
        ])
        .output()
        .expect("run curl");
    let body = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("GET /json/version => {body}");

    // Teardown before asserting so a failed assertion still reaps the browser.
    let _ = host_browser::teardown_host_browser(&host, workdir).await;

    assert!(
        body.contains("webSocketDebuggerUrl") || body.contains("Chrome"),
        "CDP banner not returned through the forward tunnel: {body:?}"
    );
}

/// Scratch WS stream type for the screencast route (plain ws://, no TLS).
type ScratchWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Pull the image bytes out of a `0x62` screencast frame — mirrors
/// `decodeBrowserScreencastFrame`. `None` if the header doesn't match.
fn decode_screencast_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 16 || bytes[0] != 0x62 || bytes[1] != 1 || bytes[2] != 1 {
        return None;
    }
    let meta_len = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let start = 16 + meta_len;
    (start <= bytes.len()).then(|| bytes[start..].to_vec())
}

/// Wait up to `budget` for a valid JPEG screencast frame whose bytes differ from
/// `baseline` (`None` = accept the first valid frame). Deterministic capture:
/// stale pre-navigate frames (equal to the previous page) are skipped, so we only
/// return once the new page has actually rendered — no window-timing race.
async fn next_jpeg_differing(
    ws: &mut ScratchWs,
    baseline: Option<&[u8]>,
    budget: Duration,
) -> Option<Vec<u8>> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(600), ws.next()).await {
            Ok(Some(Ok(WsMessage::Binary(b)))) => {
                if let Some(img) = decode_screencast_jpeg(b.as_ref()) {
                    if img.starts_with(&[0xFF, 0xD8])
                        && baseline.is_none_or(|base| img.as_slice() != base)
                    {
                        return Some(img);
                    }
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => {} // idle tick — keep waiting within the budget
        }
    }
    None
}

/// LIVE Phase-2 verification: a scratch WS client receives protocol-framed JPEG
/// frames from the host browser's CDP screencast, and an in-band navigate changes
/// the rendered frame. Pure backend (no desktop). Same env contract as above.
// Multi-thread: the CDP bridge tasks, the scratch WS client, and the SSH
// teardown all run concurrently here — a single-threaded runtime starves them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs the SSH host + chromium; set AGENTUM_LIVE_SSH_PASSWORD"]
async fn screencast_streams_jpeg_frames_and_navigate_changes_them() {
    use axum::extract::{Path as AxumPath, ws::WebSocketUpgrade};

    let Some(host) = live_host() else {
        eprintln!("skipping: AGENTUM_LIVE_SSH_PASSWORD not set");
        return;
    };
    // A DISTINCT workdir from the Phase-1 test (different basename → different
    // per-worktree session name), so the two #[ignore] tests can run concurrently
    // without colliding on `agentum-hostbrowser-<wt>`. `/tmp` exists on any host.
    let workdir = PathBuf::from("/tmp");
    let _ = host_browser::teardown_host_browser(&host, &workdir).await;

    // Mount ONLY the stateless screencast route on a minimal axum server — not
    // the full embedded server, whose watchdog/notify blocking threads never
    // terminate and would wedge runtime shutdown after the test passes.
    let app = axum::Router::new().route(
        "/api/host-browser/{id}/screencast",
        axum::routing::get(
            |ws: WebSocketUpgrade, AxumPath(id): AxumPath<String>| async move {
                ws.on_upgrade(
                    move |sock| async move { host_browser::run_screencast(&id, sock).await },
                )
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Start the browser; it registers in the in-process registry the route reads.
    let started = host_browser::start_host_browser(&host, &workdir)
        .await
        .expect("start host browser");
    let id = started.id.clone();
    println!(
        "started id={id} mac_port={} attached={}",
        started.mac_port, started.attached
    );

    let ws_url = format!("ws://{addr}/api/host-browser/{id}/screencast");
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url.as_str())
        .await
        .expect("connect screencast WS");

    // Fully percent-encoded data URLs → unambiguous solid-color viewports.
    let red = "data:text/html,%3Chtml%20style%3D%22background%3Ared%22%3E%3C/html%3E";
    let blue = "data:text/html,%3Chtml%20style%3D%22background%3Ablue%22%3E%3C/html%3E";

    // The initial (blank) frame — the baseline each navigate must change.
    let initial = next_jpeg_differing(&mut ws, None, Duration::from_secs(8))
        .await
        .expect("an initial JPEG frame");
    println!("initial frame: {} bytes", initial.len());

    ws.send(WsMessage::Text(
        format!(r#"{{"type":"navigate","url":"{red}"}}"#).into(),
    ))
    .await
    .expect("send navigate red");
    let red_frame = next_jpeg_differing(&mut ws, Some(&initial), Duration::from_secs(10))
        .await
        .expect("a RED frame (differing from the blank page)");
    println!("RED frame: {} bytes", red_frame.len());

    // Exercise the input path (a mouse move) — must not break the stream.
    ws.send(WsMessage::Text(
        r#"{"type":"mouse","action":"move","x":20,"y":20}"#.into(),
    ))
    .await
    .expect("send input");

    ws.send(WsMessage::Text(
        format!(r#"{{"type":"navigate","url":"{blue}"}}"#).into(),
    ))
    .await
    .expect("send navigate blue");
    let blue_frame = next_jpeg_differing(&mut ws, Some(&red_frame), Duration::from_secs(10))
        .await
        .expect("a BLUE frame (differing from the red page)");
    println!("BLUE frame: {} bytes", blue_frame.len());

    // Teardown before asserting so a failed assertion still reaps the browser.
    // Drop the socket (abrupt close) rather than a graceful Close handshake: we
    // stopped reading frames, so a close-flush would block on backpressure.
    // Dropping signals the server (read EOF) to end the bridge. The stop is
    // time-bounded so a slow SSH teardown can never hang the test.
    drop(ws);
    let _ = tokio::time::timeout(Duration::from_secs(20), host_browser::stop(&id)).await;

    assert!(
        red_frame.starts_with(&[0xFF, 0xD8]) && blue_frame.starts_with(&[0xFF, 0xD8]),
        "both frames must be JPEG"
    );
    assert_ne!(
        red_frame, blue_frame,
        "an in-band navigate must change the rendered frame (red vs blue)"
    );
}
