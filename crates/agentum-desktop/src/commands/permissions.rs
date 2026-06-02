use serde_json::Value;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub fn agent_trust_mark_trusted() {
    // Workspace trust is enforced by the agent CLIs themselves; marking it from here
    // isn't ported. Accept and no-op.
}

#[tauri::command]
pub fn developer_permissions_get_status() -> Vec<Value> {
    // OS developer-permission probing (accessibility, screen recording, …) isn't
    // ported; report none so the UI shows an empty list rather than erroring.
    Vec::new()
}

#[tauri::command]
pub async fn developer_permissions_open_settings(id: String) -> Result<(), String> {
    let _ = id; // The specific pane isn't routed yet; open the privacy/security root.
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        (
            "open",
            vec!["x-apple.systempreferences:com.apple.preference.security"],
        )
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", "ms-settings:privacy"])
    } else {
        ("xdg-open", vec!["gnome-control-center"])
    };
    tokio::process::Command::new(program)
        .args(&args)
        .status()
        .await
        .map_err(map_err)?;
    Ok(())
}

fn platform_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

// The native permission helper (accessibility/screen-recording grants) isn't ported,
// so status is empty, setup does nothing, and reset reports an empty status.
#[tauri::command]
pub fn computer_use_permissions_get_status() -> Value {
    serde_json::json!({
        "platform": platform_label(),
        "helperAppPath": null,
        "helperUnavailableReason": "The computer-use permission helper isn't available in this build.",
        "permissions": []
    })
}

#[tauri::command]
pub fn computer_use_permissions_open_setup() -> Value {
    serde_json::json!({
        "platform": platform_label(),
        "helperAppPath": null,
        "openedSettings": false,
        "launchedHelper": false
    })
}

#[tauri::command]
pub fn computer_use_permissions_reset() -> Value {
    serde_json::json!({
        "platform": platform_label(),
        "helperAppPath": null,
        "helperUnavailableReason": "The computer-use permission helper isn't available in this build.",
        "permissions": [],
        "bundleId": null
    })
}

#[tauri::command]
pub fn developer_permissions_request(id: String) -> Value {
    // No OS permission probing; echo the id with an unsupported status.
    serde_json::json!({ "id": id, "status": "unsupported", "openedSystemSettings": false })
}
