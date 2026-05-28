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

    /// Convenience wrapper that resolves the canonical
    /// `$XDG_CONFIG_HOME/agentum/profiles.toml` path (falling back to
    /// `$HOME/.config/agentum/profiles.toml`) and delegates to
    /// `load_from`. Used by both the TUI shim (so it stops having to
    /// duplicate the resolution logic) and the new clip-agent loop.
    ///
    /// Deliberately uses `std::env` instead of the `directories` crate
    /// so `agentum-core` stays dependency-light — pulling in
    /// `directories` here would force every consumer to drag it in too.
    pub fn load() -> Result<Self> {
        Self::load_from(default_path()?)
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

/// Resolve the canonical profiles file path.
///
/// The location is platform-specific and must match what the
/// `directories` crate (the pre-v0.8.7 path resolver) returns, so
/// users who upgrade from ≤0.8.6 keep reading their existing
/// `profiles.toml`:
///
/// - **macOS:** `$HOME/Library/Application Support/agentum/profiles.toml`.
///   This is `ProjectDirs::from("", "", "agentum").config_dir()` on
///   Darwin. v0.8.7..=v0.8.9 incorrectly used the XDG path here, so
///   every Mac user who upgraded saw an empty SERVERS list because
///   their profiles still lived at the `Library/Application Support`
///   location while the new code looked at `~/.config/agentum`.
/// - **Linux / BSD:** `$XDG_CONFIG_HOME/agentum/profiles.toml`,
///   falling back to `$HOME/.config/agentum/profiles.toml` when XDG
///   is unset, empty, or a non-absolute path. The empty/relative
///   guard matches the `directories` crate behaviour and fixes the
///   v0.8.7 regression where some login-manager Linux setups
///   exported `XDG_CONFIG_HOME=""` and silently resolved profiles to
///   a CWD-relative `agentum/profiles.toml`.
///
/// Errors only when `HOME` is missing on a Unix-like host — that's a
/// misconfigured environment, not a missing file (an absent file is
/// fine; an absent path resolver isn't).
///
/// Kept private — callers should prefer `Profiles::load()` so the
/// resolved path doesn't leak into other code's path-handling.
fn default_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; cannot resolve profiles.toml"))?;

    // On macOS the `directories` crate (which the TUI used pre-0.8.7)
    // returns `$HOME/Library/Application Support/agentum` for the
    // config dir, NOT the XDG path. Match that so existing Mac users
    // keep reading the file they've been writing for months.
    #[cfg(target_os = "macos")]
    {
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("agentum")
            .join("profiles.toml"))
    }

    // Linux + BSD: XDG, with the same empty/non-absolute guard the
    // `directories` crate applies. An empty `XDG_CONFIG_HOME=""` or
    // a relative value both fall through to `$HOME/.config`.
    #[cfg(not(target_os = "macos"))]
    {
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty() && p.is_absolute());
        let base = xdg.unwrap_or_else(|| home.join(".config"));
        Ok(base.join("agentum").join("profiles.toml"))
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

    // Serialise tests that mutate XDG_CONFIG_HOME so they don't
    // collide when `cargo test` runs them concurrently. Same pattern
    // as `agentum-server::routes::profiles::tests`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn profiles_load_reads_xdg_config_home() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let cfg_home = tmp.path().to_path_buf();
        let profiles_dir = cfg_home.join("agentum");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        let path = profiles_dir.join("profiles.toml");
        std::fs::write(
            &path,
            "default = \"vps\"\n[profiles.vps]\nurl = \"https://my-vps:8822\"\n",
        )
        .unwrap();

        // SAFETY: serialised by ENV_LOCK; no other test in this crate
        // mutates XDG_CONFIG_HOME at the same time.
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &cfg_home);
        }
        let result = Profiles::load();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let p = result.unwrap();
        assert_eq!(p.default_name(), Some("vps"));
        assert_eq!(p.get("vps").unwrap().url, "https://my-vps:8822");
    }

    /// macOS resolves to `$HOME/Library/Application Support/agentum`,
    /// matching the `directories` crate that the TUI used pre-0.8.7.
    /// XDG_CONFIG_HOME is irrelevant on Darwin.
    ///
    /// Regression for v0.8.7..=v0.8.9: every Mac user who upgraded
    /// from ≤0.8.6 saw an empty SERVERS list because `default_path()`
    /// looked at `~/.config/agentum/profiles.toml` while their actual
    /// profiles lived at `~/Library/Application Support/agentum/profiles.toml`.
    #[test]
    #[cfg(target_os = "macos")]
    fn profiles_load_reads_application_support_on_macos() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().to_path_buf();
        let profiles_dir = fake_home
            .join("Library")
            .join("Application Support")
            .join("agentum");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(
            profiles_dir.join("profiles.toml"),
            "[profiles.vps]\nurl = \"https://my-vps:8822\"\n",
        )
        .unwrap();

        // SAFETY: serialised by ENV_LOCK; no other test in this crate
        // mutates HOME at the same time. We also clobber XDG_CONFIG_HOME
        // to prove macOS resolution ignores it.
        let prev_home = std::env::var_os("HOME");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("HOME", &fake_home);
            std::env::set_var("XDG_CONFIG_HOME", "/tmp/should-be-ignored-on-mac");
        }
        let result = Profiles::load();
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let p = result.unwrap();
        assert_eq!(p.get("vps").unwrap().url, "https://my-vps:8822");
    }

    /// Regression for v0.8.7..=v0.8.9: when `XDG_CONFIG_HOME` was set
    /// to an empty string (a few login-manager setups do this on
    /// Linux), `default_path()` produced the relative path
    /// `agentum/profiles.toml`, which `Profiles::load_from` resolved
    /// against the current working directory. Users with profiles in
    /// `~/.config/agentum/profiles.toml` saw an empty server list in
    /// `agentum terminal`. The fix falls back to `$HOME/.config` for
    /// empty / non-absolute XDG values, matching the `directories`
    /// crate that was used pre-0.8.7.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn profiles_load_falls_back_when_xdg_is_empty() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().to_path_buf();
        let profiles_dir = fake_home.join(".config").join("agentum");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        let path = profiles_dir.join("profiles.toml");
        std::fs::write(&path, "[profiles.vps]\nurl = \"https://my-vps:8822\"\n").unwrap();

        // SAFETY: serialised by ENV_LOCK; no other test in this crate
        // mutates HOME / XDG_CONFIG_HOME at the same time.
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "");
            std::env::set_var("HOME", &fake_home);
        }
        let result = Profiles::load();
        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let p = result.unwrap();
        assert_eq!(p.get("vps").unwrap().url, "https://my-vps:8822");
    }

    /// Same fallback path for non-absolute XDG values. `directories`
    /// silently ignores relative paths here, so we do too rather than
    /// reading a relative `./agentum/profiles.toml` that depends on
    /// where the binary was invoked.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn profiles_load_falls_back_when_xdg_is_relative() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().to_path_buf();
        let profiles_dir = fake_home.join(".config").join("agentum");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(
            profiles_dir.join("profiles.toml"),
            "[profiles.lan]\nurl = \"https://lan:8822\"\n",
        )
        .unwrap();

        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "relative/path");
            std::env::set_var("HOME", &fake_home);
        }
        let result = Profiles::load();
        unsafe {
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        let p = result.unwrap();
        assert_eq!(p.get("lan").unwrap().url, "https://lan:8822");
    }
}
