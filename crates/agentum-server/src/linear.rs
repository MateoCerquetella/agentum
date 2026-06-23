//! Server-side Linear GraphQL client for board ↔ tracker sync (spec 014c).
//!
//! Ported from spec 011/012's Linear support (originally on `staging`) and
//! **decoupled** from the harness `task_sink::TrackerPhase` 4-phase pipeline:
//! the board has only todo/doing/done columns, mapped to Linear workflow-state
//! **`type`s** (`backlog`/`unstarted`/`started`/`completed`/`canceled`) which
//! are stable across teams — unlike per-team state *names*, so no name config.
//!
//! Token: read from the desktop's `Agentum/linear.json` (the file the desktop
//! Linear settings pane writes); `AGENTUM_LINEAR_TOKEN` (raw key) and
//! `AGENTUM_LINEAR_CREDS` (file path) override it for tests/CI. No `dirs` dep —
//! the data-local dir is computed directly (macOS / XDG) to avoid churning the
//! workspace manifest.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

const LINEAR_GRAPHQL: &str = "https://api.linear.app/graphql";
// Teams + their workflow states (id, type) in one round-trip: enough to resolve
// the bound team and pick a target state id for a push.
const TEAMS_QUERY: &str =
    "query { teams(first: 50) { nodes { id key name states { nodes { id name type } } } } }";
// A team's issues (pull). `description` is the body; `state.type` → column.
const TEAM_ISSUES_QUERY: &str = "query($id: String!) { team(id: $id) { issues(first: 100) { nodes { identifier title description url state { type } } } } }";
const ISSUE_CREATE_MUTATION: &str = "mutation($teamId: String!, $title: String!, $description: String!) { issueCreate(input: { teamId: $teamId, title: $title, description: $description }) { success issue { identifier url } } }";
// Linear accepts the human identifier (e.g. ENG-42) in `id:` args, so a linked
// card pushes by its stored identifier without a separate UUID lookup.
const ISSUE_UPDATE_MUTATION: &str = "mutation($id: String!, $title: String!, $description: String!, $stateId: String!) { issueUpdate(id: $id, input: { title: $title, description: $description, stateId: $stateId }) { success } }";

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
}

/// `<data_local_dir>/Agentum/linear.json` (the file the desktop writes), or the
/// `AGENTUM_LINEAR_CREDS` override (tests).
fn creds_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AGENTUM_LINEAR_CREDS") {
        return Some(PathBuf::from(p));
    }
    Some(data_local_dir()?.join("Agentum").join("linear.json"))
}

/// macOS `~/Library/Application Support`, else `$XDG_DATA_HOME` or
/// `~/.local/share`. Mirrors `dirs::data_local_dir` without the dependency.
fn data_local_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library").join("Application Support"));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(xdg));
    }
    home.map(|h| h.join(".local").join("share"))
}

fn read_creds() -> LinearCreds {
    creds_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Token for the selected workspace, else the first. Pure for testability.
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

/// `AGENTUM_LINEAR_TOKEN` (tests/CI) wins over the on-disk creds.
fn token() -> Option<String> {
    if let Ok(t) = std::env::var("AGENTUM_LINEAR_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t);
        }
    }
    pick_token(&read_creds())
}

/// Is Linear usable? True when a token is resolvable.
pub fn available() -> bool {
    token().is_some()
}

async fn graphql(token: &str, query: &str, variables: Value) -> Result<Value, String> {
    let resp = reqwest::Client::new()
        .post(LINEAR_GRAPHQL)
        // Linear personal API keys go in Authorization raw (no "Bearer").
        .header("Authorization", token)
        .header("Content-Type", "application/json")
        .body(json!({ "query": query, "variables": variables }).to_string())
        .send()
        .await
        .map_err(|e| format!("Linear request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let body: Value =
        serde_json::from_str(&text).map_err(|e| format!("Linear JSON parse ({status}): {e}"))?;
    // Linear returns 200 with a top-level `errors` array on failure.
    if let Some(errors) = body.get("errors") {
        return Err(format!("Linear GraphQL error ({status}): {errors}"));
    }
    Ok(body)
}

// ── Column ↔ Linear state type (pure) ────────────────────────────────────────

/// A Linear workflow state's stable `type` → board column. Types:
/// `triage` | `backlog` | `unstarted` | `started` | `completed` | `canceled`.
pub fn state_type_to_column(state_type: &str) -> &'static str {
    match state_type {
        "completed" | "canceled" => "done",
        "started" => "doing",
        _ => "todo", // triage | backlog | unstarted | unknown
    }
}

/// Inverse: board column → the Linear state `type` to transition into on push.
fn column_to_state_type(column: &str) -> &'static str {
    match column {
        "done" => "completed",
        "doing" => "started",
        _ => "unstarted",
    }
}

// ── Parsed shapes ────────────────────────────────────────────────────────────

/// A Linear issue normalized to what the board needs. `column` is already
/// mapped from `state.type`; `identifier` (e.g. `ENG-42`) is the card's
/// `external_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearIssue {
    pub identifier: String,
    pub title: String,
    pub body: Option<String>,
    pub url: String,
    pub column: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Team {
    id: String,
    key: String,
    /// `(state_id, state_type)`.
    states: Vec<(String, String)>,
}

fn parse_teams(resp: &Value) -> Result<Vec<Team>, String> {
    let nodes = resp
        .pointer("/data/teams/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("unexpected teams response: {resp}"))?;
    Ok(nodes
        .iter()
        .filter_map(|t| {
            let id = t.get("id").and_then(Value::as_str)?.to_string();
            let key = t
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let states = t
                .pointer("/states/nodes")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| {
                            let sid = s.get("id").and_then(Value::as_str)?;
                            let ty = s.get("type").and_then(Value::as_str)?;
                            Some((sid.to_string(), ty.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Team { id, key, states })
        })
        .collect())
}

/// Resolve the bound team: match `project` against a team key
/// (case-insensitive); empty/`*` uses the sole team (error on 0/many — never
/// guess, mirroring 011's contract).
fn resolve_team(teams: Vec<Team>, project: &str) -> Result<Team, String> {
    let p = project.trim();
    if !p.is_empty() && p != "*" {
        return teams
            .into_iter()
            .find(|t| t.key.eq_ignore_ascii_case(p))
            .ok_or_else(|| format!("no Linear team with key '{p}'"));
    }
    match teams.len() {
        0 => Err("no Linear teams are visible to this token".into()),
        1 => Ok(teams.into_iter().next().unwrap()),
        n => Err(format!(
            "{n} Linear teams visible — set the binding project to a team key (e.g. ENG)"
        )),
    }
}

fn parse_team_issues(resp: &Value) -> Vec<LinearIssue> {
    let Some(nodes) = resp
        .pointer("/data/team/issues/nodes")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    nodes
        .iter()
        .filter_map(|n| {
            let identifier = n.get("identifier").and_then(Value::as_str)?.to_string();
            let title = n.get("title").and_then(Value::as_str)?.to_string();
            let body = n
                .get("description")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let url = n
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let ty = n.pointer("/state/type").and_then(Value::as_str).unwrap_or("unstarted");
            Some(LinearIssue {
                identifier,
                title,
                body,
                url,
                column: state_type_to_column(ty).to_string(),
            })
        })
        .collect()
}

fn parse_issue_create(resp: &Value) -> Result<(String, String), String> {
    let ok = resp
        .pointer("/data/issueCreate/success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !ok {
        return Err(format!("Linear issueCreate reported failure: {resp}"));
    }
    let issue = resp
        .pointer("/data/issueCreate/issue")
        .ok_or_else(|| format!("issueCreate returned no issue: {resp}"))?;
    let id = issue
        .get("identifier")
        .and_then(Value::as_str)
        .ok_or("Linear issue has no identifier")?
        .to_string();
    let url = issue
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok((id, url))
}

// ── Public async API (used by board_sync) ────────────────────────────────────

/// Pull the bound team's issues (014c PULL).
pub async fn pull_issues(project: &str) -> Result<Vec<LinearIssue>, String> {
    let token = token().ok_or("no Linear token configured (connect Linear in settings)")?;
    let teams = parse_teams(&graphql(&token, TEAMS_QUERY, json!({})).await?)?;
    let team = resolve_team(teams, project)?;
    let resp = graphql(&token, TEAM_ISSUES_QUERY, json!({ "id": team.id })).await?;
    Ok(parse_team_issues(&resp))
}

/// Create a Linear issue in the bound team; returns `(identifier, url)`.
pub async fn create_issue(project: &str, title: &str, body: &str) -> Result<(String, String), String> {
    let token = token().ok_or("no Linear token configured")?;
    let teams = parse_teams(&graphql(&token, TEAMS_QUERY, json!({})).await?)?;
    let team = resolve_team(teams, project)?;
    let resp = graphql(
        &token,
        ISSUE_CREATE_MUTATION,
        json!({ "teamId": team.id, "title": title, "description": body }),
    )
    .await?;
    parse_issue_create(&resp)
}

/// Push a linked card to its Linear issue: update title/body and transition to
/// the workflow state matching the card's `column` (014c PUSH). Resolves a
/// target `stateId` of the right `type` from the bound team's states.
pub async fn update_issue(
    project: &str,
    identifier: &str,
    title: &str,
    body: &str,
    column: &str,
) -> Result<(), String> {
    let token = token().ok_or("no Linear token configured")?;
    let teams = parse_teams(&graphql(&token, TEAMS_QUERY, json!({})).await?)?;
    let team = resolve_team(teams, project)?;
    let want = column_to_state_type(column);
    let state_id = team
        .states
        .iter()
        .find(|(_, ty)| ty == want)
        .map(|(id, _)| id.clone())
        .ok_or_else(|| format!("Linear team has no '{want}'-type workflow state"))?;
    let resp = graphql(
        &token,
        ISSUE_UPDATE_MUTATION,
        json!({ "id": identifier, "title": title, "description": body, "stateId": state_id }),
    )
    .await?;
    let ok = resp
        .pointer("/data/issueUpdate/success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(format!("Linear issueUpdate reported failure: {resp}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_type_maps_to_column() {
        assert_eq!(state_type_to_column("completed"), "done");
        assert_eq!(state_type_to_column("canceled"), "done");
        assert_eq!(state_type_to_column("started"), "doing");
        assert_eq!(state_type_to_column("unstarted"), "todo");
        assert_eq!(state_type_to_column("backlog"), "todo");
        assert_eq!(state_type_to_column("triage"), "todo");
    }

    #[test]
    fn column_maps_back_to_state_type() {
        assert_eq!(column_to_state_type("done"), "completed");
        assert_eq!(column_to_state_type("doing"), "started");
        assert_eq!(column_to_state_type("todo"), "unstarted");
    }

    #[test]
    fn pick_token_prefers_selected_then_first() {
        let creds = LinearCreds {
            workspaces: vec![
                StoredWorkspace { id: "a".into(), token: "tok-a".into() },
                StoredWorkspace { id: "b".into(), token: "tok-b".into() },
            ],
            selected_workspace_id: Some("b".into()),
        };
        assert_eq!(pick_token(&creds).as_deref(), Some("tok-b"));
        assert!(pick_token(&LinearCreds::default()).is_none());
    }

    #[test]
    fn parse_teams_extracts_key_and_states() {
        let resp = json!({"data": {"teams": {"nodes": [
            {"id": "T1", "key": "ENG", "name": "Eng", "states": {"nodes": [
                {"id": "s1", "name": "Todo", "type": "unstarted"},
                {"id": "s2", "name": "Done", "type": "completed"},
            ]}}
        ]}}});
        let teams = parse_teams(&resp).unwrap();
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].key, "ENG");
        assert_eq!(teams[0].states[1], ("s2".into(), "completed".into()));
    }

    #[test]
    fn resolve_team_by_key_then_sole_then_errors() {
        let teams = vec![
            Team { id: "T1".into(), key: "ENG".into(), states: vec![] },
            Team { id: "T2".into(), key: "OPS".into(), states: vec![] },
        ];
        assert_eq!(resolve_team(teams.clone(), "ops").unwrap().id, "T2");
        // ambiguous (many) without a key → error
        assert!(resolve_team(teams.clone(), "").is_err());
        // unknown key → error
        assert!(resolve_team(teams, "NOPE").is_err());
        // sole team, empty project → ok
        let one = vec![Team { id: "T1".into(), key: "ENG".into(), states: vec![] }];
        assert_eq!(resolve_team(one, "").unwrap().id, "T1");
    }

    #[test]
    fn parse_team_issues_maps_state_type_to_column() {
        let resp = json!({"data": {"team": {"issues": {"nodes": [
            {"identifier": "ENG-1", "title": "A", "description": "body", "url": "u1",
             "state": {"type": "started"}},
            {"identifier": "ENG-2", "title": "B", "description": "", "url": "u2",
             "state": {"type": "completed"}},
        ]}}}});
        let issues = parse_team_issues(&resp);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].identifier, "ENG-1");
        assert_eq!(issues[0].column, "doing");
        assert_eq!(issues[0].body.as_deref(), Some("body"));
        assert_eq!(issues[1].column, "done");
        assert_eq!(issues[1].body, None);
    }

    #[test]
    fn parse_issue_create_extracts_identifier_and_url() {
        let resp = json!({"data": {"issueCreate": {"success": true,
            "issue": {"identifier": "ENG-42", "url": "https://linear.app/x/issue/ENG-42"}}}});
        let (id, url) = parse_issue_create(&resp).unwrap();
        assert_eq!(id, "ENG-42");
        assert_eq!(url, "https://linear.app/x/issue/ENG-42");
        let fail = json!({"data": {"issueCreate": {"success": false, "issue": null}}});
        assert!(parse_issue_create(&fail).is_err());
    }
}
