use serde::Serialize;

// Mirrors CliInstallStatus in orca/src/shared/cli-install-types.ts. camelCase so
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

fn platform_label() -> String {
    if cfg!(target_os = "macos") {
        "darwin".to_string()
    } else if cfg!(target_os = "windows") {
        "win32".to_string()
    } else {
        "linux".to_string()
    }
}

// Why: the Tauri build does not yet bundle an `orca` CLI launcher binary, so
// install is impossible and the honest, contract-defined state is
// unsupported/launcher_missing for every cli method. Detection of a stray PATH
// entry is intentionally skipped — it would point at the Electron launcher, not
// this build.
fn launcher_missing_status(reason: &str) -> CliInstallStatus {
    CliInstallStatus {
        platform: platform_label(),
        command_name: "orca".to_string(),
        command_path: None,
        path_directory: None,
        path_configured: false,
        launcher_path: None,
        install_method: None,
        supported: false,
        state: "unsupported".to_string(),
        current_target: None,
        unsupported_reason: Some(reason.to_string()),
        detail: Some("CLI launcher is not available in this build yet.".to_string()),
    }
}

#[tauri::command]
pub fn cli_get_install_status() -> CliInstallStatus {
    launcher_missing_status("launcher_missing")
}

#[tauri::command]
pub fn cli_install() -> CliInstallStatus {
    launcher_missing_status("launcher_missing")
}

#[tauri::command]
pub fn cli_remove() -> CliInstallStatus {
    launcher_missing_status("launcher_missing")
}

#[tauri::command]
pub fn cli_get_wsl_install_status() -> CliInstallStatus {
    // WSL install only makes sense on Windows; elsewhere it is platform-not-supported.
    let reason = if cfg!(target_os = "windows") {
        "launcher_missing"
    } else {
        "platform_not_supported"
    };
    launcher_missing_status(reason)
}

#[tauri::command]
pub fn cli_install_wsl() -> CliInstallStatus {
    let reason = if cfg!(target_os = "windows") {
        "launcher_missing"
    } else {
        "platform_not_supported"
    };
    launcher_missing_status(reason)
}

#[tauri::command]
pub fn cli_remove_wsl() -> CliInstallStatus {
    let reason = if cfg!(target_os = "windows") {
        "launcher_missing"
    } else {
        "platform_not_supported"
    };
    launcher_missing_status(reason)
}
