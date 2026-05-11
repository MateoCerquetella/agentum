//! `/api/preferences` — shared user preferences read/written by both
//! the dashboard and the TUI so theme picks (and any future shared
//! settings) follow the user across surfaces.
//!
//! On disk: `<data_dir>/preferences.json`. The TUI's existing flat
//! `<data_dir>/theme` file is also kept in sync — when the dashboard
//! writes a theme, the mapped TUI theme name lands in `theme` so the
//! next `agentum terminal` launch picks it up without any extra wiring.
//!
//! Live propagation: every PUT broadcasts a `preferences.changed`
//! event onto the bus. The TUI subscribes to that stream and reloads
//! its palette in place; dashboards do the same via their event-bridge.

use std::path::PathBuf;

use agentum_core::Event;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Preferences {
    /// Dashboard theme id (e.g. "tokyo-night"). When empty, the dashboard
    /// falls back to its built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// TUI theme name (e.g. "midnight"). Derived from `theme` when the
    /// dashboard writes — otherwise preserved as-is so the TUI can own
    /// its own pick when no dashboard has touched the file yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tui_theme: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/preferences", get(get_prefs).put(put_prefs))
}

fn prefs_path() -> Option<PathBuf> {
    let dir = agentum_store::paths::data_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("preferences.json"))
}

fn tui_theme_path() -> Option<PathBuf> {
    let dir = agentum_store::paths::data_dir().ok()?;
    Some(dir.join("theme"))
}

pub fn read() -> Preferences {
    let Some(path) = prefs_path() else {
        return Preferences::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Preferences::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write(prefs: &Preferences) -> std::io::Result<()> {
    let Some(path) = prefs_path() else {
        return Ok(());
    };
    let body = serde_json::to_string_pretty(prefs).unwrap_or_else(|_| "{}".into());
    std::fs::write(&path, body)?;
    // Keep the TUI's flat theme file aligned so a fresh `agentum
    // terminal` launch picks up the dashboard's choice with no extra
    // round trip.
    if let Some(tui) = prefs.tui_theme.as_deref()
        && let Some(theme_path) = tui_theme_path()
    {
        let _ = std::fs::write(theme_path, format!("{tui}\n"));
    }
    Ok(())
}

async fn get_prefs() -> Json<Preferences> {
    Json(read())
}

async fn put_prefs(
    State(state): State<AppState>,
    Json(body): Json<Preferences>,
) -> Result<Json<Preferences>, StatusCode> {
    // Merge into existing prefs so a partial PUT (e.g. only `theme`)
    // doesn't wipe the other fields.
    let mut current = read();
    if body.theme.is_some() {
        current.theme = body.theme;
    }
    if body.tui_theme.is_some() {
        current.tui_theme = body.tui_theme;
    }
    write(&current).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Best-effort fan-out so the other surface (TUI / dashboard) reflects
    // the change without polling. A dropped receiver here just means
    // there are no subscribers — not a failure.
    let _ = state
        .bus
        .send(Event::new("preferences.changed").with_payload(json!({
            "theme": current.theme,
            "tui_theme": current.tui_theme,
        })));

    Ok(Json(current))
}
