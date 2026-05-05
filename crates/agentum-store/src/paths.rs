//! XDG-compliant filesystem paths for agentum.
//!
//! Linux:    config=$XDG_CONFIG_HOME/agentum, data=$XDG_DATA_HOME/agentum,
//!           cache=$XDG_CACHE_HOME/agentum, state=$XDG_STATE_HOME/agentum
//! macOS:    everything under ~/Library/Application Support/agentum and ~/Library/Caches/agentum

use std::path::PathBuf;

use directories::ProjectDirs;

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("could not resolve user directories")]
    NoHome,
}

fn dirs() -> Result<ProjectDirs, PathError> {
    ProjectDirs::from("", "", "agentum").ok_or(PathError::NoHome)
}

pub fn data_dir() -> Result<PathBuf, PathError> {
    Ok(dirs()?.data_dir().to_path_buf())
}

pub fn config_dir() -> Result<PathBuf, PathError> {
    Ok(dirs()?.config_dir().to_path_buf())
}

pub fn cache_dir() -> Result<PathBuf, PathError> {
    Ok(dirs()?.cache_dir().to_path_buf())
}

/// State falls back to data on platforms where it is not defined (macOS).
pub fn state_dir() -> Result<PathBuf, PathError> {
    let d = dirs()?;
    Ok(d.state_dir().map(|p| p.to_path_buf()).unwrap_or_else(|| d.data_dir().to_path_buf()))
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

pub fn pane_log(session_id: &str) -> Result<PathBuf, PathError> {
    Ok(cache_dir()?.join("sessions").join(format!("{session_id}.log")))
}
