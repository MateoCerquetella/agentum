//! Response middleware that stamps the standard browser security headers.
//!
//! - `Content-Security-Policy`: same-origin everything, no inline scripts,
//!   no framing. The Svelte build doesn't ship inline JS or external CDNs.
//! - `X-Frame-Options: DENY`: belt-and-braces with the CSP `frame-ancestors`.
//! - `X-Content-Type-Options: nosniff`: prevents MIME-sniffing tricks.
//! - `Referrer-Policy: no-referrer`: don't leak URLs (which contain
//!   `?token=…` for WS upgrades) to anything we link to.
//! - `Strict-Transport-Security`: only meaningful when actually on TLS, but
//!   we set it unconditionally — harmless on plain HTTP and avoids thinking
//!   about which scheme the client used through any proxy.
//! - `Cross-Origin-Opener-Policy: same-origin`: isolates the browsing
//!   context from popups under a different origin.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

const CSP: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "img-src 'self' data: blob:; ",
    "font-src 'self' data:; ",
    "connect-src 'self' ws: wss:; ",
    "worker-src 'self' blob:; ",
    "manifest-src 'self'; ",
    "frame-ancestors 'none'; ",
    "base-uri 'self'; ",
    "form-action 'self'",
);

pub async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();

    fn put(headers: &mut axum::http::HeaderMap, name: HeaderName, value: &'static str) {
        // `from_static` is infallible for the literals we use here.
        headers
            .entry(name)
            .or_insert_with(|| HeaderValue::from_static(value));
    }

    put(headers, header::CONTENT_SECURITY_POLICY, CSP);
    put(headers, header::X_FRAME_OPTIONS, "DENY");
    put(headers, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    put(headers, header::REFERRER_POLICY, "no-referrer");
    put(
        headers,
        header::STRICT_TRANSPORT_SECURITY,
        "max-age=31536000",
    );
    put(
        headers,
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );

    resp
}
