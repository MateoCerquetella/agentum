use std::fs::OpenOptions;
use std::io::Read as _;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use reqwest::Url;
use zeroize::{Zeroize, Zeroizing};

const CLIENT_ID_ENV: &str = "AGENTUM_JIRA_BROKER_CLIENT_ID";
const CLIENT_SECRET_ENV: &str = "AGENTUM_JIRA_BROKER_CLIENT_SECRET";
const CLIENT_SECRET_FILE_ENV: &str = "AGENTUM_JIRA_BROKER_CLIENT_SECRET_FILE";
const PUBLIC_URL_ENV: &str = "AGENTUM_JIRA_BROKER_PUBLIC_URL";
const BIND_ENV: &str = "AGENTUM_JIRA_BROKER_BIND";
const TRUST_PROXY_TLS_ENV: &str = "AGENTUM_JIRA_BROKER_TRUST_PROXY_TLS";
const DATABASE_ENV: &str = "AGENTUM_JIRA_BROKER_DB";
const MAX_SECRET_BYTES: u64 = 64 * 1024;

pub(crate) struct ClientSecret(Zeroizing<String>);

impl ClientSecret {
    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl Drop for ClientSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub struct BrokerConfig {
    pub bind: SocketAddr,
    pub database_path: PathBuf,
    pub public_url: Url,
    pub(crate) callback_url: Url,
    pub(crate) client_id: String,
    pub(crate) client_secret: ClientSecret,
    /// Test-only upstream injection. Production request targets are compiled
    /// from exact Atlassian constants in `atlassian.rs` and cannot be changed
    /// by environment, broker input, or persisted state.
    #[cfg(test)]
    pub(crate) endpoints: AtlassianEndpoints,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct AtlassianEndpoints {
    pub authorization: Url,
    pub token: Url,
    pub accessible_resources: Url,
}

#[cfg(test)]
impl Default for AtlassianEndpoints {
    fn default() -> Self {
        Self {
            authorization: Url::parse("https://auth.atlassian.com/authorize")
                .expect("static Atlassian authorization URL is valid"),
            token: Url::parse("https://auth.atlassian.com/oauth/token")
                .expect("static Atlassian token URL is valid"),
            accessible_resources: Url::parse(
                "https://api.atlassian.com/oauth/token/accessible-resources",
            )
            .expect("static Atlassian resources URL is valid"),
        }
    }
}

impl BrokerConfig {
    pub fn from_environment() -> Result<Self, ConfigError> {
        let client_id = required_env(CLIENT_ID_ENV)?;
        validate_bounded_value(&client_id, 512)
            .map_err(|()| ConfigError::Invalid(CLIENT_ID_ENV))?;

        let direct_secret = std::env::var(CLIENT_SECRET_ENV).ok();
        let secret_file = std::env::var(CLIENT_SECRET_FILE_ENV).ok();
        let secret = match (direct_secret, secret_file) {
            (Some(_), Some(_)) => return Err(ConfigError::DuplicateSecret),
            (Some(value), None) => value,
            (None, Some(path)) => read_secret_file(Path::new(path.trim()))?,
            (None, None) => return Err(ConfigError::Missing(CLIENT_SECRET_FILE_ENV)),
        };
        validate_bounded_value(&secret, MAX_SECRET_BYTES as usize)
            .map_err(|()| ConfigError::Invalid(CLIENT_SECRET_FILE_ENV))?;

        let public_url = parse_public_url(&required_env(PUBLIC_URL_ENV)?)?;
        let callback_url = public_url
            .join("v1/jira/oauth/callback")
            .map_err(|_| ConfigError::Invalid(PUBLIC_URL_ENV))?;
        let bind = std::env::var(BIND_ENV)
            .unwrap_or_else(|_| "127.0.0.1:8787".to_owned())
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::Invalid(BIND_ENV))?;
        if !bind.ip().is_loopback()
            && !std::env::var(TRUST_PROXY_TLS_ENV)
                .ok()
                .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        {
            return Err(ConfigError::ProxyTlsRequired);
        }

        let database_path = PathBuf::from(required_env(DATABASE_ENV)?);
        validate_absolute_path(&database_path).map_err(|()| ConfigError::Invalid(DATABASE_ENV))?;

        Ok(Self {
            bind,
            database_path,
            public_url,
            callback_url,
            client_id,
            client_secret: ClientSecret(Zeroizing::new(secret)),
            #[cfg(test)]
            endpoints: AtlassianEndpoints::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        database_path: PathBuf,
        public_url: Url,
        endpoints: AtlassianEndpoints,
    ) -> Self {
        let callback_url = public_url.join("v1/jira/oauth/callback").unwrap();
        Self {
            bind: "127.0.0.1:0".parse().unwrap(),
            database_path,
            public_url,
            callback_url,
            client_id: "test-client".to_owned(),
            client_secret: ClientSecret(Zeroizing::new("test-secret".to_owned())),
            endpoints,
        }
    }
}

fn required_env(name: &'static str) -> Result<String, ConfigError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

fn parse_public_url(raw: &str) -> Result<Url, ConfigError> {
    let mut url = Url::parse(raw.trim()).map_err(|_| ConfigError::Invalid(PUBLIC_URL_ENV))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(ConfigError::Invalid(PUBLIC_URL_ENV));
    }
    url.set_path("/");
    Ok(url)
}

fn validate_bounded_value(value: &str, maximum: usize) -> Result<(), ()> {
    if value.trim().is_empty()
        || value.len() > maximum
        || value.chars().any(|character| character.is_control())
    {
        return Err(());
    }
    Ok(())
}

fn validate_absolute_path(path: &Path) -> Result<(), ()> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::CurDir | Component::Prefix(_)
            )
        })
    {
        return Err(());
    }
    Ok(())
}

fn read_secret_file(path: &Path) -> Result<String, ConfigError> {
    validate_absolute_path(path).map_err(|()| ConfigError::Invalid(CLIENT_SECRET_FILE_ENV))?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| ConfigError::SecretFile("cannot be opened"))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_SECRET_BYTES {
        return Err(ConfigError::SecretFile("must be a bounded regular file"));
    }
    validate_owner_only_secret(path, &metadata)?;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| ConfigError::SecretFile("cannot be opened safely"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SECRET_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ConfigError::SecretFile("cannot be read"))?;
    if bytes.len() as u64 > MAX_SECRET_BYTES {
        bytes.zeroize();
        return Err(ConfigError::SecretFile("is too large"));
    }
    let mut value =
        String::from_utf8(bytes).map_err(|_| ConfigError::SecretFile("must contain UTF-8 text"))?;
    let trimmed_len = value.trim_end_matches(['\r', '\n']).len();
    value.truncate(trimmed_len);
    Ok(value)
}

#[cfg(unix)]
fn validate_owner_only_secret(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), ConfigError> {
    use std::os::unix::fs::MetadataExt as _;

    let effective_uid = unsafe { libc::geteuid() };
    if !acceptable_secret_permissions(path, metadata.uid(), effective_uid, metadata.mode()) {
        return Err(ConfigError::SecretFile(
            "must be owner-only or a read-only root-owned /run/secrets file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn acceptable_secret_permissions(
    path: &Path,
    owner_uid: u32,
    effective_uid: u32,
    mode: u32,
) -> bool {
    let owner_only = owner_uid == effective_uid && mode & 0o077 == 0;
    let container_secret = owner_uid == 0 && path.starts_with("/run/secrets/") && mode & 0o022 == 0;
    owner_only || container_secret
}

#[cfg(not(unix))]
fn validate_owner_only_secret(
    _path: &Path,
    _metadata: &std::fs::Metadata,
) -> Result<(), ConfigError> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("required environment variable {0} is not configured")]
    Missing(&'static str),
    #[error("environment variable {0} is invalid")]
    Invalid(&'static str),
    #[error(
        "configure only one of AGENTUM_JIRA_BROKER_CLIENT_SECRET or AGENTUM_JIRA_BROKER_CLIENT_SECRET_FILE"
    )]
    DuplicateSecret,
    #[error("the configured client-secret file {0}")]
    SecretFile(&'static str),
    #[error(
        "non-loopback binding requires AGENTUM_JIRA_BROKER_TRUST_PROXY_TLS=true and a TLS reverse proxy"
    )]
    ProxyTlsRequired,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn accepts_owner_only_and_compose_style_secrets_but_not_mutable_files() {
        assert!(acceptable_secret_permissions(
            Path::new("/secure/client-secret"),
            10001,
            10001,
            0o100600,
        ));
        assert!(acceptable_secret_permissions(
            Path::new("/run/secrets/atlassian_client_secret"),
            0,
            10001,
            0o100444,
        ));
        assert!(!acceptable_secret_permissions(
            Path::new("/tmp/client-secret"),
            0,
            10001,
            0o100444,
        ));
        assert!(!acceptable_secret_permissions(
            Path::new("/run/secrets/atlassian_client_secret"),
            0,
            10001,
            0o100666,
        ));
    }
}
