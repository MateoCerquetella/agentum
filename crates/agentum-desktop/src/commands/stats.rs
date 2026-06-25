use serde::Serialize;
use serde_json::{json, Value};

// Agentum's own activity counters (agents spawned / PRs created / agent-time)
// live in the agentum-store SQLite DB (events / session_metrics), not the usage
// logs, and this command has no store handle yet. Return a typed-zeroed summary
// so Stats → Overview renders the "Start your first agent" empty state instead of
// a bare `{}` (which crashes `undefined.toLocaleString()` in StatsPane). Wiring
// the real counters from the store is a tracked follow-up.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsSummary {
    total_agents_spawned: u64,
    total_prs_created: u64,
    total_agent_time_ms: u64,
    first_event_at: Option<i64>,
}

#[tauri::command]
pub fn stats_get_summary() -> Value {
    json!(StatsSummary {
        total_agents_spawned: 0,
        total_prs_created: 0,
        total_agent_time_ms: 0,
        first_event_at: None,
    })
}
