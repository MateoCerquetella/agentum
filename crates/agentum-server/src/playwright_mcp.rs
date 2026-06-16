//! Shared Playwright MCP server manager + Claude MCP config writer (008b).
//!
//! Unified-MCP design: every agentum-launched first-class agent that supports
//! launch-time MCP (Claude, Codex) gets a Playwright MCP server wired into its
//! argv *at its own launch*. The hard constraint is that Claude Code and Codex
//! read MCP config **only at CLI startup** — there is no in-session reload — so
//! the launch site must, before spawning:
//!   1. ensure ONE shared HTTP Playwright MCP server is listening per machine
//!      ([`ensure_playwright_mcp`]), and
//!   2. for Claude, pre-write the `--mcp-config` file ([`write_claude_config`]).
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
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentum_executor::McpProvision;
use anyhow::{Context, Result};

/// tmux session name for the shared Playwright MCP HTTP server. One per machine:
/// the ensure step is idempotent on this name plus the listening port.
const MCP_TMUX_TARGET: &str = "agentum-playwright-mcp";

/// Default loopback port for the shared server. Matches Playwright MCP's own
/// documented default (`--port 8931`). Overridable via
/// `AGENTUM_PLAYWRIGHT_MCP_PORT` for hosts that already use that port.
const DEFAULT_PORT: u16 = 8931;

/// Tools whose adapters actually consume an [`McpProvision`] (i.e. their
/// `mcp_args` is non-empty). Kept explicit so we only pay the server-ensure
/// cost for sessions that can use the browser MCP; every other tool's
/// `mcp_args` is a no-op anyway.
const MCP_TOOLS: &[&str] = &["claude", "codex"];

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

/// Does this tool's adapter consume browser-MCP provisioning? Only claude/codex
/// override `mcp_args`; everything else returns empty, so we skip the expensive
/// ensure for them entirely.
pub fn tool_supports_browser_mcp(tool: &str) -> bool {
    MCP_TOOLS.contains(&tool)
}

/// One-shot provisioning for a session launch: ensure the shared server is up
/// and Claude's config file is on disk, then hand back the [`McpProvision`] the
/// launch site feeds to `adapter.mcp_args(&p)`.
pub async fn provision() -> Result<McpProvision> {
    let http_url = ensure_playwright_mcp().await?;
    let config_file = write_claude_config(&http_url)?;
    Ok(McpProvision {
        http_url,
        config_file,
    })
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
    let argv = vec![
        "npx".to_string(),
        "-y".to_string(),
        "@playwright/mcp@latest".to_string(),
        "--port".to_string(),
        port.to_string(),
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

/// Write Claude's `--mcp-config` file pointing at the shared HTTP server and
/// return its path.
///
/// File scope (passed explicitly via `--mcp-config`), never a project
/// `.mcp.json` — a project-scoped server triggers a first-run approval prompt
/// that would block an unattended launch. Idempotent: same content each call
/// for a given URL.
pub fn write_claude_config(http_url: &str) -> Result<PathBuf> {
    let dir = agentum_store::paths::state_dir().context("resolve agentum state dir")?;
    write_claude_config_in(&dir, http_url)
}

/// Inner writer parameterised on the state dir so tests exercise the exact JSON
/// shape without mutating process-global env (`AGENTUM_HOME`).
fn write_claude_config_in(state_dir: &Path, http_url: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let path = state_dir.join("playwright-mcp.json");
    // Exactly the shape Claude's `--mcp-config` expects for a streamable-HTTP
    // server. `type:"http"` (not stdio/command) is what keeps this transport
    // identical to the Codex `-c` overrides.
    let doc = serde_json::json!({
        "mcpServers": {
            "playwright": {
                "type": "http",
                "url": http_url,
            }
        }
    });
    let body = serde_json::to_string_pretty(&doc).context("serialize MCP config")?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
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
    use uuid::Uuid;

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
    fn only_claude_and_codex_support_browser_mcp() {
        // Provisioning selection: only the adapters that override `mcp_args`.
        assert!(tool_supports_browser_mcp("claude"));
        assert!(tool_supports_browser_mcp("codex"));
        for t in [
            "cursor",
            "gemini",
            "hermes",
            "terminal",
            "agent",
            "opencode",
            "aider",
            "bash",
            "totally-custom",
        ] {
            assert!(
                !tool_supports_browser_mcp(t),
                "{t} must not trigger browser-MCP provisioning"
            );
        }
    }

    #[test]
    fn claude_config_round_trips_the_http_shape() {
        // Round-trip the exact JSON Claude's `--mcp-config` consumes. Uses an
        // explicit temp dir (not AGENTUM_HOME) so the test is parallel-safe.
        let tmp = std::env::temp_dir().join(format!(
            "agentum-mcp-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let url = "http://127.0.0.1:8931/mcp";
        let path = write_claude_config_in(&tmp, url).unwrap();

        assert_eq!(path.file_name().unwrap(), "playwright-mcp.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let server = &v["mcpServers"]["playwright"];
        assert_eq!(server["type"], "http");
        assert_eq!(server["url"], url);
        // Streamable-HTTP, not stdio: there must be no `command`/`args`.
        assert!(server["command"].is_null());
        assert!(server["args"].is_null());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn written_config_drives_claude_mcp_args() {
        // The written file path, fed back through the adapter seam, must produce
        // `--mcp-config <file>` — the end-to-end contract P1 wires up.
        let tmp = std::env::temp_dir().join(format!(
            "agentum-mcp-args-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = write_claude_config_in(&tmp, "http://127.0.0.1:8931/mcp").unwrap();
        let p = McpProvision {
            http_url: "http://127.0.0.1:8931/mcp".to_string(),
            config_file: path.clone(),
        };
        // `mcp_args` lives on the `ToolAdapter` trait — bring it into scope.
        use agentum_executor::ToolAdapter;
        let args = agentum_executor::ClaudeAdapter.mcp_args(&p);
        assert_eq!(
            args,
            vec!["--mcp-config".to_string(), path.display().to_string()]
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
