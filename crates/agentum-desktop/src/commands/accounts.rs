//! Managed Claude/Codex accounts — credential-swap model (à la `claude-swap`).
//!
//! Claude Code keeps all conversations under one shared config root, so Agentum
//! does NOT fork a per-account `CLAUDE_CONFIG_DIR`. Instead it captures the
//! *currently live* auth material and stores it per account; "switching" writes
//! a saved account's material back into the live location. Live terminals keep
//! their old token in memory, which is why the UI prompts a restart after a
//! switch.
//!
//! Capture/restore targets:
//! - Claude: macOS Keychain (`Claude Code-credentials`) AND
//!   `~/.claude/.credentials.json` (same JSON shape), plus the `oauthAccount`
//!   block in `~/.claude.json` (email/org for display).
//! - Codex: the plain `~/.codex/auth.json` file.
//!
//! Account metadata is persisted in the settings KV store under
//! `claudeManagedAccounts` / `codexManagedAccounts` (matching the renderer's
//! `ManagedAccount` types); the secret material lives in
//! `<app-data>/Agentum/managed-accounts/<provider>/<id>.json` (0600).
//! Desktop is host-only — WSL runtime fields are emitted as `host`/null.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use tauri::State;
use uuid::Uuid;

use crate::state::AppState;

// Claude and Codex are parallel, independent credential integrations; each lives
// in its own child module and reaches the shared keychain/fs/settings helpers +
// state-assembly here via `use super::*`. Their op fns are `pub(super)`, imported
// back here so the tauri commands below call them unqualified.
mod claude;
mod codex;
use claude::*;
use codex::*;

const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

/// Per-provider secret store: `<app-data>/Agentum/managed-accounts/<provider>`.
/// `AGENTUM_HOME` overrides the base so tests never write into real app data
/// (same isolation convention as the CLI crates).
fn store_base(provider: &str) -> Result<PathBuf, String> {
    if let Some(home) = std::env::var_os("AGENTUM_HOME") {
        return Ok(PathBuf::from(home).join("managed-accounts").join(provider));
    }
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or_else(|| "no app data directory".to_string())?;
    Ok(base.join("Agentum").join("managed-accounts").join(provider))
}

// ---------------------------------------------------------------------------
// Settings KV helpers (one row per top-level GlobalSettings key, JSON-encoded).
// ---------------------------------------------------------------------------

fn read_setting(conn: &Connection, key: &str) -> Value {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or(Value::Null)
}

fn write_setting(conn: &Connection, key: &str, value: &Value) -> Result<(), String> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value.to_string()],
    )
    .map_err(err)?;
    Ok(())
}

fn read_accounts_array(conn: &Connection, key: &str) -> Vec<Value> {
    match read_setting(conn, key) {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

/// Write `contents` to `path` via a temp file + rename so a crash mid-write
/// never truncates the live credential. Secret files are chmod 0600 on unix.
fn write_atomic_secret(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(err)?;
    }
    std::fs::rename(&tmp, path).map_err(err)?;
    Ok(())
}

/// Snapshot the current live material before overwriting it, so a bad swap is
/// recoverable from `<store>/.backups/`.
fn backup_live(provider: &str, contents: &str) {
    if let Ok(base) = store_base(provider) {
        let path = base
            .join(".backups")
            .join(format!("live-{}.json", now_ms()));
        let _ = write_atomic_secret(&path, contents);
    }
}

// ---------------------------------------------------------------------------
// macOS Keychain (Claude Code stores its credentials there on macOS).
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn keychain_read_password(service: &str) -> Option<String> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Parse the `acct` attribute of the existing entry so an update targets the
/// same single keychain item instead of creating a duplicate.
#[cfg(target_os = "macos")]
fn keychain_read_account(service: &str) -> Option<String> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"acct\"<blob>=") {
            // Format: "acct"<blob>="value"
            let v = rest.trim().trim_matches('"');
            if !v.is_empty() && v != "<NULL>" {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn keychain_write_password(service: &str, password: &str) -> Result<(), String> {
    let account =
        keychain_read_account(service).unwrap_or_else(|| std::env::var("USER").unwrap_or_default());
    // -U updates the existing item in place (matched by -s/-a) rather than erroring.
    let status = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            service,
            "-a",
            &account,
            "-w",
            password,
        ])
        .status()
        .map_err(err)?;
    if !status.success() {
        return Err("failed to update macOS keychain entry".to_string());
    }
    Ok(())
}

/// Remove the keychain item entirely so Claude Code prompts a fresh login.
/// A missing item is fine — we only need it gone.
#[cfg(target_os = "macos")]
fn keychain_delete_password(service: &str) {
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", service])
        .output();
}

#[cfg(not(target_os = "macos"))]
fn keychain_read_password(_service: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
fn keychain_write_password(_service: &str, _password: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn keychain_delete_password(_service: &str) {}

// ---------------------------------------------------------------------------
// State assembly (renderer's *RateLimitAccountsState shape).
// ---------------------------------------------------------------------------

fn string_field<'a>(account: &'a Value, key: &str) -> Option<&'a str> {
    account.get(key).and_then(|v| v.as_str())
}

fn claude_summary(account: &Value) -> Value {
    json!({
        "id": string_field(account, "id"),
        "email": string_field(account, "email"),
        "managedAuthRuntime": account.get("managedAuthRuntime").cloned().unwrap_or(json!("host")),
        "wslDistro": account.get("wslDistro").cloned().unwrap_or(Value::Null),
        "authMethod": account.get("authMethod").cloned().unwrap_or(json!("subscription-oauth")),
        "organizationUuid": account.get("organizationUuid").cloned().unwrap_or(Value::Null),
        "organizationName": account.get("organizationName").cloned().unwrap_or(Value::Null),
        "createdAt": account.get("createdAt").cloned().unwrap_or(json!(0)),
        "updatedAt": account.get("updatedAt").cloned().unwrap_or(json!(0)),
        "lastAuthenticatedAt": account.get("lastAuthenticatedAt").cloned().unwrap_or(json!(0)),
    })
}

fn codex_summary(account: &Value) -> Value {
    json!({
        "id": string_field(account, "id"),
        "email": string_field(account, "email"),
        "managedHomeRuntime": account.get("managedHomeRuntime").cloned().unwrap_or(json!("host")),
        "wslDistro": account.get("wslDistro").cloned().unwrap_or(Value::Null),
        "providerAccountId": account.get("providerAccountId").cloned().unwrap_or(Value::Null),
        "workspaceLabel": account.get("workspaceLabel").cloned().unwrap_or(Value::Null),
        "workspaceAccountId": account.get("workspaceAccountId").cloned().unwrap_or(Value::Null),
        "createdAt": account.get("createdAt").cloned().unwrap_or(json!(0)),
        "updatedAt": account.get("updatedAt").cloned().unwrap_or(json!(0)),
        "lastAuthenticatedAt": account.get("lastAuthenticatedAt").cloned().unwrap_or(json!(0)),
    })
}

fn account_state(
    conn: &Connection,
    accounts_key: &str,
    active_key: &str,
    summarize: fn(&Value) -> Value,
) -> Value {
    let accounts = read_accounts_array(conn, accounts_key);
    let active = read_setting(conn, active_key);
    let active_id = active.as_str();
    json!({
        "accounts": accounts.iter().map(summarize).collect::<Vec<_>>(),
        "activeAccountId": active.clone(),
        "activeAccountIdsByRuntime": { "host": active_id, "wsl": {} },
    })
}

fn claude_state(conn: &Connection) -> Value {
    account_state(
        conn,
        "claudeManagedAccounts",
        "activeClaudeManagedAccountId",
        claude_summary,
    )
}

fn codex_state(conn: &Connection) -> Value {
    account_state(
        conn,
        "codexManagedAccounts",
        "activeCodexManagedAccountId",
        codex_summary,
    )
}

fn set_active(
    conn: &Connection,
    active_key: &str,
    runtime_key: &str,
    id: Option<&str>,
) -> Result<(), String> {
    let id_value = id.map(|s| json!(s)).unwrap_or(Value::Null);
    write_setting(conn, active_key, &id_value)?;
    write_setting(conn, runtime_key, &json!({ "host": id_value, "wsl": {} }))?;
    Ok(())
}

fn find_index_by(accounts: &[Value], key: &str, value: &str) -> Option<usize> {
    accounts
        .iter()
        .position(|a| a.get(key).and_then(|v| v.as_str()) == Some(value))
}

// ---------------------------------------------------------------------------
// Request plumbing
// ---------------------------------------------------------------------------

/// A string field on the JSON request body; `null`/absent → `None`.
fn body_str(request: &tauri::ipc::Request<'_>, key: &str) -> Option<String> {
    match request.body() {
        tauri::ipc::InvokeBody::Json(value) => {
            value.get(key).and_then(|v| v.as_str()).map(str::to_string)
        }
        _ => None,
    }
}

/// Run a blocking account operation against the settings DB off the async runtime.
async fn run_blocking<F>(state: &State<'_, AppState>, op: F) -> Result<Value, String>
where
    F: FnOnce(&Connection) -> Result<Value, String> + Send + 'static,
{
    let database = state.settings_db.clone();
    tokio::task::spawn_blocking(move || {
        let connection = database.lock();
        op(&connection)
    })
    .await
    .map_err(err)?
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn claude_accounts_list(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, |conn| Ok(claude_state(conn))).await
}

#[tauri::command]
pub async fn claude_accounts_add(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, claude_add).await
}

#[tauri::command]
pub async fn claude_accounts_begin_add(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, claude_begin_add).await
}

/// No DB involved — reads the live keychain/files only. Polled by the renderer
/// while it waits for the user to finish a fresh `claude` sign-in.
#[tauri::command]
pub async fn claude_accounts_live_login() -> Result<Value, String> {
    tokio::task::spawn_blocking(claude_live_login)
        .await
        .map_err(err)
}

/// Called when the accounts pane opens: saves the live login if needed and
/// marks it active so the user's real account always shows up by email.
#[tauri::command]
pub async fn claude_accounts_sync_current(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, claude_sync_current).await
}

#[tauri::command]
pub async fn claude_accounts_select(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let account_id = body_str(&request, "accountId");
    run_blocking(&state, move |conn| {
        claude_select(conn, account_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn claude_accounts_remove(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let account_id = body_str(&request, "accountId").ok_or("accountId is required")?;
    run_blocking(&state, move |conn| claude_remove(conn, &account_id)).await
}

#[tauri::command]
pub async fn claude_accounts_reauthenticate(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let account_id = body_str(&request, "accountId").ok_or("accountId is required")?;
    run_blocking(&state, move |conn| claude_reauthenticate(conn, &account_id)).await
}

#[tauri::command]
pub async fn codex_accounts_list(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, |conn| Ok(codex_state(conn))).await
}

#[tauri::command]
pub async fn codex_accounts_add(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, codex_add).await
}

#[tauri::command]
pub async fn codex_accounts_begin_add(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, codex_begin_add).await
}

#[tauri::command]
pub async fn codex_accounts_live_login() -> Result<Value, String> {
    tokio::task::spawn_blocking(codex_live_login)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn codex_accounts_sync_current(state: State<'_, AppState>) -> Result<Value, String> {
    run_blocking(&state, codex_sync_current).await
}

#[tauri::command]
pub async fn codex_accounts_select(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let account_id = body_str(&request, "accountId");
    run_blocking(&state, move |conn| {
        codex_select(conn, account_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn codex_accounts_remove(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let account_id = body_str(&request, "accountId").ok_or("accountId is required")?;
    run_blocking(&state, move |conn| codex_remove(conn, &account_id)).await
}

#[tauri::command]
pub async fn codex_accounts_reauthenticate(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let account_id = body_str(&request, "accountId").ok_or("accountId is required")?;
    run_blocking(&state, move |conn| codex_reauthenticate(conn, &account_id)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        conn
    }

    #[test]
    fn empty_state_shape_matches_contract() {
        let conn = mem_db();
        let state = claude_state(&conn);
        assert_eq!(state["accounts"], json!([]));
        assert_eq!(state["activeAccountId"], Value::Null);
        assert_eq!(state["activeAccountIdsByRuntime"]["host"], Value::Null);
    }

    #[test]
    fn summary_omits_secret_path_and_keeps_fields() {
        let account = json!({
            "id": "a1",
            "email": "user@example.com",
            "managedAuthPath": "/secret/a1.json",
            "authMethod": "subscription-oauth",
            "organizationName": "Acme",
            "createdAt": 1,
            "updatedAt": 2,
            "lastAuthenticatedAt": 3,
        });
        let summary = claude_summary(&account);
        assert_eq!(summary["email"], "user@example.com");
        assert_eq!(summary["organizationName"], "Acme");
        // The secret filesystem path must never reach the renderer summary.
        assert!(summary.get("managedAuthPath").is_none());
    }

    #[test]
    fn set_active_writes_both_scalar_and_runtime_keys() {
        let conn = mem_db();
        set_active(
            &conn,
            "activeClaudeManagedAccountId",
            "activeClaudeManagedAccountIdsByRuntime",
            Some("a1"),
        )
        .unwrap();
        assert_eq!(
            read_setting(&conn, "activeClaudeManagedAccountId"),
            json!("a1")
        );
        assert_eq!(
            read_setting(&conn, "activeClaudeManagedAccountIdsByRuntime")["host"],
            json!("a1")
        );
    }

    #[test]
    fn codex_email_and_account_id_decoded_from_token() {
        // id_token payload {"email":"dev@example.com"} as base64url(no pad).
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"dev@example.com"}"#);
        let blob = json!({
            "tokens": { "id_token": format!("h.{payload}.s"), "account_id": "acct_123" }
        })
        .to_string();
        assert_eq!(
            codex_email_from_blob(&blob).as_deref(),
            Some("dev@example.com")
        );
        assert_eq!(
            codex_account_id_from_blob(&blob).as_deref(),
            Some("acct_123")
        );
    }

    #[test]
    fn capture_upserts_by_email_and_sets_active() {
        // Isolate the secret store; this is the only test in the crate that
        // sets AGENTUM_HOME, so there is no cross-test race.
        let tmp =
            std::env::temp_dir().join(format!("agentum-accounts-test-{}", std::process::id()));
        std::env::set_var("AGENTUM_HOME", &tmp);
        let conn = mem_db();

        let oauth_a = json!({ "emailAddress": "a@example.com" });
        let (id_a, email_a) = claude_capture_account(&conn, r#"{"blob":1}"#, &oauth_a).unwrap();
        assert_eq!(email_a, "a@example.com");

        // Capturing the same email again must reuse the slot, not duplicate it
        // (the "Add gave me the same account twice" bug class).
        let (id_a2, _) = claude_capture_account(&conn, r#"{"blob":2}"#, &oauth_a).unwrap();
        assert_eq!(id_a, id_a2);
        assert_eq!(read_accounts_array(&conn, "claudeManagedAccounts").len(), 1);

        // A different email gets its own slot and becomes the active account.
        let oauth_b = json!({ "emailAddress": "b@example.com" });
        let (id_b, _) = claude_capture_account(&conn, r#"{"blob":3}"#, &oauth_b).unwrap();
        assert_ne!(id_a, id_b);
        let accounts = read_accounts_array(&conn, "claudeManagedAccounts");
        assert_eq!(accounts.len(), 2);
        assert_eq!(
            read_setting(&conn, "activeClaudeManagedAccountId"),
            json!(id_b)
        );

        // The stored secret round-trips for the new account.
        let path = string_field(&accounts[1], "managedAuthPath").unwrap();
        let (blob, _) = load_claude_secret(path).unwrap();
        assert_eq!(blob, r#"{"blob":3}"#);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sync_current_sets_active_for_already_saved_email_without_touching_timestamps() {
        let conn = mem_db();
        // An account saved earlier, currently NOT active.
        write_setting(
            &conn,
            "claudeManagedAccounts",
            &json!([{
                "id": "a1",
                "email": "a@example.com",
                "managedAuthPath": "/tmp/none.json",
                "updatedAt": 111,
                "lastAuthenticatedAt": 111,
            }]),
        )
        .unwrap();
        write_setting(&conn, "activeClaudeManagedAccountId", &Value::Null).unwrap();

        // Stand in for "this email is the live login" by exercising the
        // already-saved branch directly (read_live_* hit the real machine).
        let accounts = read_accounts_array(&conn, "claudeManagedAccounts");
        let i = find_index_by(&accounts, "email", "a@example.com").unwrap();
        let id = string_field(&accounts[i], "id").unwrap().to_string();
        set_active(
            &conn,
            "activeClaudeManagedAccountId",
            "activeClaudeManagedAccountIdsByRuntime",
            Some(&id),
        )
        .unwrap();

        // Active flipped, but the stored timestamps are untouched.
        assert_eq!(
            read_setting(&conn, "activeClaudeManagedAccountId"),
            json!("a1")
        );
        let after = read_accounts_array(&conn, "claudeManagedAccounts");
        assert_eq!(after[0]["updatedAt"], json!(111));
        assert_eq!(after[0]["lastAuthenticatedAt"], json!(111));
    }

    #[test]
    fn select_system_default_clears_active_without_touching_accounts() {
        let conn = mem_db();
        write_setting(
            &conn,
            "claudeManagedAccounts",
            &json!([{ "id": "a1", "email": "x" }]),
        )
        .unwrap();
        write_setting(&conn, "activeClaudeManagedAccountId", &json!("a1")).unwrap();
        let state = claude_select(&conn, None).unwrap();
        assert_eq!(state["activeAccountId"], Value::Null);
        // Accounts list is untouched.
        assert_eq!(state["accounts"].as_array().unwrap().len(), 1);
    }
}
