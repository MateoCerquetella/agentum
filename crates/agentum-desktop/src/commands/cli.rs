use serde::Serialize;
use std::path::{Path, PathBuf};

// Mirrors CliInstallStatus in agentum/src/shared/cli-install-types.ts. camelCase so
// the proxied invoke() result deserializes into the exact object the renderer reads.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallStatus {
    pub platform: String,
    pub command_name: String,
    pub command_path: Option<String>,
    pub path_directory: Option<String>,
    pub path_configured: bool,
    pub launcher_path: Option<String>,
    pub install_method: Option<String>,
    pub supported: bool,
    pub state: String,
    pub current_target: Option<String>,
    pub unsupported_reason: Option<String>,
    pub detail: Option<String>,
}

/// The terminal command we register.
const COMMAND_NAME: &str = "agentum";

fn platform_label() -> String {
    if cfg!(target_os = "macos") {
        "darwin".to_string()
    } else if cfg!(target_os = "windows") {
        "win32".to_string()
    } else {
        "linux".to_string()
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Directory we register the launcher into, per platform. macOS uses the
/// conventional `/usr/local/bin` (normally on PATH); Linux uses the user-writable
/// `~/.local/bin` so no sudo is needed. Windows registration is handled by the
/// separate WSL path, so it returns `None` here.
fn install_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from("/usr/local/bin"))
    } else if cfg!(target_os = "linux") {
        home_dir().map(|h| h.join(".local").join("bin"))
    } else {
        None
    }
}

/// True when `dir` is listed in `$PATH`.
fn dir_on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|entry| entry == dir))
        .unwrap_or(false)
}

/// True when `p` resolves (through symlinks) to a regular file we can run.
fn is_binary_file(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// Find a real `agentum` CLI binary to point the launcher at. Looks, in order:
/// alongside the running desktop executable (a bundled / co-located CLI), then
/// each `$PATH` entry (cargo / homebrew / manual installs), then well-known
/// install dirs. Never returns `target` itself, so we don't symlink a file to
/// itself.
fn locate_source_binary(target: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(COMMAND_NAME));
            // macOS bundle: the GUI binary lives in `Contents/MacOS`; a bundled CLI
            // resource would sit in the sibling `Contents/Resources`.
            if let Some(contents) = dir.parent() {
                candidates.push(contents.join("Resources").join(COMMAND_NAME));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&path) {
            candidates.push(entry.join(COMMAND_NAME));
        }
    }
    if let Some(home) = home_dir() {
        candidates.push(home.join(".cargo").join("bin").join(COMMAND_NAME));
        candidates.push(home.join(".local").join("bin").join(COMMAND_NAME));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin").join(COMMAND_NAME));
    candidates.push(PathBuf::from("/usr/local/bin").join(COMMAND_NAME));

    candidates
        .into_iter()
        .find(|cand| cand != target && is_binary_file(cand))
        .map(|cand| std::fs::canonicalize(&cand).unwrap_or(cand))
}

/// Honest "can't do this here" status for unsupported platforms (Windows native,
/// or an unknown OS). `reason` distinguishes platform-not-supported from a
/// missing launcher binary.
fn unsupported_status(reason: &str, detail: &str) -> CliInstallStatus {
    CliInstallStatus {
        platform: platform_label(),
        command_name: COMMAND_NAME.to_string(),
        command_path: None,
        path_directory: None,
        path_configured: false,
        launcher_path: None,
        install_method: None,
        supported: false,
        state: "unsupported".to_string(),
        current_target: None,
        unsupported_reason: Some(reason.to_string()),
        detail: Some(detail.to_string()),
    }
}

/// Read the current registration state from disk: installed (our symlink or a
/// real binary already at the target), not-installed-but-installable (a source
/// binary exists elsewhere), or unsupported (no binary anywhere to point at).
fn build_status() -> CliInstallStatus {
    let Some(dir) = install_dir() else {
        return unsupported_status(
            "platform_not_supported",
            "CLI registration isn't supported on this platform yet.",
        );
    };
    let target = dir.join(COMMAND_NAME);
    let path_configured = dir_on_path(&dir);
    let dir_str = dir.to_string_lossy().into_owned();

    if is_binary_file(&target) {
        // Resolve a symlink to the real binary; a plain binary points at itself.
        let resolved = std::fs::read_link(&target)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.to_string_lossy().into_owned());
        let detail = if path_configured {
            None
        } else {
            Some(format!(
                "`{COMMAND_NAME}` is registered, but {dir_str} is not on your PATH yet."
            ))
        };
        return CliInstallStatus {
            platform: platform_label(),
            command_name: COMMAND_NAME.to_string(),
            command_path: Some(target.to_string_lossy().into_owned()),
            path_directory: Some(dir_str),
            path_configured,
            launcher_path: Some(resolved.clone()),
            install_method: Some("symlink".to_string()),
            supported: true,
            state: "installed".to_string(),
            current_target: Some(resolved),
            unsupported_reason: None,
            detail,
        };
    }

    match locate_source_binary(&target) {
        Some(_) => CliInstallStatus {
            platform: platform_label(),
            command_name: COMMAND_NAME.to_string(),
            command_path: None,
            path_directory: Some(dir_str),
            path_configured,
            launcher_path: None,
            install_method: Some("symlink".to_string()),
            supported: true,
            state: "not_installed".to_string(),
            current_target: None,
            unsupported_reason: None,
            detail: None,
        },
        None => unsupported_status(
            "launcher_missing",
            "No `agentum` CLI binary was found. Install it with \
             `cargo install --git https://github.com/mateocerquetella/agentum agentum-tui` \
             (or download a release), then register here.",
        ),
    }
}

/// `conflict` state for a write failure (e.g. `/usr/local/bin` needs sudo).
fn write_error_status(dir: &Path, message: String) -> CliInstallStatus {
    CliInstallStatus {
        platform: platform_label(),
        command_name: COMMAND_NAME.to_string(),
        command_path: None,
        path_directory: Some(dir.to_string_lossy().into_owned()),
        path_configured: dir_on_path(dir),
        launcher_path: None,
        install_method: Some("symlink".to_string()),
        supported: true,
        state: "conflict".to_string(),
        current_target: None,
        unsupported_reason: None,
        detail: Some(message),
    }
}

#[tauri::command]
pub fn cli_get_install_status() -> CliInstallStatus {
    build_status()
}

#[tauri::command]
pub fn cli_install() -> CliInstallStatus {
    let Some(dir) = install_dir() else {
        return build_status();
    };
    let target = dir.join(COMMAND_NAME);
    // Idempotent: a valid command already at the target is a no-op success.
    if is_binary_file(&target) {
        return build_status();
    }
    let Some(src) = locate_source_binary(&target) else {
        return build_status(); // no binary to point at → honest "launcher_missing"
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return write_error_status(&dir, format!("Couldn't create {}: {e}", dir.display()));
    }
    // Clear a stale/broken entry before linking (best-effort; ignore "not found").
    let _ = std::fs::remove_file(&target);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&src, &target);
    #[cfg(not(unix))]
    let linked: std::io::Result<()> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink registration is only supported on unix",
    ));
    if let Err(e) = linked {
        return write_error_status(
            &dir,
            format!(
                "Couldn't register {} → {}: {e}. {} may require elevated permissions.",
                target.display(),
                src.display(),
                dir.display()
            ),
        );
    }
    build_status()
}

#[tauri::command]
pub fn cli_remove() -> CliInstallStatus {
    if let Some(dir) = install_dir() {
        let target = dir.join(COMMAND_NAME);
        // Only unlink an entry WE manage (a symlink). Never delete a real binary
        // the user installed themselves (e.g. via cargo/homebrew at the target).
        if let Ok(meta) = std::fs::symlink_metadata(&target) {
            if meta.file_type().is_symlink() {
                let _ = std::fs::remove_file(&target);
            }
        }
    }
    build_status()
}

#[tauri::command]
pub fn cli_get_wsl_install_status() -> CliInstallStatus {
    // WSL install only makes sense on Windows; elsewhere it is platform-not-supported.
    if cfg!(target_os = "windows") {
        unsupported_status(
            "launcher_missing",
            "CLI launcher is not available in this build yet.",
        )
    } else {
        unsupported_status(
            "platform_not_supported",
            "WSL registration is only available on Windows.",
        )
    }
}

#[tauri::command]
pub fn cli_install_wsl() -> CliInstallStatus {
    cli_get_wsl_install_status()
}

#[tauri::command]
pub fn cli_remove_wsl() -> CliInstallStatus {
    cli_get_wsl_install_status()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reports_agentum_command_and_valid_state() {
        let status = build_status();
        assert_eq!(status.command_name, "agentum");
        assert!(matches!(
            status.state.as_str(),
            "installed" | "not_installed" | "unsupported" | "conflict" | "stale"
        ));
    }

    #[test]
    fn locate_source_skips_the_target_itself() {
        // The target path must never be returned as its own source.
        let fake_target = PathBuf::from("/nonexistent-dir-xyz/agentum");
        // Even if nothing is found, the function must not return `fake_target`.
        assert_ne!(locate_source_binary(&fake_target), Some(fake_target));
    }
}
