use agentum_core::Status;
use axum::Json;
use axum::extract::State;
use axum::routing::get;
use axum::{Router, response::IntoResponse};
use serde::Serialize;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/health", get(health))
}

/// Capability tags advertised in the health response so clients can
/// feature-detect without parsing version strings. Whenever you add a
/// new capability that depends on server-side behaviour (currently:
/// PTY resize messages over the WS terminal stream), append a tag.
///
/// `browser.screencast.v1` (009c-3): this server can render an agent-driven
/// CDP-Chromium into the pane over `WS /api/cdp-browser/screencast`. The desktop
/// screencast pane checks for it before pointing at the embedded server, the same
/// tag the dormant client already gated on (`shared/protocol-version.ts`).
const CAPABILITIES: &[&str] = &["resize", "resume", "refresh", "browser.screencast.v1"];

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
    uptime_seconds: u64,
    sessions_running: i64,
    /// Optional features this daemon supports. Clients that send
    /// `{"resize":…}` over the terminal WS check for `"resize"` here
    /// before transmitting — older daemons forward unknown text frames
    /// as keystrokes, so silently downgrading is the only safe option.
    capabilities: &'static [&'static str],
    /// Short hostname of the box this daemon runs on (e.g. `omarchy`,
    /// `mateo-mac`). Used by clients to label the "this server" row
    /// with something meaningful — dashboards, sidebars, and switchers
    /// then read as `omarchy` instead of a generic placeholder. Cached
    /// in `AppState::hostname` at server boot so the read is free.
    hostname: String,
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    let running = state
        .store
        .count_by_status(Status::Running)
        .await
        .unwrap_or(0);

    Json(Health {
        status: "ok",
        version: state.version,
        uptime_seconds: uptime,
        sessions_running: running,
        capabilities: CAPABILITIES,
        hostname: state.hostname.clone(),
    })
}
