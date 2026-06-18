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
    let token = pick_token(&read_creds())
        .ok_or_else(|| anyhow::anyhow!("no Linear token configured (connect Linear in settings)"))?;
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
        };
        assert_eq!(pick_token(&creds).as_deref(), Some("tok-b"));

        let first = LinearCreds {
            workspaces: vec![StoredWorkspace {
                id: "a".into(),
                token: "tok-a".into(),
            }],
            selected_workspace_id: None,
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
}
