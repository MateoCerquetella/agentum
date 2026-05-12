//! axum HTTP(S) server for agentum.
//!
//! HTTPS via self-signed rustls cert + bearer-token middleware on `/api/*`
//! (excluding `/api/health` + `/api/cert`). A plain-HTTP cert-server runs
//! on a side port for trust-on-first-use bootstrap.

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
mod transcript_store;

pub use transcript_store::TranscriptStore;

pub use error::ApiError;

/// Resume marker for a session's WS stream. Tracks the byte offset the
/// client last received plus the pane size that was active at that
/// moment, so the next `?resume=true` can replay only the missed delta
/// — but only if the size still matches. See [`AppState::stream_positions`].
#[derive(Clone, Copy, Debug)]
pub struct StreamCheckpoint {
    pub pos: u64,
    pub cols: u16,
    pub rows: u16,
}

/// Capacity of the broadcast bus that fans `Event`s out to every
/// connected SSE / WS client. Slow consumers that lag behind by more
/// than this many events get a `Lagged(n)` error and miss those
/// events — for the activity dots that means a sticky grey while the
/// agent is actually working again. Bumped from 256 → 1024 so a
/// transient client hiccup (focus-stolen TUI, network roundtrip
/// stall, etc.) won't drop a state-change event the user is staring
/// at.
const EVENT_BUS_CAPACITY: usize = 1024;

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
    /// Per-session in-memory cache of plan/todos/tasks, populated by
    /// tailing each agent's Claude Code transcript. See
    /// [`transcript_store`] for how it stays in sync.
    pub transcripts: TranscriptStore,
    /// Per-session resume checkpoint. When a client reconnects with
    /// `{"resume":true}`, the WS handler uses this to replay only the
    /// bytes the client missed during the gap (typically a session-
    /// switch round trip) instead of sending a full `capture-pane`
    /// snapshot that would clobber the client's preserved parser state
    /// with whatever the agent's UI happens to look like *now*. The
    /// checkpoint also remembers the pane size at last save so the
    /// handler can invalidate the resume when the client's viewport
    /// changed during the disconnect — replaying bytes emitted at a
    /// different grid size produces visible layout corruption (cursor
    /// moves target stale cells) that's hard to recover from short of
    /// a full snapshot.
    pub stream_positions:
        Arc<std::sync::Mutex<std::collections::HashMap<uuid::Uuid, StreamCheckpoint>>>,
    /// Short hostname of the box this daemon runs on. Cached once at
    /// boot so the `/api/health` reads are zero-cost. Clients use it to
    /// label the "this server" row with a meaningful identity (e.g.
    /// `omarchy`, `mateo-mac`) instead of a generic placeholder. Cut
    /// at the first `.` so `omarchy.local` reads as `omarchy`.
    pub hostname: String,
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
        let transcripts = TranscriptStore::new(bus.clone());
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
            transcripts,
            stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: detect_short_hostname(),
        }
    }
}

/// Resolve the system hostname once at boot, trimming the FQDN suffix
/// so `omarchy.local` reads as `omarchy`. Falls back to `"local"` when
/// the OS lookup fails — better than empty, which would re-trigger
/// the generic placeholder on the client.
fn detect_short_hostname() -> String {
    let raw = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match raw {
        Some(name) => name
            .split('.')
            .next()
            .unwrap_or(&name)
            .to_ascii_lowercase(),
        None => "local".to_string(),
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
        .merge(routes::host::router())
        .merge(routes::cert::router())
        .merge(routes::doctor::router())
        .merge(routes::auth::router())
        .merge(routes::sessions::router())
        .merge(routes::agents::router())
        .merge(routes::agent_tasks::router())
        .merge(routes::board::router())
        .merge(routes::notes::router())
        .merge(routes::preferences::router())
        .merge(routes::channels::router())
        .merge(routes::events::router())
        .merge(routes::watchdog::router())
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
        // CORS: permissive on origin so a dashboard hosted by one daemon
        // can talk to another daemon (the named-profiles feature). The
        // API is still bearer-protected — an open `Allow-Origin` only
        // grants the same access an unauthenticated cross-origin call
        // would already get (none, except `/api/health` + `/api/cert`).
        // We intentionally do NOT set `Allow-Credentials: true` because
        // we don't use cookies; the bearer is sent via `Authorization`
        // header (or `?token=` for WS), so wildcard-origin is safe.
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
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

    let watchdog = agentum_watchdog::Watchdog::new(bus.clone(), state.store.clone());
    tokio::spawn(watchdog.run());

    // Background ticker that publishes `host.metrics` (CPU + RAM) onto
    // the broadcast bus every couple of seconds. The /api/events WS
    // fans these out to every connected dashboard, so a single sampler
    // feeds N tabs without per-client polling.
    routes::host::spawn_ticker(bus);

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
