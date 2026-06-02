// The "star us on GitHub" nag is renderer-driven; its dismissal state isn't tracked
// backend-side, so these are no-ops.

#[tauri::command]
pub fn star_nag_dismiss() {}

#[tauri::command]
pub fn star_nag_complete() {}

#[tauri::command]
pub fn star_nag_force_show() {}
