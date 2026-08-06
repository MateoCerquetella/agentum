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
    Router::new()
        .route("/api/fs/list", get(list_dir))
        .route("/api/fs/entries", get(list_entries))
        .route("/api/fs/read", get(read_file))
}

#[derive(Deserialize)]
struct ReadQuery {
    /// Absolute path of the file to read (on `host_id`'s filesystem).
    path: String,
    /// Host the file lives on. Missing means the daemon's local machine.
    #[serde(default)]
    host_id: Option<Uuid>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadResp {
    /// UTF-8 text content; empty when `is_binary`.
    content: String,
    /// True for non-UTF-8 / NUL-containing files (the editor shows a binary
    /// placeholder instead of mojibake).
    is_binary: bool,
}

/// `GET /api/fs/read?path=…&host_id=` — read a file's text content. Host-aware:
/// a local host reads from disk; an SSH host reads over the connection (`cat`)
/// via [`host_runtime::read_file_bytes`]. This backs opening files in a remote
/// SSH workspace — the desktop's native `fs_read_file` is local-only, so without
/// this the renderer reads the remote path on the *local* machine (→ ENOENT,
/// "No such file or directory"). Mirrors how `/api/fs/entries` lists remote dirs.
async fn read_file(
    State(state): State<AppState>,
    Query(q): Query<ReadQuery>,
) -> Result<Json<ReadResp>, ApiError> {
    let host_id = q.host_id.unwrap_or(LOCAL_HOST_ID);
    let _host_guard = super::sessions::acquire_host_lifecycle(host_id).await;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;
    // Local paths get `~`/relative expansion like the listing routes; a remote
    // path is already absolute on its own host, so pass it through untouched.
    let abs = match host.kind {
        HostKind::Local => resolve(&q.path)?.to_string_lossy().into_owned(),
        HostKind::Ssh { .. } => q.path.clone(),
    };
    let bytes = crate::host_runtime::read_file_bytes(&host, &abs)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest(format!("file does not exist: {abs}")))?;
    // A NUL byte or invalid UTF-8 marks the file binary; the editor then shows
    // its binary placeholder rather than corrupted text.
    let resp = match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => ReadResp {
            content: text,
            is_binary: false,
        },
        _ => ReadResp {
            content: String::new(),
            is_binary: true,
        },
    };
    Ok(Json(resp))
}

#[derive(Serialize)]
struct FileEntry {
    name: String,
    /// Absolute resolved path of this entry.
    path: String,
    /// `dir` | `file` | `symlink` (symlink = dangling/non-dir-non-file target).
    kind: &'static str,
}

#[derive(Serialize)]
struct EntriesResp {
    /// Resolved absolute path of the listed directory.
    path: String,
    /// Parent directory, or null at filesystem root.
    parent: Option<String>,
    /// All entries (dirs first, then files), each sorted case-insensitively.
    entries: Vec<FileEntry>,
}

/// `GET /api/fs/entries?path=…&show_hidden=&host_id=` — list a directory's dirs
/// AND files (the workdir picker's `/list` is dirs-only). Host-aware: a local
/// host reads the filesystem directly; an SSH host lists over the connection via
/// [`list_remote_entries`] (mirrors `list_remote_dir`). This backs the desktop's
/// remote file explorer tree.
async fn list_entries(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<EntriesResp>, ApiError> {
    let host_id = q.host_id.unwrap_or(LOCAL_HOST_ID);
    let _host_guard = super::sessions::acquire_host_lifecycle(host_id).await;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;
    if matches!(host.kind, HostKind::Ssh { .. }) {
        return list_remote_entries(&host, q).await;
    }

    let resolved = resolve(&q.path.unwrap_or_default())?;
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
    let mut entries: Vec<FileEntry> = Vec::new();
    while let Some(ent) = rd
        .next_entry()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        let name = ent.file_name().to_string_lossy().to_string();
        if !q.show_hidden && name.starts_with('.') {
            continue;
        }
        let ft = match ent.file_type().await {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Follow symlinks for the dir/file classification; dangling → 'symlink'.
        let kind = if ft.is_symlink() {
            match fs::metadata(ent.path()).await {
                Ok(m) if m.is_dir() => "dir",
                Ok(_) => "file",
                Err(_) => "symlink",
            }
        } else if ft.is_dir() {
            "dir"
        } else {
            "file"
        };
        entries.push(FileEntry {
            name,
            path: ent.path().to_string_lossy().to_string(),
            kind,
        });
    }

    // Dirs first, then files, each case-insensitive — the conventional explorer order.
    entries.sort_by(|a, b| {
        let rank = |k: &str| if k == "dir" { 0 } else { 1 };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let parent = resolved.parent().map(|p| p.to_string_lossy().to_string());
    Ok(Json(EntriesResp {
        path: resolved.to_string_lossy().to_string(),
        parent,
        entries,
    }))
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
    let _host_guard = super::sessions::acquire_host_lifecycle(host_id).await;
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
    // Strip trailing slashes (mirrors the local `resolve`) so `~/` collapses to
    // `~` and `/foo/bar/` to `/foo/bar`. Without this, `~/` skipped the `= "~"`
    // tilde-expansion branch in the remote script and resolved to the literal
    // `$HOME/~/`, which isn't a directory → 400. Keep a lone `/` intact.
    let trimmed = raw.trim();
    let trimmed = if trimmed.len() > 1 {
        trimmed.trim_end_matches('/')
    } else {
        trimmed
    };
    let quoted = shlex::try_quote(trimmed).map_err(|_| ApiError::BadRequest("bad path".into()))?;
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

/// SSH variant of [`list_entries`]: list a directory's dirs AND files on a
/// remote host. Mirrors [`list_remote_dir`] (same tilde/trailing-slash handling
/// and `sh -c` wrapping for fish/zsh login shells) but emits both `DIR` and
/// `FILE` records and classifies dangling symlinks as `SYMLINK`. One depth level
/// only — the desktop explorer loads the tree lazily per directory.
async fn list_remote_entries(
    host: &agentum_core::Host,
    q: ListQuery,
) -> Result<Json<EntriesResp>, ApiError> {
    let script = remote_entries_script(&q.path.unwrap_or_default(), q.show_hidden)?;
    let out = crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map_err(|e| ApiError::BadRequest(format!("remote fs: {e}")))?;
    Ok(Json(parse_remote_entries(&out)))
}

/// Build the `sh -c '…'` script that lists `path`'s entries (dirs + files) on a
/// remote host. Extracted from [`list_remote_entries`] so the script + parser
/// are unit-testable without an SSH round trip.
fn remote_entries_script(raw: &str, show_hidden: bool) -> Result<String, ApiError> {
    // Same trailing-slash trim as `list_remote_dir`/`resolve` so `~/` collapses
    // to `~` (otherwise the tilde branch is skipped and `$HOME/~/` 400s).
    let trimmed = raw.trim();
    let trimmed = if trimmed.len() > 1 {
        trimmed.trim_end_matches('/')
    } else {
        trimmed
    };
    let quoted = shlex::try_quote(trimmed).map_err(|_| ApiError::BadRequest("bad path".into()))?;
    let hidden_filter = if show_hidden {
        ""
    } else {
        " | awk -F '\\t' '$2 !~ /^\\./'"
    };
    // `find -printf '%y'` reports the entry type: d=dir, f=file, l=symlink. For a
    // symlink we re-test the target with `[ -d ]`/`[ -e ]` so a link to a dir is
    // classified `DIR` (matching the local walker's follow-symlinks behaviour),
    // a link to a file `FILE`, and a dangling link `SYMLINK`.
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
find "$base" -mindepth 1 -maxdepth 1 -printf '%y\t%f\t%p\n'{hidden_filter} | sort -t '	' -k2 -f | while IFS='	' read -r ty name path; do
  case "$ty" in
    d) printf 'DIR\t%s\t%s\n' "$name" "$path" ;;
    f) printf 'FILE\t%s\t%s\n' "$name" "$path" ;;
    l) if [ -d "$path" ]; then printf 'DIR\t%s\t%s\n' "$name" "$path";
       elif [ -e "$path" ]; then printf 'FILE\t%s\t%s\n' "$name" "$path";
       else printf 'SYMLINK\t%s\t%s\n' "$name" "$path"; fi ;;
  esac
done
"#
    );
    // Force a POSIX shell on the remote (see `list_remote_dir` for the fish/zsh
    // rationale — the `case`/`$(...)`/`${{#}}` syntax is bash/POSIX-only).
    Ok(format!(
        "sh -c {}",
        shlex::try_quote(inner.as_str()).map_err(|_| ApiError::BadRequest("bad path".into()))?
    ))
}

/// Parse the tab-separated `PATH`/`PARENT`/`DIR`/`FILE`/`SYMLINK` records the
/// remote script emits into an [`EntriesResp`], sorted dirs-first then
/// case-insensitively — matching the local branch's order.
fn parse_remote_entries(out: &str) -> EntriesResp {
    let mut path = String::new();
    let mut parent = None;
    let mut entries: Vec<FileEntry> = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        match parts.next() {
            Some("PATH") => path = parts.next().unwrap_or_default().to_string(),
            Some("PARENT") => parent = Some(parts.next().unwrap_or_default().to_string()),
            Some(tag @ ("DIR" | "FILE" | "SYMLINK")) => {
                let name = parts.next().unwrap_or_default().to_string();
                let entry_path = parts.next().unwrap_or_default().to_string();
                let kind = match tag {
                    "DIR" => "dir",
                    "FILE" => "file",
                    _ => "symlink",
                };
                entries.push(FileEntry {
                    name,
                    path: entry_path,
                    kind,
                });
            }
            _ => {}
        }
    }

    entries.sort_by(|a, b| {
        let rank = |k: &str| if k == "dir" { 0 } else { 1 };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    EntriesResp {
        path,
        parent,
        entries,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_entries_script_lists_dirs_and_files_under_sh() {
        let script = remote_entries_script("~/proj", false).expect("script");
        // Wrapped in `sh -c` so a fish/zsh login shell doesn't choke on the
        // bash/POSIX `case`/`$(...)`/`${#}` syntax.
        assert!(script.starts_with("sh -c "), "script: {script}");
        // Single-depth listing (lazy per-directory in the explorer).
        assert!(script.contains("maxdepth 1"), "script: {script}");
        // `find -printf '%y'` drives the per-entry kind classification.
        assert!(script.contains("%y"), "script: {script}");
        // Unlike the dirs-only `/list` find, entries emits BOTH dirs and files
        // and classifies dangling symlinks. The keyword markers survive shlex
        // quoting (the surrounding quotes shift, the words don't).
        assert!(script.contains("DIR"), "script: {script}");
        assert!(script.contains("FILE"), "script: {script}");
        assert!(script.contains("SYMLINK"), "script: {script}");
        // The base path is quoted into the script.
        assert!(script.contains("~/proj"), "script: {script}");
    }

    #[test]
    fn remote_entries_script_filters_hidden_by_default() {
        let hidden = remote_entries_script("/srv/app", false).expect("script");
        assert!(hidden.contains("awk"), "expected dotfile filter: {hidden}");
        let shown = remote_entries_script("/srv/app", true).expect("script");
        assert!(!shown.contains("awk"), "show_hidden should drop the filter");
    }

    #[test]
    fn parse_remote_entries_sorts_dirs_first_then_case_insensitive() {
        // Intentionally unsorted, mixed-kind input; PATH/PARENT lead the stream.
        let out = "PATH\t/srv/app\n\
                   PARENT\t/srv\n\
                   FILE\tREADME.md\t/srv/app/README.md\n\
                   DIR\tsrc\t/srv/app/src\n\
                   FILE\tApp.tsx\t/srv/app/App.tsx\n\
                   SYMLINK\tdangling\t/srv/app/dangling\n\
                   DIR\tassets\t/srv/app/assets\n";
        let resp = parse_remote_entries(out);
        assert_eq!(resp.path, "/srv/app");
        assert_eq!(resp.parent.as_deref(), Some("/srv"));
        let names: Vec<_> = resp
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind))
            .collect();
        // Dirs first (assets, src), then files/symlinks case-insensitively
        // (App.tsx, dangling, README.md).
        assert_eq!(
            names,
            vec![
                ("assets", "dir"),
                ("src", "dir"),
                ("App.tsx", "file"),
                ("dangling", "symlink"),
                ("README.md", "file"),
            ]
        );
    }
}
