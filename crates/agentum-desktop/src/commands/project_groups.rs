use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// Mirrors ProjectGroup in agentum/src/shared/types.ts. Nullable fields stay Option
// (serialize as null) since they are required-but-nullable in the contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroup {
    id: String,
    name: String,
    parent_path: Option<String>,
    parent_group_id: Option<String>,
    created_from: String,
    tab_order: i64,
    is_collapsed: bool,
    color: Option<String>,
    created_at: u64,
    updated_at: u64,
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn agentum_file(name: &str) -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "no home directory".to_string())?
        .join(".agentum")
        .join(name))
}

fn read_groups() -> Result<Vec<ProjectGroup>, String> {
    let path = agentum_file("project-groups.json")?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(map_err)?;
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn write_groups(groups: &[ProjectGroup]) -> Result<(), String> {
    let path = agentum_file("project-groups.json")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(map_err)?;
    }
    let serialized = serde_json::to_string_pretty(groups).map_err(map_err)?;
    std::fs::write(path, format!("{serialized}\n")).map_err(map_err)
}

#[tauri::command]
pub fn project_groups_list() -> Result<Vec<ProjectGroup>, String> {
    read_groups()
}

#[tauri::command]
pub fn project_groups_create(
    name: String,
    parent_path: Option<String>,
    parent_group_id: Option<String>,
    created_from: Option<String>,
) -> Result<ProjectGroup, String> {
    let mut groups = read_groups()?;
    let now = now_millis();
    let group = ProjectGroup {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        parent_path,
        parent_group_id,
        created_from: created_from.unwrap_or_else(|| "manual".to_string()),
        tab_order: groups.len() as i64,
        is_collapsed: false,
        color: None,
        created_at: now,
        updated_at: now,
    };
    groups.push(group.clone());
    write_groups(&groups)?;
    Ok(group)
}

#[tauri::command]
pub fn project_groups_update(
    group_id: String,
    updates: Map<String, Value>,
) -> Result<Option<ProjectGroup>, String> {
    let mut groups = read_groups()?;
    let Some(index) = groups.iter().position(|group| group.id == group_id) else {
        return Ok(None);
    };
    let mut object = serde_json::to_value(&groups[index])
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "failed to serialize group".to_string())?;
    for (key, value) in updates {
        // Only these fields are user-updatable per the contract.
        if matches!(key.as_str(), "name" | "isCollapsed" | "tabOrder" | "color") {
            object.insert(key, value);
        }
    }
    object.insert("updatedAt".into(), Value::from(now_millis()));
    let updated: ProjectGroup = serde_json::from_value(Value::Object(object)).map_err(map_err)?;
    groups[index] = updated.clone();
    write_groups(&groups)?;
    Ok(Some(updated))
}

#[tauri::command]
pub fn project_groups_delete(group_id: String) -> Result<bool, String> {
    let mut groups = read_groups()?;
    let before = groups.len();
    groups.retain(|group| group.id != group_id);
    let removed = groups.len() != before;
    if removed {
        write_groups(&groups)?;
    }
    Ok(removed)
}

#[tauri::command]
pub fn project_groups_move_project(
    project_id: String,
    group_id: Option<String>,
    order: Option<i64>,
) -> Result<Value, String> {
    // Membership lives on the Repo (projectGroupId/projectGroupOrder) in repos.json.
    let path = agentum_file("repos.json")?;
    if !path.exists() {
        return Ok(Value::Null);
    }
    let raw = std::fs::read_to_string(&path).map_err(map_err)?;
    let mut repos: Value = serde_json::from_str(&raw).map_err(map_err)?;
    let updated = {
        let Some(array) = repos.as_array_mut() else {
            return Ok(Value::Null);
        };
        let Some(repo) = array
            .iter_mut()
            .find(|repo| repo.get("id").and_then(Value::as_str) == Some(project_id.as_str()))
        else {
            return Ok(Value::Null);
        };
        if let Some(object) = repo.as_object_mut() {
            object.insert(
                "projectGroupId".into(),
                group_id.map(Value::from).unwrap_or(Value::Null),
            );
            if let Some(order) = order {
                object.insert("projectGroupOrder".into(), Value::from(order));
            }
        }
        repo.clone()
    };
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&repos).map_err(map_err)?,
    )
    .map_err(map_err)?;
    Ok(updated)
}

// Deep nested-repo scanning isn't ported; report only whether the selected path is
// itself a git repo, with no nested candidates.
#[tauri::command]
pub fn project_groups_scan_nested(path: String) -> Value {
    let kind = if std::path::Path::new(&path).join(".git").exists() {
        "git_repo"
    } else {
        "non_git_folder"
    };
    serde_json::json!({
        "selectedPath": path,
        "selectedPathKind": kind,
        "repos": [],
        "truncated": false,
        "timedOut": false,
        "durationMs": 0,
        "maxDepth": 0
    })
}

#[tauri::command]
pub fn project_groups_import_nested() -> Value {
    serde_json::json!({
        "projects": [],
        "importedCount": 0,
        "alreadyKnownCount": 0,
        "failedCount": 0
    })
}
