//! TUI-facing shim around [`agentum_core::profiles`].
//!
//! `agentum-core` keeps the `Profiles` type path-agnostic so the daemon
//! can serve the same file via `/api/profiles` without taking a runtime
//! dependency on the user-config-dir crate. This shim resolves the
//! canonical `$XDG_CONFIG_HOME/agentum/profiles.toml` path and delegates
//! everything else, so call sites in the TUI keep using
//! `profiles::Profile`, `profiles::ProfilesFile`, `profiles::load()`,
//! and `profiles::path()` as before.

pub use agentum_core::profiles::{Profile, Profiles, ProfilesFile, is_valid_name};

use anyhow::{Result, anyhow};
use std::path::PathBuf;

/// Canonical location of the shared profiles file.
pub fn path() -> Result<PathBuf> {
    let dir = agentum_store::paths::config_dir().map_err(|e| anyhow!("resolve config dir: {e}"))?;
    Ok(dir.join("profiles.toml"))
}

/// Load the profile set from the canonical XDG location.
///
/// Delegates to `agentum_core::profiles::Profiles::load()` so the TUI
/// and the new `agentum clip-agent` subcommand resolve the same path
/// the same way — no duplicated env-var handling.
pub fn load() -> Result<Profiles> {
    Profiles::load().map_err(|e| anyhow!("load profiles: {e}"))
}
