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
    #[serde(rename = "totalPRsCreated")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_summary_serializes_exact_camelcase_contract_keys() {
        let v = stats_get_summary();
        let obj = v.as_object().expect("summary is a JSON object");
        // Exact keys the TS StatsSummary contract requires (types.ts ~2856).
        assert!(obj.contains_key("totalAgentsSpawned"));
        assert!(
            obj.contains_key("totalPRsCreated"),
            "must be totalPRsCreated, not totalPrsCreated"
        );
        assert!(obj.contains_key("totalAgentTimeMs"));
        assert!(obj.contains_key("firstEventAt"));
        // Guard against the serde camelCase acronym pitfall:
        assert!(!obj.contains_key("totalPrsCreated"));
        // Typed-zeroed baseline values:
        assert_eq!(obj["totalAgentsSpawned"], 0);
        assert_eq!(obj["totalPRsCreated"], 0);
        assert_eq!(obj["totalAgentTimeMs"], 0);
        assert!(obj["firstEventAt"].is_null());
    }
}
