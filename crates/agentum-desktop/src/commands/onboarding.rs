use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Checklist {
    added_repo: bool,
    chose_agent: bool,
    ran_first_agent: bool,
    ran_second_agent_on_same_task: bool,
    tried_cmd_j: bool,
    shaped_sidebar: bool,
    reviewed_diff: bool,
    opened_pr: bool,
    added_folder: bool,
    opened_file: bool,
    ran_agent_on_file: bool,
    dismissed: bool,
}

// Mirrors OnboardingState in agentum/src/shared/types.ts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OnboardingState {
    closed_at: Option<i64>,
    outcome: Option<String>,
    last_completed_step: i64,
    checklist: Checklist,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            closed_at: None,
            outcome: None,
            last_completed_step: -1, // sentinel: not started
            checklist: Checklist::default(),
        }
    }
}

fn state_path() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(".agentum")
        .join("onboarding.json"))
}

fn read_state() -> OnboardingState {
    let Ok(path) = state_path() else {
        return OnboardingState::default();
    };
    if !path.exists() {
        return OnboardingState::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_state(state: &OnboardingState) -> Result<(), String> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized = serde_json::to_string_pretty(state).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

#[tauri::command]
pub fn onboarding_get() -> OnboardingState {
    read_state()
}

// update(partial) passes the partial as the WHOLE payload (no wrapper key), so it
// reads the raw request body. checklist merges field-by-field per the contract.
#[tauri::command]
pub fn onboarding_update(request: tauri::ipc::Request<'_>) -> Result<OnboardingState, String> {
    let tauri::ipc::InvokeBody::Json(partial) = request.body() else {
        return Ok(read_state());
    };
    let mut state_value = serde_json::to_value(read_state())
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    if let Some(updates) = partial.as_object() {
        for (key, value) in updates {
            if key == "checklist" {
                if let (Some(current), Some(incoming)) = (
                    state_value.get_mut("checklist").and_then(Value::as_object_mut),
                    value.as_object(),
                ) {
                    for (flag, flag_value) in incoming {
                        current.insert(flag.clone(), flag_value.clone());
                    }
                }
            } else {
                state_value.insert(key.clone(), value.clone());
            }
        }
    }

    let state: OnboardingState =
        serde_json::from_value(Value::Object(state_value)).map_err(map_err)?;
    write_state(&state)?;
    Ok(state)
}
