//! Collects the MCP servers to wire into an agent at launch and writes the
//! combined Claude `--mcp-config` file.
//!
//! Two servers can be provisioned per session:
//! - **agentum** — agentum's own MCP server (this daemon's `/mcp`). Free (no
//!   process to spawn — it's the already-running server), so it's wired by
//!   default whenever the agent can reach it without a bearer token (the
//!   embedded loopback server runs `no_auth`; so does a standalone `--no-auth`
//!   daemon). This is the "skills → MCP" surface: any agent gets agentum's
//!   tools with zero per-agent skill files.
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

/// Tools whose adapters consume an [`McpProvision`] (claude/codex). Other tools
/// ignore `mcp_args`, so we skip provisioning for them entirely.
const MCP_TOOLS: &[&str] = &["claude", "codex"];

/// Does this tool's adapter accept MCP wiring?
pub fn tool_supports_mcp(tool: &str) -> bool {
    MCP_TOOLS.contains(&tool)
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

    // agentum's own MCP — always wired. It's secured by the per-server bearer
    // token (every agent presents it; the `/mcp` handler 401s without it), so it
    // no longer matters whether the server is no-auth: the token is the gate.
    servers.push(McpServer {
        name: "agentum".to_string(),
        url: agentum_mcp_url.to_string(),
        auth_token: Some(state.mcp_token.as_str().to_string()),
    });

    // Playwright browser MCP — opt-in, spawns npx, best-effort. No auth.
    if playwright_mcp::feature_enabled() {
        match playwright_mcp::ensure_playwright_mcp().await {
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

/// Write the combined `{ "mcpServers": { … } }` Claude config under the state dir
/// and return its path. File scope (passed via `--mcp-config`), never a project
/// `.mcp.json` (which would trigger an approval prompt blocking an unattended
/// launch).
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
    let doc = serde_json::json!({ "mcpServers": map });
    let body = serde_json::to_string_pretty(&doc).context("serialize MCP config")?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn only_claude_and_codex_support_mcp() {
        assert!(tool_supports_mcp("claude"));
        assert!(tool_supports_mcp("codex"));
        for t in [
            "cursor", "gemini", "hermes", "terminal", "agent", "opencode", "bash",
        ] {
            assert!(!tool_supports_mcp(t));
        }
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
}
