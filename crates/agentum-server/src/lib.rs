//! axum HTTP(S) server for agentum.
//!
//! - Phase 1: plain HTTP, no auth.
//! - Phase 5: HTTPS via rustls (self-signed) + bearer-token middleware on
//!   `/api/*` (excluding `/api/health` + `/api/cert`). A small plain-HTTP
//!   cert-server runs on a side port for trust-on-first-use.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

pub mod auth;
mod embed;
mod error;
mod headers;
mod logging;
pub mod ratelimit;
mod routes;
pub mod tls;

pub use error::ApiError;

const EVENT_BUS_CAPACITY: usize = 256;

/// Login + register attempts per remote IP per window.
const AUTH_RATE_LIMIT_ATTEMPTS: usize = 8;
const AUTH_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Sweep expired auth_session rows on this cadence.
const AUTH_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub bus: broadcast::Sender<Event>,
    pub started_at: Instant,
    pub version: &'static str,
    pub auth_limiter: Arc<ratelimit::RateLimiter>,
    /// SHA-256 fingerprint of the active TLS cert, formatted `AB:CD:…`.
    /// Empty when running with `--no-tls`. The dashboard wizard needs
    /// this anonymously (before login) so the user can verify it matches
    /// what `agentum serve` printed on the host TTY.
    pub cert_fingerprint: Arc<String>,
}

impl AppState {
    pub fn new(store: Store, bus: broadcast::Sender<Event>) -> Self {
        Self::with_fingerprint(store, bus, String::new())
    }

    pub fn with_fingerprint(
        store: Store,
        bus: broadcast::Sender<Event>,
        cert_fingerprint: String,
    ) -> Self {
        Self {
            store: Arc::new(store),
            bus,
            started_at: Instant::now(),
            version: env!("CARGO_PKG_VERSION"),
            auth_limiter: Arc::new(ratelimit::RateLimiter::new(
                AUTH_RATE_LIMIT_ATTEMPTS,
                AUTH_RATE_LIMIT_WINDOW,
            )),
            cert_fingerprint: Arc::new(cert_fingerprint),
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
    Router::new()
        .merge(routes::health::router())
        .merge(routes::cert::router())
        .merge(routes::doctor::router())
        .merge(routes::auth::router())
        .merge(routes::sessions::router())
        .merge(routes::board::router())
        .merge(routes::notes::router())
        .merge(routes::channels::router())
        .merge(routes::events::router())
        .merge(routes::fs::router())
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .fallback(embed::static_handler)
        .with_state(state)
        // Security response headers wrap everything, including the embedded
        // SPA. See `headers.rs` for the CSP rationale.
        .layer(axum_mw::from_fn(headers::security_headers))
        .layer(logging::redacting_trace_layer())
}

/// Bind and serve forever. Drives both the main API server (TLS or plain),
/// the small plain-HTTP cert-server, the watchdog reconcile loop, and a
/// periodic auth-session sweeper.
pub async fn serve(opts: ServeOptions, store: Store) -> anyhow::Result<()> {
    // rustls 0.23 requires picking a CryptoProvider when both ring and
    // aws-lc-rs are pulled in transitively. We don't care which one wins;
    // ring is the smaller of the two.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Load cert artifacts up-front so the fingerprint can flow into
    // AppState (needed by `/api/cert/fingerprint` for the wizard).
    let tls_artifacts = if opts.tls {
        Some(tls::ensure_artifacts()?)
    } else {
        None
    };
    let cert_fingerprint = tls_artifacts
        .as_ref()
        .and_then(|a| tls::cert_fingerprint(&a.cert_pem).ok())
        .unwrap_or_default();

    let (bus, _) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
    let state = AppState::with_fingerprint(store, bus.clone(), cert_fingerprint);

    if state.store.count_users().await.unwrap_or(0) == 0 {
        tracing::warn!(
            "no users registered yet — open the dashboard to register the first one (or run `agentum auth add <name>` on the host)"
        );
    }

    // Sweep stale tokens once at boot, then on a slow timer.
    let sweep_state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(AUTH_SWEEP_INTERVAL);
        // First tick fires immediately; that's fine for the boot sweep.
        loop {
            tick.tick().await;
            match sweep_state.store.sweep_expired_auth_sessions().await {
                Ok(0) => {}
                Ok(n) => tracing::info!(rows = n, "swept expired auth sessions"),
                Err(e) => tracing::warn!(error = %e, "auth session sweep failed"),
            }
        }
    });

    let watchdog = agentum_watchdog::Watchdog::new(bus, state.store.clone());
    tokio::spawn(watchdog.run());

    let app = router(state.clone());

    if let Some(artifacts) = tls_artifacts {
        // Print SHA-256 fingerprint so the operator can verify out-of-band
        // when a second device first trusts the cert.
        if !state.cert_fingerprint.is_empty() {
            tracing::info!(
                "TLS cert fingerprint (verify on second device): SHA-256 {}",
                state.cert_fingerprint
            );
        }
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
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await;
        cert_handle.abort();
        result.map_err(Into::into)
    } else {
        tracing::warn!(addr = %opts.addr, "agentum-server listening (plain http; --no-tls)");
        let listener = TcpListener::bind(opts.addr).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .map_err(Into::into)
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
                        [(axum::http::header::CONTENT_TYPE, "application/x-pem-file")],
                        pem.as_str().to_string(),
                    )
                }
            }),
        )
        .fallback(get(cert_redirect_hint))
        .layer(logging::redacting_trace_layer())
}

async fn cert_redirect_hint(_: Request<Body>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        "agentum cert server. GET /api/cert for the PEM. The full app is on the TLS port.\n",
    )
}
