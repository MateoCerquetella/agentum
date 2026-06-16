//! `agentum status` — a one-glance summary of the control plane the CLI is
//! pointed at (the desktop's embedded server when run inside a pane, else the
//! configured/standalone daemon). Composed from existing server routes; no new
//! server endpoint needed.

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::http::ApiClient;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct StatusReport {
    /// The base URL these numbers came from — makes it obvious whether the CLI
    /// reached the live desktop (an ephemeral 127.0.0.1 port) or the daemon.
    pub api_base: String,
    pub sessions_total: u64,
    pub sessions_running: u64,
    pub worktrees: u64,
    pub hosts_total: u64,
    pub hosts_reachable: u64,
}

/// Pure composition over the three list responses, so the counting logic is
/// unit-testable without a live server. Defensive against shape: anything not
/// an array counts as empty rather than erroring.
pub fn render_status(
    api_base: &str,
    sessions: &Value,
    worktrees: &Value,
    hosts: &Value,
) -> StatusReport {
    let arr = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let sessions = arr(sessions);
    let hosts = arr(hosts);
    let sessions_running = sessions
        .iter()
        .filter(|s| s.get("status").and_then(Value::as_str) == Some("running"))
        .count() as u64;
    // A host is "reachable" when it reports neither an explicit unreachable
    // readiness nor a false `reachable` flag — be lenient about which key the
    // route uses so this keeps working as the host shape evolves.
    let hosts_reachable = hosts
        .iter()
        .filter(|h| {
            h.get("reachable").and_then(Value::as_bool).unwrap_or(true)
                && h.get("readiness")
                    .and_then(|r| r.get("status"))
                    .and_then(Value::as_str)
                    != Some("unreachable")
        })
        .count() as u64;
    StatusReport {
        api_base: api_base.to_string(),
        sessions_total: sessions.len() as u64,
        sessions_running,
        worktrees: arr(worktrees).len() as u64,
        hosts_total: hosts.len() as u64,
        hosts_reachable,
    }
}

pub async fn run(json: bool) -> Result<()> {
    let client = ApiClient::from_env();
    // Tolerate routes that aren't present on older/standalone servers: a single
    // failed list counts as empty rather than aborting the whole status.
    let sessions = client.get_json("/api/sessions").await;
    let worktrees = client.get_json("/api/worktrees").await;
    let hosts = client.get_json("/api/hosts").await;
    // But don't paper over a server we can't reach AT ALL: if every endpoint
    // errors, the daemon is down/unreachable (or the scheme is wrong, e.g. a
    // plaintext base against a TLS daemon). Report that instead of a misleading
    // all-zeros summary.
    if sessions.is_err() && worktrees.is_err() && hosts.is_err() {
        anyhow::bail!(
            "couldn't reach the agentum control plane at {} — is a server running there?\n\
             Run inside an agentum pane (sets $AGENTUM_API_URL), or `agentum profiles use <name>`.",
            client.base()
        );
    }
    let report = render_status(
        client.base(),
        &sessions.unwrap_or(Value::Null),
        &worktrees.unwrap_or(Value::Null),
        &hosts.unwrap_or(Value::Null),
    );

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("agentum @ {}", report.api_base);
        println!(
            "  sessions   {} ({} running)",
            report.sessions_total, report.sessions_running
        );
        println!("  worktrees  {}", report.worktrees);
        println!(
            "  hosts      {} ({} reachable)",
            report.hosts_total, report.hosts_reachable
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_running_sessions_and_reachable_hosts() {
        let sessions = json!([
            {"status": "running"}, {"status": "stopped"}, {"status": "running"}
        ]);
        let worktrees = json!([{"id": 1}, {"id": 2}]);
        let hosts = json!([
            {"reachable": true},
            {"readiness": {"status": "unreachable"}},
            {"reachable": true}
        ]);
        let r = render_status("http://127.0.0.1:8822", &sessions, &worktrees, &hosts);
        assert_eq!(r.sessions_total, 3);
        assert_eq!(r.sessions_running, 2);
        assert_eq!(r.worktrees, 2);
        assert_eq!(r.hosts_total, 3);
        assert_eq!(r.hosts_reachable, 2);
        assert_eq!(r.api_base, "http://127.0.0.1:8822");
    }

    #[test]
    fn null_or_nonarray_responses_count_as_empty() {
        let r = render_status("x", &Value::Null, &json!({"oops": true}), &Value::Null);
        assert_eq!(r.sessions_total, 0);
        assert_eq!(r.worktrees, 0);
        assert_eq!(r.hosts_total, 0);
        assert_eq!(r.sessions_running, 0);
    }
}
