use chrono::Utc;
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

use crate::state::{AgentRecord, AppState};

#[tauri::command]
pub async fn agents_list(state: State<'_, AppState>) -> Result<Vec<AgentRecord>, String> {
    let runtime = state.runtime.lock();
    Ok(runtime.agents.values().cloned().collect())
}

#[tauri::command]
pub async fn agents_spawn(
    state: State<'_, AppState>,
    kind: String,
    config: Value,
) -> Result<String, String> {
    let id = Uuid::new_v4().to_string();
    let record = AgentRecord {
        id: id.clone(),
        kind,
        status: "running".to_string(),
        config,
        created_at: Utc::now(),
    };

    state.runtime.lock().agents.insert(id.clone(), record);
    Ok(id)
}

#[tauri::command]
pub async fn agents_kill(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut runtime = state.runtime.lock();
    let agent = runtime
        .agents
        .get_mut(&id)
        .ok_or_else(|| format!("unknown agent: {id}"))?;
    agent.status = "killed".to_string();
    Ok(())
}

#[tauri::command]
pub async fn agents_get_status(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let runtime = state.runtime.lock();
    runtime
        .agents
        .get(&id)
        .map(|agent| agent.status.clone())
        .ok_or_else(|| format!("unknown agent: {id}"))
}
