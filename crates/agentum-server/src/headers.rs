//! Response middleware that stamps the standard browser security headers.
//!
//! - `Content-Security-Policy`: same-origin everything, no framing. The
//!   embedded SvelteKit SPA needs `'unsafe-inline'` for its bootstrap
//!   script (per-build random `__sveltekit_<id>` suffix can't be
//!   externalised) and for component-level inline styles.
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

// SvelteKit injects an inline bootstrap script in index.html that calls
// `import("/_app/immutable/...")` to start the SPA. That script can't be
// extracted to an external file (it carries a per-build random suffix on
// `__sveltekit_<id>` and must run inline). We therefore allow
// `'unsafe-inline'` for script-src; everything else still requires
// same-origin. Same posture for style-src — Svelte components emit inline
// `style` attributes for transitions/dynamic props.
const CSP: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self' 'unsafe-inline'; ",
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
