use serde::Serialize;
use tauri_plugin_notification::{NotificationExt, PermissionState};

// Result shapes mirror the renderer contract in orca/src/shared/types.ts so the
// proxied invoke() calls deserialize into the exact objects callers expect.
#[derive(Debug, Serialize)]
pub struct NotificationDispatchResult {
    pub delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NotificationSoundResult {
    pub played: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NotificationPermissionStatusResult {
    pub supported: bool,
    pub platform: String,
    pub requested: bool,
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

// A non-Prompt state means the user already made a grant/deny decision.
fn was_requested(state: &PermissionState) -> bool {
    !matches!(state, PermissionState::Prompt)
}

#[tauri::command]
pub fn notifications_dispatch(
    app: tauri::AppHandle,
    source: String,
    repo_label: Option<String>,
    worktree_label: Option<String>,
    terminal_title: Option<String>,
    agent_state: Option<String>,
    agent_prompt: Option<String>,
) -> NotificationDispatchResult {
    let title = repo_label
        .or(worktree_label)
        .unwrap_or_else(|| "Orca".to_string());

    let mut body_parts: Vec<String> = Vec::new();
    if let Some(value) = terminal_title {
        body_parts.push(value);
    }
    if let Some(value) = agent_state {
        body_parts.push(value);
    }
    if let Some(value) = agent_prompt {
        body_parts.push(value);
    }
    let body = if body_parts.is_empty() {
        source
    } else {
        body_parts.join(" — ")
    };

    match app.notification().builder().title(title).body(body).show() {
        Ok(()) => NotificationDispatchResult {
            delivered: true,
            reason: None,
        },
        Err(_) => NotificationDispatchResult {
            delivered: false,
            reason: Some("not-displayed".to_string()),
        },
    }
}

#[tauri::command]
pub fn notifications_get_permission_status(
    app: tauri::AppHandle,
) -> NotificationPermissionStatusResult {
    let requested = app
        .notification()
        .permission_state()
        .map(|state| was_requested(&state))
        .unwrap_or(false);

    NotificationPermissionStatusResult {
        supported: true,
        platform: platform_label(),
        requested,
    }
}

#[tauri::command]
pub fn notifications_request_permission(
    app: tauri::AppHandle,
) -> NotificationPermissionStatusResult {
    let requested = app
        .notification()
        .request_permission()
        .map(|state| was_requested(&state))
        .unwrap_or(false);

    NotificationPermissionStatusResult {
        supported: true,
        platform: platform_label(),
        requested,
    }
}

#[tauri::command]
pub async fn notifications_open_system_settings() -> Result<(), String> {
    // Per-OS deep link to the notification settings pane.
    let (program, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
        (
            "open",
            vec!["x-apple.systempreferences:com.apple.preference.notifications".to_string()],
        )
    } else if cfg!(target_os = "windows") {
        (
            "cmd",
            vec![
                "/C".into(),
                "start".into(),
                "".into(),
                "ms-settings:notifications".into(),
            ],
        )
    } else {
        ("xdg-open", vec!["gnome-control-center".to_string()])
    };

    let status = tokio::process::Command::new(program)
        .args(&args)
        .status()
        .await
        .map_err(|error| error.to_string())?;

    if status.success() {
        Ok(())
    } else {
        Err("failed to open notification settings".to_string())
    }
}

#[tauri::command]
pub fn notifications_play_sound() -> NotificationSoundResult {
    // Why: custom notification-sound path config isn't ported yet, so there is
    // no file to play. Mirrors the web adapter's missing-path fallback.
    NotificationSoundResult {
        played: false,
        reason: Some("missing-path".to_string()),
    }
}
