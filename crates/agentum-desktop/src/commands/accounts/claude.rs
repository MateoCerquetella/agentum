//! Claude Code account capture / restore / operations (Keychain-backed OAuth).
//! Parallel to `codex`; shares the keychain/fs/settings helpers + state assembly
//! in the parent `accounts` module via `use super::*`. The op fns are `pub(super)`
//! so the parent tauri commands + tests can drive them.

use super::*;

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

pub(super) fn load_claude_secret(path: &str) -> Result<(String, Value), String> {
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
// Claude operations
// ---------------------------------------------------------------------------

/// Capture the currently-live Claude login as a managed account (re-using the
/// existing entry when the same email is added again), and make it active.
pub(super) fn claude_add(conn: &Connection) -> Result<Value, String> {
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
pub(super) fn claude_begin_add(conn: &Connection) -> Result<Value, String> {
    let blob = read_live_claude_blob()
        .ok_or("No Claude login found on this machine. Sign in with Claude Code first.")?;
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
pub(super) fn claude_live_login() -> Value {
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
pub(super) fn claude_sync_current(conn: &Connection) -> Result<Value, String> {
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
pub(super) fn claude_capture_account(
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

pub(super) fn claude_select(conn: &Connection, account_id: Option<&str>) -> Result<Value, String> {
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

pub(super) fn claude_remove(conn: &Connection, account_id: &str) -> Result<Value, String> {
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
pub(super) fn claude_reauthenticate(conn: &Connection, account_id: &str) -> Result<Value, String> {
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
