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

use agentum_core::Host;
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

/// Agents that load MCP from a config FILE in the project dir instead of a launch
/// flag (Cursor, Gemini, OpenCode). For these agentum writes a per-session config
/// in the workdir at launch — each has its own path, top-level key, and HTTP
/// field. This is what makes the MCP **agent-agnostic**: every agent gets the
/// agentum server, however it happens to load MCP.
#[derive(Debug, Clone, Copy)]
pub struct AgentMcpFile {
    /// Config path relative to the session workdir, e.g. `.cursor/mcp.json`.
    pub rel_path: &'static str,
    /// Top-level servers key: `mcpServers` (Cursor/Gemini) or `mcp` (OpenCode).
    pub servers_key: &'static str,
    /// The field holding the HTTP URL: `url` (Cursor/OpenCode) or `httpUrl`
    /// (Gemini).
    pub url_field: &'static str,
    /// Extra fixed fields on the server entry (OpenCode needs `type:"remote"`).
    pub extra: &'static [(&'static str, &'static str)],
}

/// The project-config descriptor for a file-based agent, or `None` for arg-based
/// / unsupported tools.
pub fn agent_mcp_file(tool: &str) -> Option<AgentMcpFile> {
    match tool {
        "cursor" | "agent" => Some(AgentMcpFile {
            rel_path: ".cursor/mcp.json",
            servers_key: "mcpServers",
            url_field: "url",
            extra: &[],
        }),
        "gemini" => Some(AgentMcpFile {
            rel_path: ".gemini/settings.json",
            servers_key: "mcpServers",
            url_field: "httpUrl",
            extra: &[],
        }),
        "opencode" => Some(AgentMcpFile {
            rel_path: "opencode.json",
            servers_key: "mcp",
            url_field: "url",
            extra: &[("type", "remote")],
        }),
        _ => None,
    }
}

/// The agentum server entry for a file-based agent (its HTTP field + bearer
/// header + any fixed extras).
fn agent_entry(file: &AgentMcpFile, server: &McpServer) -> serde_json::Value {
    let mut e = serde_json::Map::new();
    for (k, v) in file.extra {
        e.insert((*k).to_string(), serde_json::json!(v));
    }
    e.insert(file.url_field.to_string(), serde_json::json!(server.url));
    if let Some(token) = &server.auth_token {
        e.insert(
            "headers".to_string(),
            serde_json::json!({ "Authorization": format!("Bearer {token}") }),
        );
    }
    serde_json::Value::Object(e)
}

/// Merge the agentum server into an existing agent config (preserving the user's
/// other servers and settings — we never read or rewrite their secret values,
/// just round-trip the JSON) and return the new file content. `existing` is the
/// current file text, or `None` when it doesn't exist yet.
pub fn merge_agent_config(
    existing: Option<&str>,
    file: &AgentMcpFile,
    server: &McpServer,
) -> String {
    let mut root = existing
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = root.as_object_mut().expect("object");
    let servers = obj
        .entry(file.servers_key)
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        *servers = serde_json::json!({});
    }
    servers
        .as_object_mut()
        .expect("object")
        .insert("agentum".to_string(), agent_entry(file, server));
    serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".to_string())
}

/// Wire a **file-based** agent (Cursor/Gemini/OpenCode) by merging the agentum
/// MCP server into its project config in the session workdir — local fs or, for
/// an SSH session, on the host. Reads any existing config first so the user's
/// other servers survive. Best-effort: a failure logs and the agent just
/// launches without the agentum MCP.
pub async fn write_agent_project_config(
    state: &AppState,
    host: &Host,
    workdir: &str,
    tool: &str,
    agentum_mcp_url: &str,
) {
    let Some(file) = agent_mcp_file(tool) else {
        return;
    };
    let abs = format!("{}/{}", workdir.trim_end_matches('/'), file.rel_path);
    let server = agentum_server(state, agentum_mcp_url);
    let existing = crate::host_runtime::read_remote_file(host, &abs)
        .await
        .ok()
        .flatten();
    let merged = merge_agent_config(existing.as_deref(), &file, &server);
    if let Err(e) = crate::host_runtime::write_remote_file(host, &abs, &merged).await {
        tracing::warn!(tool, "could not write agentum MCP config to {abs}: {e:#}");
    }
}

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
    parse_engine(
        std::env::var("AGENTUM_BROWSER_MCP_ENGINE")
            .ok()
            .as_deref(),
    )
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
    std::fs::write(&path, config_json(servers))
        .with_context(|| format!("write {}", path.display()))?;
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
        assert_eq!(parse_engine(Some("bound")), BrowserMcpEngine::PlaywrightBound);
        assert_eq!(parse_engine(Some("whatever")), BrowserMcpEngine::PlaywrightBound);
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
        // Claude/Codex wire via launch args; Cursor/Gemini/OpenCode via a file.
        assert!(tool_supports_mcp("claude"));
        assert!(tool_supports_mcp("codex"));
        for t in [
            "cursor", "gemini", "hermes", "terminal", "agent", "opencode", "bash",
        ] {
            assert!(!tool_supports_mcp(t));
        }
    }

    #[test]
    fn file_based_agents_have_the_right_config_descriptor() {
        assert_eq!(
            agent_mcp_file("cursor").unwrap().rel_path,
            ".cursor/mcp.json"
        );
        assert_eq!(agent_mcp_file("cursor").unwrap().url_field, "url");
        assert_eq!(agent_mcp_file("gemini").unwrap().url_field, "httpUrl"); // gemini quirk
        assert_eq!(agent_mcp_file("opencode").unwrap().servers_key, "mcp"); // opencode quirk
        assert!(agent_mcp_file("claude").is_none()); // arg-based, no file
    }

    #[test]
    fn merge_preserves_existing_servers_and_adds_agentum() {
        let server = McpServer {
            name: "agentum".into(),
            url: "http://127.0.0.1:5555/mcp".into(),
            auth_token: Some("tok".into()),
        };
        // An existing Cursor config with the user's own (stdio) server.
        let existing = r#"{"mcpServers":{"toolbox":{"command":"npx","args":["x"]}}}"#;
        let file = agent_mcp_file("cursor").unwrap();
        let merged = merge_agent_config(Some(existing), &file, &server);
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        // user's server survives untouched
        assert_eq!(v["mcpServers"]["toolbox"]["command"], "npx");
        // agentum added with cursor's `url` + bearer header
        assert_eq!(
            v["mcpServers"]["agentum"]["url"],
            "http://127.0.0.1:5555/mcp"
        );
        assert_eq!(
            v["mcpServers"]["agentum"]["headers"]["Authorization"],
            "Bearer tok"
        );

        // OpenCode uses `mcp` + type:"remote".
        let oc = agent_mcp_file("opencode").unwrap();
        let m2: serde_json::Value =
            serde_json::from_str(&merge_agent_config(None, &oc, &server)).unwrap();
        assert_eq!(m2["mcp"]["agentum"]["type"], "remote");
        assert_eq!(m2["mcp"]["agentum"]["url"], "http://127.0.0.1:5555/mcp");
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
