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
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use crate::AppState;
use crate::playwright_mcp;

/// Tools that take MCP via a launch argument (`adapter.mcp_args`): Claude
/// (`--mcp-config <file>`) and Codex (`-c`). Wired in the argv at launch.
const MCP_ARG_TOOLS: &[&str] = &["claude", "codex"];

/// File-based clients resolve this launch-scoped variable from their project
/// config. The value is the complete Authorization header (`Bearer …`) so the
/// stable credential never lands in a repository file.
const PROJECT_MCP_AUTH_ENV_PREFIX: &str = "AGENTUM_MCP_AUTH_";
const CODEX_MCP_AUTH_ENV_PREFIX: &str = "AGENTUM_CODEX_MCP_AUTH_";
const SESSION_MCP_TOKEN_PREFIX: &str = "mcpv1";

/// Derive a stable, session-scoped bearer from the persisted server master
/// credential. It survives daemon restarts for a preserved pane, but a leaked
/// value identifies only one session and is rejected as soon as that session is
/// stopped/deleted. HMAC prevents one session token from deriving another.
pub fn session_mcp_token(master_token: &str, session_id: Uuid) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(master_token.as_bytes())
        .expect("HMAC accepts keys of every length");
    mac.update(b"agentum-mcp-session-v1\0");
    mac.update(session_id.as_bytes());
    let signature =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!(
        "{SESSION_MCP_TOKEN_PREFIX}_{}_{signature}",
        session_id.simple()
    )
}

/// Verify a scoped token without exposing the persisted master credential.
/// `verify_slice` is constant-time for equal-length MACs.
pub fn verify_session_mcp_token(master_token: &str, token: &str) -> Option<Uuid> {
    // The URL-safe base64 signature may itself contain `_`, so split only the
    // two structural separators. An unrestricted split made valid tokens fail
    // nondeterministically depending on their MAC bytes.
    let mut parts = token.splitn(3, '_');
    if parts.next()? != SESSION_MCP_TOKEN_PREFIX {
        return None;
    }
    let session_id = Uuid::parse_str(parts.next()?).ok()?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts.next()?)
        .ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(master_token.as_bytes()).ok()?;
    mac.update(b"agentum-mcp-session-v1\0");
    mac.update(session_id.as_bytes());
    mac.verify_slice(&signature).ok().map(|_| session_id)
}

#[derive(Debug, Clone, Copy)]
enum AuthEnvSyntax {
    Cursor,
    Gemini,
    OpenCode,
}

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
    /// Tool-native environment interpolation for an HTTP header value.
    auth_env_syntax: AuthEnvSyntax,
}

impl AgentMcpFile {
    pub fn auth_header_value(self, env_name: &str) -> String {
        match self.auth_env_syntax {
            AuthEnvSyntax::Cursor => format!("${{env:{env_name}}}"),
            AuthEnvSyntax::Gemini => format!("${env_name}"),
            AuthEnvSyntax::OpenCode => format!("{{env:{env_name}}}"),
        }
    }
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
            auth_env_syntax: AuthEnvSyntax::Cursor,
        }),
        "gemini" => Some(AgentMcpFile {
            rel_path: ".gemini/settings.json",
            servers_key: "mcpServers",
            url_field: "httpUrl",
            extra: &[],
            auth_env_syntax: AuthEnvSyntax::Gemini,
        }),
        "opencode" => Some(AgentMcpFile {
            rel_path: "opencode.json",
            servers_key: "mcp",
            url_field: "url",
            extra: &[("type", "remote")],
            auth_env_syntax: AuthEnvSyntax::OpenCode,
        }),
        _ => None,
    }
}

/// The agentum server entry for a file-based agent (its HTTP field + bearer
/// header + any fixed extras).
fn agent_entry(file: &AgentMcpFile, server: &McpServer, auth_env_name: &str) -> serde_json::Value {
    let mut e = serde_json::Map::new();
    for (k, v) in file.extra {
        e.insert((*k).to_string(), serde_json::json!(v));
    }
    e.insert(file.url_field.to_string(), serde_json::json!(server.url));
    if server.auth_token.is_some() {
        e.insert(
            "headers".to_string(),
            serde_json::json!({ "Authorization": file.auth_header_value(auth_env_name) }),
        );
    }
    serde_json::Value::Object(e)
}

/// Launch-scoped secret backing a file-based agent's non-secret config
/// placeholder. The tmux launch transactions stage environment privately and
/// unlink it before exec, so the value is absent from argv and project files.
fn fresh_project_auth_env_name(existing: Option<&str>) -> String {
    loop {
        let suffix = uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .to_ascii_uppercase();
        let name = format!("{PROJECT_MCP_AUTH_ENV_PREFIX}{suffix}");
        if !existing.is_some_and(|content| content.contains(&name)) {
            return name;
        }
    }
}

/// Generate the opaque process-environment handle Codex receives for one MCP
/// bearer. The UUID is intentionally unrelated to the server name or session
/// id: observing argv must not reveal a reusable, predictable token locator.
fn fresh_codex_auth_env_name() -> String {
    format!(
        "{CODEX_MCP_AUTH_ENV_PREFIX}{}",
        Uuid::new_v4().simple().to_string().to_ascii_uppercase()
    )
}

/// Merge the agentum server into an existing agent config (preserving the user's
/// other servers and settings — we never read or rewrite their secret values,
/// just round-trip the JSON) and return the new file content. `existing` is the
/// current file text, or `None` when it doesn't exist yet.
pub fn merge_agent_config(
    existing: Option<&str>,
    file: &AgentMcpFile,
    server: &McpServer,
    auth_env_name: &str,
) -> Result<String> {
    let mut root = match existing {
        Some(content) => serde_json::from_str::<serde_json::Value>(content)
            .context("existing agent project config is not valid JSON")?,
        None => serde_json::json!({}),
    };
    let obj = root
        .as_object_mut()
        .context("existing agent project config root is not a JSON object")?;
    let servers = obj
        .entry(file.servers_key)
        .or_insert_with(|| serde_json::json!({}));
    servers
        .as_object_mut()
        .with_context(|| {
            format!(
                "existing `{}` value is not a JSON object; refusing to replace it",
                file.servers_key
            )
        })?
        .insert(
            "agentum".to_string(),
            agent_entry(file, server, auth_env_name),
        );
    serde_json::to_string_pretty(&root).context("serialize merged agent project config")
}

/// Wire a **file-based** agent (Cursor/Gemini/OpenCode) by merging the agentum
/// MCP server into its project config in the session workdir — local fs or, for
/// an SSH session, on the host. Reads any existing config first so the user's
/// other servers survive. Invalid/unreadable input is returned instead of being
/// replaced: Agentum never erases a user's JSON/JSONC on a transient failure.
/// On success the returned random environment name/value backs only the owned
/// `agentum` entry for this one launch.
pub async fn write_agent_project_config(
    state: &AppState,
    session_id: Uuid,
    host: &Host,
    workdir: &str,
    tool: &str,
    agentum_mcp_url: &str,
) -> Result<Option<(String, String)>> {
    let Some(file) = agent_mcp_file(tool) else {
        return Ok(None);
    };
    let abs = format!("{}/{}", workdir.trim_end_matches('/'), file.rel_path);
    let server = agentum_server(state, session_id, agentum_mcp_url);
    let existing = crate::host_runtime::read_remote_file(host, &abs)
        .await
        .with_context(|| format!("read existing {tool} MCP config {abs}"))?;
    let env_name = fresh_project_auth_env_name(existing.as_deref());
    let merged = merge_agent_config(existing.as_deref(), &file, &server, &env_name)?;
    crate::host_runtime::write_remote_file(host, &abs, &merged)
        .await
        .with_context(|| format!("write {tool} MCP config {abs}"))?;
    Ok(server
        .auth_token
        .map(|token| (env_name, format!("Bearer {token}"))))
}

/// The Mac-side MCP-only port to reverse-tunnel to. This deliberately never
/// falls back to `api_base_url`: the embedded REST listener is loopback/no-auth
/// for the local TUI and must not become reachable from an SSH host.
pub fn local_mcp_port(state: &AppState) -> Option<u16> {
    state
        .mcp_base_url
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
    session_id: Uuid,
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
    servers.push(agentum_server(state, session_id, agentum_mcp_url));

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
                auth_env_var: None,
            }),
            Err(e) => {
                tracing::warn!("Playwright MCP not provisioned; skipping: {e:#}")
            }
        }
    }

    if servers.is_empty() {
        return None;
    }

    // Only Claude consumes the combined file. Codex receives the same servers
    // through argv references + its private child environment, so writing a
    // second at-rest copy of its bearer would provide no value.
    let config_file = if tool == "claude" {
        match write_combined_config(session_id, &servers) {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!("could not write Claude MCP config; launching without MCP: {e:#}");
                return None;
            }
        }
    } else {
        PathBuf::new()
    };
    Some(McpProvision {
        servers,
        config_file,
    })
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

/// The agentum-MCP server entry for a session: the endpoint the agent should
/// reach (loopback locally, tunnel port remotely) plus the bearer token.
pub fn agentum_server(state: &AppState, session_id: Uuid, agentum_mcp_url: &str) -> McpServer {
    McpServer {
        name: "agentum".to_string(),
        url: agentum_mcp_url.to_string(),
        auth_token: Some(session_mcp_token(state.mcp_token.as_str(), session_id)),
        auth_env_var: Some(fresh_codex_auth_env_name()),
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
pub fn write_combined_config(session_id: Uuid, servers: &[McpServer]) -> Result<PathBuf> {
    let dir = agentum_store::paths::state_dir().context("resolve agentum state dir")?;
    write_combined_config_in(&dir, session_id, servers)
}

/// Inner writer parameterised on the state dir so tests exercise the exact JSON
/// shape without mutating process-global env (`AGENTUM_HOME`).
fn write_combined_config_in(
    state_dir: &Path,
    session_id: Uuid,
    servers: &[McpServer],
) -> Result<PathBuf> {
    ensure_private_state_dir(state_dir)?;
    let path = combined_config_path_in(state_dir, session_id);
    atomic_private_write(&path, config_json(servers).as_bytes())?;
    Ok(path)
}

fn combined_config_path_in(state_dir: &Path, session_id: Uuid) -> PathBuf {
    state_dir.join(format!("mcp-{session_id}.json"))
}

/// Remove the local secret-bearing Claude config for a completed session.
///
/// Lifecycle callers should invoke this after a local Claude pane is stopped,
/// killed, deleted, or a launch transaction rolls back. Active panes retain
/// the file because Claude may continue to read its startup configuration.
pub fn remove_combined_config(session_id: Uuid) -> Result<()> {
    let dir = agentum_store::paths::state_dir().context("resolve agentum state dir")?;
    remove_combined_config_in(&dir, session_id)
}

fn remove_combined_config_in(state_dir: &Path, session_id: Uuid) -> Result<()> {
    let path = combined_config_path_in(state_dir, session_id);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
                "refusing to remove non-regular MCP config: {}",
                path.display()
            );
            std::fs::remove_file(&path)
                .with_context(|| format!("remove MCP config {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect MCP config {}", path.display())),
    }
}

#[cfg(unix)]
fn ensure_private_state_dir(state_dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let metadata = std::fs::symlink_metadata(state_dir)
        .with_context(|| format!("inspect state dir {}", state_dir.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "agentum state path is not a real directory: {}",
        state_dir.display()
    );
    std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure state dir {}", state_dir.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_state_dir(state_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))
}

/// Replace a secret-bearing MCP config in one rename. Readers see either the
/// complete old file or the complete new file, and Unix creates the staged
/// inode owner-only from its first instant (the process umask cannot widen it).
fn atomic_private_write(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .context("MCP config destination has no parent directory")?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "refusing to replace non-regular MCP config: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("inspect MCP config {}", path.display()));
        }
    }
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("mcp-config"),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .with_context(|| format!("stage MCP config in {}", parent.display()))?;
    let staged = file.write_all(contents).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = staged {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("stage MCP config {}", path.display()));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
        {
            let _ = std::fs::remove_file(&temp);
            return Err(error).with_context(|| format!("secure MCP config {}", path.display()));
        }
    }

    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("replace MCP config {}", path.display()));
    }
    Ok(())
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
            auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_TEST".into()),
        };
        // An existing Cursor config with the user's own (stdio) server.
        let existing = r#"{"mcpServers":{"toolbox":{"command":"npx","args":["x"]}}}"#;
        let file = agent_mcp_file("cursor").unwrap();
        let env_name = "AGENTUM_MCP_AUTH_TEST";
        let merged = merge_agent_config(Some(existing), &file, &server, env_name).unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        // user's server survives untouched
        assert_eq!(v["mcpServers"]["toolbox"]["command"], "npx");
        // agentum added with cursor's `url` + non-secret environment reference
        assert_eq!(
            v["mcpServers"]["agentum"]["url"],
            "http://127.0.0.1:5555/mcp"
        );
        assert_eq!(
            v["mcpServers"]["agentum"]["headers"]["Authorization"],
            "${env:AGENTUM_MCP_AUTH_TEST}"
        );
        assert!(!merged.contains("Bearer tok"));

        // OpenCode uses `mcp` + type:"remote".
        let oc = agent_mcp_file("opencode").unwrap();
        let m2: serde_json::Value =
            serde_json::from_str(&merge_agent_config(None, &oc, &server, env_name).unwrap())
                .unwrap();
        assert_eq!(m2["mcp"]["agentum"]["type"], "remote");
        assert_eq!(m2["mcp"]["agentum"]["url"], "http://127.0.0.1:5555/mcp");
        assert_eq!(
            m2["mcp"]["agentum"]["headers"]["Authorization"],
            "{env:AGENTUM_MCP_AUTH_TEST}"
        );
    }

    #[test]
    fn project_auth_names_are_unpredictable_and_avoid_existing_config_refs() {
        let first = fresh_project_auth_env_name(None);
        let existing = format!(r#"{{"other":"${{{first}}}"}}"#);
        let second = fresh_project_auth_env_name(Some(&existing));
        assert!(first.starts_with(PROJECT_MCP_AUTH_ENV_PREFIX));
        assert!(second.starts_with(PROJECT_MCP_AUTH_ENV_PREFIX));
        assert_ne!(first, second);
        assert!(!existing.contains(&second));
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        );
    }

    #[test]
    fn codex_auth_names_are_valid_and_unpredictable_per_provisioning() {
        let first = fresh_codex_auth_env_name();
        let second = fresh_codex_auth_env_name();
        assert!(first.starts_with(CODEX_MCP_AUTH_ENV_PREFIX));
        assert!(second.starts_with(CODEX_MCP_AUTH_ENV_PREFIX));
        assert_ne!(first, second);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        );
    }

    #[test]
    fn session_mcp_tokens_are_scoped_stable_and_unforgeable_across_sessions() {
        let master = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first = session_mcp_token(master, first_id);
        let second = session_mcp_token(master, second_id);

        assert_eq!(first, session_mcp_token(master, first_id));
        assert_ne!(first, second);
        assert_eq!(verify_session_mcp_token(master, &first), Some(first_id));
        assert_eq!(verify_session_mcp_token(master, &second), Some(second_id));
        assert_eq!(verify_session_mcp_token("different-master", &first), None);
        let mut tampered = first;
        tampered.push('x');
        assert_eq!(verify_session_mcp_token(master, &tampered), None);
    }

    #[test]
    fn invalid_existing_project_config_is_never_replaced() {
        let file = agent_mcp_file("cursor").unwrap();
        let server = McpServer {
            name: "agentum".into(),
            url: "http://127.0.0.1:5555/mcp".into(),
            auth_token: Some("tok".into()),
            auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_TEST".into()),
        };
        assert!(merge_agent_config(Some("{/* jsonc */}"), &file, &server, "SAFE_ENV").is_err());
        assert!(merge_agent_config(Some("[]"), &file, &server, "SAFE_ENV").is_err());
        assert!(
            merge_agent_config(
                Some(r#"{"mcpServers":"do-not-replace"}"#),
                &file,
                &server,
                "SAFE_ENV"
            )
            .is_err()
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
                auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_TEST".to_string()),
            },
            McpServer {
                name: "playwright".to_string(),
                url: "http://127.0.0.1:8931/mcp".to_string(),
                auth_token: None,
                auth_env_var: None,
            },
        ];
        let session_id = Uuid::new_v4();
        let path = write_combined_config_in(&tmp, session_id, &servers).unwrap();
        assert_eq!(
            path.file_name().unwrap(),
            format!("mcp-{session_id}.json").as_str()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

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

    #[test]
    fn combined_configs_are_isolated_by_session_and_cleanup_is_scoped() {
        let tmp = std::env::temp_dir().join(format!(
            "agentum-mcpprov-isolation-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first_servers = vec![McpServer {
            name: "agentum".into(),
            url: "http://127.0.0.1:8822/mcp".into(),
            auth_token: Some("first-private-token".into()),
            auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_FIRST".into()),
        }];
        let second_servers = vec![McpServer {
            name: "agentum".into(),
            url: "http://127.0.0.1:8822/mcp".into(),
            auth_token: Some("second-private-token".into()),
            auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_SECOND".into()),
        }];

        let first = write_combined_config_in(&tmp, first_id, &first_servers).unwrap();
        let second = write_combined_config_in(&tmp, second_id, &second_servers).unwrap();
        assert_ne!(first, second);
        assert!(
            std::fs::read_to_string(&first)
                .unwrap()
                .contains("first-private-token")
        );
        assert!(
            std::fs::read_to_string(&second)
                .unwrap()
                .contains("second-private-token")
        );

        remove_combined_config_in(&tmp, first_id).unwrap();
        assert!(!first.exists());
        assert!(second.exists());
        remove_combined_config_in(&tmp, first_id).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn combined_config_rejects_a_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "agentum-mcpprov-link-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let outside = root.join("outside.json");
        std::fs::write(&outside, "do-not-touch").unwrap();
        let session_id = Uuid::new_v4();
        let path = combined_config_path_in(&state, session_id);
        symlink(&outside, &path).unwrap();

        let servers = vec![McpServer {
            name: "agentum".to_string(),
            url: "http://127.0.0.1:8822/mcp".to_string(),
            auth_token: Some("private-token".to_string()),
            auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_TEST".to_string()),
        }];
        let error = write_combined_config_in(&state, session_id, &servers).unwrap_err();

        assert!(error.to_string().contains("non-regular MCP config"));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "do-not-touch");
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let cleanup_error = remove_combined_config_in(&state, session_id).unwrap_err();
        assert!(cleanup_error.to_string().contains("non-regular MCP config"));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "do-not-touch");
        assert!(std::fs::read_dir(&state).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
        let _ = std::fs::remove_dir_all(&root);
    }
}
