//! The agent's state (cerq-mail's state.json, imported into kv "mail.state"):
//! rules, cold-email settings, Reply Zero dismissals, the rules' cursor,
//! recent runs, aliases, Microsoft's rotating refresh tokens and the push mark.
//! Held in memory and written back on every change.
use super::rules::Rule;
use crate::state::App;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use tokio::sync::Mutex;

const KEY: &str = "mail.state";

#[derive(Serialize, Deserialize, Clone)]
pub struct Cold {
    pub mode: String,
    #[serde(default)]
    pub allow: Vec<String>,
}
impl Default for Cold {
    // "list" just shows them; nothing acts until you choose.
    fn default() -> Self { Cold { mode: "list".into(), allow: vec![] } }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Chain {
    pub seed: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct State {
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub cold: Cold,
    /// Message-ids marked done in Reply Zero (newest 2000).
    #[serde(default)]
    pub dismissed: Vec<String>,
    /// account id → last INBOX uid the rules saw.
    #[serde(default)]
    pub cursor: HashMap<String, i64>,
    #[serde(default)]
    pub runs: Vec<Value>,
    /// Extra addresses that are you (forwarders, old accounts).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// account id → Microsoft's newest refresh token (encrypted).
    #[serde(default)]
    pub oauth_chain: HashMap<String, Chain>,
    /// Mail newer than this has not been pushed yet.
    #[serde(default)]
    pub push_until: i64,
    #[serde(default)]
    pub enrich_v: i64,
    /// Anything else the old agent kept, carried along untouched.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

static STATE: Mutex<Option<State>> = Mutex::const_new(None);

async fn load(app: &App) -> State {
    match app.db.kv_get(KEY).await {
        Ok(Some(v)) => serde_json::from_value(v).unwrap_or_else(|e| {
            tracing::error!("mail.state unreadable ({e}); starting from defaults");
            State::default()
        }),
        _ => State::default(),
    }
}

/// A copy of the current state.
pub async fn get(app: &App) -> State {
    let mut g = STATE.lock().await;
    if g.is_none() { *g = Some(load(app).await); }
    g.clone().unwrap()
}

/// Change the state and save it.
pub async fn update<T>(app: &App, f: impl FnOnce(&mut State) -> T) -> anyhow::Result<T> {
    let mut g = STATE.lock().await;
    if g.is_none() { *g = Some(load(app).await); }
    let st = g.as_mut().unwrap();
    let out = f(st);
    app.db.kv_set(KEY, &serde_json::to_value(&*st)?).await?;
    Ok(out)
}
