use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use tauri::State;

use crate::state::AppState;

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
pub async fn settings_get(
    state: State<'_, AppState>,
    key: String,
) -> Result<Option<Value>, String> {
    let database = state.settings_db.clone();
    tokio::task::spawn_blocking(move || {
        let connection = database.lock();
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_err)?;

        Ok(value.map(|stored| serde_json::from_str(&stored).unwrap_or(Value::String(stored))))
    })
    .await
    .map_err(map_err)?
}

#[tauri::command]
pub async fn settings_set(
    state: State<'_, AppState>,
    key: String,
    value: Value,
) -> Result<(), String> {
    let database = state.settings_db.clone();
    tokio::task::spawn_blocking(move || {
        let connection = database.lock();
        connection
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)                  ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value.to_string()],
            )
            .map(|_| ())
            .map_err(map_err)
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
