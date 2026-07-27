//! Verification for Tauri updater artifacts.
//!
//! Tauri stores both the Minisign public-key file and detached signature as
//! base64 strings. Release automation uses this module so it exercises the
//! same decoding and cryptographic verification contract as installed clients.

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};

pub fn verify_tauri_updater_signature(
    public_key_base64: &str,
    signature_base64: &str,
    artifact: &[u8],
) -> Result<()> {
    let public_key_file = STANDARD
        .decode(public_key_base64.trim())
        .context("embedded updater public key is not valid base64")?;
    let public_key_file = std::str::from_utf8(&public_key_file)
        .context("embedded updater public key is not UTF-8")?;
    let public_key = PublicKey::decode(public_key_file)
        .map_err(|error| anyhow::anyhow!("invalid Minisign public key: {error}"))?;

    let signature_file = STANDARD
        .decode(signature_base64.trim())
        .context("updater signature is not valid base64")?;
    let signature_file =
        std::str::from_utf8(&signature_file).context("updater signature is not UTF-8")?;
    let signature = Signature::decode(signature_file)
        .map_err(|error| anyhow::anyhow!("invalid Minisign signature: {error}"))?;

    if let Err(error) = public_key.verify(artifact, &signature, true) {
        bail!("updater signature verification failed: {error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_KEY_FILE: &str = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n";
    const SIGNATURE_FILE: &str = "untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==\n";

    #[test]
    fn accepts_valid_tauri_encoded_signature() {
        verify_tauri_updater_signature(
            &STANDARD.encode(PUBLIC_KEY_FILE),
            &STANDARD.encode(SIGNATURE_FILE),
            b"test",
        )
        .unwrap();
    }

    #[test]
    fn rejects_artifact_tampering() {
        assert!(
            verify_tauri_updater_signature(
                &STANDARD.encode(PUBLIC_KEY_FILE),
                &STANDARD.encode(SIGNATURE_FILE),
                b"Test",
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_a_signature_from_a_different_updater_key() {
        const AGENTUM_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc5MTQ2QzM5QzkwQzUwQTEKUldTaFVBekpPV3dVZVhVNVc1NFBpVnBFeUpYcEE5NUEydkUxMFFxblhEV3VBUXM0QzlDUXo1K1oK";
        assert!(
            verify_tauri_updater_signature(
                AGENTUM_PUBLIC_KEY,
                &STANDARD.encode(SIGNATURE_FILE),
                b"test",
            )
            .is_err()
        );
    }
}
