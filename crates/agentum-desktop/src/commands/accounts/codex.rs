//! Codex CLI account capture / restore / operations (`~/.codex/auth.json`).
//! Parallel to `claude`; shares the parent `accounts` helpers via `use super::*`.
//! The op fns are `pub(super)` for the parent tauri commands + tests.

use super::*;

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

/// Sign Codex out on this machine (back up first) so a fresh `codex login`
/// prompts a different account. Verified so the renderer's poll can't re-capture
/// the same account.
fn sign_out_live_codex() -> Result<(), String> {
    if let Some(current) = read_live_codex_blob() {
        backup_live("codex", &current);
    }
    let path = codex_auth_file()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(err)?;
    }
    if read_live_codex_blob().is_some() {
        return Err("Could not sign out: Codex credentials are still present.".to_string());
    }
    Ok(())
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
pub(super) fn codex_email_from_blob(blob: &str) -> Option<String> {
    let v: Value = serde_json::from_str(blob).ok()?;
    let id_token = v.get("tokens")?.get("id_token")?.as_str()?;
    let payload = jwt_payload(id_token)?;
    payload
        .get("email")
        .and_then(|e| e.as_str())
        .map(str::to_string)
}

pub(super) fn codex_account_id_from_blob(blob: &str) -> Option<String> {
    let v: Value = serde_json::from_str(blob).ok()?;
    v.get("tokens")?
        .get("account_id")?
        .as_str()
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Codex operations
// ---------------------------------------------------------------------------

pub(super) fn codex_add(conn: &Connection) -> Result<Value, String> {
    let blob = read_live_codex_blob().ok_or(
        "No Codex login found on this machine. Sign in with Codex first, then add the account.",
    )?;
    codex_capture_account(conn, &blob)?;
    Ok(codex_state(conn))
}

/// Upsert the live Codex `blob` as a managed account keyed by email and make it
/// active. Returns `(id, email)`. Mirrors `claude_capture_account`.
pub(super) fn codex_capture_account(
    conn: &Connection,
    blob: &str,
) -> Result<(String, String), String> {
    let email = codex_email_from_blob(blob).unwrap_or_else(|| "Codex account".to_string());
    let provider_account_id = codex_account_id_from_blob(blob);

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

    let path = write_codex_secret(&id, blob)?;
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
    write_account_metadata(conn, "codexManagedAccounts", accounts)?;
    set_active(
        conn,
        "activeCodexManagedAccountId",
        "activeCodexManagedAccountIdsByRuntime",
        Some(&id),
    )?;
    Ok((id, email))
}

/// "Add a different Codex account": stash the live login, then sign Codex out so
/// `codex login` (localhost-callback flow) can sign in with another account.
pub(super) fn codex_begin_add(conn: &Connection) -> Result<Value, String> {
    let blob = read_live_codex_blob()
        .ok_or("No Codex login found on this machine. Sign in with Codex first.")?;
    let (id, email) = codex_capture_account(conn, &blob)?;
    sign_out_live_codex()?;
    Ok(json!({
        "state": codex_state(conn),
        "stashedAccountId": id,
        "stashedEmail": email,
    }))
}

/// What Codex login is live right now (identity only, no secrets).
pub(super) fn codex_live_login() -> Value {
    match read_live_codex_blob() {
        Some(blob) => json!({ "hasCredentials": true, "email": codex_email_from_blob(&blob) }),
        None => json!({ "hasCredentials": false, "email": Value::Null }),
    }
}

/// Save the live Codex login if new and mark it active, so the real account
/// shows by email on pane open (mirrors `claude_sync_current`).
pub(super) fn codex_sync_current(conn: &Connection) -> Result<Value, String> {
    let Some(blob) = read_live_codex_blob() else {
        return Ok(codex_state(conn));
    };
    if let Some(email) = codex_email_from_blob(&blob) {
        let accounts = read_accounts_array(conn, "codexManagedAccounts");
        if let Some(i) = find_index_by(&accounts, "email", &email) {
            if let Some(id) = string_field(&accounts[i], "id").map(str::to_string) {
                set_active(
                    conn,
                    "activeCodexManagedAccountId",
                    "activeCodexManagedAccountIdsByRuntime",
                    Some(&id),
                )?;
                return Ok(codex_state(conn));
            }
        }
    }
    codex_capture_account(conn, &blob)?;
    Ok(codex_state(conn))
}

pub(super) fn codex_select(conn: &Connection, account_id: Option<&str>) -> Result<Value, String> {
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

pub(super) fn codex_remove(conn: &Connection, account_id: &str) -> Result<Value, String> {
    let mut accounts = read_accounts_array(conn, "codexManagedAccounts");
    if let Some(account) = accounts
        .iter()
        .find(|account| string_field(account, "id") == Some(account_id))
    {
        if let Some(path) = string_field(account, "managedHomePath") {
            let _ = std::fs::remove_file(path);
        }
    }
    accounts.retain(|account| string_field(account, "id") != Some(account_id));
    write_account_metadata(conn, "codexManagedAccounts", accounts)?;
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

pub(super) fn codex_reauthenticate(conn: &Connection, account_id: &str) -> Result<Value, String> {
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
    write_account_metadata(conn, "codexManagedAccounts", accounts)?;
    set_active(
        conn,
        "activeCodexManagedAccountId",
        "activeCodexManagedAccountIdsByRuntime",
        Some(account_id),
    )?;
    Ok(codex_state(conn))
}
