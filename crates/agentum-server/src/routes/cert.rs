//! Anonymous endpoint exposing the active TLS cert's SHA-256 fingerprint.
//!
//! The first-run wizard uses it to display "verify this matches what the
//! host TTY printed" *before* the user has logged in (so it must be on
//! the public allowlist in `auth::is_public`).
//!
//! Empty when running with `--no-tls` — the wizard hides the verify step
//! in that case (there's no cert to pin).

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/cert/fingerprint", get(fingerprint))
}

#[derive(Serialize)]
struct Resp {
    /// Empty string when TLS is disabled.
    sha256: String,
    /// True when running on TLS. Saves the client a string-empty check.
    tls: bool,
}

async fn fingerprint(State(state): State<AppState>) -> Json<Resp> {
    let fp = state.cert_fingerprint.as_str();
    Json(Resp {
        sha256: fp.to_string(),
        tls: !fp.is_empty(),
    })
}
