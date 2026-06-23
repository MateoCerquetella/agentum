//! Stable MCP endpoint persistence: keep the embedded loopback **port** and the
//! `/mcp` bearer **token** the same across desktop/server restarts.
//!
//! An agent session bakes its agentum MCP connection (`--mcp-config` URL +
//! `Authorization: Bearer …`, and the `AGENTUM_API_URL` env) **once at spawn**
//! and never re-reads it. The embedded server, however, used to bind a fresh
//! ephemeral port and mint a new token on every boot — so a desktop restart
//! silently invalidated every live session's MCP connection (dial the old port
//! → connection refused; dial the new port with the old token → 401).
//!
//! Persisting both values makes the spawn-time snapshot *stay valid* across
//! restarts, which is the only thing that repairs a live agent process (it
//! can't be told to re-read its config). See
//! `docs/plans/MCP_CONNECTION_PERSISTENCE_PRD.md` (R1 token, R2 port).
//!
//! Both files live under `state_dir()` next to `mcp.json`. The `*_in(dir)`
//! seams mirror `mcp_provision::write_combined_config_in` so the logic is
//! unit-testable without mutating the process-global `AGENTUM_HOME`.

use std::net::Ipv4Addr;
use std::path::Path;

use tokio::net::TcpListener;

/// agentum's conventional loopback port — matches the standalone daemon default
/// and the `127.0.0.1:8822` profile default. The embedded server prefers this
/// on first run (no port persisted yet) so a fresh install lands on a
/// predictable, stable port rather than a random one.
const DEFAULT_MCP_PORT: u16 = 8822;

/// Persisted last-bound loopback port (plain text, e.g. `8822`).
const PORT_FILE: &str = "server_port";
/// Persisted `/mcp` bearer token, `0600`.
const TOKEN_FILE: &str = "mcp_token";

/// Bind a loopback [`TcpListener`], preferring the last port we successfully
/// bound (or [`DEFAULT_MCP_PORT`] on first run). If that port is taken — a
/// standalone `agentum serve` already holds 8822, or a stale instance lingers —
/// fall back to an OS-assigned ephemeral port so boot never hard-fails. The
/// actual bound port is persisted for next time.
pub async fn bind_stable_loopback() -> anyhow::Result<TcpListener> {
    let dir = agentum_store::paths::state_dir().ok();
    let preferred = dir
        .as_deref()
        .and_then(read_saved_port_in)
        .unwrap_or(DEFAULT_MCP_PORT);
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, preferred)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                port = preferred,
                error = %e,
                "preferred MCP port unavailable; binding an ephemeral port instead"
            );
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?
        }
    };
    if let (Some(dir), Ok(addr)) = (dir.as_deref(), listener.local_addr()) {
        save_port_in(dir, addr.port());
    }
    Ok(listener)
}

/// Load the persisted `/mcp` bearer token, or mint **and persist** a new one on
/// first run. Infallible: on any IO error we fall back to a fresh in-memory
/// token so boot still succeeds — that one session just reverts to the old
/// "doesn't survive a restart" behavior instead of crashing the server.
pub fn load_or_create_mcp_token() -> String {
    match agentum_store::paths::state_dir() {
        Ok(dir) => load_or_create_mcp_token_in(&dir),
        Err(e) => {
            tracing::warn!(error = %e, "no state dir; MCP token will not persist this boot");
            crate::auth::new_token()
        }
    }
}

fn load_or_create_mcp_token_in(state_dir: &Path) -> String {
    if let Some(tok) = read_token_in(state_dir) {
        return tok;
    }
    let tok = crate::auth::new_token();
    write_token_in(state_dir, &tok);
    tok
}

fn read_saved_port_in(state_dir: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(state_dir.join(PORT_FILE)).ok()?;
    let port: u16 = raw.trim().parse().ok()?;
    // A persisted 0 (or anything unparsable) means "no stable port yet" → let
    // the caller fall back to the default rather than asking the OS for a
    // random ephemeral port that would never re-stabilize.
    (port != 0).then_some(port)
}

fn save_port_in(state_dir: &Path, port: u16) {
    let _ = std::fs::create_dir_all(state_dir);
    let path = state_dir.join(PORT_FILE);
    if let Err(e) = std::fs::write(&path, port.to_string()) {
        tracing::warn!(path = %path.display(), error = %e, "could not persist MCP server port");
    }
}

fn read_token_in(state_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(state_dir.join(TOKEN_FILE)).ok()?;
    let tok = raw.trim().to_string();
    (!tok.is_empty()).then_some(tok)
}

fn write_token_in(state_dir: &Path, token: &str) {
    let _ = std::fs::create_dir_all(state_dir);
    let path = state_dir.join(TOKEN_FILE);
    if let Err(e) = std::fs::write(&path, token) {
        tracing::warn!(path = %path.display(), error = %e, "could not persist MCP token");
        return;
    }
    // The token guards the /mcp tool surface — keep it owner-only at rest.
    set_owner_only(&path);
}

#[cfg(unix)]
fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(path = %path.display(), error = %e, "could not chmod 0600 MCP token");
    }
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_roundtrips_through_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_saved_port_in(dir.path()), None, "absent → None");
        save_port_in(dir.path(), 8822);
        assert_eq!(read_saved_port_in(dir.path()), Some(8822));
    }

    #[test]
    fn persisted_zero_or_garbage_reads_as_none() {
        // So the caller falls back to DEFAULT_MCP_PORT, not a doomed bind(0).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PORT_FILE), "0").unwrap();
        assert_eq!(read_saved_port_in(dir.path()), None);
        std::fs::write(dir.path().join(PORT_FILE), "not-a-port").unwrap();
        assert_eq!(read_saved_port_in(dir.path()), None);
    }

    #[test]
    fn token_is_created_once_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_mcp_token_in(dir.path());
        assert_eq!(first.len(), 43, "fresh token has new_token() shape");
        let second = load_or_create_mcp_token_in(dir.path());
        assert_eq!(first, second, "second boot reuses the persisted token");
    }

    #[test]
    fn blank_token_file_is_treated_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(TOKEN_FILE), "   \n").unwrap();
        assert_eq!(read_token_in(dir.path()), None);
        let tok = load_or_create_mcp_token_in(dir.path());
        assert!(!tok.is_empty());
        assert_eq!(read_token_in(dir.path()).as_deref(), Some(tok.as_str()));
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        load_or_create_mcp_token_in(dir.path());
        let mode = std::fs::metadata(dir.path().join(TOKEN_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
