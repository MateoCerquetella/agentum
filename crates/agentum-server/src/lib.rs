//! axum HTTP(S) server for agentum.
//!
//! - Phase 1: plain HTTP, no auth.
//! - Phase 5: HTTPS via rustls (self-signed) + bearer-token middleware on
//!   `/api/*` (excluding `/api/health` + `/api/cert`). A small plain-HTTP
//!   cert-server runs on a side port for trust-on-first-use.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use agentum_core::Event;
use agentum_store::Store;
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware as axum_mw;
use axum::response::IntoResponse;
use axum::routing::get;
use axum_server::tls_rustls::RustlsConfig;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub mod auth;
mod embed;
mod error;
mod routes;
pub mod tls;

pub use error::ApiError;

const EVENT_BUS_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub bus: broadcast::Sender<Event>,
    pub started_at: Instant,
    pub version: &'static str,
}

impl AppState {
    pub fn new(store: Store, bus: broadcast::Sender<Event>) -> Self {
        Self {
            store: Arc::new(store),
            bus,
            started_at: Instant::now(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub addr: SocketAddr,
    pub cert_addr: SocketAddr,
    pub tls: bool,
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_origin(Any);

    Router::new()
        .merge(routes::health::router())
        .merge(routes::doctor::router())
        .merge(routes::auth::router())
        .merge(routes::sessions::router())
        .merge(routes::board::router())
        .merge(routes::notes::router())
        .merge(routes::channels::router())
        .merge(routes::events::router())
        .layer(axum_mw::from_fn_with_state(state.clone(), auth::require_token))
        .fallback(embed::static_handler)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
}

/// Bind and serve forever. Drives both the main API server (TLS or plain),
/// the small plain-HTTP cert-server, and the watchdog reconcile loop.
pub async fn serve(opts: ServeOptions, store: Store) -> anyhow::Result<()> {
    // rustls 0.23 requires picking a CryptoProvider when both ring and
    // aws-lc-rs are pulled in transitively. We don't care which one wins;
    // ring is the smaller of the two.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (bus, _) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
    let state = AppState::new(store, bus.clone());

    if state.store.count_users().await.unwrap_or(0) == 0 {
        tracing::warn!(
            "no users registered yet — visit the dashboard to create the first one (or run `agentum auth add <name>`)"
        );
    }

    let watchdog = agentum_watchdog::Watchdog::new(bus, state.store.clone());
    tokio::spawn(watchdog.run());

    let app = router(state);

    if opts.tls {
        let artifacts = tls::ensure_artifacts()?;
        let tls_config =
            RustlsConfig::from_pem_file(&artifacts.cert_path, &artifacts.key_path).await?;

        let cert_router = cert_server_router(artifacts.cert_pem.clone());
        let cert_listener = TcpListener::bind(opts.cert_addr).await?;
        tracing::info!(addr = %opts.cert_addr, "cert-server listening (plain http)");
        let cert_handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(cert_listener, cert_router).await {
                tracing::error!("cert server exited: {e}");
            }
        });

        tracing::info!(addr = %opts.addr, "agentum-server listening (https)");
        let result = axum_server::bind_rustls(opts.addr, tls_config)
            .serve(app.into_make_service())
            .await;
        cert_handle.abort();
        result.map_err(Into::into)
    } else {
        tracing::warn!(addr = %opts.addr, "agentum-server listening (plain http; --no-tls)");
        let listener = TcpListener::bind(opts.addr).await?;
        axum::serve(listener, app).await.map_err(Into::into)
    }
}

fn cert_server_router(cert_pem: String) -> Router {
    let pem = Arc::new(cert_pem);
    let pem_for_route = pem.clone();
    Router::new()
        .route(
            "/api/cert",
            get(move || {
                let pem = pem_for_route.clone();
                async move {
                    (
                        [(
                            axum::http::header::CONTENT_TYPE,
                            "application/x-pem-file",
                        )],
                        pem.as_str().to_string(),
                    )
                }
            }),
        )
        .fallback(get(cert_redirect_hint))
        .layer(TraceLayer::new_for_http())
}

async fn cert_redirect_hint(_: Request<Body>) -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "agentum cert server. GET /api/cert for the PEM. The full app is on the TLS port.\n",
    )
}
