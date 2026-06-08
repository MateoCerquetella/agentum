use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

// Connection surface (connect/status/test/disconnect/selectWorkspace) is wired to
// the real Linear GraphQL API + a small on-disk credential store, so the
// Integrations config pane can actually connect a workspace. The data-read surface
// (issues/projects/custom views) is still stubbed below and returns empty/null
// until those GraphQL queries are ported.

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
        return Err(format!("Linear API returned HTTP {}.", http_status.as_u16()));
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

// ─── Data-read surface (still stubbed) ───────────────────────────────────────
// List/search return empty, gets return null, until the GraphQL queries land.

fn empty_collection() -> Value {
    json!({ "items": [] })
}

#[tauri::command]
pub fn linear_list_issues() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn linear_search_issues() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn linear_list_teams() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn linear_team_states() -> Vec<Value> {
    Vec::new()
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
pub fn linear_list_projects() -> Value {
    empty_collection()
}

#[tauri::command]
pub fn linear_list_project_issues() -> Value {
    empty_collection()
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
pub fn linear_get_issue() -> Option<Value> {
    None
}

#[tauri::command]
pub fn linear_get_project() -> Option<Value> {
    None
}

#[tauri::command]
pub fn linear_get_custom_view() -> Option<Value> {
    None
}

#[tauri::command]
pub fn linear_create_issue() -> Option<Value> {
    None
}

#[tauri::command]
pub fn linear_update_issue() -> Option<Value> {
    None
}

#[tauri::command]
pub fn linear_add_issue_comment() -> Option<Value> {
    None
}
