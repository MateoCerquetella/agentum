//! Response middleware that stamps the standard browser security headers.
//!
//! - `Content-Security-Policy`: same-origin everything, no framing. The
//!   server is JSON-only — the dashboard frontend is hosted separately
//!   (Netlify) and talks to this API cross-origin.
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

// CSP for a JSON API. `default-src 'self'` is mostly defence-in-depth
// since responses don't render HTML; the inline-script/style allowances
// kept from the old embedded-SPA era are dropped. If you ever serve
// HTML directly from this server again, revisit this.
const CSP: &str = concat!(
    "default-src 'none'; ",
    "frame-ancestors 'none'; ",
    "base-uri 'none'; ",
    "form-action 'none'",
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
