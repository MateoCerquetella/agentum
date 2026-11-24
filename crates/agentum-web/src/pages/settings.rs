//! Settings.
use super::Pane;
use leptos::prelude::*;
use serde_json::Value;

pub fn page(_pane: Pane) -> AnyView {
    super::todo("Settings")
}

/// Can this browser use passkeys (WebAuthn over https)?
pub fn passkeys_work() -> bool {
    web_sys::window().is_some_and(|w| w.location().protocol().unwrap_or_default() == "https:" && js_sys::Reflect::has(&w, &"PublicKeyCredential".into()).unwrap_or(false))
}

/// Sign in with a passkey. Err("") when the person cancelled.
pub async fn passkey_sign_in() -> Result<Value, String> {
    Err("Passkeys arrive with Settings.".into())
}
