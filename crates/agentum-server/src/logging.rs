//! Tracing integration that redacts the `token=` query parameter from
//! access logs.
//!
//! WebSocket upgrades from browsers can't carry a custom `Authorization`
//! header, so the only practical channel is `?token=…`. We still want
//! tracing to log the URI for debugging, but the literal token must not
//! land in log files / log shippers / shell histories.

use axum::http::Uri;
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::trace::TraceLayer;
use tracing::Span;

/// Drop-in replacement for `TraceLayer::new_for_http()` that swaps the
/// span maker for one that scrubs `token=` out of the URI.
pub fn redacting_trace_layer()
-> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, RedactingMakeSpan> {
    TraceLayer::new_for_http().make_span_with(RedactingMakeSpan)
}

#[derive(Clone, Copy)]
pub struct RedactingMakeSpan;

impl<B> tower_http::trace::MakeSpan<B> for RedactingMakeSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> Span {
        let redacted = redact_token(request.uri());
        tracing::info_span!(
            "request",
            method = %request.method(),
            uri = %redacted,
            version = ?request.version(),
        )
    }
}

/// Returns the URI rendered as a string with `token=…` query values
/// replaced by `token=REDACTED`. Other params pass through.
pub fn redact_token(uri: &Uri) -> String {
    let path = uri.path();
    let Some(query) = uri.query() else {
        return path.to_string();
    };
    if !query.contains("token=") {
        return format!("{path}?{query}");
    }

    let scrubbed: String = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some(("token", _)) => "token=REDACTED".to_string(),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&");

    format!("{path}?{scrubbed}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Uri;

    #[test]
    fn redacts_only_the_token_param() {
        let u: Uri = "/api/sessions/abc/stream?token=secretvalue&format=json"
            .parse()
            .unwrap();
        assert_eq!(
            redact_token(&u),
            "/api/sessions/abc/stream?token=REDACTED&format=json"
        );
    }

    #[test]
    fn no_query_passes_through() {
        let u: Uri = "/api/health".parse().unwrap();
        assert_eq!(redact_token(&u), "/api/health");
    }

    #[test]
    fn other_params_unchanged() {
        let u: Uri = "/api/foo?a=1&b=2".parse().unwrap();
        assert_eq!(redact_token(&u), "/api/foo?a=1&b=2");
    }
}
