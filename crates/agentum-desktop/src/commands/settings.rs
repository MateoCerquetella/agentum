use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use tauri::State;

use crate::state::AppState;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// Read every stored setting into one JSON object (the renderer's GlobalSettings shape).
fn read_all_settings(connection: &rusqlite::Connection) -> Result<Value, String> {
    let mut statement = connection
        .prepare("SELECT key, value FROM settings ORDER BY key")
        .map_err(map_err)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(map_err)?;
    let mut object = serde_json::Map::new();
    for row in rows {
        let (key, raw) = row.map_err(map_err)?;
        object.insert(
            key,
            serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
        );
    }
    Ok(Value::Object(object))
}

// The renderer uses the orca bulk convention: `settings.get()` (no key) returns the
// whole settings object; `settings.get(key)` returns a single value. The old port
// took a required `key` and broke the no-arg call ("missing required key key").
#[tauri::command]
pub async fn settings_get(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let key = match request.body() {
        tauri::ipc::InvokeBody::Json(value) => value
            .get("key")
            .or_else(|| value.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    };
    let database = state.settings_db.clone();
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let connection = database.lock();
        match key {
            Some(key) => {
                let value: Option<String> = connection
                    .query_row(
                        "SELECT value FROM settings WHERE key = ?1",
                        params![key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(map_err)?;
                Ok(value
                    .map(|stored| serde_json::from_str(&stored).unwrap_or(Value::String(stored)))
                    .unwrap_or(Value::Null))
            }
            None => read_all_settings(&connection),
        }
    })
    .await
    .map_err(map_err)?
}

// The renderer calls `settings.set(partialUpdates)` with a bulk object and expects
// the full merged settings back. The old port took a single (key, value) and
// returned nothing, so updates silently failed.
#[tauri::command]
pub async fn settings_set(
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<Value, String> {
    let updates: serde_json::Map<String, Value> = match request.body() {
        tauri::ipc::InvokeBody::Json(value) => value.as_object().cloned().unwrap_or_default(),
        _ => serde_json::Map::new(),
    };
    let database = state.settings_db.clone();
    tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let connection = database.lock();
        for (key, value) in &updates {
            connection
                .execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2) \
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value.to_string()],
                )
                .map_err(map_err)?;
        }
        read_all_settings(&connection)
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn settings_get_all(
    state: State<'_, AppState>,
) -> Result<HashMap<String, Value>, String> {
    let database = state.settings_db.clone();
    tokio::task::spawn_blocking(move || {
        let connection = database.lock();
        let mut statement = connection
            .prepare("SELECT key, value FROM settings ORDER BY key")
            .map_err(map_err)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_err)?;

        let mut settings = HashMap::new();
        for row in rows {
            let (key, raw_value) = row.map_err(map_err)?;
            let value = serde_json::from_str(&raw_value).unwrap_or(Value::String(raw_value));
            settings.insert(key, value);
        }

        Ok(settings)
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub fn settings_list_fonts() -> Vec<String> {
    // System font-family enumeration isn't ported; the renderer falls back to defaults.
    Vec::new()
}

#[tauri::command]
pub fn settings_preview_ghostty_import() -> Value {
    // No Ghostty terminal config is imported; report nothing found.
    serde_json::json!({ "found": false, "diff": {}, "unsupportedKeys": [] })
}
