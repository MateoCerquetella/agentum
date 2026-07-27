//! Secure credential boundary for SDD source integrations.
//!
//! The embedded desktop selects [`OsCredentialVault`], which delegates secret
//! persistence to the platform credential store. A standalone server selects
//! [`HeadlessEncryptedVault`] only when an operator supplies a 256-bit master
//! key through `AGENTUM_SDD_VAULT_MASTER_KEY`. There is deliberately no
//! plaintext-file or legacy-integration fallback.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const VAULT_ENV: &str = "AGENTUM_SDD_VAULT_MASTER_KEY";
const VAULT_FORMAT: &str = "agentum-sdd-credential-vault";
const VAULT_VERSION: u32 = 1;
const VAULT_AAD: &[u8] = b"agentum-sdd-credential-vault:v1";
const MAX_VAULT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SECRET_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("secure credential storage is unavailable: {0}")]
    Unavailable(String),
    #[error("credential key is invalid")]
    InvalidKey,
    #[error("credential value is invalid")]
    InvalidValue,
    #[error("credential vault is unsafe or corrupted")]
    Unsafe,
    #[error("credential vault I/O failed")]
    Io(#[source] std::io::Error),
    #[error("credential vault operation failed")]
    Backend,
}

impl From<std::io::Error> for VaultError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// A namespaced key. Components are intentionally conservative because they
/// become OS-keyring attributes and encrypted-map keys.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CredentialKey {
    provider: String,
    kind: String,
    connection_id: String,
}

impl CredentialKey {
    pub fn new(
        provider: impl Into<String>,
        kind: impl Into<String>,
        connection_id: impl Into<String>,
    ) -> Result<Self, VaultError> {
        let key = Self {
            provider: provider.into(),
            kind: kind.into(),
            connection_id: connection_id.into(),
        };
        if !valid_component(&key.provider, 32)
            || !valid_component(&key.kind, 64)
            || !valid_component(&key.connection_id, 256)
        {
            return Err(VaultError::InvalidKey);
        }
        Ok(key)
    }

    fn map_key(&self) -> String {
        format!("{}/{}/{}", self.provider, self.kind, self.connection_id)
    }

    fn service(&self) -> String {
        format!("dev.agentum.sdd.{}.{}", self.provider, self.kind)
    }

    fn account(&self) -> &str {
        &self.connection_id
    }
}

fn valid_component(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@')
        })
}

/// Secret bytes zeroized when dropped. Debug output never includes the value.
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self, VaultError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_SECRET_BYTES {
            return Err(VaultError::InvalidValue);
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub backend: &'static str,
    pub persistent: bool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub trait SddCredentialVault: Send + Sync {
    fn status(&self) -> VaultStatus;
    fn get(&self, key: &CredentialKey) -> Result<Option<SecretValue>, VaultError>;
    fn put(&self, key: &CredentialKey, value: &SecretValue) -> Result<(), VaultError>;
    fn delete(&self, key: &CredentialKey) -> Result<(), VaultError>;
}

pub fn get_json<T: for<'de> Deserialize<'de>>(
    vault: &dyn SddCredentialVault,
    key: &CredentialKey,
) -> Result<Option<T>, VaultError> {
    vault
        .get(key)?
        .map(|secret| serde_json::from_slice(secret.expose()).map_err(|_| VaultError::Unsafe))
        .transpose()
}

pub fn put_json<T: Serialize>(
    vault: &dyn SddCredentialVault,
    key: &CredentialKey,
    value: &T,
) -> Result<(), VaultError> {
    let bytes = serde_json::to_vec(value).map_err(|_| VaultError::InvalidValue)?;
    vault.put(key, &SecretValue::new(bytes)?)
}

fn provider_conformance_signing_key() -> CredentialKey {
    CredentialKey::new(
        "agentum",
        "provider-conformance",
        "installation-signing-key",
    )
    .expect("static provider conformance credential key is valid")
}

/// Load the installation-owned Ed25519 key used to authenticate custom SDD
/// provider approvals. The bytes are PKCS#8 and never leave the credential
/// boundary except while the conformance runner signs or verifies a receipt.
pub(crate) fn get_provider_conformance_signing_key(
    vault: &dyn SddCredentialVault,
) -> Result<Option<SecretValue>, VaultError> {
    vault.get(&provider_conformance_signing_key())
}

/// Persist the installation-owned Ed25519 key. There is deliberately no file
/// fallback: if the selected secure vault cannot persist it, custom provider
/// approval fails closed.
pub(crate) fn put_provider_conformance_signing_key(
    vault: &dyn SddCredentialVault,
    pkcs8: &[u8],
) -> Result<(), VaultError> {
    vault.put(
        &provider_conformance_signing_key(),
        &SecretValue::new(pkcs8.to_vec())?,
    )
}

pub struct LinearCredential {
    pub connection_id: String,
    token: SecretValue,
}

impl LinearCredential {
    pub fn token(&self) -> &str {
        // All constructors validate UTF-8 before creating this value.
        std::str::from_utf8(self.token.expose()).expect("validated Linear credential")
    }
}

pub fn linear_credential_key(connection_id: &str) -> Result<CredentialKey, VaultError> {
    CredentialKey::new("linear", "api-token", connection_id)
}

pub fn linear_selected_key() -> CredentialKey {
    CredentialKey::new("linear", "api-token", "selected")
        .expect("static Linear credential key is valid")
}

pub fn put_linear_credential(
    vault: &dyn SddCredentialVault,
    connection_id: &str,
    token: &str,
    selected: bool,
) -> Result<(), VaultError> {
    if token.trim().is_empty() || token.len() > 16 * 1024 || token.chars().any(char::is_control) {
        return Err(VaultError::InvalidValue);
    }
    let key = linear_credential_key(connection_id)?;
    vault.put(&key, &SecretValue::new(token.as_bytes().to_vec())?)?;
    if selected {
        select_linear_credential(vault, connection_id)?;
    }
    Ok(())
}

pub fn select_linear_credential(
    vault: &dyn SddCredentialVault,
    connection_id: &str,
) -> Result<(), VaultError> {
    linear_credential_key(connection_id)?;
    if vault.get(&linear_credential_key(connection_id)?)?.is_none() {
        return Err(VaultError::InvalidValue);
    }
    vault.put(
        &linear_selected_key(),
        &SecretValue::new(connection_id.as_bytes().to_vec())?,
    )
}

pub fn get_linear_credential(
    vault: &dyn SddCredentialVault,
    connection_id: Option<&str>,
) -> Result<Option<LinearCredential>, VaultError> {
    if let Some(connection_id) = connection_id {
        let Some(token) = vault.get(&linear_credential_key(connection_id)?)? else {
            return Ok(None);
        };
        std::str::from_utf8(token.expose()).map_err(|_| VaultError::Unsafe)?;
        return Ok(Some(LinearCredential {
            connection_id: connection_id.to_owned(),
            token,
        }));
    }
    let Some(selected) = vault.get(&linear_selected_key())? else {
        return Ok(None);
    };
    let connection_id = std::str::from_utf8(selected.expose())
        .map_err(|_| VaultError::Unsafe)?
        .to_owned();
    linear_credential_key(&connection_id)?;
    let token = vault
        .get(&linear_credential_key(&connection_id)?)?
        .ok_or(VaultError::Unsafe)?;
    std::str::from_utf8(token.expose()).map_err(|_| VaultError::Unsafe)?;
    Ok(Some(LinearCredential {
        connection_id,
        token,
    }))
}

pub fn delete_linear_credential(
    vault: &dyn SddCredentialVault,
    connection_id: &str,
) -> Result<(), VaultError> {
    vault.delete(&linear_credential_key(connection_id)?)
}

pub fn clear_selected_linear_credential(vault: &dyn SddCredentialVault) -> Result<(), VaultError> {
    vault.delete(&linear_selected_key())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JiraSite {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JiraCredential {
    pub connection_id: String,
    pub account_id: String,
    pub display_name: String,
    access_token: String,
    refresh_token: String,
    scopes: Vec<String>,
    pub expires_at_unix: i64,
    pub sites: Vec<JiraSite>,
    pub selected_site_id: String,
    pub credential_revision: i64,
    pub device_key_ref: String,
}

impl fmt::Debug for JiraCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JiraCredential")
            .field("connection_id", &self.connection_id)
            .field("account_id", &self.account_id)
            .field("display_name", &self.display_name)
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("scopes", &self.scopes)
            .field("expires_at_unix", &self.expires_at_unix)
            .field("sites", &self.sites)
            .field("selected_site_id", &self.selected_site_id)
            .field("credential_revision", &self.credential_revision)
            .finish()
    }
}

impl Drop for JiraCredential {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

impl JiraCredential {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection_id: String,
        account_id: String,
        display_name: String,
        access_token: String,
        refresh_token: String,
        scopes: Vec<String>,
        expires_at_unix: i64,
        sites: Vec<JiraSite>,
        selected_site_id: String,
        credential_revision: i64,
        device_key_ref: String,
    ) -> Result<Self, VaultError> {
        CredentialKey::new("jira", "oauth-token", &connection_id)?;
        CredentialKey::new("jira", "device-proof", &device_key_ref)?;
        if account_id.trim().is_empty()
            || account_id.len() > 256
            || display_name.trim().is_empty()
            || display_name.len() > 512
            || access_token.trim().is_empty()
            || access_token.len() > 64 * 1024
            || refresh_token.trim().is_empty()
            || refresh_token.len() > 64 * 1024
            || scopes.is_empty()
            || scopes.len() > 32
            || scopes.iter().any(|scope| {
                scope.trim().is_empty()
                    || scope.len() > 128
                    || scope.chars().any(char::is_whitespace)
            })
            || expires_at_unix <= 0
            || credential_revision <= 0
            || sites.is_empty()
            || sites.len() > 100
            || !sites.iter().any(|site| site.id == selected_site_id)
            || sites.iter().any(|site| {
                site.id.is_empty()
                    || site.id.len() > 256
                    || site.name.is_empty()
                    || site.name.len() > 512
                    || site.url.len() > 4096
                    || !site.url.starts_with("https://")
            })
        {
            return Err(VaultError::InvalidValue);
        }
        Ok(Self {
            connection_id,
            account_id,
            display_name,
            access_token,
            refresh_token,
            scopes,
            expires_at_unix,
            sites,
            selected_site_id,
            credential_revision,
            device_key_ref,
        })
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    pub fn granted_scopes(&self) -> &[String] {
        &self.scopes
    }

    pub fn has_scope(&self, expected: &str) -> bool {
        self.scopes.iter().any(|scope| scope == expected)
    }

    pub fn select_site(&mut self, site_id: &str) -> Result<(), VaultError> {
        if !self.sites.iter().any(|site| site.id == site_id) {
            return Err(VaultError::InvalidValue);
        }
        self.selected_site_id = site_id.to_owned();
        Ok(())
    }

    pub fn selected_site(&self) -> Option<&JiraSite> {
        self.sites
            .iter()
            .find(|site| site.id == self.selected_site_id)
    }
}

pub fn jira_credential_key(connection_id: &str) -> Result<CredentialKey, VaultError> {
    CredentialKey::new("jira", "oauth-token", connection_id)
}

pub fn jira_selected_key() -> CredentialKey {
    CredentialKey::new("jira", "oauth-token", "selected")
        .expect("static Jira credential key is valid")
}

pub fn put_jira_credential(
    vault: &dyn SddCredentialVault,
    credential: &JiraCredential,
    selected: bool,
) -> Result<(), VaultError> {
    put_json(
        vault,
        &jira_credential_key(&credential.connection_id)?,
        credential,
    )?;
    if selected {
        vault.put(
            &jira_selected_key(),
            &SecretValue::new(credential.connection_id.as_bytes().to_vec())?,
        )?;
    }
    Ok(())
}

pub fn get_jira_credential(
    vault: &dyn SddCredentialVault,
    connection_id: Option<&str>,
) -> Result<Option<JiraCredential>, VaultError> {
    let resolved_connection_id = match connection_id {
        Some(connection_id) => connection_id.to_owned(),
        None => {
            let Some(selected) = vault.get(&jira_selected_key())? else {
                return Ok(None);
            };
            std::str::from_utf8(selected.expose())
                .map_err(|_| VaultError::Unsafe)?
                .to_owned()
        }
    };
    let key = jira_credential_key(&resolved_connection_id)?;
    let credential: Option<JiraCredential> = get_json(vault, &key)?;
    if let Some(credential) = credential.as_ref() {
        CredentialKey::new("jira", "oauth-token", &credential.connection_id)?;
        if credential.credential_revision <= 0
            || credential.sites.is_empty()
            || credential.selected_site().is_none()
            || credential.access_token.is_empty()
            || credential.refresh_token.is_empty()
            || credential.scopes.is_empty()
            || credential.scopes.len() > 32
            || credential.scopes.iter().any(|scope| {
                scope.trim().is_empty()
                    || scope.len() > 128
                    || scope.chars().any(char::is_whitespace)
            })
        {
            return Err(VaultError::Unsafe);
        }
        if resolved_connection_id != credential.connection_id {
            return Err(VaultError::Unsafe);
        }
    }
    Ok(credential)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JiraApiTokenCredential {
    pub connection_id: String,
    pub account_id: String,
    pub display_name: String,
    email: String,
    api_token: String,
    pub site: JiraSite,
    pub credential_revision: i64,
}

impl fmt::Debug for JiraApiTokenCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JiraApiTokenCredential")
            .field("connection_id", &self.connection_id)
            .field("account_id", &self.account_id)
            .field("display_name", &self.display_name)
            .field("email", &"[REDACTED]")
            .field("api_token", &"[REDACTED]")
            .field("site", &self.site)
            .field("credential_revision", &self.credential_revision)
            .finish()
    }
}

impl Drop for JiraApiTokenCredential {
    fn drop(&mut self) {
        self.email.zeroize();
        self.api_token.zeroize();
    }
}

impl JiraApiTokenCredential {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection_id: String,
        account_id: String,
        display_name: String,
        email: String,
        api_token: String,
        site: JiraSite,
        credential_revision: i64,
    ) -> Result<Self, VaultError> {
        CredentialKey::new("jira", "api-token", &connection_id)?;
        if account_id.trim().is_empty()
            || account_id.len() > 256
            || display_name.trim().is_empty()
            || display_name.len() > 512
            || email.trim().is_empty()
            || email.len() > 512
            || !email.contains('@')
            || email.chars().any(char::is_control)
            || api_token.trim().is_empty()
            || api_token.len() > 64 * 1024
            || api_token.chars().any(char::is_control)
            || credential_revision <= 0
            || site.id.trim().is_empty()
            || site.id.len() > 256
            || site.name.trim().is_empty()
            || site.name.len() > 512
            || site.url.len() > 4096
            || !site.url.starts_with("https://")
        {
            return Err(VaultError::InvalidValue);
        }
        Ok(Self {
            connection_id,
            account_id,
            display_name,
            email,
            api_token,
            site,
            credential_revision,
        })
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    pub fn api_token(&self) -> &str {
        &self.api_token
    }
}

pub fn jira_api_token_credential_key(connection_id: &str) -> Result<CredentialKey, VaultError> {
    CredentialKey::new("jira", "api-token", connection_id)
}

pub fn put_jira_api_token_credential(
    vault: &dyn SddCredentialVault,
    credential: &JiraApiTokenCredential,
    selected: bool,
) -> Result<(), VaultError> {
    put_json(
        vault,
        &jira_api_token_credential_key(&credential.connection_id)?,
        credential,
    )?;
    if selected {
        vault.put(
            &jira_selected_key(),
            &SecretValue::new(credential.connection_id.as_bytes().to_vec())?,
        )?;
    }
    Ok(())
}

pub fn get_jira_api_token_credential(
    vault: &dyn SddCredentialVault,
    connection_id: Option<&str>,
) -> Result<Option<JiraApiTokenCredential>, VaultError> {
    let resolved_connection_id = match connection_id {
        Some(connection_id) => connection_id.to_owned(),
        None => {
            let Some(selected) = vault.get(&jira_selected_key())? else {
                return Ok(None);
            };
            std::str::from_utf8(selected.expose())
                .map_err(|_| VaultError::Unsafe)?
                .to_owned()
        }
    };
    let credential: Option<JiraApiTokenCredential> = get_json(
        vault,
        &jira_api_token_credential_key(&resolved_connection_id)?,
    )?;
    if let Some(credential) = credential.as_ref() {
        if credential.connection_id != resolved_connection_id
            || credential.credential_revision <= 0
            || credential.email.is_empty()
            || credential.api_token.is_empty()
        {
            return Err(VaultError::Unsafe);
        }
    }
    Ok(credential)
}

pub fn delete_jira_api_token_credential(
    vault: &dyn SddCredentialVault,
    connection_id: &str,
) -> Result<(), VaultError> {
    vault.delete(&jira_api_token_credential_key(connection_id)?)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JiraFlowSecret {
    state: String,
    device_private_key: String,
}

impl Drop for JiraFlowSecret {
    fn drop(&mut self) {
        self.state.zeroize();
        self.device_private_key.zeroize();
    }
}

pub fn jira_flow_key(flow_id: &str) -> Result<CredentialKey, VaultError> {
    CredentialKey::new("jira", "oauth-flow", flow_id)
}

pub fn jira_device_key(device_key_ref: &str) -> Result<CredentialKey, VaultError> {
    CredentialKey::new("jira", "device-proof", device_key_ref)
}

pub fn put_jira_device_private_key(
    vault: &dyn SddCredentialVault,
    device_key_ref: &str,
    private_key: &[u8],
) -> Result<(), VaultError> {
    vault.put(
        &jira_device_key(device_key_ref)?,
        &SecretValue::new(private_key.to_vec())?,
    )
}

pub fn get_jira_device_private_key(
    vault: &dyn SddCredentialVault,
    device_key_ref: &str,
) -> Result<Option<SecretValue>, VaultError> {
    vault.get(&jira_device_key(device_key_ref)?)
}

pub fn put_jira_flow_secret(
    vault: &dyn SddCredentialVault,
    flow_id: &str,
    state: &str,
    device_private_key: &[u8],
) -> Result<(), VaultError> {
    let secret = JiraFlowSecret {
        state: state.to_owned(),
        device_private_key: BASE64.encode(device_private_key),
    };
    put_json(vault, &jira_flow_key(flow_id)?, &secret)
}

pub fn get_jira_flow_secret(
    vault: &dyn SddCredentialVault,
    flow_id: &str,
) -> Result<Option<(String, SecretValue)>, VaultError> {
    let Some(secret): Option<JiraFlowSecret> = get_json(vault, &jira_flow_key(flow_id)?)? else {
        return Ok(None);
    };
    let private_key = BASE64
        .decode(secret.device_private_key.as_bytes())
        .map_err(|_| VaultError::Unsafe)?;
    Ok(Some((secret.state.clone(), SecretValue::new(private_key)?)))
}

pub fn delete_jira_flow_secret(
    vault: &dyn SddCredentialVault,
    flow_id: &str,
) -> Result<(), VaultError> {
    vault.delete(&jira_flow_key(flow_id)?)
}

/// Platform credential-store backend used only by the embedded desktop.
#[derive(Debug, Default)]
pub struct OsCredentialVault;

impl OsCredentialVault {
    pub fn new() -> Self {
        Self
    }

    fn entry(key: &CredentialKey) -> Result<keyring::Entry, VaultError> {
        keyring::Entry::new(&key.service(), key.account()).map_err(|_| VaultError::Backend)
    }
}

impl SddCredentialVault for OsCredentialVault {
    fn status(&self) -> VaultStatus {
        let probe = CredentialKey::new("system", "availability-probe", "sdd")
            .expect("static keyring probe key is valid");
        match Self::entry(&probe).and_then(|entry| match entry.get_secret() {
            Ok(mut value) => {
                let bounded = value.len() <= MAX_SECRET_BYTES;
                value.zeroize();
                bounded.then_some(()).ok_or(VaultError::Backend)
            }
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(VaultError::Backend),
        }) {
            Ok(()) => VaultStatus {
                backend: "os",
                persistent: true,
                available: true,
                reason: None,
            },
            Err(_) => VaultStatus {
                backend: "os",
                persistent: true,
                available: false,
                reason: Some(
                    "the operating-system credential vault is unavailable or locked".into(),
                ),
            },
        }
    }

    fn get(&self, key: &CredentialKey) -> Result<Option<SecretValue>, VaultError> {
        match Self::entry(key)?.get_secret() {
            Ok(value) => SecretValue::new(value).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(VaultError::Backend),
        }
    }

    fn put(&self, key: &CredentialKey, value: &SecretValue) -> Result<(), VaultError> {
        Self::entry(key)?
            .set_secret(value.expose())
            .map_err(|_| VaultError::Backend)
    }

    fn delete(&self, key: &CredentialKey) -> Result<(), VaultError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(VaultError::Backend),
        }
    }
}

#[derive(Debug)]
pub struct UnavailableVault {
    reason: String,
}

impl UnavailableVault {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl SddCredentialVault for UnavailableVault {
    fn status(&self) -> VaultStatus {
        VaultStatus {
            backend: "unavailable",
            persistent: false,
            available: false,
            reason: Some(self.reason.clone()),
        }
    }

    fn get(&self, _key: &CredentialKey) -> Result<Option<SecretValue>, VaultError> {
        Err(VaultError::Unavailable(self.reason.clone()))
    }

    fn put(&self, _key: &CredentialKey, _value: &SecretValue) -> Result<(), VaultError> {
        Err(VaultError::Unavailable(self.reason.clone()))
    }

    fn delete(&self, _key: &CredentialKey) -> Result<(), VaultError> {
        Err(VaultError::Unavailable(self.reason.clone()))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VaultEnvelope {
    format: String,
    version: u32,
    nonce: String,
    ciphertext: String,
}

/// AES-256-GCM vault for standalone/headless deployments. The master key is
/// accepted only from the process environment and is never written to disk.
pub struct HeadlessEncryptedVault {
    path: PathBuf,
    master_key: Mutex<[u8; 32]>,
    io_lock: Mutex<()>,
}

impl fmt::Debug for HeadlessEncryptedVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HeadlessEncryptedVault")
            .field("path", &self.path)
            .field("master_key", &"[REDACTED]")
            .finish()
    }
}

impl Drop for HeadlessEncryptedVault {
    fn drop(&mut self) {
        if let Ok(mut key) = self.master_key.lock() {
            key.zeroize();
        }
    }
}

impl HeadlessEncryptedVault {
    pub fn from_environment() -> Result<Self, VaultError> {
        let encoded = std::env::var(VAULT_ENV).map_err(|_| {
            VaultError::Unavailable(format!(
                "{VAULT_ENV} must contain an externally supplied base64 256-bit key"
            ))
        })?;
        let decoded = BASE64
            .decode(encoded.trim())
            .map_err(|_| VaultError::Unavailable(format!("{VAULT_ENV} is not valid base64")))?;
        let master_key: [u8; 32] = decoded.try_into().map_err(|_| {
            VaultError::Unavailable(format!("{VAULT_ENV} must decode to exactly 32 bytes"))
        })?;
        let path = agentum_store::paths::data_dir()
            .map_err(|error| VaultError::Unavailable(error.to_string()))?
            .join("sdd-credentials.vault");
        Ok(Self::new(path, master_key))
    }

    pub fn new(path: PathBuf, master_key: [u8; 32]) -> Self {
        Self {
            path,
            master_key: Mutex::new(master_key),
            io_lock: Mutex::new(()),
        }
    }

    fn read_entries(&self) -> Result<BTreeMap<String, String>, VaultError> {
        let Some(bytes) = read_vault_file(&self.path)? else {
            return Ok(BTreeMap::new());
        };
        let envelope: VaultEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| VaultError::Unsafe)?;
        if envelope.format != VAULT_FORMAT || envelope.version != VAULT_VERSION {
            return Err(VaultError::Unsafe);
        }
        let nonce: [u8; 12] = URL_SAFE_NO_PAD
            .decode(&envelope.nonce)
            .map_err(|_| VaultError::Unsafe)?
            .try_into()
            .map_err(|_| VaultError::Unsafe)?;
        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(&envelope.ciphertext)
            .map_err(|_| VaultError::Unsafe)?;
        let key = self.master_key.lock().map_err(|_| VaultError::Backend)?;
        let key = LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key.as_slice()).map_err(|_| VaultError::Unsafe)?,
        );
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(VAULT_AAD),
                &mut ciphertext,
            )
            .map_err(|_| VaultError::Unsafe)?;
        serde_json::from_slice(plaintext).map_err(|_| VaultError::Unsafe)
    }

    fn write_entries(&self, entries: &BTreeMap<String, String>) -> Result<(), VaultError> {
        let mut plaintext = serde_json::to_vec(entries).map_err(|_| VaultError::Unsafe)?;
        if plaintext.len() > MAX_VAULT_BYTES as usize {
            plaintext.zeroize();
            return Err(VaultError::InvalidValue);
        }
        let mut nonce = [0_u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| VaultError::Backend)?;
        let key = self.master_key.lock().map_err(|_| VaultError::Backend)?;
        let key = LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key.as_slice()).map_err(|_| VaultError::Unsafe)?,
        );
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(VAULT_AAD),
            &mut plaintext,
        )
        .map_err(|_| VaultError::Backend)?;
        let envelope = VaultEnvelope {
            format: VAULT_FORMAT.into(),
            version: VAULT_VERSION,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(&plaintext),
        };
        plaintext.zeroize();
        let published = serde_json::to_vec(&envelope).map_err(|_| VaultError::Unsafe)?;
        atomic_write_vault(&self.path, &published)
    }
}

impl SddCredentialVault for HeadlessEncryptedVault {
    fn status(&self) -> VaultStatus {
        let available = self
            .io_lock
            .lock()
            .map_err(|_| VaultError::Backend)
            .and_then(|_guard| self.read_entries().map(|_| ()))
            .is_ok();
        VaultStatus {
            backend: "headless_encrypted",
            persistent: true,
            available,
            reason: (!available).then(|| {
                "the encrypted credential vault is inaccessible or cannot be authenticated".into()
            }),
        }
    }

    fn get(&self, key: &CredentialKey) -> Result<Option<SecretValue>, VaultError> {
        let _guard = self.io_lock.lock().map_err(|_| VaultError::Backend)?;
        self.read_entries()?
            .get(&key.map_key())
            .map(|value| {
                BASE64
                    .decode(value)
                    .map_err(|_| VaultError::Unsafe)
                    .and_then(SecretValue::new)
            })
            .transpose()
    }

    fn put(&self, key: &CredentialKey, value: &SecretValue) -> Result<(), VaultError> {
        let _guard = self.io_lock.lock().map_err(|_| VaultError::Backend)?;
        let mut entries = self.read_entries()?;
        entries.insert(key.map_key(), BASE64.encode(value.expose()));
        self.write_entries(&entries)
    }

    fn delete(&self, key: &CredentialKey) -> Result<(), VaultError> {
        let _guard = self.io_lock.lock().map_err(|_| VaultError::Backend)?;
        let mut entries = self.read_entries()?;
        if entries.remove(&key.map_key()).is_some() {
            self.write_entries(&entries)?;
        }
        Ok(())
    }
}

pub fn headless_vault_or_unavailable() -> Arc<dyn SddCredentialVault> {
    match HeadlessEncryptedVault::from_environment() {
        Ok(vault) => Arc::new(vault),
        Err(error) => Arc::new(UnavailableVault::new(error.to_string())),
    }
}

fn read_vault_file(path: &Path) -> Result<Option<Vec<u8>>, VaultError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if unsafe_vault_file(&metadata) || metadata.len() > MAX_VAULT_BYTES {
        return Err(VaultError::Unsafe);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 {
            return Err(VaultError::Unsafe);
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_VAULT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_VAULT_BYTES {
        return Err(VaultError::Unsafe);
    }
    Ok(Some(bytes))
}

fn atomic_write_vault(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let parent = path.parent().ok_or(VaultError::Unsafe)?;
    super::workspace::ensure_directory_chain_nofollow(parent).map_err(|_| VaultError::Unsafe)?;
    if std::fs::symlink_metadata(parent)
        .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        .unwrap_or(true)
    {
        return Err(VaultError::Unsafe);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    if std::fs::symlink_metadata(path)
        .map(|metadata| unsafe_vault_file(&metadata))
        .unwrap_or(false)
    {
        return Err(VaultError::Unsafe);
    }
    let temporary = parent.join(format!(".sdd-credentials-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temporary)?;
    let publication = (|| -> Result<(), VaultError> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        atomic_replace(&temporary, path)?;
        sync_directory(parent)?;
        Ok(())
    })();
    if publication.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    publication
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), VaultError> {
    std::fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), VaultError> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    // SAFETY: both vectors are live NUL-terminated UTF-16 paths. MoveFileExW
    // provides replacement semantics that std::fs::rename lacks on Windows.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn unsafe_vault_file(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), VaultError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), VaultError> {
    Ok(())
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryCredentialVault {
    entries: Mutex<BTreeMap<String, Vec<u8>>>,
}

#[cfg(test)]
impl SddCredentialVault for MemoryCredentialVault {
    fn status(&self) -> VaultStatus {
        VaultStatus {
            backend: "memory_test",
            persistent: false,
            available: true,
            reason: None,
        }
    }

    fn get(&self, key: &CredentialKey) -> Result<Option<SecretValue>, VaultError> {
        self.entries
            .lock()
            .map_err(|_| VaultError::Backend)?
            .get(&key.map_key())
            .cloned()
            .map(SecretValue::new)
            .transpose()
    }

    fn put(&self, key: &CredentialKey, value: &SecretValue) -> Result<(), VaultError> {
        self.entries
            .lock()
            .map_err(|_| VaultError::Backend)?
            .insert(key.map_key(), value.expose().to_vec());
        Ok(())
    }

    fn delete(&self, key: &CredentialKey) -> Result<(), VaultError> {
        self.entries
            .lock()
            .map_err(|_| VaultError::Backend)?
            .remove(&key.map_key());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_components_are_closed_and_bounded() {
        assert!(CredentialKey::new("linear", "api-token", "workspace-1").is_ok());
        for value in ["", "../escape", "contains/slash", "contains space"] {
            assert!(CredentialKey::new("linear", "api-token", value).is_err());
        }
    }

    #[test]
    fn headless_vault_encrypts_round_trips_and_rejects_wrong_key_or_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.json");
        let key = CredentialKey::new("linear", "api-token", "selected").unwrap();
        let vault = HeadlessEncryptedVault::new(path.clone(), [7; 32]);
        vault
            .put(&key, &SecretValue::new(b"linear-secret".to_vec()).unwrap())
            .unwrap();
        let published = std::fs::read_to_string(&path).unwrap();
        assert!(!published.contains("linear-secret"));
        assert_eq!(vault.get(&key).unwrap().unwrap().expose(), b"linear-secret");

        let wrong = HeadlessEncryptedVault::new(path.clone(), [8; 32]);
        assert!(matches!(wrong.get(&key), Err(VaultError::Unsafe)));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = directory.path().join("target");
            std::fs::rename(&path, &target).unwrap();
            symlink(&target, &path).unwrap();
            assert!(matches!(vault.get(&key), Err(VaultError::Unsafe)));
        }
    }

    #[test]
    fn headless_delete_reencrypts_without_the_secret() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.json");
        let vault = HeadlessEncryptedVault::new(path, [4; 32]);
        let key = CredentialKey::new("jira", "oauth", "selected").unwrap();
        vault
            .put(&key, &SecretValue::new(b"oauth-secret".to_vec()).unwrap())
            .unwrap();
        vault.delete(&key).unwrap();
        assert!(vault.get(&key).unwrap().is_none());
    }

    #[test]
    fn selected_aliases_store_only_connection_ids_and_resolve_canonical_secrets() {
        let vault = MemoryCredentialVault::default();
        put_linear_credential(&vault, "workspace-1", "token-one", true).unwrap();
        assert_eq!(
            vault.get(&linear_selected_key()).unwrap().unwrap().expose(),
            b"workspace-1"
        );
        put_linear_credential(&vault, "workspace-1", "token-two", false).unwrap();
        let selected = get_linear_credential(&vault, None).unwrap().unwrap();
        assert_eq!(selected.connection_id, "workspace-1");
        assert_eq!(selected.token(), "token-two");
    }

    #[cfg(unix)]
    #[test]
    fn headless_vault_rejects_symlinked_ancestor_without_creating_through_it() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let linked = directory.path().join("linked");
        symlink(&real, &linked).unwrap();
        let path = linked.join("nested").join("vault.json");
        let vault = HeadlessEncryptedVault::new(path, [9; 32]);
        let key = CredentialKey::new("linear", "api-token", "selected").unwrap();
        assert!(matches!(
            vault.put(&key, &SecretValue::new(b"secret".to_vec()).unwrap()),
            Err(VaultError::Unsafe)
        ));
        assert!(!real.join("nested").exists());
    }
}
