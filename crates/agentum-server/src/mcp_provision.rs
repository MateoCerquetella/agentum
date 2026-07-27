//! Collects the MCP servers to wire into an agent at launch and writes the
//! combined Claude `--mcp-config` file.
//!
//! Two servers can be provisioned per session:
//! - **agentum** — agentum's own MCP server (this daemon's `/mcp`). Free (no
//!   process to spawn — it's the already-running server), so it's wired by
//!   default whenever the agent can reach it. The embedded loopback server and
//!   authenticated standalone daemon both require the dedicated MCP bearer;
//!   that credential is distinct from the desktop UI capability. This is the
//!   "skills → MCP" surface: any agent gets Agentum's tools with zero per-agent
//!   skill files.
//! - **playwright** — the shared Playwright browser MCP (see [`playwright_mcp`]).
//!   Opt-in (`AGENTUM_BROWSER_VERIFY`) because it spawns `npx`.
//!
//! Both go into ONE combined config file (Claude reads all servers from a single
//! `--mcp-config`); Codex gets a `-c` block per server. The per-tool half lives
//! in `agentum_executor::ToolAdapter::mcp_args`; this module is the I/O half.

use std::path::{Path, PathBuf};

use agentum_executor::{McpProvision, McpServer};
use anyhow::{Context, Result};

use crate::AppState;
use crate::playwright_mcp;

/// Tools that take MCP via a launch argument (`adapter.mcp_args`): Claude
/// (`--mcp-config <file>`) and Codex (`-c`). Wired in the argv at launch.
const MCP_ARG_TOOLS: &[&str] = &["claude", "codex"];

/// Does this tool take MCP via a launch argument?
pub fn tool_supports_mcp(tool: &str) -> bool {
    MCP_ARG_TOOLS.contains(&tool)
}

/// Fixed loopback port the reverse SSH tunnel binds on each host. A remote
/// agent reaches agentum's MCP at `http://127.0.0.1:<REMOTE_MCP_PORT>/mcp`,
/// which tunnels back to the Mac's embedded server.
pub const REMOTE_MCP_PORT: u16 = 8990;

/// The Mac-side embedded-server port to reverse-tunnel to, parsed from
/// `api_base_url` (e.g. `http://127.0.0.1:60102` → `60102`). `None` for a
/// standalone daemon that didn't set its own URL (then remote wiring is skipped).
pub fn local_mcp_port(state: &AppState) -> Option<u16> {
    state
        .api_base_url
        .as_deref()?
        .trim_end_matches('/')
        .rsplit(':')
        .next()?
        .parse()
        .ok()
}

/// Build the MCP provisioning for a session launch, or `None` when there is
/// nothing to wire. `agentum_mcp_url` is the endpoint *this session's agent*
/// should reach agentum's own MCP at — the Mac loopback for a local session, or
/// the host's reverse-tunnel port for an SSH session. Ensures any servers that
/// need starting (Playwright) are up and writes the combined config. Best-effort:
/// a server that can't be provisioned is logged and skipped, never fatal.
pub async fn provision(
    state: &AppState,
    tool: &str,
    agentum_mcp_url: &str,
) -> Option<McpProvision> {
    if !tool_supports_mcp(tool) {
        return None;
    }

    let mut servers = Vec::new();

    // agentum's own MCP — wired by default, but the user can turn the whole
    // agentum MCP off via the master switch (Settings → Agent MCP). When off, no
    // agentum tools reach any agent. Default ON so existing setups are unchanged;
    // this is a launch-time gate, so flipping it affects the next agent launch.
    // It's secured by the per-server bearer token (every agent presents it; the
    // `/mcp` handler 401s without it). A standalone `--no-auth` daemon relaxes
    // legacy HTTP automation only; it never makes MCP or SDD public.
    if state
        .store
        .setting_get_bool(crate::routes::mcp::MCP_ENABLED_SETTING, true)
        .await
        .unwrap_or(true)
    {
        servers.push(McpServer {
            name: "agentum".to_string(),
            url: agentum_mcp_url.to_string(),
            auth_token: Some(state.mcp_token.as_str().to_string()),
        });
    }

    // Playwright browser MCP — opt-in, spawns npx, best-effort. No auth. The
    // engine seam (`browser_mcp_engine`) decides whether the MCP is BOUND to
    // agentum's displayed CDP-Chromium (009c-1: the agent drives the browser the
    // user watches) or spawns its own HIDDEN headless one (the legacy path).
    if playwright_mcp::feature_enabled() {
        match provision_browser_mcp(browser_mcp_engine()).await {
            Ok(url) => servers.push(McpServer {
                name: "playwright".to_string(),
                url,
                auth_token: None,
            }),
            Err(e) => {
                tracing::warn!("Playwright MCP not provisioned; skipping: {e:#}")
            }
        }
    }

    if servers.is_empty() {
        return None;
    }

    match write_combined_config(&servers) {
        Ok(config_file) => Some(McpProvision {
            servers,
            config_file,
        }),
        Err(e) => {
            tracing::warn!("could not write MCP config; launching without MCP: {e:#}");
            None
        }
    }
}

/// The browser MCP engine for this machine — the **agnostic seam**. A closed set
/// (Playwright is the concrete reference binding), NOT a plugin registry: a
/// future engine adds an arm here + a launch fn, no dynamic loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMcpEngine {
    /// Playwright MCP **bound** to agentum's displayed CDP-Chromium over
    /// `--cdp-endpoint` — the agent drives the same browser the user watches
    /// (009c-1). The default whenever a local CDP browser can be launched.
    PlaywrightBound,
    /// Playwright MCP spawning its **own hidden headless** Chromium (the original
    /// lightweight verify path; the fallback when the bound browser is absent).
    PlaywrightHeadless,
}

/// Select the browser MCP engine. Defaults to bound (009c-1); an explicit
/// `AGENTUM_BROWSER_MCP_ENGINE=headless` forces the legacy hidden path. This one
/// function is the seam.
pub fn browser_mcp_engine() -> BrowserMcpEngine {
    parse_engine(std::env::var("AGENTUM_BROWSER_MCP_ENGINE").ok().as_deref())
}

/// Pure engine parser (split out so it's unit-testable without mutating
/// process-global env — tests run in parallel).
fn parse_engine(val: Option<&str>) -> BrowserMcpEngine {
    match val.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("headless") => BrowserMcpEngine::PlaywrightHeadless,
        _ => BrowserMcpEngine::PlaywrightBound,
    }
}

/// Provision the browser MCP per the chosen engine, returning the agent-facing
/// `…/mcp` URL.
///
/// Bound engine: launch agentum's displayed CDP-Chromium, then bind the MCP to
/// it. The degrade-to-headless leg fires **only when the CDP browser itself
/// can't launch** (not installed, no display) — there, headless is the sensible
/// fallback since the user couldn't watch a browser anyway. If the browser *did*
/// launch but binding the MCP fails, that error is propagated (NOT degraded):
/// silently falling back to a hidden headless browser while a visible one is
/// open would be exactly the confusing split this feature exists to remove.
async fn provision_browser_mcp(engine: BrowserMcpEngine) -> Result<String> {
    match engine {
        BrowserMcpEngine::PlaywrightHeadless => playwright_mcp::ensure_playwright_mcp().await,
        BrowserMcpEngine::PlaywrightBound => {
            match crate::cdp_browser::ensure_local_cdp_browser().await {
                Ok(endpoint) => playwright_mcp::ensure_playwright_mcp_bound(&endpoint).await,
                Err(e) => {
                    // CDP browser couldn't launch — degrade, loudly logged: the
                    // user can still verify via the hidden headless browser, they
                    // just can't watch it. (A bound-MCP failure above is NOT
                    // degraded — see the doc comment.)
                    tracing::warn!(
                        "local CDP browser unavailable ({e:#}); \
                         falling back to headless Playwright MCP"
                    );
                    playwright_mcp::ensure_playwright_mcp().await
                }
            }
        }
    }
}

/// Hex SHA-256 of the `/mcp` bearer token. We persist this (never the token
/// itself) on a session row so the boot drift scan can compare "what the session
/// was provisioned with" against the live token without storing the secret at
/// rest. Shared by the spawn-time record and the boot scan so both hash the same
/// way.
pub fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    format!("{digest:x}")
}

/// The agentum-MCP server entry for a session: the endpoint the agent should
/// reach (loopback locally, tunnel port remotely) plus the bearer token.
pub fn agentum_server(state: &AppState, agentum_mcp_url: &str) -> McpServer {
    McpServer {
        name: "agentum".to_string(),
        url: agentum_mcp_url.to_string(),
        auth_token: Some(state.mcp_token.as_str().to_string()),
    }
}

/// The `{ "mcpServers": { … } }` JSON content for a Claude `--mcp-config` file —
/// works the same whether the file lands on the Mac (local session) or on the
/// host (remote session, where the agent can actually read it).
pub fn config_json(servers: &[McpServer]) -> String {
    let mut map = serde_json::Map::new();
    for s in servers {
        // `type:"http"` (streamable-HTTP) keeps every server identical to the
        // Codex `-c` overrides — no stdio/command transport anywhere.
        let mut entry = serde_json::json!({ "type": "http", "url": s.url });
        if let Some(token) = &s.auth_token {
            // Claude's `--mcp-config` http servers accept a `headers` map; the
            // agent presents this on every request so the server authorizes it.
            entry["headers"] = serde_json::json!({ "Authorization": format!("Bearer {token}") });
        }
        map.insert(s.name.clone(), entry);
    }
    serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": map }))
        .unwrap_or_else(|_| "{\"mcpServers\":{}}".to_string())
}

/// Write the combined Claude config under the state dir (LOCAL sessions only —
/// remote sessions write the file on the host) and return its path.
pub fn write_combined_config(servers: &[McpServer]) -> Result<PathBuf> {
    let dir = agentum_store::paths::state_dir().context("resolve agentum state dir")?;
    write_combined_config_in(&dir, servers)
}

/// Inner writer parameterised on the state dir so tests exercise the exact JSON
/// shape without mutating process-global env (`AGENTUM_HOME`).
fn write_combined_config_in(state_dir: &Path, servers: &[McpServer]) -> Result<PathBuf> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let path = state_dir.join("mcp.json");
    crate::sdd_v2::artifacts::atomic_write(&path, config_json(servers).as_bytes(), None)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("securely publish {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn engine_seam_defaults_to_bound_and_honors_headless_override() {
        // Default (unset / empty / unknown) → bound: the agent drives the
        // browser the user watches (009c-1).
        assert_eq!(parse_engine(None), BrowserMcpEngine::PlaywrightBound);
        assert_eq!(parse_engine(Some("")), BrowserMcpEngine::PlaywrightBound);
        assert_eq!(
            parse_engine(Some("bound")),
            BrowserMcpEngine::PlaywrightBound
        );
        assert_eq!(
            parse_engine(Some("whatever")),
            BrowserMcpEngine::PlaywrightBound
        );
        // Explicit opt-out → the legacy hidden headless path.
        assert_eq!(
            parse_engine(Some("headless")),
            BrowserMcpEngine::PlaywrightHeadless
        );
        assert_eq!(
            parse_engine(Some("  HEADLESS  ")),
            BrowserMcpEngine::PlaywrightHeadless
        );
    }

    #[test]
    fn only_claude_and_codex_take_mcp_args() {
        // Only launch-argument transports are supported; Agentum never writes
        // provider configuration into a customer repository.
        assert!(tool_supports_mcp("claude"));
        assert!(tool_supports_mcp("codex"));
        for t in [
            "cursor", "gemini", "hermes", "terminal", "agent", "opencode", "bash",
        ] {
            assert!(!tool_supports_mcp(t));
        }
    }

    #[test]
    fn token_hash_is_stable_hex_sha256_and_hides_the_token() {
        // Deterministic + 64 hex chars (SHA-256), and never echoes the token.
        let h = token_hash("super-secret-token");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, token_hash("super-secret-token")); // stable
        assert_ne!(h, token_hash("different-token")); // differs on input
        assert!(!h.contains("super-secret-token"));
        // Known vector: SHA-256("") — guards against an accidental algo swap.
        assert_eq!(
            token_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn combined_config_writes_every_server_as_http() {
        // Round-trip the exact JSON Claude's `--mcp-config` consumes for N servers.
        let tmp = std::env::temp_dir().join(format!(
            "agentum-mcpprov-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let servers = vec![
            McpServer {
                name: "agentum".to_string(),
                url: "http://127.0.0.1:8822/mcp".to_string(),
                auth_token: Some("tok123".to_string()),
            },
            McpServer {
                name: "playwright".to_string(),
                url: "http://127.0.0.1:8931/mcp".to_string(),
                auth_token: None,
            },
        ];
        let path = write_combined_config_in(&tmp, &servers).unwrap();
        assert_eq!(path.file_name().unwrap(), "mcp.json");

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for (name, url) in [
            ("agentum", "http://127.0.0.1:8822/mcp"),
            ("playwright", "http://127.0.0.1:8931/mcp"),
        ] {
            assert_eq!(v["mcpServers"][name]["type"], "http");
            assert_eq!(v["mcpServers"][name]["url"], url);
            assert!(v["mcpServers"][name]["command"].is_null()); // streamable-HTTP, not stdio
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn combined_config_is_owner_only_and_never_follows_a_final_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let state = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "do-not-replace").unwrap();
        let target = state.path().join("mcp.json");
        symlink(outside.path(), &target).unwrap();
        let server = McpServer {
            name: "agentum".into(),
            url: "http://127.0.0.1:8822/mcp".into(),
            auth_token: Some("secret-token".into()),
        };
        assert!(write_combined_config_in(state.path(), std::slice::from_ref(&server)).is_err());
        assert_eq!(
            std::fs::read_to_string(outside.path()).unwrap(),
            "do-not-replace"
        );

        std::fs::remove_file(&target).unwrap();
        write_combined_config_in(state.path(), std::slice::from_ref(&server)).unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
