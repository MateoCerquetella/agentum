//! Response middleware that stamps the standard browser security headers.
//!
//! The daemon is API-only (no embedded web UI), so these headers are
//! defense-in-depth on `/api` responses rather than a policy for served
//! HTML. They cost nothing on JSON and harden any accidental HTML (error
//! pages, future endpoints).
//!
//! - `Content-Security-Policy`: same-origin everything, no framing.
//! - `X-Frame-Options: DENY`: belt-and-braces with the CSP `frame-ancestors`.
//! - `X-Content-Type-Options: nosniff`: prevents MIME-sniffing tricks.
//! - `Referrer-Policy: no-referrer`: don't leak URLs (which contain
//!   `?token=…` for WS upgrades) to anything we link to.
//! - `Cross-Origin-Opener-Policy: same-origin`: isolates the browsing
//!   context from popups under a different origin.
//!
//! Notably absent: `Strict-Transport-Security`. Self-signed certs +
//! HSTS = footgun (cert rotation strands users on the un-skippable
//! cert warning). Add HSTS at the reverse-proxy layer if you terminate
//! TLS with a real CA cert.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

// `connect-src` allows any HTTP/HTTPS origin (plus any ws/wss) so the
// named-profiles feature works: a client served by one daemon can
// fetch /api/health, /api/auth/*, /api/sessions, etc. from another
// daemon the user added as an endpoint. The bearer-token wall on the
// daemon side is what actually gates access. The `'unsafe-inline'`
// allowances are retained for any HTML the daemon might emit (error
// pages); they are inert for JSON responses.
const CSP: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self' 'unsafe-inline'; ",
    "style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; ",
    "img-src 'self' data: blob:; ",
    "font-src 'self' data: https://fonts.gstatic.com; ",
    "connect-src 'self' http: https: ws: wss:; ",
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
    // HSTS deliberately omitted: agentum ships with a self-signed cert by
    // default, and HSTS on a self-signed origin is a footgun. Once a browser
    // pins HSTS for the host, rotating the cert (e.g. `agentum auth reset`
    // or `tls/` regeneration) leaves users stuck on the un-skippable cert
    // warning interstitial. Operators terminating TLS at a reverse proxy
    // with a real CA cert should set HSTS at the proxy layer instead.
    put(
        headers,
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );

    resp
}
