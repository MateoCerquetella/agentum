// Detection for alternate shell runtimes (PowerShell, WSL).

#[tauri::command]
pub fn pwsh_is_available() -> bool {
    which::which("pwsh").is_ok()
}

#[tauri::command]
pub fn wsl_is_available() -> bool {
    // WSL only exists on Windows.
    cfg!(target_os = "windows") && which::which("wsl").is_ok()
}

#[tauri::command]
pub fn wsl_list_distros() -> Vec<String> {
    // Distro enumeration (UTF-16 `wsl -l` parsing) isn't ported; none off Windows.
    Vec::new()
}

#[tauri::command]
pub fn git_bash_is_available() -> bool {
    // Git Bash is a Windows-only concept (Git for Windows ships bash.exe).
    cfg!(target_os = "windows") && which::which("bash").is_ok()
}
