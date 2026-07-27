//! Stable MCP endpoint persistence with explicit one-time bearer rotations.
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
const TOKEN_EPOCH_FILE: &str = "mcp_token_epoch";
/// Bumping this value is an intentional credential incident migration. Existing
/// installs rotate once; later boots reuse the replacement token.
const CURRENT_TOKEN_EPOCH: &str = "agentum-sdd-boundary-v1";

#[derive(Debug)]
pub struct McpTokenLoad {
    pub token: String,
    pub rotated: bool,
}

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
    load_or_rotate_mcp_token().token
}

pub fn load_or_rotate_mcp_token() -> McpTokenLoad {
    match agentum_store::paths::state_dir() {
        Ok(dir) => load_or_rotate_mcp_token_in(&dir),
        Err(e) => {
            tracing::warn!(error = %e, "no state dir; MCP token will not persist this boot");
            McpTokenLoad {
                token: crate::auth::new_token(),
                rotated: true,
            }
        }
    }
}

#[cfg(test)]
fn load_or_create_mcp_token_in(state_dir: &Path) -> String {
    load_or_rotate_mcp_token_in(state_dir).token
}

fn load_or_rotate_mcp_token_in(state_dir: &Path) -> McpTokenLoad {
    let epoch_is_current = std::fs::read_to_string(state_dir.join(TOKEN_EPOCH_FILE))
        .ok()
        .is_some_and(|value| value.trim() == CURRENT_TOKEN_EPOCH);
    if epoch_is_current && let Some(token) = read_token_in(state_dir) {
        return McpTokenLoad {
            token,
            rotated: false,
        };
    }
    let token = crate::auth::new_token();
    // Token first, epoch second. A crash between them rotates once more on the
    // next boot; it can never mark an old/exposed token as migrated.
    if write_owner_only_atomic(state_dir, TOKEN_FILE, &token)
        && write_owner_only_atomic(state_dir, TOKEN_EPOCH_FILE, CURRENT_TOKEN_EPOCH)
    {
        tracing::info!("MCP bearer rotation epoch applied");
    } else {
        tracing::warn!("MCP bearer rotation could not be persisted; using an in-memory token");
    }
    McpTokenLoad {
        token,
        rotated: true,
    }
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

fn write_owner_only_atomic(state_dir: &Path, name: &str, value: &str) -> bool {
    if let Err(error) = std::fs::create_dir_all(state_dir) {
        tracing::warn!(error = %error, "could not create MCP state directory");
        return false;
    }
    let path = state_dir.join(name);
    let result = crate::sdd::artifacts::atomic_write(&path, value.as_bytes(), None);
    if let Err(error) = result {
        tracing::warn!(path = %path.display(), error = %error, "could not persist MCP credential state");
        false
    } else {
        true
    }
}

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
    fn legacy_persisted_token_rotates_once_and_records_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let exposed = "x".repeat(43);
        std::fs::write(dir.path().join(TOKEN_FILE), &exposed).unwrap();

        let first = load_or_rotate_mcp_token_in(dir.path());
        assert!(first.rotated);
        assert_ne!(first.token, exposed);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(TOKEN_EPOCH_FILE)).unwrap(),
            CURRENT_TOKEN_EPOCH
        );

        let second = load_or_rotate_mcp_token_in(dir.path());
        assert!(!second.rotated);
        assert_eq!(second.token, first.token);
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
