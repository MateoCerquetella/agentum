//! Named-server profiles for the agentum TUI.
//!
//! A *profile* is a pinned set of connection inputs the user wants to
//! reach repeatedly: a base URL (with scheme + port), an optional cert
//! fingerprint, and the `insecure` flag. The bearer token is *not*
//! stored here — it lives in `credentials.toml`, keyed by the same
//! `host:port` the trust layer already uses, so a profile and its
//! credentials stay coherent without duplicating secrets.
//!
//! Storage: `$XDG_CONFIG_HOME/agentum/profiles.toml`, chmod 0600.
//! Format:
//!
//! ```toml
//! default = "local"
//!
//! [profiles.local]
//! url = "https://127.0.0.1:8822"
//!
//! [profiles.vps]
//! url = "https://my-vps.example.com:8822"
//! fingerprint = "AB:CD:..."
//! ```
//!
//! Surfaces:
//!
//! - `agentum profiles` subcommand (list / add / remove / use) for
//!   non-interactive management.
//! - `agentum terminal --profile NAME` to select a profile at startup;
//!   defaults to `default = …` if omitted, then to the loopback probe.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// One stored server. `url` is the user-facing string; we re-parse on
/// every load so a malformed entry surfaces immediately instead of
/// drifting into the active client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub url: String,
    /// Pre-pinned SHA-256 fingerprint. Optional — when absent, the
    /// trust layer's known_hosts file is consulted, exactly as for an
    /// ad-hoc `--api` invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Skip TLS verification entirely. `false` by default; here only
    /// for parity with the `--insecure` flag so a profile can encode
    /// "this throwaway dev box" once.
    #[serde(default)]
    pub insecure: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProfilesFile {
    /// Name of the profile to use when neither `--profile` nor `--api`
    /// is given. `None` ⇒ fall back to the loopback probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

pub struct Profiles {
    path: PathBuf,
    file: ProfilesFile,
}

impl Profiles {
    pub fn load() -> Result<Self> {
        let path = path()?;
        let file = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            toml::from_str::<ProfilesFile>(&raw)
                .with_context(|| format!("parse {}", path.display()))?
        } else {
            ProfilesFile::default()
        };
        Ok(Self { path, file })
    }

    pub fn list(&self) -> Vec<(String, Profile, bool)> {
        let default = self.normalized_default();
        self.file
            .profiles
            .iter()
            .map(|(name, p)| (name.clone(), p.clone(), Some(name.as_str()) == default))
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.file.profiles.get(name)
    }

    pub fn default_name(&self) -> Option<&str> {
        self.normalized_default()
    }

    /// `self.file.default` filtered so an empty string reads as "no
    /// default". A stray `default = ""` (legacy file, manual edit,
    /// pre-validation migration) used to make startup bail with
    /// `profile `` not found` and refuse to launch the TUI; we treat
    /// it as a missing field instead and fall through to the loopback
    /// probe.
    fn normalized_default(&self) -> Option<&str> {
        self.file
            .default
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn upsert(&mut self, name: String, profile: Profile) -> Result<()> {
        if name.trim().is_empty() {
            bail!("profile name must not be empty");
        }
        if !is_valid_name(&name) {
            bail!("profile names may only contain [a-zA-Z0-9._-]");
        }
        self.file.profiles.insert(name, profile);
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<bool> {
        let removed = self.file.profiles.remove(name).is_some();
        if removed {
            // Clear the default pointer if it pointed at the removed
            // entry; otherwise the next launch would still try to
            // resolve it and fall through to the loopback probe with a
            // confusing "profile X not found" error.
            if self.normalized_default() == Some(name) {
                self.file.default = None;
            }
            self.save()?;
        }
        Ok(removed)
    }

    pub fn set_default(&mut self, name: Option<String>) -> Result<()> {
        if let Some(ref n) = name {
            if !self.file.profiles.contains_key(n) {
                bail!("no such profile: {n}");
            }
        }
        self.file.default = name;
        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dir for {}", self.path.display()))?;
        }
        let body = toml::to_string_pretty(&self.file)?;
        std::fs::write(&self.path, body)
            .with_context(|| format!("write {}", self.path.display()))?;
        chmod_0600(&self.path)?;
        Ok(())
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// Profile name allowlist. The set is a strict subset of what TOML
/// keys allow so we never need quoting in the file. Reserved words
/// like `default` are fine here — they're scoped to the `profiles.`
/// table.
fn is_valid_name(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn path() -> Result<PathBuf> {
    let dir = agentum_store::paths::config_dir().map_err(|e| anyhow!("resolve config dir: {e}"))?;
    Ok(dir.join("profiles.toml"))
}

#[cfg(unix)]
fn chmod_0600(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    perm.set_mode(0o600);
    std::fs::set_permissions(path, perm)?;
    Ok(())
}

#[cfg(not(unix))]
fn chmod_0600(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(is_valid_name("local"));
        assert!(is_valid_name("vps-prod_2"));
        assert!(is_valid_name("alpha.beta"));
        assert!(!is_valid_name("with space"));
        assert!(!is_valid_name("slash/in/name"));
    }
}
