//! Embed the SvelteKit static bundle into the binary and serve it.
//!
//! Layout produced by `pnpm --dir dashboard build`:
//!   dashboard/build/index.html
//!   dashboard/build/_app/immutable/{chunks,nodes,entry,assets}/...  (content-hashed)
//!   dashboard/build/favicon.svg
//!   dashboard/build/manifest.webmanifest
//!
//! Strategy:
//! - Direct hits (e.g. `/_app/immutable/.../foo.js`) get served with the
//!   matching mime type and a long-lived cache header.
//! - Anything else falls back to `index.html` so client-side routing works.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../dashboard/build"]
struct DashboardAssets;

pub async fn static_handler(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    if let Some(file) = DashboardAssets::get(path) {
        let mime: &str = file.metadata.mimetype();
        let cache = if path.starts_with("_app/immutable/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime)
                    .unwrap_or(HeaderValue::from_static("application/octet-stream")),
            )
            .header(header::CACHE_CONTROL, cache)
            .body(Body::from(file.data.into_owned()))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    // SPA fallback. Anything not under /api/* and not matching a real asset
    // resolves to the SvelteKit shell so the client router takes over.
    match DashboardAssets::get("index.html") {
        Some(file) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(file.data.into_owned()))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        None => (StatusCode::NOT_FOUND, "dashboard bundle missing").into_response(),
    }
}
