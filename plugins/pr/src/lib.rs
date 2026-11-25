//! Pull request: the branch's GitHub PR (state, checks, review, merge) in a tab of the thread's
//! side panel, and a badge on the thread's sidebar row (its number and how its checks stand).
//!
//! The hub part keeps each thread's PR in its storage (`t:<thread>`) and every thread's badge in
//! one key (`badges`), refreshed when a turn ends, so the sidebar reads one cached map instead of
//! asking `gh` per row. Changes go out on `plugin:pr` as `{thread, badge}`.
use agentum_plugin::{Group, Manifest};
use serde_json::{json, Value};

pub static MANIFEST: Manifest = Manifest {
    id: "pr",
    name: "Pull request",
    description: "The branch's GitHub pull request: checks, review, merge.",
    icon: "github",
    group: Group::Code,
    default_on: true,
    settings: &[],
};

/// How one check of `statusCheckRollup` stands: "pass", "fail", "pending" or "skip".
pub fn check_state(c: &Value) -> &'static str {
    // A CheckRun has status + conclusion; a StatusContext has state.
    if let Some(state) = c["state"].as_str() {
        return match state {
            "SUCCESS" => "pass",
            "PENDING" | "EXPECTED" => "pending",
            _ => "fail",
        };
    }
    if c["status"].as_str().is_some_and(|s| s != "COMPLETED") {
        return "pending";
    }
    match c["conclusion"].as_str().unwrap_or("") {
        "SUCCESS" => "pass",
        "NEUTRAL" | "SKIPPED" | "STALE" => "skip",
        "" => "pending",
        _ => "fail",
    }
}

/// (passed, failed, pending) of a PR's checks.
pub fn check_counts(pr: &Value) -> (u32, u32, u32) {
    let mut n = (0, 0, 0);
    for c in pr["statusCheckRollup"].as_array().into_iter().flatten() {
        match check_state(c) {
            "pass" => n.0 += 1,
            "fail" => n.1 += 1,
            "pending" => n.2 += 1,
            _ => {}
        }
    }
    n
}

/// What a thread's sidebar row shows: `{number, state, draft, checks}` (checks: "fail" if any
/// failed, else "pending" if any runs, else "pass", or "none" without checks).
pub fn badge(pr: &Value) -> Value {
    let (pass, fail, pending) = check_counts(pr);
    let checks = if fail > 0 {
        "fail"
    } else if pending > 0 {
        "pending"
    } else if pass > 0 {
        "pass"
    } else {
        "none"
    };
    json!({"number": pr["number"], "state": pr["state"], "draft": pr["isDraft"].as_bool().unwrap_or(false), "checks": checks, "review": pr["reviewDecision"]})
}

#[cfg(feature = "hub")]
mod hub;
#[cfg(feature = "hub")]
pub use hub::PLUGIN as HUB;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::PLUGIN as WEB;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_from_checks() {
        let pr = json!({"number": 12, "state": "OPEN", "isDraft": false, "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "name": "lint", "status": "IN_PROGRESS", "conclusion": ""},
            {"__typename": "StatusContext", "context": "ci", "state": "SUCCESS"},
            {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED"}
        ]});
        assert_eq!(check_counts(&pr), (2, 0, 1));
        assert_eq!(badge(&pr)["checks"], "pending");
        let failed = json!({"number": 3, "statusCheckRollup": [{"state": "ERROR"}, {"status": "QUEUED"}]});
        assert_eq!(badge(&failed)["checks"], "fail");
        assert_eq!(badge(&json!({"number": 4, "statusCheckRollup": []}))["checks"], "none");
    }
}
