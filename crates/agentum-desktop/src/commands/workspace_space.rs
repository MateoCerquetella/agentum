// Workspace disk-space scans aren't ported; the in-flight-scan cancel is a no-op.
#[tauri::command]
pub fn workspace_space_cancel() {}
