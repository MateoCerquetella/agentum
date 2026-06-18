//! Shared Playwright MCP server lifecycle (008b).
//!
//! Owns starting and reusing ONE Playwright browser MCP HTTP server per machine
//! ([`ensure_playwright_mcp`] → URL). The launch-site orchestration that turns
//! this URL (plus agentum's own MCP) into an agent's startup flags lives in
//! [`crate::mcp_provision`]; this module is just the Playwright server manager.
//!
//! The server lives in its own long-lived tmux session (`agentum-playwright-mcp`)
//! so it survives agent restarts — that is what makes the loop "continuous". The
//! ensure step is idempotent on the listening port + that session name, so the
//! N-th session reuses the same server rather than spawning N of them.
//!
//! Provisioning is **opt-in** (`AGENTUM_BROWSER_VERIFY`) and best-effort at the
//! call site: a normal coding session must not pay for a browser MCP it won't
//! use, and a missing toolchain must not block launches. [`ensure_playwright_mcp`]
//! still *fails loud* (returns a descriptive error) when `npx`/Playwright is
//! missing — the caller decides whether to surface or swallow it. See the spec
//! `docs/superpowers/plans/2026-06-16-008b-browser-verify-unified-mcp-and-in-desktop-view.md`.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;

/// tmux session name for the **headless** Playwright MCP HTTP server (its own
/// hidden Chromium). One per machine: idempotent on this name plus its port.
const MCP_TMUX_TARGET: &str = "agentum-playwright-mcp";

/// tmux session name for the **CDP-bound** Playwright MCP (attached to agentum's
/// displayed browser, 009c-1). Deliberately SEPARATE from the headless session +
/// port so the two never collide: a stale headless server from an earlier run
/// must never be silently reused as if it were the bound one — that would point
/// the agent at a HIDDEN browser and break the whole "drive the browser the user
/// watches" guarantee. `provision()` wires exactly one engine, so only one is
/// ever handed to an agent; the other simply lingers, never wired.
const BOUND_MCP_TMUX_TARGET: &str = "agentum-playwright-mcp-bound";

/// Default loopback port for the headless server. Matches Playwright MCP's own
/// documented default (`--port 8931`). Overridable via
/// `AGENTUM_PLAYWRIGHT_MCP_PORT` for hosts that already use that port.
const DEFAULT_PORT: u16 = 8931;

/// Default loopback port for the CDP-bound server — distinct from the headless
/// port (see [`BOUND_MCP_TMUX_TARGET`]). Overridable via
/// `AGENTUM_PLAYWRIGHT_MCP_BOUND_PORT`.
const DEFAULT_BOUND_PORT: u16 = 8933;

/// Resolve the headless server port (env override → default).
fn port() -> u16 {
    std::env::var("AGENTUM_PLAYWRIGHT_MCP_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Resolve the CDP-bound server port (env override → default).
fn bound_port() -> u16 {
    std::env::var("AGENTUM_PLAYWRIGHT_MCP_BOUND_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_BOUND_PORT)
}

/// Streamable-HTTP URL Playwright MCP serves at — `--port` exposes the endpoint
/// under the `/mcp` path. This is the exact shape both Claude (`type:"http"`)
/// and Codex (`-c mcp_servers.playwright.url`) point at.
fn http_url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Whether the browser-verification feature is enabled for this process.
///
/// Provisioning is opt-in: a plain coding session should never spawn a browser
/// MCP server it won't use. Any truthy `AGENTUM_BROWSER_VERIFY` value turns it
/// on. (No server-persisted setting exists yet — this env flag is the minimal
/// gate; a Settings-pane toggle can drive it later.)
pub fn feature_enabled() -> bool {
    std::env::var("AGENTUM_BROWSER_VERIFY")
        .map(|v| truthy(&v))
        .unwrap_or(false)
}

/// Shared truthiness rule for the feature flag — split out so it can be unit
/// tested without mutating process-global env (tests run in parallel).
fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Ensure the shared Playwright MCP HTTP server is listening, starting it in its
/// dedicated long-lived tmux session if not. Idempotent: a port already serving
/// (or a still-booting session we started earlier) is reused. Returns the
/// streamable-HTTP URL agents point at.
///
/// Fails loud if `npx` is missing — we can't start Playwright MCP without it,
/// and launching the agent with a dangling MCP URL would only surface later as
/// a confusing tool-call error inside the session.
pub async fn ensure_playwright_mcp() -> Result<String> {
    ensure(LaunchMode::Headless).await
}

/// Like [`ensure_playwright_mcp`], but the MCP **attaches to an already-running
/// CDP browser** (`--cdp-endpoint <endpoint>`) instead of spawning its own
/// hidden Chromium — so the agent drives the *same* browser the user watches
/// (009c-1). `cdp_endpoint` is the displayed browser's CDP base URL, e.g.
/// `http://127.0.0.1:9300`. The agent-facing `…/mcp` URL is unchanged; only the
/// MCP's backing browser differs.
///
/// The bound server uses its OWN tmux session + port ([`BOUND_MCP_TMUX_TARGET`] /
/// [`bound_port`]), distinct from the headless one — so a stale headless server
/// is never silently reused as the bound one (which would point the agent at a
/// hidden browser). `provision()` wires exactly one engine; the bound server,
/// once up, is reused across sessions (it stays attached to agentum's long-lived
/// CDP-browser singleton).
pub async fn ensure_playwright_mcp_bound(cdp_endpoint: &str) -> Result<String> {
    ensure(LaunchMode::BoundToCdp(cdp_endpoint)).await
}

/// Stop the CDP-bound Playwright MCP singleton. Must be called whenever the
/// CDP browser it attaches to is torn down: the bound MCP holds an open CDP
/// WebSocket to that browser process, so if the browser is killed and relaunched
/// the still-listening MCP would serve a dead connection and every tool call
/// would fail with a target-closed error. Killing it forces the next
/// [`ensure_playwright_mcp_bound`] to reconnect cleanly. Idempotent / best-effort.
pub async fn stop_bound_mcp() -> Result<()> {
    agentum_tmux::kill_session(BOUND_MCP_TMUX_TARGET)
        .await
        .context("kill the CDP-bound Playwright MCP tmux session")
}

/// How the shared Playwright MCP backs its browser.
enum LaunchMode<'a> {
    /// Spawn its own hidden Chromium (`--headless`) — the default lightweight
    /// verify path; works on display-less hosts.
    Headless,
    /// Attach to an already-running CDP browser (009c-1: agentum's displayed
    /// Chromium) over `--cdp-endpoint` rather than spawning one.
    BoundToCdp(&'a str),
}

impl LaunchMode<'_> {
    /// The tmux session that backs this mode (headless and bound are separate
    /// singletons so they never collide).
    fn tmux_target(&self) -> &'static str {
        match self {
            LaunchMode::Headless => MCP_TMUX_TARGET,
            LaunchMode::BoundToCdp(_) => BOUND_MCP_TMUX_TARGET,
        }
    }

    /// The loopback port this mode's server listens on (distinct per mode).
    fn server_port(&self) -> u16 {
        match self {
            LaunchMode::Headless => port(),
            LaunchMode::BoundToCdp(_) => bound_port(),
        }
    }
}

/// Build the `npx @playwright/mcp` argv for a launch mode. The base flags are
/// shared; the mode only decides whether we spawn a hidden Chromium
/// (`--headless`) or attach to an existing one (`--cdp-endpoint <endpoint>`).
///
/// `--host 127.0.0.1` is load-bearing: Playwright MCP's default `--host
/// localhost` resolves to IPv6 `::1` only on macOS, so a server started with the
/// default would NOT be reachable at the `http://127.0.0.1:<port>/mcp` URL we
/// write into the agent config (connection refused) — and our IPv4
/// `port_listening` probe would also miss it and wrongly report "did not start".
/// Pinning IPv4 keeps the bind, the probe, and the config URL all consistent.
/// (Found via P1 live test.) `npx -y` so a first run installs the package
/// non-interactively (an interactive prompt would hang the detached pane).
fn build_argv(port: u16, mode: &LaunchMode) -> Vec<String> {
    let mut argv = vec![
        "npx".to_string(),
        "-y".to_string(),
        "@playwright/mcp@latest".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
    ];
    match mode {
        LaunchMode::Headless => argv.push("--headless".to_string()),
        LaunchMode::BoundToCdp(endpoint) => {
            // Attach to agentum's displayed browser instead of spawning a hidden
            // one — the unification proof (verified flag, 009c-1 notes). No
            // `--headless`: we connect to a browser we don't own here.
            argv.push("--cdp-endpoint".to_string());
            argv.push((*endpoint).to_string());
        }
    }
    argv
}

/// Shared ensure body parameterised on the launch mode (headless vs CDP-bound).
/// Each mode has its own tmux session + port, so they are independent singletons.
async fn ensure(mode: LaunchMode<'_>) -> Result<String> {
    let port = mode.server_port();
    let target = mode.tmux_target();
    let url = http_url_for(port);

    // Fast path (no lock): this mode's server is already serving its port →
    // reuse it. One server per mode per machine, shared across sessions and
    // surviving restarts.
    if port_listening(port).await {
        return Ok(url);
    }

    // Serialize the launch so concurrent session starts don't race on the tmux
    // session (the loser would hit "duplicate session"). Double-checked: a peer
    // may have started it while we waited for the lock.
    let _guard = launch_lock().lock().await;
    if port_listening(port).await {
        return Ok(url);
    }

    // Need to start it — fail loud now if the toolchain is absent rather than
    // spawning a tmux session that immediately dies with an opaque error.
    ensure_npx_available()?;

    // A leftover session that isn't (yet) listening is either still booting or
    // dead. Give a slow boot a brief grace window; otherwise reset the
    // singleton so we start cleanly.
    if agentum_tmux::has_session(target).await.unwrap_or(false) {
        if wait_until_listening(port, Duration::from_secs(2)).await {
            return Ok(url);
        }
        let _ = agentum_tmux::kill_session(target).await;
    }

    let argv = build_argv(port, &mode);
    // The MCP server has no project context — run it from $HOME so tmux has a
    // valid, always-present cwd.
    let workdir = home_dir();
    agentum_tmux::new_session(target, &workdir, &argv, &[])
        .await
        .context("start the shared Playwright MCP tmux session")?;

    // First boot may `npm install` the package, so allow a generous window.
    if wait_until_listening(port, Duration::from_secs(20)).await {
        Ok(url)
    } else {
        anyhow::bail!(
            "Playwright MCP did not start listening on 127.0.0.1:{port} within 20s \
             (tmux session `{target}`). Check the pane; you may need \
             `npx playwright install chromium` or a working Node toolchain."
        )
    }
}

/// A plain TCP connect is enough to know "something is serving here"; the
/// agent's own MCP client performs the protocol handshake. Short timeout so a
/// dead port fails fast on the launch hot-path.
async fn port_listening(port: u16) -> bool {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    matches!(
        tokio::time::timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Poll the port until it accepts connections or the deadline passes.
async fn wait_until_listening(port: u16, max: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + max;
    loop {
        if port_listening(port).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Fail loud when `npx` isn't installed. A PATH scan (no process spawn) keeps
/// the launch hot-path cheap.
fn ensure_npx_available() -> Result<()> {
    if binary_on_path("npx") {
        Ok(())
    } else {
        anyhow::bail!(
            "`npx` not found on PATH — Playwright MCP needs a Node.js/npm toolchain. \
             Install Node, then run `npx playwright install chromium`."
        )
    }
}

/// Is `bin` present in any PATH directory?
fn binary_on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

/// Process-wide lock serializing MCP-server launches (see `ensure`). Shared
/// across modes — launches are rare and the fast path skips it entirely.
fn launch_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// `$HOME` as a tmux cwd, falling back to `/` so the spawn never fails on an
/// unset HOME.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_url_targets_the_mcp_path() {
        // Both Claude and Codex point at `…/mcp`; the port flag exposes it there.
        assert_eq!(http_url_for(8931), "http://127.0.0.1:8931/mcp");
        assert_eq!(http_url_for(9000), "http://127.0.0.1:9000/mcp");
    }

    #[test]
    fn feature_flag_truthiness() {
        for v in ["1", "true", "TRUE", "yes", "on", " 1 ", "On"] {
            assert!(truthy(v), "{v:?} should enable the feature");
        }
        for v in ["0", "false", "no", "off", "", "  ", "maybe"] {
            assert!(!truthy(v), "{v:?} should not enable the feature");
        }
    }

    #[test]
    fn headless_argv_spawns_its_own_hidden_chromium() {
        let argv = build_argv(8931, &LaunchMode::Headless);
        assert!(argv.contains(&"--headless".to_string()));
        // Headless mode never attaches to an external browser.
        assert!(!argv.iter().any(|a| a == "--cdp-endpoint"));
        // Base flags stay consistent (IPv4 host, the port we probe).
        assert!(argv.windows(2).any(|w| w[0] == "--host" && w[1] == "127.0.0.1"));
        assert!(argv.windows(2).any(|w| w[0] == "--port" && w[1] == "8931"));
    }

    #[test]
    fn bound_argv_attaches_to_cdp_and_drops_headless() {
        // 009c-1: the bound MCP must attach to agentum's displayed browser and
        // NOT spawn its own hidden one — the whole point of the unification.
        let argv = build_argv(8931, &LaunchMode::BoundToCdp("http://127.0.0.1:9300"));
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--cdp-endpoint" && w[1] == "http://127.0.0.1:9300"),
            "bound argv must pass --cdp-endpoint <endpoint>: {argv:?}"
        );
        assert!(
            !argv.contains(&"--headless".to_string()),
            "bound mode must NOT be headless (it attaches, not spawns): {argv:?}"
        );
    }

    #[test]
    fn headless_and_bound_are_separate_singletons() {
        // The bug fix: distinct tmux session + default port per mode, so a stale
        // headless server is never silently reused as the bound one (which would
        // point the agent at a HIDDEN browser, breaking the unification).
        assert_ne!(
            LaunchMode::Headless.tmux_target(),
            LaunchMode::BoundToCdp("x").tmux_target(),
            "headless and bound must use different tmux sessions"
        );
        assert_ne!(
            DEFAULT_PORT, DEFAULT_BOUND_PORT,
            "headless and bound must default to different ports"
        );
    }
}
