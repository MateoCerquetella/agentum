use crate::util::{Cache, Db};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Config {
    pub data_dir: String,
    pub bind: String,
    pub searxng: String,
}
impl Config {
    pub fn from_env() -> Config {
        // AGENTUM_* first; CERQ_* still work (an install from before the rename).
        let e = |k: &str, d: &str| std::env::var(format!("AGENTUM_{k}")).or_else(|_| std::env::var(format!("CERQ_{k}"))).unwrap_or_else(|_| d.to_string());
        Config {
            data_dir: e("DATA", "./data"),
            bind: e("BIND", "127.0.0.1:7100"),
            searxng: e("SEARXNG", ""),
        }
    }
}

#[derive(Clone)]
pub struct App {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub http: reqwest::Client,
    pub cache: Cache,
    /// Server-sent events for every open tab: {"kind": "mail"|"calendar"|..., ...}
    pub events: broadcast::Sender<serde_json::Value>,
    /// One model call at a time; interactive (foreground) calls go first.
    pub gate: Arc<crate::llm::Gate>,
    /// Saved CERQ_* settings (see settings.rs), read with app.setting(KEY).
    pub settings: Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>,
    /// The Code section: connected machines, thread summaries, live fan-out.
    pub code: Arc<crate::code::Hub>,
}
impl App {
    pub fn emit(&self, v: serde_json::Value) { let _ = self.events.send(v); }
}
