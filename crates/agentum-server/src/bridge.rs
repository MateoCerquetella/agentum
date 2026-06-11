//! `DesktopBridge` — the in-process hook the embedded server uses to reach the
//! Tauri desktop process (its webviews and the macOS Accessibility engine).
//!
//! Browser automation and computer-use can only be done by the process that
//! owns the webviews / holds the Accessibility TCC grant — i.e. the desktop
//! app, NOT a standalone `agentum serve`. So the server keeps an optional
//! bridge: present when the server is embedded in the desktop (it forwards
//! `/api/browser/*` and `/api/computer/*` ops to it), absent for the standalone
//! daemon (those routes honestly return `501 Not Implemented`).
//!
//! Ops are passed as `serde_json::Value` so this crate needs no knowledge of
//! every op shape (and no Tauri dependency); the desktop-side implementation
//! interprets them.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// A boxed, sendable future of a bridge op result. (`async-trait` isn't a dep,
/// so the trait is made object-safe with an explicit boxed future.)
pub type BridgeFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send + 'a>>;

/// Implemented by the desktop shell; stored as `Option<Arc<dyn DesktopBridge>>`
/// in `AppState`. Each method takes the request body verbatim and returns the
/// JSON the route forwards back to the client.
pub trait DesktopBridge: Send + Sync {
    /// Drive a browser webview op: `{op: "tabs"|"navigate"|"snapshot"|"click"|
    /// "fill"|"screenshot", ...}`.
    fn browser(&self, op: Value) -> BridgeFuture<'_>;
    /// Drive a macOS computer-use op: `{op: "capabilities"|"permissions"|
    /// "list-apps"|"get-app-state"|"click"|"set-value"|"type-text"|"press-key"|
    /// "scroll", ...}`.
    fn computer(&self, op: Value) -> BridgeFuture<'_>;
}
