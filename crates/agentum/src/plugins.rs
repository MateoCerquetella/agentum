//! The plugin host: every plugin's hub part, its context (machines, threads, storage, push),
//! and the routes its web part calls.
//!
//! - `GET /api/plugins`: every plugin's manifest, whether it is on, and its settings.
//! - `PUT /api/plugins/{id}`: `{enabled?, settings?}`.
//! - `POST /api/p/{id}/{method}`: the plugin's RPC.
use crate::state::App;
use crate::util::{bad, not_found, R};
use agentum_plugin::hub::{Ctx, HubCtx, HubPlugin};
use agentum_plugin::Manifest;
use agentum_proto::api::{NewThread, PatchThread, SendBody, ThreadSummary, WsOut};
use agentum_proto::machine::Req;
use agentum_proto::Event;
use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;

/// Every plugin's hub part (their web parts are listed in crates/agentum-web/src/plugins.rs).
static ALL: &[&dyn HubPlugin] = &[
        &agentum_plugin_git::HUB,
        &agentum_plugin_pr::HUB,
        &agentum_plugin_files::HUB,
        &agentum_plugin_editor::HUB,
        &agentum_plugin_history::HUB,
        &agentum_plugin_terminal::HUB,
        &agentum_plugin_hover_cards::HUB,
        &agentum_plugin_timestamps::HUB,
        &agentum_plugin_sticky_prompt::HUB,
        &agentum_plugin_nudger::HUB,
        &agentum_plugin_scheduled_send::HUB,
        &agentum_plugin_side_chat::HUB,
        &agentum_plugin_drafts::HUB,
        &agentum_plugin_usage::HUB,
        &agentum_plugin_context::HUB,
        &agentum_plugin_concurrency::HUB,
        &agentum_plugin_notify::HUB,
        &agentum_plugin_aura::HUB,
        &agentum_plugin_genui::HUB,
        &agentum_plugin_browser::HUB,
        &agentum_plugin_themes::HUB,
];

pub fn all() -> &'static [&'static dyn HubPlugin] {
    ALL
}

fn find(id: &str) -> Option<&'static dyn HubPlugin> {
    all().iter().copied().find(|p| p.manifest().id == id)
}

struct PluginCtx {
    app: App,
    m: &'static Manifest,
}

#[async_trait::async_trait]
impl HubCtx for PluginCtx {
    fn plugin(&self) -> &'static str {
        self.m.id
    }
    async fn machine(&self, machine: &str, req: Req) -> Result<Value, String> {
        crate::code::machine_call(&self.app, machine, req).await
    }
    async fn thread(&self, id: &str) -> Option<ThreadSummary> {
        self.app.code.summary(id)
    }
    async fn threads(&self) -> Vec<ThreadSummary> {
        self.app.code.threads.read().unwrap().values().cloned().collect()
    }
    async fn events(&self, id: &str, after: u64) -> Vec<Event> {
        let id = id.to_string();
        self.app.db.call(move |c| crate::code::db::events_after(c, &id, after)).await.unwrap_or_default()
    }
    async fn send(&self, id: &str, body: SendBody) -> Result<(), String> {
        crate::code::send(&self.app, id, body).await.map_err(|e| e.1)
    }
    async fn new_thread(&self, body: NewThread) -> Result<ThreadSummary, String> {
        crate::code::create_thread(&self.app, body).await.map_err(|e| e.1)
    }
    async fn patch_thread(&self, id: &str, patch: PatchThread) -> Result<ThreadSummary, String> {
        crate::code::patch_thread(&self.app, id, patch).await.map_err(|e| e.1)
    }
    async fn kv_get(&self, key: &str) -> Option<Value> {
        kv_get(&self.app, self.m.id, key).await
    }
    async fn kv_set(&self, key: &str, value: Value) {
        let (p, k, v) = (self.m.id.to_string(), key.to_string(), value.to_string());
        let _ = self.app.db.call(move |c| c.execute("INSERT INTO plugin_kv(plugin,k,v) VALUES(?1,?2,?3) ON CONFLICT(plugin,k) DO UPDATE SET v=excluded.v", [p, k, v])).await;
    }
    async fn kv_delete(&self, key: &str) {
        let (p, k) = (self.m.id.to_string(), key.to_string());
        let _ = self.app.db.call(move |c| c.execute("DELETE FROM plugin_kv WHERE plugin=?1 AND k=?2", [p, k])).await;
    }
    async fn setting(&self, key: &str) -> Value {
        setting(&self.app, self.m, key).await
    }
    async fn enabled(&self) -> bool {
        enabled(&self.app, self.m).await
    }
    fn publish(&self, payload: Value) {
        self.app.code.publish(&format!("plugin:{}", self.m.id), &WsOut::Plugin { plugin: self.m.id.to_string(), payload });
    }
    async fn notify(&self, title: &str, body: &str, url: &str) {
        let msg = json!({"title": title, "body": body, "url": url, "tag": format!("agentum-{}", self.m.id)});
        crate::push::send_all(&self.app, msg.clone()).await;
        self.app.emit(json!({"kind": "toast", "title": title, "body": body, "url": url}));
    }
}

async fn kv_get(app: &App, plugin: &str, key: &str) -> Option<Value> {
    let (p, k) = (plugin.to_string(), key.to_string());
    app.db
        .call(move |c| {
            use rusqlite::OptionalExtension;
            c.query_row("SELECT v FROM plugin_kv WHERE plugin=?1 AND k=?2", [p, k], |r| r.get::<_, String>(0)).optional()
        })
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
}

async fn enabled(app: &App, m: &Manifest) -> bool {
    kv_get(app, m.id, "_enabled").await.and_then(|v| v.as_bool()).unwrap_or(m.default_on)
}

async fn setting(app: &App, m: &Manifest, key: &str) -> Value {
    if let Some(v) = kv_get(app, m.id, &format!("_set.{key}")).await {
        return v;
    }
    m.settings.iter().find(|s| s.key == key).and_then(|s| serde_json::from_str(s.default).ok()).unwrap_or(Value::Null)
}

pub fn ctx(app: &App, m: &'static Manifest) -> Ctx {
    Arc::new(PluginCtx { app: app.clone(), m })
}

/// Start every plugin's background work.
pub fn start(app: &App) {
    for p in all() {
        p.start(ctx(app, p.manifest()));
    }
}

/// A stored event: every plugin that is on hears it (in the background).
pub fn on_event(app: &App, t: &ThreadSummary, e: &Event) {
    if all().is_empty() {
        return;
    }
    let (app, t, e) = (app.clone(), t.clone(), e.clone());
    tokio::spawn(async move {
        for p in all() {
            if enabled(&app, p.manifest()).await {
                p.on_event(&ctx(&app, p.manifest()), &t, &e).await;
            }
        }
    });
}

/// Every plugin that is on may refuse a turn (Err: why).
pub async fn before_turn(app: &App, t: &ThreadSummary, text: &str) -> Result<(), String> {
    for p in all() {
        if enabled(app, p.manifest()).await {
            p.before_turn(&ctx(app, p.manifest()), t, text).await?;
        }
    }
    Ok(())
}

pub fn routes() -> Router<App> {
    Router::new().route("/api/plugins", get(list)).route("/api/plugins/{id}", put(update)).route("/api/p/{id}/{method}", post(rpc))
}

async fn list(State(app): State<App>) -> Json<Value> {
    let mut out = vec![];
    for p in all() {
        let m = p.manifest();
        let mut settings = json!({});
        for s in m.settings {
            settings[s.key] = setting(&app, m, s.key).await;
        }
        out.push(json!({"manifest": m, "id": m.id, "enabled": enabled(&app, m).await, "settings": settings}));
    }
    Json(json!(out))
}

async fn update(State(app): State<App>, Path(id): Path<String>, Json(b): Json<Value>) -> R<Json<Value>> {
    let p = find(&id).ok_or_else(not_found)?;
    let cx = ctx(&app, p.manifest());
    if let Some(on) = b["enabled"].as_bool() {
        cx.kv_set("_enabled", json!(on)).await;
    }
    if let Some(s) = b["settings"].as_object() {
        for (k, v) in s {
            if !p.manifest().settings.iter().any(|d| d.key == k) {
                return Err(bad(format!("{id} has no setting {k}")));
            }
            cx.kv_set(&format!("_set.{k}"), v.clone()).await;
        }
    }
    app.code.publish(&format!("plugin:{id}"), &WsOut::Plugin { plugin: id.clone(), payload: json!({"settings": true}) });
    Ok(Json(json!({"ok": true})))
}

async fn rpc(State(app): State<App>, Path((id, method)): Path<(String, String)>, body: Option<Json<Value>>) -> R<Json<Value>> {
    let p = find(&id).ok_or_else(not_found)?;
    // A plugin that is off still answers its own calls (its settings are edited before it is turned
    // on); its slots, events and background work are what being off stops.
    let input = body.map(|b| b.0).unwrap_or(Value::Null);
    p.rpc(&ctx(&app, p.manifest()), &method, input).await.map(Json).map_err(bad)
}
