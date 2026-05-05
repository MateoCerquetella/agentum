//! Self-signed TLS for `agentum serve`.
//!
//! On first boot we generate a long-lived self-signed cert covering
//! `localhost` + common LAN IPs and write it to
//! `$XDG_DATA_HOME/agentum/tls/{cert,key}.pem`. Subsequent boots reuse the
//! files. Browsers will warn — that's expected; the cert-server on :8823
//! serves the same PEM so a phone can trust-on-first-use.

use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("cert generation failed: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("path: {0}")]
    Path(#[from] agentum_store::paths::PathError),
}

pub struct TlsArtifacts {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    /// PEM contents, used by the cert-server endpoint to return cert bytes
    /// without a re-read.
    pub cert_pem: String,
}

/// Ensure `cert.pem` and `key.pem` exist under `$XDG_DATA_HOME/agentum/tls/`
/// (creating them if missing). Returns paths + cert PEM contents.
pub fn ensure_artifacts() -> Result<TlsArtifacts, TlsError> {
    let dir = agentum_store::paths::tls_dir()?;
    std::fs::create_dir_all(&dir)?;
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    if !cert_path.exists() || !key_path.exists() {
        let (cert_pem, key_pem) = generate()?;
        write_secret(&cert_path, &cert_pem)?;
        write_secret(&key_path, &key_pem)?;
        tracing::info!(?cert_path, "generated self-signed certificate");
    }

    let cert_pem = std::fs::read_to_string(&cert_path)?;
    Ok(TlsArtifacts {
        cert_path,
        key_path,
        cert_pem,
    })
}

fn generate() -> Result<(String, String), TlsError> {
    let mut params =
        CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into(), "::1".into()])?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "agentum self-signed");

    // Some browsers want SANs explicit
    params.subject_alt_names.push(SanType::DnsName("localhost".try_into()?));

    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok((cert.pem(), key.serialize_pem()))
}

fn write_secret(path: &Path, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)?;
    set_mode_0600(path)
}

#[cfg(unix)]
fn set_mode_0600(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    perm.set_mode(0o600);
    std::fs::set_permissions(path, perm)
}

#[cfg(not(unix))]
fn set_mode_0600(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
