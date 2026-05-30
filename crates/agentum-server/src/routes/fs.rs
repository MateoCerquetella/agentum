//! Filesystem listing endpoint, for the dashboard's workdir picker.
//!
//! Auth-gated by the global middleware. The server runs as the user, so
//! it has the same filesystem reach the user already does — no extra
//! sandboxing needed for a single-tenant local tool.

use std::path::{Path, PathBuf};

use agentum_core::{HostKind, LOCAL_HOST_ID};
use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

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
    /// Host to list on. Missing means the daemon's local machine.
    #[serde(default)]
    host_id: Option<Uuid>,
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

async fn list_dir(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResp>, ApiError> {
    let host_id = q.host_id.unwrap_or(LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;
    if matches!(host.kind, HostKind::Ssh { .. }) {
        return list_remote_dir(&host, q).await;
    }

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

async fn list_remote_dir(
    host: &agentum_core::Host,
    q: ListQuery,
) -> Result<Json<ListResp>, ApiError> {
    let raw = q.path.unwrap_or_default();
    let quoted =
        shlex::try_quote(raw.trim()).map_err(|_| ApiError::BadRequest("bad path".into()))?;
    let hidden_filter = if q.show_hidden {
        ""
    } else {
        " | awk -F '\\t' '$2 !~ /^\\./'"
    };
    let inner = format!(
        r#"base={quoted}
if [ -z "$base" ] || [ "$base" = "~" ]; then
  base="$HOME"
else
  case "$base" in "~/"*) base="$HOME/${{base#~/}}" ;; esac
fi
if [ ! -d "$base" ]; then echo "not a directory: $base" >&2; exit 2; fi
printf 'PATH\t%s\n' "$base"
parent=$(dirname -- "$base")
if [ "$parent" != "$base" ]; then printf 'PARENT\t%s\n' "$parent"; fi
find "$base" -mindepth 1 -maxdepth 1 -type d -printf 'DIR\t%f\t%p\n'{hidden_filter} | sort -f
"#
    );
    // Force a POSIX shell on the remote. The login shell may be fish or
    // zsh (this is common — the operator runs fish), and this script's
    // `case` / `$(...)` / `${{#}}` syntax is bash/POSIX-only, so a fish
    // login shell rejects it and the listing fails. Every other remote
    // command (probe, bootstrap, install, tmux) wraps in `sh -c` for
    // exactly this reason — this path was the one that forgot, which
    // surfaced as "couldn't list host home: 400" in the New Session
    // workdir field whenever the target host logged into fish.
    let script = format!(
        "sh -c {}",
        shlex::try_quote(inner.as_str()).map_err(|_| ApiError::BadRequest("bad path".into()))?
    );
    let out = crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map_err(|e| ApiError::BadRequest(format!("remote fs: {e}")))?;
    let mut path = String::new();
    let mut parent = None;
    let mut dirs = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        match parts.next() {
            Some("PATH") => path = parts.next().unwrap_or_default().to_string(),
            Some("PARENT") => parent = Some(parts.next().unwrap_or_default().to_string()),
            Some("DIR") => {
                let name = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                dirs.push(Entry { name, path });
            }
            _ => {}
        }
    }
    Ok(Json(ListResp { path, parent, dirs }))
}

/// Expand `~` / `~/x` and turn empty input into `$HOME`. Relative paths get
/// resolved against `$HOME` (rare in practice — the dashboard sends
/// absolute paths).
fn resolve(raw: &str) -> Result<PathBuf, ApiError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::Internal("HOME not set".into()))?;

    let trimmed = raw.trim();
    // Strip trailing slashes so `/foo/bar/` and `/foo/bar` resolve to
    // the same canonical PathBuf — otherwise the metadata check below
    // can be jittery on certain filesystems / fuse mounts and the user
    // sees "not a directory" for a path that's clearly there. Keep a
    // bare `/` intact (root has no parent to trim).
    let trimmed = if trimmed.len() > 1 {
        trimmed.trim_end_matches('/')
    } else {
        trimmed
    };
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
