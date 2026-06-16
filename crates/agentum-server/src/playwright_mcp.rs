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
use std::time::Duration;

use anyhow::{Context, Result};

/// tmux session name for the shared Playwright MCP HTTP server. One per machine:
/// the ensure step is idempotent on this name plus the listening port.
const MCP_TMUX_TARGET: &str = "agentum-playwright-mcp";

/// Default loopback port for the shared server. Matches Playwright MCP's own
/// documented default (`--port 8931`). Overridable via
/// `AGENTUM_PLAYWRIGHT_MCP_PORT` for hosts that already use that port.
const DEFAULT_PORT: u16 = 8931;

/// Resolve the shared server port (env override → default).
fn port() -> u16 {
    std::env::var("AGENTUM_PLAYWRIGHT_MCP_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT)
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
    let port = port();
    let url = http_url_for(port);

    // Fast path: someone is already serving the port → reuse it. One server per
    // machine, shared across every session and surviving agent restarts.
    if port_listening(port).await {
        return Ok(url);
    }

    // Need to start it — fail loud now if the toolchain is absent rather than
    // spawning a tmux session that immediately dies with an opaque error.
    ensure_npx_available()?;

    // A leftover session that isn't (yet) listening is either still booting or
    // dead. Give a slow boot a brief grace window; otherwise reset the
    // singleton so we start cleanly.
    if agentum_tmux::has_session(MCP_TMUX_TARGET)
        .await
        .unwrap_or(false)
    {
        if wait_until_listening(port, Duration::from_secs(2)).await {
            return Ok(url);
        }
        let _ = agentum_tmux::kill_session(MCP_TMUX_TARGET).await;
    }

    // `npx -y` so a first run installs the package non-interactively (an
    // interactive prompt would hang the detached pane forever). `--headless`
    // so it works on display-less hosts and never steals focus.
    //
    // `--host 127.0.0.1` is load-bearing: Playwright MCP's default `--host
    // localhost` resolves to IPv6 `::1` only on macOS, so a server started with
    // the default would NOT be reachable at the `http://127.0.0.1:<port>/mcp`
    // URL we write into the agent config (connection refused) — and our IPv4
    // `port_listening` probe would also miss it and wrongly report "did not
    // start". Pinning IPv4 keeps the bind, the probe, and the config URL all
    // consistent. (Found via P1 live test.)
    let argv = vec![
        "npx".to_string(),
        "-y".to_string(),
        "@playwright/mcp@latest".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--headless".to_string(),
    ];
    // The MCP server has no project context — run it from $HOME so tmux has a
    // valid, always-present cwd.
    let workdir = home_dir();
    agentum_tmux::new_session(MCP_TMUX_TARGET, &workdir, &argv, &[])
        .await
        .context("start the shared Playwright MCP tmux session")?;

    // First boot may `npm install` the package, so allow a generous window.
    if wait_until_listening(port, Duration::from_secs(20)).await {
        Ok(url)
    } else {
        anyhow::bail!(
            "Playwright MCP did not start listening on 127.0.0.1:{port} within 20s \
             (tmux session `{MCP_TMUX_TARGET}`). Check the pane; you may need \
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
}
