use serde_json::{json, Value};

// Linear integration needs an API token + selected workspace, which isn't connected
// here. List/search methods are empty, gets are null, disconnect no-ops. The
// connection/mutation methods (status/connect/createIssue/...) are omitted until the
// API client lands. Bare-array vs LinearCollectionResult ({items}) shapes are matched.

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
pub fn linear_disconnect() {}

// Connection + mutation surface: status reports disconnected, test/connect report
// not-available, workspace select no-ops, and issue create/update/comment return null
// until the Linear API client lands.
#[tauri::command]
pub fn linear_status() -> Value {
    json!({ "connected": false })
}

#[tauri::command]
pub fn linear_test_connection() -> Value {
    json!({ "ok": false })
}

#[tauri::command]
pub fn linear_connect() -> Value {
    json!({ "ok": false, "error": "Linear isn't available in this build." })
}

#[tauri::command]
pub fn linear_select_workspace() {}

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
