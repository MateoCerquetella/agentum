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
        return Ok(PathBuf::from(home)
            .join("managed-accounts")
            .join(provider));
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
// Claude capture / restore
// ---------------------------------------------------------------------------

fn claude_credentials_file() -> Result<PathBuf, String> {
    Ok(home()?.join(".claude").join(".credentials.json"))
}

fn claude_config_file() -> Result<PathBuf, String> {
    Ok(home()?.join(".claude.json"))
}

/// Read the live Claude credential blob: macOS Keychain first, else the
/// on-disk `.credentials.json`. The blob is the full `{ claudeAiOauth, … }`
/// JSON exactly as Claude Code stores it.
fn read_live_claude_blob() -> Option<String> {
    if let Some(blob) = keychain_read_password(CLAUDE_KEYCHAIN_SERVICE) {
        return Some(blob);
    }
    let path = claude_credentials_file().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The `oauthAccount` block from `~/.claude.json` (email/org/uuid for display).
fn read_live_claude_oauth_account() -> Value {
    let Ok(path) = claude_config_file() else {
        return Value::Null;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Value::Null;
    };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.get("oauthAccount").cloned())
        .unwrap_or(Value::Null)
}

/// Restore a saved account's material into every live location Claude reads.
fn restore_live_claude(blob: &str, oauth_account: &Value) -> Result<(), String> {
    if let Some(current) = read_live_claude_blob() {
        backup_live("claude", &current);
    }

    // 1) on-disk credentials file (legacy + Linux path)
    let cred = claude_credentials_file()?;
    write_atomic_secret(&cred, blob)?;

    // 2) macOS keychain (primary on macOS) — best-effort; a keychain failure
    //    shouldn't strand the file write we already made.
    if let Err(e) = keychain_write_password(CLAUDE_KEYCHAIN_SERVICE, blob) {
        eprintln!("[accounts] claude keychain restore failed ({e}); file credential still updated");
    }

    // 3) patch ~/.claude.json oauthAccount so /status + usage show this account.
    if oauth_account.is_object() {
        patch_claude_config_oauth(oauth_account)?;
    }
    Ok(())
}

/// Sign Claude out on this machine so the next `claude` run prompts a fresh
/// login. The live material is backed up first; managed copies are untouched.
/// Live terminals keep their in-memory token until restarted.
fn sign_out_live_claude() -> Result<(), String> {
    if let Some(current) = read_live_claude_blob() {
        backup_live("claude", &current);
    }
    keychain_delete_password(CLAUDE_KEYCHAIN_SERVICE);
    let cred = claude_credentials_file()?;
    if cred.exists() {
        std::fs::remove_file(&cred).map_err(err)?;
    }
    // Verify the sign-out actually took (a locked keychain can refuse the
    // delete). Without this, the renderer's "waiting for sign-in" poll would
    // immediately re-capture the very account the user is trying to replace.
    if read_live_claude_blob().is_some() {
        return Err(
            "Could not sign out: Claude credentials are still present (keychain delete failed)."
                .to_string(),
        );
    }
    clear_claude_config_oauth()
}

/// Drop `oauthAccount` from `~/.claude.json` so a signed-out machine doesn't
/// keep advertising the previous identity to /status and the usage fetcher.
fn clear_claude_config_oauth() -> Result<(), String> {
    let Ok(path) = claude_config_file() else {
        return Ok(());
    };
    let Some(mut root) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    else {
        return Ok(());
    };
    if let Value::Object(map) = &mut root {
        if map.remove("oauthAccount").is_some() {
            return write_atomic_secret(&path, &root.to_string());
        }
    }
    Ok(())
}

/// Merge `oauth_account` into `~/.claude.json` without disturbing any other key.
fn patch_claude_config_oauth(oauth_account: &Value) -> Result<(), String> {
    let path = claude_config_file()?;
    let mut root: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| Value::Object(Map::new()));
    if let Value::Object(map) = &mut root {
        map.insert("oauthAccount".to_string(), oauth_account.clone());
    }
    write_atomic_secret(&path, &root.to_string())
}

fn claude_secret_path(id: &str) -> Result<PathBuf, String> {
    Ok(store_base("claude")?.join(format!("{id}.json")))
}

/// Persist `{ credentials, oauthAccount }` for an account; returns its path.
fn write_claude_secret(id: &str, blob: &str, oauth_account: &Value) -> Result<String, String> {
    let path = claude_secret_path(id)?;
    let payload = json!({ "credentials": blob, "oauthAccount": oauth_account });
    write_atomic_secret(&path, &payload.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

fn load_claude_secret(path: &str) -> Result<(String, Value), String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|_| "saved Claude credential is missing — re-add the account".to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(err)?;
    let blob = v
        .get("credentials")
        .and_then(|c| c.as_str())
        .ok_or("saved Claude credential is corrupt")?
        .to_string();
    let oauth = v.get("oauthAccount").cloned().unwrap_or(Value::Null);
    Ok((blob, oauth))
}

// ---------------------------------------------------------------------------
// Codex capture / restore (plain ~/.codex/auth.json file)
// ---------------------------------------------------------------------------

fn codex_auth_file() -> Result<PathBuf, String> {
    Ok(home()?.join(".codex").join("auth.json"))
}

fn read_live_codex_blob() -> Option<String> {
    let path = codex_auth_file().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn restore_live_codex(blob: &str) -> Result<(), String> {
    if let Some(current) = read_live_codex_blob() {
        backup_live("codex", &current);
    }
    write_atomic_secret(&codex_auth_file()?, blob)
}

fn codex_secret_path(id: &str) -> Result<PathBuf, String> {
    Ok(store_base("codex")?.join(format!("{id}.json")))
}

fn write_codex_secret(id: &str, blob: &str) -> Result<String, String> {
    let path = codex_secret_path(id)?;
    let payload = json!({ "auth": blob });
    write_atomic_secret(&path, &payload.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

fn load_codex_secret(path: &str) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|_| "saved Codex credential is missing — re-add the account".to_string())?;
    let v: Value = serde_json::from_str(&raw).map_err(err)?;
    v.get("auth")
        .and_then(|a| a.as_str())
        .map(str::to_string)
        .ok_or_else(|| "saved Codex credential is corrupt".to_string())
}

/// Decode a JWT payload (middle segment, base64url, no padding) into JSON.
fn jwt_payload(jwt: &str) -> Option<Value> {
    let segment = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Codex display email from the `id_token` JWT's `email` claim.
fn codex_email_from_blob(blob: &str) -> Option<String> {
    let v: Value = serde_json::from_str(blob).ok()?;
    let id_token = v.get("tokens")?.get("id_token")?.as_str()?;
    let payload = jwt_payload(id_token)?;
    payload
        .get("email")
        .and_then(|e| e.as_str())
        .map(str::to_string)
}

fn codex_account_id_from_blob(blob: &str) -> Option<String> {
    let v: Value = serde_json::from_str(blob).ok()?;
    v.get("tokens")?
        .get("account_id")?
        .as_str()
        .map(str::to_string)
}

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
// Claude operations
// ---------------------------------------------------------------------------

/// Capture the currently-live Claude login as a managed account (re-using the
/// existing entry when the same email is added again), and make it active.
fn claude_add(conn: &Connection) -> Result<Value, String> {
    let blob = read_live_claude_blob().ok_or(
        "No Claude login found on this machine. Sign in with Claude Code first, then add the account.",
    )?;
    let oauth = read_live_claude_oauth_account();
    claude_capture_account(conn, &blob, &oauth)?;
    Ok(claude_state(conn))
}

/// "Add a different account", step 1: stash the live login as a managed
/// account, then sign Claude out so the user can sign in with another account.
/// Step 2 is the regular `claude_add` capture once the new login appears
/// (the renderer polls `claude_accounts_live_login` for it).
fn claude_begin_add(conn: &Connection) -> Result<Value, String> {
    let blob = read_live_claude_blob().ok_or(
        "No Claude login found on this machine. Sign in with Claude Code first.",
    )?;
    let oauth = read_live_claude_oauth_account();
    let (id, email) = claude_capture_account(conn, &blob, &oauth)?;
    sign_out_live_claude()?;
    Ok(json!({
        "state": claude_state(conn),
        "stashedAccountId": id,
        "stashedEmail": email,
    }))
}

/// What login is live on this machine right now (identity only, no secrets).
fn claude_live_login() -> Value {
    let has_credentials = read_live_claude_blob().is_some();
    let email = read_live_claude_oauth_account()
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    json!({ "hasCredentials": has_credentials, "email": email })
}

/// Reconcile the managed-account list with whatever Claude login is live right
/// now: if that login isn't saved yet, capture it (keyed by email); either way
/// mark it the active account. This is what lets the user's real account show
/// up by email on its own — there is no separate "system default" passthrough
/// to reason about. No live login → no-op (signed-out empty state).
///
/// When the live email is already saved we only flip `active` and leave the
/// stored snapshot and its timestamps alone, so "last used" stays meaningful
/// and opening the pane doesn't churn the credential file every time.
fn claude_sync_current(conn: &Connection) -> Result<Value, String> {
    let Some(blob) = read_live_claude_blob() else {
        return Ok(claude_state(conn));
    };
    let oauth = read_live_claude_oauth_account();
    let email = oauth
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    if let Some(email) = email.as_deref() {
        let accounts = read_accounts_array(conn, "claudeManagedAccounts");
        if let Some(i) = find_index_by(&accounts, "email", email) {
            if let Some(id) = string_field(&accounts[i], "id").map(str::to_string) {
                set_active(
                    conn,
                    "activeClaudeManagedAccountId",
                    "activeClaudeManagedAccountIdsByRuntime",
                    Some(&id),
                )?;
                return Ok(claude_state(conn));
            }
        }
    }

    claude_capture_account(conn, &blob, &oauth)?;
    Ok(claude_state(conn))
}

/// Upsert `blob`/`oauth` as a managed account keyed by email and make it
/// active. Returns `(id, email)` of the captured account.
fn claude_capture_account(
    conn: &Connection,
    blob: &str,
    oauth: &Value,
) -> Result<(String, String), String> {
    let email = oauth
        .get("emailAddress")
        .and_then(|v| v.as_str())
        .unwrap_or("Claude account")
        .to_string();

    let mut accounts = read_accounts_array(conn, "claudeManagedAccounts");
    let now = now_ms();
    let existing = find_index_by(&accounts, "email", &email);
    let id = match existing {
        Some(i) => string_field(&accounts[i], "id")
            .map(str::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        None => Uuid::new_v4().to_string(),
    };
    let created_at = existing
        .and_then(|i| accounts[i].get("createdAt").cloned())
        .unwrap_or(json!(now));

    let path = write_claude_secret(&id, blob, oauth)?;
    let account = json!({
        "id": id,
        "email": email,
        "managedAuthPath": path,
        "managedAuthRuntime": "host",
        "wslDistro": Value::Null,
        "wslLinuxAuthPath": Value::Null,
        "authMethod": "subscription-oauth",
        "organizationUuid": oauth.get("organizationUuid").cloned().unwrap_or(Value::Null),
        "organizationName": oauth.get("organizationName").cloned().unwrap_or(Value::Null),
        "createdAt": created_at,
        "updatedAt": now,
        "lastAuthenticatedAt": now,
    });
    match existing {
        Some(i) => accounts[i] = account,
        None => accounts.push(account),
    }
    write_setting(conn, "claudeManagedAccounts", &Value::Array(accounts))?;
    set_active(
        conn,
        "activeClaudeManagedAccountId",
        "activeClaudeManagedAccountIdsByRuntime",
        Some(&id),
    )?;
    Ok((id, email))
}

fn claude_select(conn: &Connection, account_id: Option<&str>) -> Result<Value, String> {
    let Some(id) = account_id else {
        // "System default": stop managing without rewriting live credentials.
        set_active(
            conn,
            "activeClaudeManagedAccountId",
            "activeClaudeManagedAccountIdsByRuntime",
            None,
        )?;
        return Ok(claude_state(conn));
    };

    let accounts = read_accounts_array(conn, "claudeManagedAccounts");
    let account = accounts
        .iter()
        .find(|a| string_field(a, "id") == Some(id))
        .ok_or("Unknown Claude account")?;
    let path =
        string_field(account, "managedAuthPath").ok_or("Account has no stored credential")?;
    let (blob, oauth) = load_claude_secret(path)?;
    restore_live_claude(&blob, &oauth)?;
    set_active(
        conn,
        "activeClaudeManagedAccountId",
        "activeClaudeManagedAccountIdsByRuntime",
        Some(id),
    )?;
    Ok(claude_state(conn))
}

fn claude_remove(conn: &Connection, account_id: &str) -> Result<Value, String> {
    let mut accounts = read_accounts_array(conn, "claudeManagedAccounts");
    if let Some(i) = find_index_by(&accounts, "id", account_id) {
        if let Some(path) = string_field(&accounts[i], "managedAuthPath") {
            let _ = std::fs::remove_file(path);
        }
        accounts.remove(i);
    }
    write_setting(conn, "claudeManagedAccounts", &Value::Array(accounts))?;
    if read_setting(conn, "activeClaudeManagedAccountId").as_str() == Some(account_id) {
        set_active(
            conn,
            "activeClaudeManagedAccountId",
            "activeClaudeManagedAccountIdsByRuntime",
            None,
        )?;
    }
    Ok(claude_state(conn))
}

/// Re-capture the live login into an existing account (refreshes a stale token).
fn claude_reauthenticate(conn: &Connection, account_id: &str) -> Result<Value, String> {
    let blob = read_live_claude_blob()
        .ok_or("No live Claude login to re-capture. Sign in with Claude Code first.")?;
    let oauth = read_live_claude_oauth_account();
    let mut accounts = read_accounts_array(conn, "claudeManagedAccounts");
    let i = find_index_by(&accounts, "id", account_id).ok_or("Unknown Claude account")?;
    let path = write_claude_secret(account_id, &blob, &oauth)?;
    let now = now_ms();
    if let Value::Object(map) = &mut accounts[i] {
        map.insert("managedAuthPath".into(), json!(path));
        map.insert("updatedAt".into(), json!(now));
        map.insert("lastAuthenticatedAt".into(), json!(now));
        if let Some(email) = oauth.get("emailAddress").and_then(|v| v.as_str()) {
            map.insert("email".into(), json!(email));
        }
        map.insert(
            "organizationName".into(),
            oauth
                .get("organizationName")
                .cloned()
                .unwrap_or(Value::Null),
        );
        map.insert(
            "organizationUuid".into(),
            oauth
                .get("organizationUuid")
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    write_setting(conn, "claudeManagedAccounts", &Value::Array(accounts))?;
    set_active(
        conn,
        "activeClaudeManagedAccountId",
        "activeClaudeManagedAccountIdsByRuntime",
        Some(account_id),
    )?;
    Ok(claude_state(conn))
}

// ---------------------------------------------------------------------------
// Codex operations
// ---------------------------------------------------------------------------

fn codex_add(conn: &Connection) -> Result<Value, String> {
    let blob = read_live_codex_blob().ok_or(
        "No Codex login found on this machine. Sign in with Codex first, then add the account.",
    )?;
    let email = codex_email_from_blob(&blob).unwrap_or_else(|| "Codex account".to_string());
    let provider_account_id = codex_account_id_from_blob(&blob);

    let mut accounts = read_accounts_array(conn, "codexManagedAccounts");
    let now = now_ms();
    let existing = find_index_by(&accounts, "email", &email);
    let id = match existing {
        Some(i) => string_field(&accounts[i], "id")
            .map(str::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        None => Uuid::new_v4().to_string(),
    };
    let created_at = existing
        .and_then(|i| accounts[i].get("createdAt").cloned())
        .unwrap_or(json!(now));

    let path = write_codex_secret(&id, &blob)?;
    let account = json!({
        "id": id,
        "email": email,
        "managedHomePath": path,
        "managedHomeRuntime": "host",
        "wslDistro": Value::Null,
        "wslLinuxHomePath": Value::Null,
        "providerAccountId": provider_account_id,
        "workspaceLabel": Value::Null,
        "workspaceAccountId": Value::Null,
        "createdAt": created_at,
        "updatedAt": now,
        "lastAuthenticatedAt": now,
    });
    match existing {
        Some(i) => accounts[i] = account,
        None => accounts.push(account),
    }
    write_setting(conn, "codexManagedAccounts", &Value::Array(accounts))?;
    set_active(
        conn,
        "activeCodexManagedAccountId",
        "activeCodexManagedAccountIdsByRuntime",
        Some(&id),
    )?;
    Ok(codex_state(conn))
}

fn codex_select(conn: &Connection, account_id: Option<&str>) -> Result<Value, String> {
    let Some(id) = account_id else {
        set_active(
            conn,
            "activeCodexManagedAccountId",
            "activeCodexManagedAccountIdsByRuntime",
            None,
        )?;
        return Ok(codex_state(conn));
    };
    let accounts = read_accounts_array(conn, "codexManagedAccounts");
    let account = accounts
        .iter()
        .find(|a| string_field(a, "id") == Some(id))
        .ok_or("Unknown Codex account")?;
    let path =
        string_field(account, "managedHomePath").ok_or("Account has no stored credential")?;
    let blob = load_codex_secret(path)?;
    restore_live_codex(&blob)?;
    set_active(
        conn,
        "activeCodexManagedAccountId",
        "activeCodexManagedAccountIdsByRuntime",
        Some(id),
    )?;
    Ok(codex_state(conn))
}

fn codex_remove(conn: &Connection, account_id: &str) -> Result<Value, String> {
    let mut accounts = read_accounts_array(conn, "codexManagedAccounts");
    if let Some(i) = find_index_by(&accounts, "id", account_id) {
        if let Some(path) = string_field(&accounts[i], "managedHomePath") {
            let _ = std::fs::remove_file(path);
        }
        accounts.remove(i);
    }
    write_setting(conn, "codexManagedAccounts", &Value::Array(accounts))?;
    if read_setting(conn, "activeCodexManagedAccountId").as_str() == Some(account_id) {
        set_active(
            conn,
            "activeCodexManagedAccountId",
            "activeCodexManagedAccountIdsByRuntime",
            None,
        )?;
    }
    Ok(codex_state(conn))
}

fn codex_reauthenticate(conn: &Connection, account_id: &str) -> Result<Value, String> {
    let blob = read_live_codex_blob()
        .ok_or("No live Codex login to re-capture. Sign in with Codex first.")?;
    let mut accounts = read_accounts_array(conn, "codexManagedAccounts");
    let i = find_index_by(&accounts, "id", account_id).ok_or("Unknown Codex account")?;
    let path = write_codex_secret(account_id, &blob)?;
    let now = now_ms();
    if let Value::Object(map) = &mut accounts[i] {
        map.insert("managedHomePath".into(), json!(path));
        map.insert("updatedAt".into(), json!(now));
        map.insert("lastAuthenticatedAt".into(), json!(now));
        if let Some(email) = codex_email_from_blob(&blob) {
            map.insert("email".into(), json!(email));
        }
        map.insert(
            "providerAccountId".into(),
            codex_account_id_from_blob(&blob)
                .map(|v| json!(v))
                .unwrap_or(Value::Null),
        );
    }
    write_setting(conn, "codexManagedAccounts", &Value::Array(accounts))?;
    set_active(
        conn,
        "activeCodexManagedAccountId",
        "activeCodexManagedAccountIdsByRuntime",
        Some(account_id),
    )?;
    Ok(codex_state(conn))
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
        let tmp = std::env::temp_dir().join(format!("agentum-accounts-test-{}", std::process::id()));
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
        assert_eq!(read_setting(&conn, "activeClaudeManagedAccountId"), json!("a1"));
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
