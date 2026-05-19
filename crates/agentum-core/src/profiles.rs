//! Named-server profiles shared by the TUI and the daemon.
//!
//! A *profile* is a pinned set of connection inputs the user wants to
//! reach repeatedly: a base URL (with scheme + port), an optional cert
//! fingerprint, and the `insecure` flag. Bearer tokens are *not*
//! stored here — they live next to the trust layer (`credentials.toml`
//! on the TUI side, browser-local on the dashboard) so a profile and
//! its credentials stay coherent without duplicating secrets.
//!
//! Storage: TOML file at a caller-supplied path, chmod 0600.
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
//! This module is path-agnostic on purpose: `agentum-core` mustn't pull
//! in `directories` or anything filesystem-shaped beyond `std`. Callers
//! resolve the canonical location (`$XDG_CONFIG_HOME/agentum/profiles.toml`)
//! via `agentum_store::paths::config_dir()` and pass it in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
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
    /// Load the profile set from `path`. A missing file is treated as
    /// an empty set so first-run callers don't need a separate branch.
    pub fn load_from(path: PathBuf) -> Result<Self> {
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

    /// Read-only access to the underlying file for callers that need
    /// to serialize the full document (e.g. the REST `GET /api/profiles`
    /// handler).
    pub fn file(&self) -> &ProfilesFile {
        &self.file
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

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Profile name allowlist. The set is a strict subset of what TOML
/// keys allow so we never need quoting in the file. Reserved words
/// like `default` are fine here — they're scoped to the `profiles.`
/// table.
pub fn is_valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(unix)]
fn chmod_0600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    perm.set_mode(0o600);
    std::fs::set_permissions(path, perm)?;
    Ok(())
}

#[cfg(not(unix))]
fn chmod_0600(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn name_validation() {
        assert!(is_valid_name("local"));
        assert!(is_valid_name("vps-prod_2"));
        assert!(is_valid_name("alpha.beta"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("with space"));
        assert!(!is_valid_name("slash/in/name"));
    }

    #[test]
    fn round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("profiles.toml");

        let mut p = Profiles::load_from(path.clone()).unwrap();
        assert!(p.list().is_empty());

        p.upsert(
            "local".into(),
            Profile {
                url: "https://127.0.0.1:8822".into(),
                fingerprint: None,
                insecure: false,
            },
        )
        .unwrap();
        p.upsert(
            "vps".into(),
            Profile {
                url: "https://my-vps:8822".into(),
                fingerprint: Some("AB:CD".into()),
                insecure: false,
            },
        )
        .unwrap();
        p.set_default(Some("local".into())).unwrap();

        // Drop and reload — assert persistence.
        drop(p);
        let p2 = Profiles::load_from(path.clone()).unwrap();
        assert_eq!(p2.list().len(), 2);
        assert_eq!(p2.default_name(), Some("local"));
        assert_eq!(p2.get("vps").unwrap().fingerprint.as_deref(), Some("AB:CD"));
    }

    #[test]
    fn removing_default_clears_pointer() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("profiles.toml");
        let mut p = Profiles::load_from(path).unwrap();
        p.upsert(
            "local".into(),
            Profile {
                url: "https://127.0.0.1:8822".into(),
                fingerprint: None,
                insecure: false,
            },
        )
        .unwrap();
        p.set_default(Some("local".into())).unwrap();
        p.remove("local").unwrap();
        assert!(p.default_name().is_none());
    }

    #[test]
    fn empty_default_string_is_ignored() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("profiles.toml");
        std::fs::write(
            &path,
            "default = \"\"\n[profiles.local]\nurl = \"https://x\"\n",
        )
        .unwrap();
        let p = Profiles::load_from(path).unwrap();
        assert!(p.default_name().is_none());
    }
}
