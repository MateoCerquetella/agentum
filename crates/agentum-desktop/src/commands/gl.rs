use serde_json::{json, Value};

// GitLab integration needs a token + the GitLab API, which isn't connected here.
// Query methods report empty/null; the {ok,error} unions report a not-available
// failure. Methods with richer result shapes (listMRs, comment results) are omitted
// until the API client lands.
fn not_available() -> Value {
    json!({ "ok": false, "error": "The GitLab API isn't available in this build." })
}

#[tauri::command]
pub fn gl_list_labels() -> Vec<String> {
    Vec::new()
}

#[tauri::command]
pub fn gl_list_assignable_users() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gl_work_item_details() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gl_rate_limit() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_job_trace() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_retry_job() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_update_mr() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_close_mr() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_reopen_mr() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_merge_mr() -> Value {
    not_available()
}

// Discussion/comment/reviewer mutations report not-available; listMRs is empty until
// the GitLab API client lands.
#[tauri::command]
pub fn gl_resolve_mr_discussion() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_update_mr_reviewers() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_add_mr_inline_comment() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_add_mr_comment() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_add_issue_comment() -> Value {
    not_available()
}

#[tauri::command]
pub fn gl_list_m_rs() -> Vec<Value> {
    Vec::new()
}
