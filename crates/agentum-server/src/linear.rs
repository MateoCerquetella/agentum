//! Server-side Linear support for the chat-to-features task sink (spec 011c).
//!
//! Reads the Linear token the desktop already stored (see
//! `agentum-desktop/src/commands/linear.rs`) and creates issues via the Linear
//! GraphQL API. **Team resolution**: a workspace with exactly one team uses it;
//! more than one is an explicit error until a default-team setting exists
//! (011d follow-up) — we never guess which team a feature belongs to.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

const LINEAR_GRAPHQL: &str = "https://api.linear.app/graphql";
const TEAMS_QUERY: &str = "query { teams(first: 2) { nodes { id name } } }";
const ISSUE_CREATE_MUTATION: &str = "mutation($teamId: String!, $title: String!, $description: String!) { issueCreate(input: { teamId: $teamId, title: $title, description: $description }) { success issue { identifier url } } }";
/// Resolve an issue (by UUID *or* human identifier like `ENG-42` — Linear's
/// `issue(id:)` accepts both) to its UUID and the workflow states of its team in
/// one round-trip, so a transition is two calls (lookup + update) at most.
const ISSUE_STATES_QUERY: &str =
    "query($id: String!) { issue(id: $id) { id team { states { nodes { id name } } } } }";
const ISSUE_UPDATE_STATE_MUTATION: &str = "mutation($id: String!, $stateId: String!) { issueUpdate(id: $id, input: { stateId: $stateId }) { success } }";

#[derive(Debug, Deserialize)]
struct StoredWorkspace {
    id: String,
    token: String,
}

#[derive(Debug, Default, Deserialize)]
struct LinearCreds {
    #[serde(default)]
    workspaces: Vec<StoredWorkspace>,
    #[serde(default)]
    selected_workspace_id: Option<String>,
    /// Optional per-workspace override of the pipeline → workflow-state names.
    /// Written by the desktop Settings pane; absent fields keep the default.
    #[serde(default)]
    state_map: Option<StoredStateMap>,
}

/// The persisted state-name overrides (Settings → Integrations → Linear). Each
/// field is optional so a partial override is fine.
#[derive(Debug, Default, Deserialize)]
struct StoredStateMap {
    #[serde(default)]
    todo: Option<String>,
    #[serde(default)]
    in_progress: Option<String>,
    #[serde(default)]
    ready_to_test: Option<String>,
    #[serde(default)]
    done: Option<String>,
}

/// Path to the desktop's Linear creds file. Mirrors
/// `agentum-desktop/src/commands/linear.rs::creds_path` exactly
/// (`<data_local_dir|data_dir>/Agentum/linear.json`) so the server reads the
/// same file the desktop writes. `AGENTUM_LINEAR_CREDS` overrides it (tests).
fn creds_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AGENTUM_LINEAR_CREDS") {
        return Some(PathBuf::from(p));
    }
    let base = dirs::data_local_dir().or_else(dirs::data_dir)?;
    Some(base.join("Agentum").join("linear.json"))
}

fn read_creds() -> LinearCreds {
    creds_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Token for the selected workspace, else the first. Pure for testability;
/// mirrors the desktop's `pick_token` (selected → first).
fn pick_token(creds: &LinearCreds) -> Option<String> {
    if let Some(sel) = creds.selected_workspace_id.as_deref() {
        if sel != "all" {
            if let Some(w) = creds.workspaces.iter().find(|w| w.id == sel) {
                return Some(w.token.clone());
            }
        }
    }
    creds.workspaces.first().map(|w| w.token.clone())
}

/// Is Linear usable as a sink? True when a token is on disk.
pub fn available() -> bool {
    pick_token(&read_creds()).is_some()
}

async fn graphql(token: &str, query: &str, variables: Value) -> anyhow::Result<Value> {
    let resp = reqwest::Client::new()
        .post(LINEAR_GRAPHQL)
        .header("Authorization", token)
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await?;
    let status = resp.status();
    let body: Value = resp.json().await?;
    // Linear returns 200 with a top-level `errors` array on failure.
    if let Some(errors) = body.get("errors") {
        anyhow::bail!("Linear GraphQL error ({status}): {errors}");
    }
    Ok(body)
}

/// Resolve the single team to create issues in. Pure parse for testability.
/// Exactly one team → its id; zero or many → an explicit error (we never guess).
fn parse_team_id(teams_response: &Value) -> anyhow::Result<String> {
    let nodes = teams_response
        .pointer("/data/teams/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("unexpected teams response: {teams_response}"))?;
    match nodes.len() {
        0 => anyhow::bail!("no Linear teams are visible to this token"),
        1 => Ok(nodes[0]
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Linear team has no id"))?
            .to_string()),
        n => {
            let names: Vec<&str> = nodes
                .iter()
                .filter_map(|t| t.get("name").and_then(Value::as_str))
                .collect();
            anyhow::bail!(
                "{n} Linear teams ({names:?}); configure a default team (011d) — refusing to guess"
            );
        }
    }
}

/// Parse an `issueCreate` response into `(identifier, url)`. Pure for testability.
fn parse_issue_create(response: &Value) -> anyhow::Result<(String, Option<String>)> {
    let success = response
        .pointer("/data/issueCreate/success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !success {
        anyhow::bail!("Linear issueCreate reported failure: {response}");
    }
    let issue = response
        .pointer("/data/issueCreate/issue")
        .ok_or_else(|| anyhow::anyhow!("issueCreate returned no issue: {response}"))?;
    let identifier = issue
        .get("identifier")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Linear issue has no identifier"))?
        .to_string();
    let url = issue.get("url").and_then(Value::as_str).map(str::to_string);
    Ok((identifier, url))
}

/// Create a Linear issue from a feature; returns `(identifier, url)`. Errors
/// loudly when no token is configured or the team can't be resolved — the
/// pipeline surfaces it rather than dropping the feature.
pub async fn create_issue(
    title: &str,
    description: &str,
) -> anyhow::Result<(String, Option<String>)> {
    let token = pick_token(&read_creds()).ok_or_else(|| {
        anyhow::anyhow!("no Linear token configured (connect Linear in settings)")
    })?;
    let teams = graphql(&token, TEAMS_QUERY, json!({})).await?;
    let team_id = parse_team_id(&teams)?;
    let resp = graphql(
        &token,
        ISSUE_CREATE_MUTATION,
        json!({ "teamId": team_id, "title": title, "description": description }),
    )
    .await?;
    parse_issue_create(&resp)
}

/// The four pipeline phases mapped onto a team's workflow-state *names*. Linear
/// workspaces have custom states, so we resolve by name at runtime rather than
/// hard-coding ids. Defaults to the names the user asked for; `Ready to Test`
/// is usually a custom state (Linear ships `In Review` instead), so a missing
/// target is a logged skip, never a run failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearStateMap {
    pub todo: String,
    pub in_progress: String,
    pub ready_to_test: String,
    pub done: String,
}

impl Default for LinearStateMap {
    fn default() -> Self {
        Self {
            todo: "Todo".into(),
            in_progress: "In Progress".into(),
            ready_to_test: "Ready to Test".into(),
            done: "Done".into(),
        }
    }
}

impl LinearStateMap {
    /// Resolve the effective state map: defaults → creds-file overrides
    /// (`linear.json` `state_map`, written by Settings) → env overrides
    /// (`AGENTUM_LINEAR_STATE_*`, highest precedence, for tests/CI). A partial
    /// override at any layer keeps the lower layer's value.
    pub fn from_env() -> Self {
        let mut m = Self::default();
        // Layer 1: persisted Settings overrides from the creds file.
        if let Some(sm) = read_creds().state_map {
            if let Some(v) = sm.todo.filter(|s| !s.trim().is_empty()) {
                m.todo = v.trim().to_string();
            }
            if let Some(v) = sm.in_progress.filter(|s| !s.trim().is_empty()) {
                m.in_progress = v.trim().to_string();
            }
            if let Some(v) = sm.ready_to_test.filter(|s| !s.trim().is_empty()) {
                m.ready_to_test = v.trim().to_string();
            }
            if let Some(v) = sm.done.filter(|s| !s.trim().is_empty()) {
                m.done = v.trim().to_string();
            }
        }
        // Layer 2: env overrides (win over the creds file).
        if let Ok(v) = std::env::var("AGENTUM_LINEAR_STATE_TODO") {
            if !v.trim().is_empty() {
                m.todo = v.trim().to_string();
            }
        }
        if let Ok(v) = std::env::var("AGENTUM_LINEAR_STATE_IN_PROGRESS") {
            if !v.trim().is_empty() {
                m.in_progress = v.trim().to_string();
            }
        }
        if let Ok(v) = std::env::var("AGENTUM_LINEAR_STATE_READY_TO_TEST") {
            if !v.trim().is_empty() {
                m.ready_to_test = v.trim().to_string();
            }
        }
        if let Ok(v) = std::env::var("AGENTUM_LINEAR_STATE_DONE") {
            if !v.trim().is_empty() {
                m.done = v.trim().to_string();
            }
        }
        m
    }

    /// The configured state name for a pipeline phase.
    pub fn name_for(&self, phase: crate::task_sink::TrackerPhase) -> &str {
        use crate::task_sink::TrackerPhase::*;
        match phase {
            Todo => &self.todo,
            InProgress => &self.in_progress,
            ReadyToTest => &self.ready_to_test,
            Done => &self.done,
        }
    }
}

/// Outcome of a transition attempt. `Skipped` carries why so the harness log can
/// say *which* state was missing without surfacing it as a hard error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionOutcome {
    Applied,
    Skipped(String),
}

/// Parse the `issue { id team { states } }` response into `(issue_uuid, states)`
/// where `states` is `(state_id, state_name)`. Pure for testability.
fn parse_issue_states(response: &Value) -> anyhow::Result<(String, Vec<(String, String)>)> {
    let issue = response
        .pointer("/data/issue")
        .filter(|v| !v.is_null())
        .ok_or_else(|| anyhow::anyhow!("Linear issue not found: {response}"))?;
    let id = issue
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Linear issue has no id"))?
        .to_string();
    let states = issue
        .pointer("/team/states/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Linear issue has no team workflow states"))?
        .iter()
        .filter_map(|s| {
            let sid = s.get("id").and_then(Value::as_str)?;
            let name = s.get("name").and_then(Value::as_str)?;
            Some((sid.to_string(), name.to_string()))
        })
        .collect();
    Ok((id, states))
}

/// Find a workflow-state id whose name matches `wanted` (case-insensitive, trimmed).
/// Pure for testability. `None` = the team has no such state (caller skips).
fn match_state_by_name(states: &[(String, String)], wanted: &str) -> Option<String> {
    let want = wanted.trim().to_lowercase();
    states
        .iter()
        .find(|(_, name)| name.trim().to_lowercase() == want)
        .map(|(id, _)| id.clone())
}

/// Move a Linear issue (by identifier or UUID) into the workflow state named for
/// `phase`. Best-effort by contract: a missing token, unresolved issue, or absent
/// target state returns `Skipped`/`Err` for the caller to log — the harness must
/// never halt because the tracker side-channel hiccuped.
pub async fn transition_issue(
    identifier: &str,
    phase: crate::task_sink::TrackerPhase,
    map: &LinearStateMap,
) -> anyhow::Result<TransitionOutcome> {
    let token =
        pick_token(&read_creds()).ok_or_else(|| anyhow::anyhow!("no Linear token configured"))?;
    let target_name = map.name_for(phase);
    let resp = graphql(&token, ISSUE_STATES_QUERY, json!({ "id": identifier })).await?;
    let (issue_uuid, states) = parse_issue_states(&resp)?;
    let Some(state_id) = match_state_by_name(&states, target_name) else {
        let available: Vec<&str> = states.iter().map(|(_, n)| n.as_str()).collect();
        return Ok(TransitionOutcome::Skipped(format!(
            "no Linear state named {target_name:?} on this team (have {available:?})"
        )));
    };
    let update = graphql(
        &token,
        ISSUE_UPDATE_STATE_MUTATION,
        json!({ "id": issue_uuid, "stateId": state_id }),
    )
    .await?;
    let success = update
        .pointer("/data/issueUpdate/success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if success {
        Ok(TransitionOutcome::Applied)
    } else {
        anyhow::bail!("Linear issueUpdate reported failure: {update}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pick_token_prefers_selected_then_first() {
        let creds = LinearCreds {
            workspaces: vec![
                StoredWorkspace {
                    id: "a".into(),
                    token: "tok-a".into(),
                },
                StoredWorkspace {
                    id: "b".into(),
                    token: "tok-b".into(),
                },
            ],
            selected_workspace_id: Some("b".into()),
            state_map: None,
        };
        assert_eq!(pick_token(&creds).as_deref(), Some("tok-b"));

        let first = LinearCreds {
            workspaces: vec![StoredWorkspace {
                id: "a".into(),
                token: "tok-a".into(),
            }],
            selected_workspace_id: None,
            state_map: None,
        };
        assert_eq!(pick_token(&first).as_deref(), Some("tok-a"));

        assert!(pick_token(&LinearCreds::default()).is_none());
    }

    #[test]
    fn parse_team_id_one_team_ok() {
        let resp = json!({"data": {"teams": {"nodes": [{"id": "T1", "name": "Core"}]}}});
        assert_eq!(parse_team_id(&resp).unwrap(), "T1");
    }

    #[test]
    fn parse_team_id_zero_or_many_errors() {
        let none = json!({"data": {"teams": {"nodes": []}}});
        assert!(parse_team_id(&none).is_err(), "zero teams must error");

        let many = json!({"data": {"teams": {"nodes": [
            {"id": "T1", "name": "Core"}, {"id": "T2", "name": "Growth"}
        ]}}});
        let err = parse_team_id(&many).unwrap_err().to_string();
        assert!(err.contains("refusing to guess"), "got: {err}");
    }

    #[test]
    fn parse_issue_create_extracts_identifier_and_url() {
        let resp = json!({"data": {"issueCreate": {
            "success": true,
            "issue": {"identifier": "ENG-42", "url": "https://linear.app/acme/issue/ENG-42"}
        }}});
        let (id, url) = parse_issue_create(&resp).unwrap();
        assert_eq!(id, "ENG-42");
        assert_eq!(url.as_deref(), Some("https://linear.app/acme/issue/ENG-42"));
    }

    #[test]
    fn parse_issue_create_failure_errors() {
        let resp = json!({"data": {"issueCreate": {"success": false, "issue": null}}});
        assert!(parse_issue_create(&resp).is_err());
    }

    #[test]
    fn state_map_defaults_match_requested_names() {
        let m = LinearStateMap::default();
        assert_eq!(m.todo, "Todo");
        assert_eq!(m.in_progress, "In Progress");
        assert_eq!(m.ready_to_test, "Ready to Test");
        assert_eq!(m.done, "Done");
    }

    #[test]
    fn parse_issue_states_extracts_uuid_and_states() {
        let resp = json!({"data": {"issue": {
            "id": "uuid-1",
            "team": {"states": {"nodes": [
                {"id": "s1", "name": "Todo"},
                {"id": "s2", "name": "In Progress"},
            ]}}
        }}});
        let (uuid, states) = parse_issue_states(&resp).unwrap();
        assert_eq!(uuid, "uuid-1");
        assert_eq!(states.len(), 2);
        assert_eq!(states[1], ("s2".into(), "In Progress".into()));
    }

    #[test]
    fn parse_issue_states_missing_issue_errors() {
        let resp = json!({"data": {"issue": null}});
        assert!(parse_issue_states(&resp).is_err());
    }

    #[test]
    fn match_state_by_name_is_case_insensitive_and_trims() {
        let states = vec![
            ("s1".to_string(), "Todo".to_string()),
            ("s2".to_string(), "In Progress".to_string()),
        ];
        assert_eq!(
            match_state_by_name(&states, "in progress").as_deref(),
            Some("s2")
        );
        assert_eq!(
            match_state_by_name(&states, "  TODO ").as_deref(),
            Some("s1")
        );
        assert_eq!(match_state_by_name(&states, "Ready to Test"), None);
    }
}
