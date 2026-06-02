// Workspace-cleanup scanning (finding stale worktrees/branches to prune) isn't
// ported. The dismiss/clear-dismissals state mutators are no-ops.

#[tauri::command]
pub fn workspace_cleanup_dismiss() {}

#[tauri::command]
pub fn workspace_cleanup_clear_dismissals() {}

// Cleanup scanning (finding stale worktrees/branches) isn't ported; report nothing.
#[tauri::command]
pub fn workspace_cleanup_scan() -> Vec<serde_json::Value> {
    Vec::new()
}
