mod accounts;
mod auth;
mod calendar;
mod chat;
mod code;
mod creatures;
mod db;
mod genui;
mod llm;
mod mail;
mod notes;
mod floatnote;
mod passkey;
mod plugins;
mod push;
mod secrets;
mod settings;
mod state;
mod today;
mod util;
mod voice;
mod web;
mod widgets;

use axum::{extract::State, response::sse::{Event, KeepAlive, Sse}, routing::{get, post, put}, Json, Router};
use futures::Stream;
use serde_json::{json, Value};
use state::{App, Config};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = Config::from_env();
    std::fs::create_dir_all(&cfg.data_dir)?;
    secrets::init(&cfg.data_dir)?;
    // agentum.db; a CERQ install keeps its cerq.db.
    let old = format!("{}/cerq.db", cfg.data_dir);
    let path = if std::path::Path::new(&old).exists() && !std::path::Path::new(&format!("{}/agentum.db", cfg.data_dir)).exists() { old } else { format!("{}/agentum.db", cfg.data_dir) };
    let db = util::Db::open(&path)?;
    db.call(|c| db::migrate(c)).await?;
    let http = reqwest::Client::builder()
        .tls_backend_preconfigured(rustls_config())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .pool_max_idle_per_host(2)
        // Node (BB) closes an idle connection after 5 s: drop ours first, so a write never lands on a closed one.
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .build()?;
    let (events, _) = tokio::sync::broadcast::channel(256);
    let app = App { cfg: Arc::new(cfg), db, http, cache: Default::default(), events, gate: Default::default(), settings: Default::default(), code: Default::default() };

    settings::load(&app).await?;
    code::start(&app).await?;
    plugins::start(&app);

    // The background jobs start one after another, not all at once: Mercury (a Raspberry Pi) has reset
    // within a minute of CERQ starting, when mail, calendar, the brief and the journal all began together.
    {
        let a = app.clone();
        tokio::spawn(async move {
            let wait = |s| tokio::time::sleep(std::time::Duration::from_secs(s));
            wait(15).await; mail::start(a.clone());
            wait(30).await; calendar::start(a.clone());
            wait(30).await; today::start(a);
        });
    }

    let api = Router::new()
        .route("/api/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/check", get(|| async { "ok" }))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/events", get(sse))
        .route("/api/model/status", get(|State(a): State<App>| async move { Json(llm::status(&a)) }))
        .route("/api/settings", get(settings::get).post(settings::post))
        .route("/api/settings/sidebar", axum::routing::put(settings::sidebar))
        .route("/api/settings/look", axum::routing::put(settings::look))
        .route("/api/settings/geocode", get(settings::geocode))
        .route("/api/settings/test-model", post(settings::test_model))
        .route("/api/dashboard", get(widgets::get_dashboard))
        .route("/api/dashboard/layout", put(widgets::put_layout))
        .route("/api/dashboard/layout/reset", post(widgets::reset_layout))
        .route("/api/widgets", put(widgets::put_widget))
        .route("/api/widgets/{id}", axum::routing::delete(widgets::delete_widget))
        .route("/api/widgets/{id}/data", get(widgets::widget_data))
        .route("/api/chart", get(widgets::chart))
        .merge(accounts::routes())
        .merge(notes::routes())
        .merge(floatnote::routes())
        .merge(mail::routes())
        .merge(calendar::routes())
        .merge(today::routes())
        .merge(chat::routes())
        .merge(creatures::routes())
        .merge(voice::routes())
        .merge(passkey::routes())
        .merge(code::routes())
        .merge(plugins::routes());

    let router = api
        .fallback(web::static_handler)
        .layer(axum::middleware::from_fn_with_state(app.clone(), auth::guard))
        .layer(tower_http::compression::CompressionLayer::new().gzip(true).br(true).compress_when(SseSafe))
        .with_state(app.clone());

    let bind = app.cfg.bind.clone();
    tracing::info!("Agentum listening on {bind}");
    let l = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(l, router).with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await?;
    Ok(())
}

/// Don't compress event streams (buffering would delay them).
#[derive(Clone, Copy)]
struct SseSafe;
impl tower_http::compression::predicate::Predicate for SseSafe {
    fn should_compress<B: axum::body::HttpBody>(&self, r: &axum::http::Response<B>) -> bool {
        let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
        !ct.starts_with("text/event-stream") && !ct.starts_with("application/x-ndjson") && tower_http::compression::predicate::DefaultPredicate::new().should_compress(r)
    }
}

pub fn rustls_config() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()
}

async fn sse(State(app): State<App>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = BroadcastStream::new(app.events.subscribe());
    let s = rx.filter_map(|m: Result<Value, _>| m.ok().map(|v| Ok(Event::default().data(v.to_string()))));
    Sse::new(s).keep_alive(KeepAlive::default())
}
