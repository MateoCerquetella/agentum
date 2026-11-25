//! The web app (Leptos, wasm), embedded in the binary. Unknown paths get index.html so
//! client-side routes (/mail, /calendar, /code…) load directly. Hashed assets go out as the
//! build's brotli copies when the browser takes them.
use axum::{http::{header, StatusCode, Uri}, response::{IntoResponse, Response}};

#[derive(rust_embed::Embed)]
#[folder = "../agentum-web/dist/"]
struct Assets;

pub async fn static_handler(uri: Uri, headers: axum::http::HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    // The build's brotli copy of a hashed asset, for browsers that take it.
    let br_ok = headers.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("br"));
    if br_ok && path.starts_with("assets/") {
        if let Some(f) = Assets::get(&format!("{path}.br")) {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            return ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, "public, max-age=31536000, immutable".to_string()), (header::CONTENT_ENCODING, "br".to_string()), (header::VARY, "accept-encoding".to_string())], f.data).into_response();
        }
    }
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
