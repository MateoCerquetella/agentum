//! axum HTTP server for agentum. Plain HTTP only in phase 1.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use agentum_store::Store;
use axum::Router;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

mod embed;
mod error;
mod routes;

pub use error::ApiError;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub started_at: Instant,
    pub version: &'static str,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self {
            store: Arc::new(store),
            started_at: Instant::now(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .allow_origin(Any);

    Router::new()
        .merge(routes::health::router())
        .merge(routes::sessions::router())
        .fallback(embed::static_handler)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
}

/// Bind to `addr` and serve forever.
pub async fn serve(addr: SocketAddr, store: Store) -> anyhow::Result<()> {
    let state = AppState::new(store);
    let app = router(state);
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "agentum-server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
