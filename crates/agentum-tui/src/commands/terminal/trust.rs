//! SSH-style trust-on-first-use for the agentum CLI / TUI.
//!
//! The server's TLS cert is self-signed by default. Rather than ship a
//! third-party tunnel or force users into a CA flow, we pin the SHA-256
//! fingerprint of each host the first time we see it (after the user
//! confirms it matches what `agentum serve` prints on the host TTY).
//! Subsequent connections silently verify the cert against the pin.
//!
//! Storage: `$XDG_CONFIG_HOME/agentum/known_hosts.toml` (chmod 0600).
//! Format:
//!
//! ```toml
//! ["my-vps.example.com:8822"]
//! sha256 = "AB:CD:..."
//! added_at = "2026-05-05T12:34:56Z"
//! ```
//!
//! Plus a sibling `credentials.toml` with one bearer token per host so
//! you don't retype credentials when switching between machines.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustls::ClientConfig;
use rustls::DigitallySignedStruct;
use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use url::Url;

/// Convenient host:port key. We don't pin per-path because all agentum
/// servers live on the same authority.
pub fn host_key(url: &Url) -> Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("URL has no host: {url}"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL has no port: {url}"))?;
    Ok(format!("{host}:{port}"))
}

/// Lowercase hex SHA-256 with colons every two digits, matching what
/// `agentum serve` prints on the host TTY.
pub fn format_fingerprint(digest: &[u8]) -> String {
    let mut s = String::with_capacity(digest.len() * 3);
    for (i, b) in digest.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Strip whitespace and uppercase, accepting either `AB:CD:…` or `abcd…`
/// shapes. Returns `Err` if the canonical form isn't 32 colon-separated
/// hex pairs.
pub fn normalize_fingerprint(raw: &str) -> Result<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace(':', "")
        .to_uppercase();
    if cleaned.len() != 64 || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "fingerprint must be 32 hex pairs (got {} chars)",
            cleaned.len()
        );
    }
    let mut out = String::with_capacity(95);
    for (i, c) in cleaned.chars().enumerate() {
        if i > 0 && i % 2 == 0 {
            out.push(':');
        }
        out.push(c);
    }
    Ok(out)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KnownHostsFile {
    #[serde(flatten)]
    entries: BTreeMap<String, KnownHostEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnownHostEntry {
    sha256: String,
    #[serde(default)]
    added_at: Option<String>,
}

pub struct KnownHosts {
    path: PathBuf,
    file: KnownHostsFile,
}

impl KnownHosts {
    pub fn load() -> Result<Self> {
        let path = path()?;
        let file = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            toml::from_str::<KnownHostsFile>(&raw)
                .with_context(|| format!("parse {}", path.display()))?
        } else {
            KnownHostsFile::default()
        };
        Ok(Self { path, file })
    }

    pub fn pin(&self, host_key: &str) -> Option<&str> {
        self.file.entries.get(host_key).map(|e| e.sha256.as_str())
    }

    pub fn entries(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.file
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.sha256.clone()))
    }

    pub fn add(&mut self, host_key: String, fingerprint: String) -> Result<()> {
        self.file.entries.insert(
            host_key,
            KnownHostEntry {
                sha256: fingerprint,
                added_at: Some(now_rfc3339()),
            },
        );
        self.save()
    }

    pub fn remove(&mut self, host_key: &str) -> Result<bool> {
        let removed = self.file.entries.remove(host_key).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(&self.file)?;
        std::fs::write(&self.path, body)?;
        chmod_0600(&self.path)?;
        Ok(())
    }
}

fn path() -> Result<PathBuf> {
    let dir = agentum_store::paths::config_dir().map_err(|e| anyhow!("resolve config dir: {e}"))?;
    Ok(dir.join("known_hosts.toml"))
}

fn now_rfc3339() -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(unix)]
fn chmod_0600(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    perm.set_mode(0o600);
    std::fs::set_permissions(path, perm)?;
    Ok(())
}

#[cfg(not(unix))]
fn chmod_0600(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

/// Per-host bearer-token cache. Stored next to known_hosts so users can
/// reason about (and `rm`) one file per identity concern.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CredentialsFile {
    #[serde(flatten)]
    entries: BTreeMap<String, CredentialEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialEntry {
    token: String,
    #[serde(default)]
    username: Option<String>,
}

pub struct Credentials {
    path: PathBuf,
    file: CredentialsFile,
}

impl Credentials {
    pub fn load() -> Result<Self> {
        let path = creds_path()?;
        let file = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            toml::from_str::<CredentialsFile>(&raw).unwrap_or_default()
        } else {
            CredentialsFile::default()
        };
        Ok(Self { path, file })
    }

    pub fn token(&self, host_key: &str) -> Option<&str> {
        self.file.entries.get(host_key).map(|e| e.token.as_str())
    }

    pub fn put(&mut self, host_key: String, token: String, username: Option<String>) -> Result<()> {
        self.file
            .entries
            .insert(host_key, CredentialEntry { token, username });
        self.save()
    }

    pub fn remove(&mut self, host_key: &str) -> Result<bool> {
        let removed = self.file.entries.remove(host_key).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(&self.file)?;
        std::fs::write(&self.path, body)?;
        chmod_0600(&self.path)?;
        Ok(())
    }
}

fn creds_path() -> Result<PathBuf> {
    let dir = agentum_store::paths::config_dir().map_err(|e| anyhow!("resolve config dir: {e}"))?;
    Ok(dir.join("credentials.toml"))
}

/// Look up the bearer token stored for the daemon at `url`.
///
/// The key is derived from the URL's `host:port` pair (the same format used
/// when `agentum auth login` persists the token). Returns `None` when
/// `credentials.toml` exists but has no entry for this host — the caller is
/// responsible for surfacing the "run `agentum auth login`" hint.
pub(crate) fn token_for_url(url: &str) -> Result<Option<String>> {
    let parsed = url::Url::parse(url).with_context(|| format!("invalid profile URL: {url}"))?;
    let key = host_key(&parsed)?;
    let creds = Credentials::load()?;
    Ok(creds.token(&key).map(|t| t.to_owned()))
}

/// Open a TLS connection to `url`, accepting any cert just long enough to
/// capture the leaf, and return its SHA-256 fingerprint formatted as
/// `AB:CD:…`. Used during the TOFU prompt.
pub async fn fetch_fingerprint(url: &Url) -> Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("URL has no host: {url}"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL has no port: {url}"))?;

    install_default_provider();

    let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let verifier = Arc::new(CapturingVerifier(captured.clone()));
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect((host, port)))
        .await
        .with_context(|| format!("connect {host}:{port} timed out"))??;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| anyhow!("invalid server name: {host}"))?;
    let _tls = tokio::time::timeout(Duration::from_secs(10), connector.connect(server_name, tcp))
        .await
        .with_context(|| format!("TLS handshake to {host}:{port} timed out"))??;

    let der = captured
        .lock()
        .ok()
        .and_then(|mut g| g.take())
        .ok_or_else(|| anyhow!("server did not present a certificate"))?;
    Ok(format_fingerprint(&Sha256::digest(&der)))
}

/// Build a rustls `ClientConfig` that accepts any certificate without
/// verification. Only used when a profile has `insecure = true` — an
/// explicit, user-authored opt-in. Never the default.
///
/// Uses `use_preconfigured_tls` in reqwest rather than
/// `danger_accept_invalid_certs` so the code path is identical to the
/// fingerprint-pinned case (same builder pattern, just with a NoVerify
/// verifier instead of PinningVerifier).
pub(crate) fn accept_any_tls_config() -> Arc<ClientConfig> {
    install_default_provider();
    #[derive(Debug)]
    struct NoVerify;
    impl ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            all_schemes()
        }
    }
    let cfg = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify))
        .with_no_client_auth();
    Arc::new(cfg)
}

/// Build a rustls `ClientConfig` that accepts only certs whose SHA-256
/// matches `expected_fingerprint`. Use this for both reqwest and
/// tungstenite once a host is pinned.
pub fn pinned_tls_config(expected_fingerprint: String) -> Arc<ClientConfig> {
    install_default_provider();
    let verifier = Arc::new(PinningVerifier {
        expected: expected_fingerprint,
    });
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(config)
}

fn install_default_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[derive(Debug)]
struct CapturingVerifier(Arc<Mutex<Option<Vec<u8>>>>);

impl ServerCertVerifier for CapturingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut g) = self.0.lock() {
            *g = Some(end_entity.as_ref().to_vec());
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        all_schemes()
    }
}

#[derive(Debug)]
struct PinningVerifier {
    expected: String,
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = format_fingerprint(&Sha256::digest(end_entity.as_ref()));
        if actual.eq_ignore_ascii_case(&self.expected) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "cert fingerprint mismatch — expected {} but server presented {}",
                self.expected, actual
            )))
        }
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        all_schemes()
    }
}

fn all_schemes() -> Vec<SignatureScheme> {
    vec![
        SignatureScheme::RSA_PSS_SHA256,
        SignatureScheme::RSA_PSS_SHA384,
        SignatureScheme::RSA_PSS_SHA512,
        SignatureScheme::RSA_PKCS1_SHA256,
        SignatureScheme::RSA_PKCS1_SHA384,
        SignatureScheme::RSA_PKCS1_SHA512,
        SignatureScheme::ECDSA_NISTP256_SHA256,
        SignatureScheme::ECDSA_NISTP384_SHA384,
        SignatureScheme::ED25519,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_format_round_trips() {
        let bytes = [0xab, 0xcd, 0xef, 0x01];
        let s = format_fingerprint(&bytes);
        assert_eq!(s, "AB:CD:EF:01");
    }

    #[test]
    fn normalize_accepts_colons_or_plain() {
        // 32 hex pairs = 64 hex chars in canonical form.
        let raw = format!("ab:cd:{}", "ef".repeat(30));
        let normalized = normalize_fingerprint(&raw).unwrap();
        assert_eq!(normalized.len(), 95); // 32 pairs + 31 separators
        assert!(normalized.starts_with("AB:CD:"));

        // Same fingerprint, no separators: should normalize identically.
        let plain = "abcd".to_string() + &"ef".repeat(30);
        assert_eq!(normalize_fingerprint(&plain).unwrap(), normalized);
    }

    #[test]
    fn normalize_rejects_short_input() {
        assert!(normalize_fingerprint("AB:CD").is_err());
    }

    #[test]
    fn host_key_includes_port() {
        let u = Url::parse("https://example.com:9999/path").unwrap();
        assert_eq!(host_key(&u).unwrap(), "example.com:9999");
    }

    #[test]
    fn host_key_uses_default_port() {
        let u = Url::parse("https://example.com/path").unwrap();
        assert_eq!(host_key(&u).unwrap(), "example.com:443");
    }
}
