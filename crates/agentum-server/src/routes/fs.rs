//! Filesystem listing endpoint, for the dashboard's workdir picker.
//!
//! Auth-gated by the global middleware. The server runs as the user, so
//! it has the same filesystem reach the user already does — no extra
//! sandboxing needed for a single-tenant local tool.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::Router;
use axum::extract::Query;
use axum::routing::get;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::AppState;
use crate::error::ApiError;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/fs/list", get(list_dir))
}

#[derive(Deserialize)]
struct ListQuery {
    /// Absolute path to list. Empty / missing → user $HOME.
    /// `~` and `~/foo` are expanded.
    #[serde(default)]
    path: Option<String>,
    /// Include dotfiles. Defaults to false.
    #[serde(default)]
    show_hidden: bool,
}

#[derive(Serialize)]
struct Entry {
    name: String,
    /// Absolute resolved path of this entry.
    path: String,
}

#[derive(Serialize)]
struct ListResp {
    /// Resolved absolute path of the listed directory.
    path: String,
    /// Parent directory, or null at filesystem root.
    parent: Option<String>,
    /// Subdirectories, sorted case-insensitively.
    dirs: Vec<Entry>,
}

async fn list_dir(Query(q): Query<ListQuery>) -> Result<Json<ListResp>, ApiError> {
    let raw = q.path.unwrap_or_default();
    let resolved = resolve(&raw)?;

    let meta = fs::metadata(&resolved)
        .await
        .map_err(|e| ApiError::BadRequest(format!("path error: {e}")))?;
    if !meta.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "{} is not a directory",
            resolved.display()
        )));
    }

    let mut rd = fs::read_dir(&resolved)
        .await
        .map_err(|e| ApiError::BadRequest(format!("read_dir failed: {e}")))?;

    let mut dirs: Vec<Entry> = Vec::new();
    while let Some(ent) = rd
        .next_entry()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        let name = ent.file_name().to_string_lossy().to_string();
        if !q.show_hidden && name.starts_with('.') {
            continue;
        }
        // file_type follows symlinks=false; resolve once for the dir test.
        let ft = match ent.file_type().await {
            Ok(t) => t,
            Err(_) => continue,
        };
        let is_dir = if ft.is_symlink() {
            // Follow the symlink for the dir check, but skip dangling ones.
            fs::metadata(ent.path())
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)
        } else {
            ft.is_dir()
        };
        if !is_dir {
            continue;
        }
        dirs.push(Entry {
            name,
            path: ent.path().to_string_lossy().to_string(),
        });
    }

    dirs.sort_by_key(|d| d.name.to_lowercase());

    let parent = resolved.parent().map(|p| p.to_string_lossy().to_string());
    Ok(Json(ListResp {
        path: resolved.to_string_lossy().to_string(),
        parent,
        dirs,
    }))
}

/// Expand `~` / `~/x` and turn empty input into `$HOME`. Relative paths get
/// resolved against `$HOME` (rare in practice — the dashboard sends
/// absolute paths).
fn resolve(raw: &str) -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::Internal("HOME not set".into()))?;

    let trimmed = raw.trim();
    let path = if trimmed.is_empty() || trimmed == "~" {
        home
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.join(rest)
    } else {
        let p = Path::new(trimmed);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            home.join(p)
        }
    };

    // Don't canonicalize — the path may not exist yet (autocomplete is OK
    // hitting partials). The fs::metadata call above will reject anything
    // that isn't a real directory.
    Ok(path)
}
