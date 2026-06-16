//! Loopback discovery for the desktop's embedded server.
//!
//! The desktop boots `agentum-server` on an EPHEMERAL loopback port and only
//! advertises that port to panes it spawns (via `$AGENTUM_API_URL`). A CLI run
//! OUTSIDE such a pane has no way to find that port: it falls back to the
//! standalone-daemon address (TLS `:8822`), where `/api/browser/*` and
//! `/api/computer/*` 501 — and where a plain-http client can't even speak to
//! the TLS listener. That is exactly the failure the capability-card example
//! prompts hit when pasted into a user's own Codex/Claude Code terminal.
//!
//! To bridge that gap the desktop writes its base URL to a well-known file at
//! boot; the CLI reads it as a discovery fallback (after `$AGENTUM_API_URL` and
//! any active profile, before the conventional `:8822` default). The file is
//! best-effort: a missing entry simply means "no desktop found", and a stale
//! entry (desktop quit, or an OS-recycled ephemeral port) is caught by a
//! liveness probe at the call site. The standalone daemon does NOT write this
//! file — it lives at a fixed address reachable via profiles.

use std::path::PathBuf;

use crate::paths::{self, PathError};

/// `…/state/desktop-api.url` — the single source of truth for the path. Lives
/// under [`paths::state_dir`] (ephemeral runtime state, honors `$AGENTUM_HOME`).
pub fn desktop_api_url_path() -> Result<PathBuf, PathError> {
    Ok(paths::state_dir()?.join("desktop-api.url"))
}

/// Advertise the desktop's embedded-server base URL (e.g.
/// `http://127.0.0.1:50990`). Called once at desktop boot. Best-effort:
/// discovery is a convenience, never a hard dependency, so any I/O error is
/// swallowed rather than blocking the app.
pub fn advertise_desktop_api_url(url: &str) {
    let Ok(path) = desktop_api_url_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, url.trim());
}

/// Remove the advertisement (desktop shutdown). Best-effort — a leftover file
/// is harmless because readers probe liveness before trusting it.
pub fn clear_desktop_api_url() {
    if let Ok(path) = desktop_api_url_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// The advertised desktop URL, if a non-empty file exists. Does NOT verify the
/// server is still listening — callers must probe before trusting it.
pub fn advertised_desktop_api_url() -> Option<String> {
    let path = desktop_api_url_path().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `AGENTUM_HOME` is process-global; serialise the tests that flip it so they
    // don't clobber each other's temp roots.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn advertise_then_read_then_clear_round_trips() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: serialised by ENV_LOCK — only one thread mutates env at a time.
        unsafe {
            std::env::set_var("AGENTUM_HOME", dir.path());
        }

        // Nothing advertised yet.
        assert_eq!(advertised_desktop_api_url(), None);

        advertise_desktop_api_url("http://127.0.0.1:50990");
        assert_eq!(
            advertised_desktop_api_url().as_deref(),
            Some("http://127.0.0.1:50990")
        );

        clear_desktop_api_url();
        assert_eq!(advertised_desktop_api_url(), None);

        unsafe {
            std::env::remove_var("AGENTUM_HOME");
        }
    }

    #[test]
    fn blank_advertisement_reads_as_none() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: serialised by ENV_LOCK.
        unsafe {
            std::env::set_var("AGENTUM_HOME", dir.path());
        }

        // A whitespace-only file must not shadow the daemon default downstream.
        advertise_desktop_api_url("   \n");
        assert_eq!(advertised_desktop_api_url(), None);

        unsafe {
            std::env::remove_var("AGENTUM_HOME");
        }
    }
}
