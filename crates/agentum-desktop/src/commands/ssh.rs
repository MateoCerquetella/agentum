use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use dirs::home_dir;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// Mirrors SshTarget in agentum/src/shared/ssh-types.ts. `extra` round-trips fields
// this layer doesn't manage yet (jumpHost, proxyCommand, portForwards, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTarget {
    id: String,
    label: String,
    host: String,
    port: u32,
    username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_file: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn targets_path() -> Result<PathBuf, String> {
    Ok(home_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(".agentum")
        .join("ssh-targets.json"))
}

fn read_targets() -> Result<Vec<SshTarget>, String> {
    let path = targets_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(map_err)?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn write_targets(targets: &[SshTarget]) -> Result<(), String> {
    let path = targets_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized = serde_json::to_string_pretty(targets).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

// Legacy host-alias list (kept registered; the renderer uses listTargets instead).
#[tauri::command]
pub async fn ssh_list_hosts() -> Result<Vec<String>, String> {
    let config_path = home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh")
        .join("config");
    if !config_path.exists() {
        return Ok(Vec::new());
    }
    let content = tokio::fs::read_to_string(config_path)
        .await
        .map_err(map_err)?;
    let mut hosts = BTreeSet::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("Host ") {
            continue;
        }
        for host in trimmed.split_whitespace().skip(1) {
            if host.contains('*') || host.contains('?') || host.starts_with('!') {
                continue;
            }
            hosts.insert(host.to_string());
        }
    }
    Ok(hosts.into_iter().collect())
}

#[tauri::command]
pub fn ssh_list_targets() -> Result<Vec<SshTarget>, String> {
    read_targets()
}

#[tauri::command]
pub fn ssh_add_target(target: Value) -> Result<SshTarget, String> {
    let mut object = target
        .as_object()
        .cloned()
        .ok_or_else(|| "invalid target".to_string())?;
    object.insert("id".into(), Value::String(uuid::Uuid::new_v4().to_string()));
    object.entry("port").or_insert(Value::from(22));
    let parsed: SshTarget = serde_json::from_value(Value::Object(object)).map_err(map_err)?;
    let mut targets = read_targets()?;
    targets.push(parsed.clone());
    write_targets(&targets)?;
    Ok(parsed)
}

#[tauri::command]
pub fn ssh_update_target(id: String, updates: Map<String, Value>) -> Result<SshTarget, String> {
    let mut targets = read_targets()?;
    let index = targets
        .iter()
        .position(|target| target.id == id)
        .ok_or_else(|| format!("target not found: {id}"))?;
    let mut object = serde_json::to_value(&targets[index])
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "failed to serialize target".to_string())?;
    for (key, value) in updates {
        if key == "id" {
            continue;
        }
        object.insert(key, value);
    }
    let updated: SshTarget = serde_json::from_value(Value::Object(object)).map_err(map_err)?;
    targets[index] = updated.clone();
    write_targets(&targets)?;
    Ok(updated)
}

#[tauri::command]
pub fn ssh_remove_target(id: String) -> Result<(), String> {
    let mut targets = read_targets()?;
    targets.retain(|target| target.id != id);
    write_targets(&targets)
}

fn flush_target(
    alias: &Option<String>,
    fields: &BTreeMap<String, String>,
    default_user: &str,
    out: &mut Vec<SshTarget>,
) {
    let Some(alias) = alias else { return };
    out.push(SshTarget {
        id: uuid::Uuid::new_v4().to_string(),
        label: alias.clone(),
        host: fields
            .get("hostname")
            .cloned()
            .unwrap_or_else(|| alias.clone()),
        port: fields
            .get("port")
            .and_then(|port| port.parse::<u32>().ok())
            .unwrap_or(22),
        username: fields
            .get("user")
            .cloned()
            .unwrap_or_else(|| default_user.to_string()),
        config_host: Some(alias.clone()),
        identity_file: fields.get("identityfile").cloned(),
        extra: Map::new(),
    });
}

fn parse_ssh_config(content: &str, default_user: &str) -> Vec<SshTarget> {
    let mut targets = Vec::new();
    let mut alias: Option<String> = None;
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.to_lowercase().starts_with("host ") {
            flush_target(&alias, &fields, default_user, &mut targets);
            fields.clear();
            alias = trimmed[5..]
                .split_whitespace()
                .find(|host| !host.contains('*') && !host.contains('?') && !host.starts_with('!'))
                .map(str::to_string);
        } else if let Some((key, value)) = trimmed.split_once(char::is_whitespace) {
            fields.insert(key.to_lowercase(), value.trim().to_string());
        }
    }
    flush_target(&alias, &fields, default_user, &mut targets);
    targets
}

#[tauri::command]
pub async fn ssh_import_config() -> Result<Vec<SshTarget>, String> {
    let config_path = home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh")
        .join("config");
    let mut targets = read_targets()?;
    let existing: BTreeSet<String> = targets
        .iter()
        .filter_map(|target| target.config_host.clone())
        .collect();
    if config_path.exists() {
        let content = tokio::fs::read_to_string(&config_path)
            .await
            .map_err(map_err)?;
        let default_user = std::env::var("USER").unwrap_or_default();
        for parsed in parse_ssh_config(&content, &default_user) {
            if parsed
                .config_host
                .as_ref()
                .is_some_and(|alias| existing.contains(alias))
            {
                continue;
            }
            targets.push(parsed);
        }
        write_targets(&targets)?;
    }
    Ok(targets)
}

#[tauri::command]
pub fn ssh_get_state(target_id: String) -> Result<Value, String> {
    let _ = target_id;
    // No live SSH transport yet → no connection state.
    Ok(Value::Null)
}

#[tauri::command]
pub fn ssh_connect(target_id: String) -> Result<Value, String> {
    let _ = target_id;
    // SSH transport (connection pool + relay) isn't ported yet; cannot connect.
    Ok(Value::Null)
}

#[tauri::command]
pub fn ssh_needs_passphrase_prompt(target_id: String) -> Result<bool, String> {
    let _ = target_id;
    Ok(false)
}

// Mirrors PortForwardEntry in agentum/src/shared/ssh-types.ts. `connectionId` is the
// owning target id. Forwards persist so they auto-restore on (future) reconnect.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortForwardEntry {
    id: String,
    connection_id: String,
    local_port: u32,
    remote_host: String,
    remote_port: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn forwards_path() -> Result<PathBuf, String> {
    Ok(home_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(".agentum")
        .join("ssh-port-forwards.json"))
}

fn read_forwards() -> Result<Vec<PortForwardEntry>, String> {
    let path = forwards_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(map_err)?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn write_forwards(forwards: &[PortForwardEntry]) -> Result<(), String> {
    let path = forwards_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized = serde_json::to_string_pretty(forwards).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

#[tauri::command]
pub fn ssh_list_port_forwards(target_id: Option<String>) -> Result<Vec<PortForwardEntry>, String> {
    let forwards = read_forwards()?;
    Ok(match target_id {
        Some(target_id) => forwards
            .into_iter()
            .filter(|forward| forward.connection_id == target_id)
            .collect(),
        None => forwards,
    })
}

#[tauri::command]
pub fn ssh_add_port_forward(
    target_id: String,
    local_port: u32,
    remote_host: String,
    remote_port: u32,
    label: Option<String>,
) -> Result<PortForwardEntry, String> {
    let entry = PortForwardEntry {
        id: uuid::Uuid::new_v4().to_string(),
        connection_id: target_id,
        local_port,
        remote_host,
        remote_port,
        label,
        extra: Map::new(),
    };
    let mut forwards = read_forwards()?;
    forwards.push(entry.clone());
    write_forwards(&forwards)?;
    Ok(entry)
}

#[tauri::command]
pub fn ssh_update_port_forward(
    id: String,
    target_id: String,
    local_port: u32,
    remote_host: String,
    remote_port: u32,
    label: Option<String>,
) -> Result<PortForwardEntry, String> {
    let mut forwards = read_forwards()?;
    let index = forwards
        .iter()
        .position(|forward| forward.id == id)
        .ok_or_else(|| format!("port forward not found: {id}"))?;
    forwards[index].connection_id = target_id;
    forwards[index].local_port = local_port;
    forwards[index].remote_host = remote_host;
    forwards[index].remote_port = remote_port;
    forwards[index].label = label;
    let updated = forwards[index].clone();
    write_forwards(&forwards)?;
    Ok(updated)
}

#[tauri::command]
pub fn ssh_remove_port_forward(id: String) -> Result<Option<PortForwardEntry>, String> {
    let mut forwards = read_forwards()?;
    let Some(index) = forwards.iter().position(|forward| forward.id == id) else {
        return Ok(None);
    };
    let removed = forwards.remove(index);
    write_forwards(&forwards)?;
    Ok(Some(removed))
}

#[tauri::command]
pub fn ssh_list_detected_ports(target_id: String) -> Result<Vec<Value>, String> {
    let _ = target_id;
    // Port detection requires a live SSH session (PTY scanning); none yet.
    Ok(Vec::new())
}

// Live SSH session control (credential prompts, disconnect, relay reset, connection
// test, remote dir browsing) needs an active transport, which isn't ported. Submit/
// terminate/disconnect/reset no-op; browse is empty; test reports failure.
#[tauri::command]
pub fn ssh_submit_credential() {}

#[tauri::command]
pub fn ssh_terminate_sessions() {}

#[tauri::command]
pub fn ssh_disconnect() {}

#[tauri::command]
pub fn ssh_browse_dir() -> Result<Vec<Value>, String> {
    Ok(Vec::new())
}

#[tauri::command]
pub fn ssh_reset_relay() {}

// Non-interactive connectivity probe: `ssh -o BatchMode=yes … true`. BatchMode
// fails fast if the host needs a password/passphrase prompt (so this tests
// key-based reachability). Returns the renderer's {success,state}|{success,error}.
#[tauri::command]
pub fn ssh_test_connection(target_id: String) -> Value {
    let Some(target) = read_targets().ok().and_then(|targets| {
        targets
            .into_iter()
            .find(|candidate| candidate.id == target_id)
    }) else {
        return serde_json::json!({
            "success": false,
            "error": format!("SSH target \"{target_id}\" not found")
        });
    };

    let mut command = std::process::Command::new("ssh");
    command.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "StrictHostKeyChecking=accept-new",
    ]);
    if let Some(identity) = &target.identity_file {
        command.args(["-i", identity]);
    }
    // Use the ssh-config alias as a bare destination (letting ~/.ssh/config
    // supply port/identity) ONLY when it's a genuine alias — i.e. it differs
    // from the literal host. A config_host equal to the host (some imports set
    // it to the IP itself) is NOT a real alias; treating it as one drops `-p`
    // and ssh falls back to port 22. In that case connect to user@host -p port.
    let alias = target
        .config_host
        .as_deref()
        .map(str::trim)
        .filter(|alias| !alias.is_empty() && *alias != target.host);
    let destination = match alias {
        Some(alias) => alias.to_string(),
        None => {
            command.args(["-p", &target.port.to_string()]);
            format!("{}@{}", target.username, target.host)
        }
    };
    command.arg(destination).arg("true");

    match command.output() {
        Ok(output) if output.status.success() => {
            serde_json::json!({ "success": true, "state": "ready" })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            serde_json::json!({
                "success": false,
                "error": if stderr.is_empty() { "Connection test failed".to_string() } else { stderr }
            })
        }
        Err(error) => serde_json::json!({ "success": false, "error": error.to_string() }),
    }
}
