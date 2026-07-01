//! XDG-compliant filesystem paths for agentum.
//!
//! Linux:    config=$XDG_CONFIG_HOME/agentum, data=$XDG_DATA_HOME/agentum,
//!           cache=$XDG_CACHE_HOME/agentum, state=$XDG_STATE_HOME/agentum
//! macOS:    everything under ~/Library/Application Support/agentum and ~/Library/Caches/agentum
//!
//! Override: when `AGENTUM_HOME` is set, all four roots live under
//! `$AGENTUM_HOME/{config,data,cache,state}` on every platform. This is the
//! single cross-platform isolation seam — `directories` ignores `XDG_*` on
//! macOS, so tests (and power users who want a self-contained install) need a
//! lever that works there too. Unset → default platform behavior, unchanged.

use std::path::PathBuf;

use directories::ProjectDirs;

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("could not resolve user directories")]
    NoHome,
}

/// `$AGENTUM_HOME`, when set and non-empty. Read per call so tests can flip it.
fn override_home() -> Option<PathBuf> {
    std::env::var_os("AGENTUM_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn dirs() -> Result<ProjectDirs, PathError> {
    ProjectDirs::from("", "", "agentum").ok_or(PathError::NoHome)
}

pub fn data_dir() -> Result<PathBuf, PathError> {
    if let Some(home) = override_home() {
        return Ok(home.join("data"));
    }
    Ok(dirs()?.data_dir().to_path_buf())
}

pub fn config_dir() -> Result<PathBuf, PathError> {
    if let Some(home) = override_home() {
        return Ok(home.join("config"));
    }
    Ok(dirs()?.config_dir().to_path_buf())
}

pub fn cache_dir() -> Result<PathBuf, PathError> {
    if let Some(home) = override_home() {
        return Ok(home.join("cache"));
    }
    Ok(dirs()?.cache_dir().to_path_buf())
}

/// State falls back to data on platforms where it is not defined (macOS).
pub fn state_dir() -> Result<PathBuf, PathError> {
    if let Some(home) = override_home() {
        return Ok(home.join("state"));
    }
    let d = dirs()?;
    Ok(d.state_dir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| d.data_dir().to_path_buf()))
}

pub fn db_path() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("db.sqlite"))
}

pub fn auth_token_path() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("auth_token"))
}

pub fn tls_dir() -> Result<PathBuf, PathError> {
    Ok(data_dir()?.join("tls"))
}

/// The directory holding per-session pane logs. The single owner of the
/// `cache/sessions` layout: [`pane_log`] (the writer path) and the boot-time
/// orphan reaper both derive from here, so the location can't silently drift
/// between them.
pub fn pane_log_dir() -> Result<PathBuf, PathError> {
    Ok(cache_dir()?.join("sessions"))
}

pub fn pane_log(session_id: &str) -> Result<PathBuf, PathError> {
    Ok(pane_log_dir()?.join(format!("{session_id}.log")))
}

/// Path to the per-server planner config: `$XDG_CONFIG_HOME/agentum/planner.toml`.
/// Sibling to `profiles.toml` and `credentials.toml` — all operator config
/// lives under `config_dir()`, not `data_dir()`.
pub fn planner_config_path() -> Result<PathBuf, PathError> {
    Ok(config_dir()?.join("planner.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise all tests that mutate XDG_CONFIG_HOME so they don't
    // race on the process-wide env variable.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn planner_config_path_under_config_dir() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: serialised by TEST_LOCK — only one thread mutates env at a time.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
        }
        let p = planner_config_path().expect("planner_config_path should succeed");
        let cfg = config_dir().expect("config_dir should succeed");
        assert!(
            p.ends_with("planner.toml"),
            "expected path to end with planner.toml, got {p:?}"
        );
        // The path returned must be a direct child of config_dir().
        // We compare without canonicalize because the directory may not
        // exist on disk yet — only the computed path matters here.
        assert_eq!(
            p.parent().unwrap(),
            cfg.as_path(),
            "planner_config_path must be a direct child of config_dir()"
        );
    }
}
