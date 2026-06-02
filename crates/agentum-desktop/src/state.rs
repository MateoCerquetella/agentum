use std::{collections::HashMap, io::Write, sync::Arc};

use anyhow::Context;
use chrono::{DateTime, Utc};
use notify::RecommendedWatcher;
use parking_lot::Mutex;
use portable_pty::{Child, MasterPty};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub struct PtyHandle {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub workspace_root: Option<String>,
    pub active_project: Option<String>,
    pub active_session_id: Option<String>,
    pub healthy: bool,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            workspace_root: None,
            active_project: None,
            active_session_id: None,
            healthy: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct RuntimeStateData {
    pub workspace: WorkspaceState,
    pub agents: HashMap<String, AgentRecord>,
}

pub struct AppState {
    pub ptys: Arc<Mutex<HashMap<String, PtyHandle>>>,
    pub settings_db: Arc<Mutex<Connection>>,
    pub watchers: Arc<Mutex<HashMap<String, RecommendedWatcher>>>,
    pub runtime: Arc<Mutex<RuntimeStateData>>,
}

impl AppState {
    pub fn new() -> anyhow::Result<Self> {
        let base_dir = dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .unwrap_or(std::env::current_dir().context("failed to resolve current directory")?);
        let app_dir = base_dir.join("Agentum");
        std::fs::create_dir_all(&app_dir).context("failed to create app data directory")?;

        let connection = Connection::open(app_dir.join("settings.sqlite3"))
            .context("failed to open settings database")?;
        connection
            .execute_batch(
                "                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS settings (
                  key TEXT PRIMARY KEY,
                  value TEXT NOT NULL
                );",
            )
            .context("failed to initialize settings database")?;

        Ok(Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            settings_db: Arc::new(Mutex::new(connection)),
            watchers: Arc::new(Mutex::new(HashMap::new())),
            runtime: Arc::new(Mutex::new(RuntimeStateData::default())),
        })
    }
}
