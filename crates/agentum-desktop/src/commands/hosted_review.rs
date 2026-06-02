use serde_json::Value;

// Hosted code review (a backend service) isn't wired up; no review exists for a branch.
#[tauri::command]
pub fn hosted_review_for_branch() -> Option<Value> {
    None
}

// Creating a hosted review + its eligibility check need the backend service; report
// ineligible and return no created review.
#[tauri::command]
pub fn hosted_review_get_creation_eligibility() -> Value {
    serde_json::json!({ "eligible": false })
}

#[tauri::command]
pub fn hosted_review_create() -> Option<Value> {
    None
}
