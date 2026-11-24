//! The web app (Leptos, wasm), embedded in the binary. Unknown paths get index.html so
//! client-side routes (/mail, /calendar, /today…) load directly.
use axum::{http::{header, StatusCode, Uri}, response::{IntoResponse, Response}};

#[derive(rust_embed::Embed)]
#[folder = "../agentum-web/dist/"]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(f) = Assets::get(path).filter(|_| !path.is_empty()) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        let mut r = ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.to_string())], f.data).into_response();
        // The old worker lived under /static/ but controlled the whole site.
        if path == "static/homelab-sw.js" { r.headers_mut().insert("service-worker-allowed", axum::http::HeaderValue::from_static("/")); }
        return r;
    }
    if path.starts_with("api/") { return StatusCode::NOT_FOUND.into_response(); }
    match Assets::get("index.html") {
        Some(f) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()), (header::CACHE_CONTROL, "no-cache".to_string())], f.data).into_response(),
        None => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}
