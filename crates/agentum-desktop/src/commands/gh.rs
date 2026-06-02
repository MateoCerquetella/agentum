use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// Most of the GitHub namespace is networked API work (PRs, issues, reviews) that
// needs a token + the gh REST/GraphQL API. These two are self-contained: repoSlug
// derives owner/repo from the git remote, and enqueuePRRefresh has no queue yet.

// owner/repo = the last two path segments of the remote URL (scp-like or http(s)).
fn owner_repo_from_remote(remote: &str) -> Option<(String, String)> {
    let url = remote.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    let parts: Vec<&str> = url.split(['/', ':']).filter(|part| !part.is_empty()).collect();
    (parts.len() >= 2).then(|| {
        (
            parts[parts.len() - 2].to_string(),
            parts[parts.len() - 1].to_string(),
        )
    })
}

#[tauri::command]
pub async fn gh_repo_slug(repo_path: String, repo_id: Option<String>) -> Option<Value> {
    let _ = repo_id;
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&repo_path)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let (owner, repo) = owner_repo_from_remote(&remote)?;
    Some(json!({ "owner": owner, "repo": repo }))
}

#[tauri::command]
pub fn gh_enqueue_pr_refresh() -> bool {
    // No PR-refresh queue is ported; report not enqueued.
    false
}

// The rest of the GitHub namespace needs a token + REST/GraphQL. Mutations and the
// rate-limit query report a not-available failure; boolean acks are false; data
// lookups are null; lists/counts are empty. The richer *BySlug/projects result
// shapes are omitted until the API client lands.
fn not_available() -> Value {
    json!({ "ok": false, "error": "The GitHub API isn't available in this build." })
}

#[tauri::command]
pub fn gh_update_issue() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_merge_pr() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_set_pr_auto_merge() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_request_pr_reviewers() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_pr_state() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_remove_pr_reviewers() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_rerun_pr_checks() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_create_issue() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_rate_limit() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_star_agentum() -> bool {
    false
}

#[tauri::command]
pub fn gh_set_pr_file_viewed() -> bool {
    false
}

#[tauri::command]
pub fn gh_update_pr_title() -> bool {
    false
}

#[tauri::command]
pub fn gh_check_agentum_starred() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_work_item() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_work_item_by_owner_repo() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_work_item_details() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_issue() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_pr_check_details() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_pr_checks() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_count_work_items() -> i64 {
    0
}

// Remaining GitHub surface: REST/GraphQL mutations (issue/PR/comment/project edits)
// report not-available; *BySlug list reads and project-view reads are empty/null;
// fire-and-forget reporting no-ops. All await the API client described above.
#[tauri::command]
pub fn gh_update_pull_request_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_issue_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_add_issue_comment() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_add_pr_review_comment_reply() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_add_pr_review_comment() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_list_project_views() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_list_work_items() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_refresh_pr_now() -> bool {
    false
}

#[tauri::command]
pub fn gh_list_accessible_projects() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_resolve_project_ref() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_delete_issue_comment_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_issue_comment_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_add_issue_comment_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_list_issue_types_by_slug() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_list_labels_by_slug() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_list_assignable_users_by_slug() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_get_project_view_table() -> Option<Value> {
    None
}

#[tauri::command]
pub fn gh_update_project_item_field() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_clear_project_item_field() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_update_issue_type_by_slug() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_pr_comments() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
pub fn gh_resolve_review_thread() -> Value {
    not_available()
}

#[tauri::command]
pub fn gh_report_visible_pr_refresh_candidates() {}
