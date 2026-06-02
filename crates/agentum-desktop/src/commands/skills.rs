use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// Skill discovery (scanning for agent skill definitions) isn't ported; report none.
#[tauri::command]
pub fn skills_discover() -> Value {
    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0);
    json!({ "skills": [], "sources": [], "scannedAt": scanned_at })
}
