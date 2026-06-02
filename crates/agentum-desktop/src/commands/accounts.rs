use serde_json::{json, Value};

// Managed-account state (Claude/Codex) shares the same shape. Account management
// needs OAuth login flows that aren't ported, so every method returns the empty
// state ({ accounts: [], activeAccountId: null }).
fn empty_state() -> Value {
    json!({ "accounts": [], "activeAccountId": null })
}

#[tauri::command]
pub fn claude_accounts_list() -> Value {
    empty_state()
}

#[tauri::command]
pub fn claude_accounts_select() -> Value {
    empty_state()
}

#[tauri::command]
pub fn claude_accounts_add() -> Value {
    empty_state()
}

#[tauri::command]
pub fn claude_accounts_reauthenticate() -> Value {
    empty_state()
}

#[tauri::command]
pub fn claude_accounts_remove() -> Value {
    empty_state()
}

#[tauri::command]
pub fn codex_accounts_list() -> Value {
    empty_state()
}

#[tauri::command]
pub fn codex_accounts_select() -> Value {
    empty_state()
}

#[tauri::command]
pub fn codex_accounts_add() -> Value {
    empty_state()
}

#[tauri::command]
pub fn codex_accounts_reauthenticate() -> Value {
    empty_state()
}

#[tauri::command]
pub fn codex_accounts_remove() -> Value {
    empty_state()
}
