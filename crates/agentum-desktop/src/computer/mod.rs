//! macOS computer-use engine: enumerate apps, read an app's Accessibility tree,
//! and drive it (press a control, set a value, type text). Runs in the desktop
//! .app process, which holds the Accessibility TCC grant (see
//! `commands/permissions.rs`) — that's why computer-use lives here and is
//! reachable only through the `DesktopBridge`, never a standalone daemon.
//!
//! Non-macOS builds compile a stub that reports the feature unsupported.

#[cfg(target_os = "macos")]
mod imp;

use serde_json::Value;

/// Dispatch a `{op: "...", ...}` computer-use request. Returns the JSON the
/// `/api/computer/*` route forwards back. Unknown ops are a 4xx-style error
/// value (the bridge maps Err → 500, so we return Ok with an `error` field for
/// "unsupported op" to keep the contract simple).
pub fn handle(op: &str, args: &Value) -> anyhow::Result<Value> {
    #[cfg(target_os = "macos")]
    {
        imp::handle(op, args)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (op, args);
        Ok(serde_json::json!({ "error": "computer-use is macOS-only", "platform": "non-macos" }))
    }
}
