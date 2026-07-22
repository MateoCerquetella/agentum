use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

// Connection surface (connect/status/test/disconnect/selectWorkspace) is wired to
// the real Linear GraphQL API + a small on-disk credential store, so the
// Integrations config pane can actually connect a workspace. The core data-read
// surface (issues / search / projects / teams) is wired to GraphQL below using
// the stored token. The remaining reads (custom views, comments, single gets)
// and the mutations stay stubbed pending their queries.

// ─── Credential store ───────────────────────────────────────────────────────
// Linear personal API keys + the resolved viewer per workspace. Persisted next to
// the desktop's other state (settings.sqlite3) so a connected workspace survives
// restarts. Multiple workspaces are supported; `selected_workspace_id` may be a
// concrete id or the sentinel "all".

#[derive(Clone, Serialize, Deserialize)]
struct StoredViewer {
    display_name: String,
    email: Option<String>,
    organization_id: Option<String>,
    organization_name: String,
    organization_url_key: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredWorkspace {
    id: String,
    token: String,
    viewer: StoredViewer,
    #[serde(default)]
    credential_revision: u32,
}

#[derive(Default, Serialize, Deserialize)]
struct LinearCreds {
    #[serde(default)]
    workspaces: Vec<StoredWorkspace>,
    #[serde(default)]
    selected_workspace_id: Option<String>,
    /// Pipeline → Linear workflow-state name overrides (spec 012). Read by the
    /// embedded server's `linear::LinearStateMap` to drive ticket transitions.
    /// Kept here (and preserved across writes) so it lives beside the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state_map: Option<StoredStateMap>,
}

/// The four pipeline phases → Linear workflow-state names. Each optional so a
/// partial override keeps the default for the rest. Field names match the
/// server's `linear::StoredStateMap` exactly — the server reads this same file.
#[derive(Default, Clone, Serialize, Deserialize)]
struct StoredStateMap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    todo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    in_progress: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ready_to_test: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    done: Option<String>,
}

// Serialize read-modify-write so concurrent connect/disconnect calls can't clobber
// the file. Held only across the synchronous file IO, never across an .await.
static STORE_LOCK: Mutex<()> = Mutex::new(());

fn creds_path() -> Result<PathBuf, String> {
    // Mirror AppState::new()'s data dir so Linear creds sit beside settings.sqlite3.
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or("failed to resolve data directory")?;
    let dir = base.join("Agentum");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("linear.json"))
}

fn read_creds() -> LinearCreds {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    creds_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn update_creds<F: FnOnce(&mut LinearCreds)>(apply: F) -> Result<LinearCreds, String> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = creds_path()?;
    let mut creds: LinearCreds = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    apply(&mut creds);
    let serialized = serde_json::to_string_pretty(&creds).map_err(|e| e.to_string())?;
    std::fs::write(&path, serialized).map_err(|e| e.to_string())?;
    Ok(creds)
}

fn pick_token(creds: &LinearCreds, workspace_id: Option<&str>) -> Option<String> {
    if let Some(id) = workspace_id {
        if id != "all" {
            return creds
                .workspaces
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.token.clone());
        }
    }
    if let Some(sel) = creds.selected_workspace_id.as_deref() {
        if sel != "all" {
            if let Some(w) = creds.workspaces.iter().find(|w| w.id == sel) {
                return Some(w.token.clone());
            }
        }
    }
    creds.workspaces.first().map(|w| w.token.clone())
}

// ─── Shape mappers (match shared/types.ts LinearViewer/Workspace/Status) ──────

fn viewer_to_json(viewer: &StoredViewer) -> Value {
    json!({
        "displayName": viewer.display_name,
        "email": viewer.email,
        "organizationId": viewer.organization_id,
        "organizationName": viewer.organization_name,
        "organizationUrlKey": viewer.organization_url_key,
    })
}

fn workspace_to_json(workspace: &StoredWorkspace) -> Value {
    json!({
        "id": workspace.id,
        "organizationId": workspace.viewer.organization_id.clone().unwrap_or_else(|| workspace.id.clone()),
        "displayName": workspace.viewer.display_name,
        "email": workspace.viewer.email,
        "organizationName": workspace.viewer.organization_name,
        "organizationUrlKey": workspace.viewer.organization_url_key,
        "credentialRevision": workspace.credential_revision,
    })
}

fn status_to_json(creds: &LinearCreds) -> Value {
    if creds.workspaces.is_empty() {
        return json!({ "connected": false, "viewer": null });
    }
    let selected = creds
        .selected_workspace_id
        .clone()
        .unwrap_or_else(|| creds.workspaces[0].id.clone());
    let active = if selected == "all" {
        &creds.workspaces[0]
    } else {
        creds
            .workspaces
            .iter()
            .find(|w| w.id == selected)
            .unwrap_or(&creds.workspaces[0])
    };
    json!({
        "connected": true,
        "viewer": viewer_to_json(&active.viewer),
        "workspaces": creds.workspaces.iter().map(workspace_to_json).collect::<Vec<_>>(),
        "activeWorkspaceId": active.id,
        "selectedWorkspaceId": selected,
    })
}

// ─── GraphQL ──────────────────────────────────────────────────────────────--

// Validate a key and resolve the viewer + organization. Linear personal API keys
// are sent as the raw key in the Authorization header (no "Bearer" prefix).
async fn fetch_viewer(token: &str) -> Result<StoredViewer, String> {
    let body = json!({
        "query": "query { viewer { id name displayName email organization { id name urlKey } } }"
    });
    let response = reqwest::Client::new()
        .post("https://api.linear.app/graphql")
        .header("Authorization", token)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Couldn’t reach Linear: {e}"))?;
    let http_status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| format!("Unexpected Linear response: {e}"))?;
    if let Some(errors) = payload.get("errors").and_then(|e| e.as_array()) {
        let message = errors
            .first()
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Linear rejected the API key.");
        return Err(message.to_string());
    }
    if !http_status.is_success() {
        return Err(format!(
            "Linear API returned HTTP {}.",
            http_status.as_u16()
        ));
    }
    let viewer = payload
        .get("data")
        .and_then(|d| d.get("viewer"))
        .ok_or("Linear response had no viewer.")?;
    let org = viewer.get("organization");
    let as_str = |value: Option<&Value>| value.and_then(|v| v.as_str()).map(str::to_string);
    Ok(StoredViewer {
        display_name: as_str(viewer.get("displayName"))
            .or_else(|| as_str(viewer.get("name")))
            .unwrap_or_default(),
        email: as_str(viewer.get("email")),
        organization_id: as_str(org.and_then(|o| o.get("id"))),
        organization_name: as_str(org.and_then(|o| o.get("name"))).unwrap_or_default(),
        organization_url_key: as_str(org.and_then(|o| o.get("urlKey"))),
    })
}

// Run an authenticated GraphQL query and return the `data` object. Mirrors
// fetch_viewer's auth + error handling (personal keys go in the raw
// Authorization header, no "Bearer"). GraphQL errors and non-2xx both surface
// as Err so list commands can reject the IPC promise instead of looking empty.
async fn graphql(token: &str, query: &str, variables: Value) -> Result<Value, String> {
    let body = json!({ "query": query, "variables": variables });
    let response = reqwest::Client::new()
        .post("https://api.linear.app/graphql")
        .header("Authorization", token)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Couldn’t reach Linear: {e}"))?;
    let http_status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|e| format!("Unexpected Linear response: {e}"))?;
    if let Some(errors) = payload.get("errors").and_then(|e| e.as_array()) {
        let message = errors
            .first()
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Linear rejected the query.");
        return Err(message.to_string());
    }
    if !http_status.is_success() {
        return Err(format!(
            "Linear API returned HTTP {}.",
            http_status.as_u16()
        ));
    }
    payload
        .get("data")
        .cloned()
        .ok_or_else(|| "Linear response had no data.".to_string())
}

// Resolve (token, workspace_id, workspace_name) for a read. Mirrors pick_token's
// "explicit id → selected → first" precedence but also returns the workspace
// identity so list rows can be stamped with workspaceId/workspaceName (the UI
// needs these to route a row back to the right workspace in "all" mode).
fn active_workspace(
    creds: &LinearCreds,
    workspace_id: Option<&str>,
) -> Option<(String, String, String)> {
    let identify = |w: &StoredWorkspace| {
        (
            w.token.clone(),
            w.id.clone(),
            w.viewer.organization_name.clone(),
        )
    };
    if let Some(id) = workspace_id {
        if id != "all" {
            return creds.workspaces.iter().find(|w| w.id == id).map(identify);
        }
    }
    if let Some(sel) = creds.selected_workspace_id.as_deref() {
        if sel != "all" {
            if let Some(w) = creds.workspaces.iter().find(|w| w.id == sel) {
                return Some(identify(w));
            }
        }
    }
    creds.workspaces.first().map(identify)
}

// Fields shared by every issue list/search read. The filter is the only thing
// that varies, so the query string is reused.
const ISSUES_QUERY: &str = "query($filter: IssueFilter, $first: Int) { \
    issues(filter: $filter, first: $first, orderBy: updatedAt) { nodes { \
        id identifier title description url priority estimate updatedAt \
        state { name type color } team { id name key } \
        assignee { id displayName avatarUrl } \
        labels { nodes { id name } } project { id name url color } } } }";

fn map_issue_node(node: &Value, ws_id: &str, ws_name: &str) -> Value {
    let label_nodes = node
        .pointer("/labels/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let labels: Vec<String> = label_nodes
        .iter()
        .filter_map(|l| l.get("name").and_then(Value::as_str).map(String::from))
        .collect();
    let label_ids: Vec<String> = label_nodes
        .iter()
        .filter_map(|l| l.get("id").and_then(Value::as_str).map(String::from))
        .collect();
    let mut item = json!({
        "id": node.get("id").and_then(Value::as_str).unwrap_or_default(),
        "workspaceId": ws_id,
        "workspaceName": ws_name,
        "identifier": node.get("identifier").and_then(Value::as_str).unwrap_or_default(),
        "title": node.get("title").and_then(Value::as_str).unwrap_or_default(),
        "url": node.get("url").and_then(Value::as_str).unwrap_or_default(),
        "state": {
            "name": node.pointer("/state/name").and_then(Value::as_str).unwrap_or_default(),
            "type": node.pointer("/state/type").and_then(Value::as_str).unwrap_or_default(),
            "color": node.pointer("/state/color").and_then(Value::as_str).unwrap_or_default(),
        },
        "team": {
            "id": node.pointer("/team/id").and_then(Value::as_str).unwrap_or_default(),
            "name": node.pointer("/team/name").and_then(Value::as_str).unwrap_or_default(),
            "key": node.pointer("/team/key").and_then(Value::as_str).unwrap_or_default(),
        },
        "labels": labels,
        "labelIds": label_ids,
        "priority": node.get("priority").and_then(Value::as_i64).unwrap_or(0),
        "updatedAt": node.get("updatedAt").and_then(Value::as_str).unwrap_or_default(),
    });
    if let Some(description) = node.get("description").and_then(Value::as_str) {
        item["description"] = json!(description);
    }
    if let Some(estimate) = node.get("estimate").and_then(Value::as_f64) {
        item["estimate"] = json!(estimate);
    }
    if node.get("assignee").map(Value::is_object).unwrap_or(false) {
        let assignee = &node["assignee"];
        item["assignee"] = json!({
            "id": assignee.get("id").and_then(Value::as_str).unwrap_or_default(),
            "displayName": assignee.get("displayName").and_then(Value::as_str).unwrap_or_default(),
            "avatarUrl": assignee.get("avatarUrl").and_then(Value::as_str),
        });
    }
    if node.get("project").map(Value::is_object).unwrap_or(false) {
        let project = &node["project"];
        item["project"] = json!({
            "id": project.get("id").and_then(Value::as_str).unwrap_or_default(),
            "workspaceId": ws_id,
            "workspaceName": ws_name,
            "name": project.get("name").and_then(Value::as_str).unwrap_or_default(),
            "url": project.get("url").and_then(Value::as_str),
            "color": project.get("color").and_then(Value::as_str),
        });
    }
    item
}

// Build the GraphQL IssueFilter for the renderer's simple filter enum.
fn issue_filter(filter: &str) -> Value {
    match filter {
        "created" => json!({ "creator": { "isMe": { "eq": true } } }),
        "completed" => json!({ "state": { "type": { "eq": "completed" } } }),
        "all" => json!({}),
        // Default ("assigned"): issues assigned to the connected viewer.
        _ => json!({ "assignee": { "isMe": { "eq": true } } }),
    }
}

async fn run_issue_query(
    workspace_id: Option<String>,
    filter: Value,
    limit: u32,
) -> Result<Vec<Value>, String> {
    let creds = read_creds();
    // Not connected → empty (no error); connection is gated by linear_status.
    let Some((token, ws_id, ws_name)) = active_workspace(&creds, workspace_id.as_deref()) else {
        return Ok(Vec::new());
    };
    let vars = json!({ "filter": filter, "first": limit.clamp(1, 100) });
    let data = graphql(&token, ISSUES_QUERY, vars).await?;
    let nodes = data
        .pointer("/issues/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(nodes
        .iter()
        .map(|node| map_issue_node(node, &ws_id, &ws_name))
        .collect())
}

// ─── Connection commands ────────────────────────────────────────────────────

#[tauri::command]
pub fn linear_status() -> Value {
    status_to_json(&read_creds())
}

#[tauri::command]
pub async fn linear_connect(api_key: String) -> Value {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return json!({ "ok": false, "error": "An API key is required." });
    }
    let viewer = match fetch_viewer(&key).await {
        Ok(viewer) => viewer,
        Err(error) => return json!({ "ok": false, "error": error }),
    };
    let id = viewer
        .organization_id
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let result = update_creds(|creds| {
        let revision = creds
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.credential_revision + 1)
            .unwrap_or(1);
        creds.workspaces.retain(|w| w.id != id);
        creds.workspaces.push(StoredWorkspace {
            id: id.clone(),
            token: key.clone(),
            viewer: viewer.clone(),
            credential_revision: revision,
        });
        if creds.selected_workspace_id.is_none() {
            creds.selected_workspace_id = Some(id.clone());
        }
    });
    match result {
        Ok(_) => json!({ "ok": true, "viewer": viewer_to_json(&viewer) }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

#[tauri::command]
pub async fn linear_test_connection(workspace_id: Option<String>) -> Value {
    let token = pick_token(&read_creds(), workspace_id.as_deref());
    match token {
        Some(token) => match fetch_viewer(&token).await {
            Ok(viewer) => json!({ "ok": true, "viewer": viewer_to_json(&viewer) }),
            Err(error) => json!({ "ok": false, "error": error }),
        },
        None => json!({ "ok": false, "error": "Linear is not connected." }),
    }
}

#[tauri::command]
pub fn linear_select_workspace(workspace_id: String) -> Value {
    match update_creds(move |creds| creds.selected_workspace_id = Some(workspace_id)) {
        Ok(creds) => status_to_json(&creds),
        Err(_) => status_to_json(&read_creds()),
    }
}

/// The effective pipeline → Linear state-name map (spec 012): stored overrides
/// filled in with the defaults the server uses, so the Settings inputs always
/// show concrete names the user can edit.
#[tauri::command]
pub fn linear_get_state_map() -> Value {
    let sm = read_creds().state_map.unwrap_or_default();
    json!({
        "todo": sm.todo.unwrap_or_else(|| "Todo".into()),
        "inProgress": sm.in_progress.unwrap_or_else(|| "In Progress".into()),
        "readyToTest": sm.ready_to_test.unwrap_or_else(|| "Ready to Test".into()),
        "done": sm.done.unwrap_or_else(|| "Done".into()),
    })
}

/// Persist the pipeline → Linear state-name overrides into `linear.json`. An
/// empty/blank field clears that override (the server falls back to its
/// default). Returns the effective map so the UI can re-render.
#[tauri::command]
pub fn linear_set_state_map(
    todo: Option<String>,
    in_progress: Option<String>,
    ready_to_test: Option<String>,
    done: Option<String>,
) -> Value {
    let norm = |s: Option<String>| s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let _ = update_creds(move |creds| {
        creds.state_map = Some(StoredStateMap {
            todo: norm(todo),
            in_progress: norm(in_progress),
            ready_to_test: norm(ready_to_test),
            done: norm(done),
        });
    });
    linear_get_state_map()
}

#[tauri::command]
pub fn linear_disconnect(workspace_id: Option<String>) {
    let _ = update_creds(move |creds| {
        if let Some(id) = workspace_id.as_deref() {
            creds.workspaces.retain(|w| w.id != id);
            if creds.selected_workspace_id.as_deref() == Some(id) {
                creds.selected_workspace_id = creds.workspaces.first().map(|w| w.id.clone());
            }
        } else {
            creds.workspaces.clear();
            creds.selected_workspace_id = None;
        }
    });
}

// ─── Data-read surface ───────────────────────────────────────────────────────
// Core lists (issues/search/projects/teams) hit GraphQL with the stored token.
// The rest (custom views, comments, single gets) stay stubbed below.

fn empty_collection() -> Value {
    json!({ "items": [] })
}

#[tauri::command]
pub async fn linear_list_issues(
    filter: Option<String>,
    limit: Option<u32>,
    workspace_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let filter = issue_filter(filter.as_deref().unwrap_or("assigned"));
    run_issue_query(workspace_id, filter, limit.unwrap_or(20)).await
}

#[tauri::command]
pub async fn linear_search_issues(
    query: Option<String>,
    limit: Option<u32>,
    workspace_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let term = query.unwrap_or_default();
    let term = term.trim();
    if term.is_empty() {
        return Ok(Vec::new());
    }
    // Match title or description; broad enough for the board's search box
    // without depending on Linear's separate full-text search endpoint.
    let filter = json!({
        "or": [
            { "title": { "containsIgnoreCase": term } },
            { "description": { "containsIgnoreCase": term } },
        ]
    });
    run_issue_query(workspace_id, filter, limit.unwrap_or(20)).await
}

#[tauri::command]
pub async fn linear_list_teams(workspace_id: Option<String>) -> Result<Vec<Value>, String> {
    let creds = read_creds();
    let Some((token, ws_id, ws_name)) = active_workspace(&creds, workspace_id.as_deref()) else {
        return Ok(Vec::new());
    };
    let data = graphql(
        &token,
        "query { teams(first: 250) { nodes { id name key } } }",
        json!({}),
    )
    .await?;
    let nodes = data
        .pointer("/teams/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(nodes
        .iter()
        .map(|team| {
            json!({
                "id": team.get("id").and_then(Value::as_str).unwrap_or_default(),
                "name": team.get("name").and_then(Value::as_str).unwrap_or_default(),
                "key": team.get("key").and_then(Value::as_str).unwrap_or_default(),
                "workspaceId": ws_id,
                "workspaceName": ws_name,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn linear_team_states(
    team_id: String,
    workspace_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let creds = read_creds();
    let Some((token, ws_id, _)) = active_workspace(&creds, workspace_id.as_deref()) else {
        return Ok(Vec::new());
    };
    let data = graphql(
        &token,
        "query($teamId: String!) { team(id: $teamId) { id states { nodes { id name type color position } } } }",
        json!({ "teamId": team_id }),
    ).await?;
    if data.pointer("/team/id").and_then(Value::as_str) != Some(team_id.as_str()) {
        return Err(format!("Linear team does not belong to workspace {ws_id}."));
    }
    Ok(data
        .pointer("/team/states/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

#[tauri::command]
pub fn linear_team_labels() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn linear_team_members() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn linear_issue_comments() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub async fn linear_list_projects(
    query: Option<String>,
    limit: Option<u32>,
    workspace_id: Option<String>,
) -> Result<Value, String> {
    let creds = read_creds();
    let Some((token, ws_id, ws_name)) = active_workspace(&creds, workspace_id.as_deref()) else {
        return Ok(empty_collection());
    };
    let vars = json!({ "first": limit.unwrap_or(50).clamp(1, 100) });
    let data = graphql(
        &token,
        "query($first: Int) { projects(first: $first, orderBy: updatedAt) { nodes { \
            id name url description color icon progress startDate targetDate createdAt updatedAt } } }",
        vars,
    )
    .await?;
    let nodes = data
        .pointer("/projects/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Linear's project query has no name filter param; apply the search term
    // client-side so the board's project search still narrows results.
    let needle = query.unwrap_or_default().trim().to_lowercase();
    let items: Vec<Value> = nodes
        .iter()
        .filter(|project| {
            needle.is_empty()
                || project
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| name.to_lowercase().contains(&needle))
                    .unwrap_or(false)
        })
        .map(|project| {
            let mut mapped = json!({
                "id": project.get("id").and_then(Value::as_str).unwrap_or_default(),
                "workspaceId": ws_id,
                "workspaceName": ws_name,
                "name": project.get("name").and_then(Value::as_str).unwrap_or_default(),
            });
            for key in [
                "url",
                "description",
                "color",
                "icon",
                "startDate",
                "targetDate",
                "createdAt",
                "updatedAt",
            ] {
                if let Some(value) = project.get(key).filter(|v| !v.is_null()) {
                    mapped[key] = value.clone();
                }
            }
            if let Some(progress) = project.get("progress").and_then(Value::as_f64) {
                mapped["progress"] = json!(progress);
            }
            mapped
        })
        .collect();
    Ok(json!({ "items": items }))
}

#[tauri::command]
pub async fn linear_list_project_issues(
    project_id: String,
    limit: Option<u32>,
    workspace_id: String,
) -> Result<Value, String> {
    let creds = read_creds();
    let Some((token, ws_id, ws_name)) = active_workspace(&creds, Some(&workspace_id)) else {
        return Ok(empty_collection());
    };
    if ws_id != workspace_id {
        return Err("Linear workspace identity mismatch.".into());
    }
    let data = graphql(
        &token,
        ISSUES_QUERY,
        json!({ "filter": { "project": { "id": { "eq": project_id } } }, "first": limit.unwrap_or(100).clamp(1, 100) }),
    ).await?;
    let items: Vec<Value> = data
        .pointer("/issues/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|node| map_issue_node(node, &ws_id, &ws_name))
        .filter(|issue| {
            issue.pointer("/project/id").and_then(Value::as_str) == Some(project_id.as_str())
        })
        .collect();
    Ok(json!({ "items": items }))
}

#[tauri::command]
pub fn linear_list_custom_views() -> Value {
    empty_collection()
}

#[tauri::command]
pub fn linear_list_custom_view_issues() -> Value {
    empty_collection()
}

#[tauri::command]
pub fn linear_list_custom_view_projects() -> Value {
    empty_collection()
}

#[tauri::command]
pub async fn linear_get_issue(
    id: String,
    workspace_id: Option<String>,
) -> Result<Option<Value>, String> {
    let creds = read_creds();
    let Some((token, ws_id, ws_name)) = active_workspace(&creds, workspace_id.as_deref()) else {
        return Ok(None);
    };
    let query = "query($id: String!) { issue(id: $id) { id identifier title description url priority estimate updatedAt state { name type color } team { id name key } assignee { id displayName avatarUrl } labels { nodes { id name } } project { id name url color } } }";
    let data = graphql(&token, query, json!({ "id": id })).await?;
    Ok(data
        .get("issue")
        .filter(|node| node.is_object())
        .map(|node| map_issue_node(node, &ws_id, &ws_name)))
}

#[tauri::command]
pub async fn linear_get_project(id: String, workspace_id: String) -> Result<Option<Value>, String> {
    let creds = read_creds();
    let Some((token, ws_id, ws_name)) = active_workspace(&creds, Some(&workspace_id)) else {
        return Ok(None);
    };
    if ws_id != workspace_id {
        return Err("Linear workspace identity mismatch.".into());
    }
    let data = graphql(&token, "query($id: String!) { project(id: $id) { id name url description color icon progress startDate targetDate createdAt updatedAt teams { nodes { id name key } } } }", json!({ "id": id })).await?;
    let Some(project) = data.get("project").filter(|value| value.is_object()) else {
        return Ok(None);
    };
    if project.get("id").and_then(Value::as_str) != Some(id.as_str()) {
        return Err("Linear project identity mismatch.".into());
    }
    let mut result = project.clone();
    result["workspaceId"] = json!(ws_id);
    result["workspaceName"] = json!(ws_name);
    if let Some(nodes) = project.pointer("/teams/nodes") {
        result["teams"] = nodes.clone();
    }
    Ok(Some(result))
}

#[tauri::command]
pub fn linear_get_custom_view() -> Option<Value> {
    None
}

#[tauri::command]
pub async fn linear_create_issue(
    team_id: String,
    title: String,
    description: Option<String>,
    workspace_id: Option<String>,
    parent_issue_id: Option<String>,
    project_id: Option<String>,
    state_id: Option<String>,
    priority: Option<i32>,
    assignee_id: Option<String>,
    label_ids: Option<Vec<String>>,
) -> Value {
    let creds = read_creds();
    let Some((token, _, _)) = active_workspace(&creds, workspace_id.as_deref()) else {
        return json!({ "ok": false, "error": "Linear is not connected." });
    };
    let mut input = json!({ "teamId": team_id, "title": title });
    for (key, value) in [
        ("description", description.map(Value::String)),
        ("parentId", parent_issue_id.map(Value::String)),
        ("projectId", project_id.map(Value::String)),
        ("stateId", state_id.map(Value::String)),
        ("assigneeId", assignee_id.map(Value::String)),
    ] {
        if let Some(value) = value {
            input[key] = value;
        }
    }
    if let Some(value) = priority {
        input["priority"] = json!(value);
    }
    if let Some(value) = label_ids {
        input["labelIds"] = json!(value);
    }
    match graphql(&token, "mutation($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { id identifier title url project { id } team { id } } } }", json!({ "input": input })).await {
        Ok(data) => match data.pointer("/issueCreate/issue") { Some(issue) => json!({ "ok": true, "id": issue["id"], "identifier": issue["identifier"], "title": issue["title"], "url": issue["url"], "projectId": issue.pointer("/project/id"), "teamId": issue.pointer("/team/id") }), None => json!({ "ok": false, "error": "Linear did not create the issue." }) },
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

#[tauri::command]
pub async fn linear_update_issue(
    id: String,
    updates: Value,
    workspace_id: Option<String>,
) -> Value {
    let creds = read_creds();
    let Some((token, _, _)) = active_workspace(&creds, workspace_id.as_deref()) else {
        return json!({ "ok": false, "error": "Linear is not connected." });
    };
    match graphql(&token, "mutation($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success issue { id project { id } team { id } } } }", json!({ "id": id, "input": updates })).await {
        Ok(data) if data.pointer("/issueUpdate/success").and_then(Value::as_bool) == Some(true) => json!({ "ok": true }),
        Ok(_) => json!({ "ok": false, "error": "Linear did not update the issue." }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_issue_mapper_stamps_exact_workspace_and_project() {
        let issue = json!({ "id":"i", "identifier":"ENG-1", "title":"T", "url":"https://linear.app/i", "priority":0, "updatedAt":"now", "state":{}, "team":{}, "labels":{"nodes":[]}, "project":{"id":"p", "name":"P"} });
        let mapped = map_issue_node(&issue, "w", "W");
        assert_eq!(mapped["workspaceId"], "w");
        assert_eq!(mapped["project"]["id"], "p");
        assert_eq!(mapped["project"]["workspaceId"], "w");
    }
}

// Post a comment to a Linear issue via the GraphQL `commentCreate` mutation using
// the stored workspace token, so the browser-verification loop (spec 008a) can
// report pass/fail back to the linked issue. A missing token returns a typed
// not-connected error rather than a silent no-op.
#[tauri::command]
pub async fn linear_add_issue_comment(
    issue_id: String,
    body: String,
    workspace_id: Option<String>,
) -> Value {
    let creds = read_creds();
    let Some(token) = pick_token(&creds, workspace_id.as_deref()) else {
        return json!({ "ok": false, "error": "Linear is not connected." });
    };
    const ADD_COMMENT_MUTATION: &str = "mutation($issueId: String!, $body: String!) { commentCreate(input: { issueId: $issueId, body: $body }) { success comment { id url } } }";
    let variables = json!({ "issueId": issue_id, "body": body });
    match graphql(&token, ADD_COMMENT_MUTATION, variables).await {
        Ok(data) => {
            let comment = data.get("commentCreate").and_then(|c| c.get("comment"));
            json!({
                "ok": true,
                "id": comment.and_then(|c| c.get("id")).and_then(Value::as_str).unwrap_or_default(),
                "url": comment.and_then(|c| c.get("url")).and_then(Value::as_str).unwrap_or_default(),
            })
        }
        Err(message) => json!({ "ok": false, "error": message }),
    }
}
