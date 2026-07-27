//! Jira Cloud read-only source adapter and confidential-client OAuth broker.
//!
//! Agentum never embeds an Atlassian client secret. The configured broker owns
//! the confidential-client exchange while Agentum binds each one-time
//! redemption and refresh to a device Ed25519 key held in the secure vault.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use agentum_store::sdd_integrations::{
    NewSddOauthFlow, SddOauthFlowRecord, UpsertSddIntegrationConnection,
};

use crate::AppState;
use crate::sdd_v2::credentials::{
    JiraApiTokenCredential, JiraCredential, JiraSite, delete_jira_api_token_credential,
    delete_jira_flow_secret, get_jira_api_token_credential, get_jira_credential,
    get_jira_device_private_key, get_jira_flow_secret, put_jira_api_token_credential,
    put_jira_credential, put_jira_device_private_key, put_jira_flow_secret,
};
use crate::sdd_v2::sha256;

const BROKER_ENV: &str = "AGENTUM_JIRA_OAUTH_BROKER_URL";
const API_TOKEN_FALLBACK_ENV: &str = "AGENTUM_JIRA_ALLOW_API_TOKEN_AUTH";
const MAX_BROKER_RESPONSE: usize = 512 * 1024;
const MAX_JIRA_RESPONSE: usize = 2 * 1024 * 1024;
const FLOW_TTL_SECONDS: i64 = 15 * 60;
const REQUIRED_SCOPES: [&str; 3] = ["read:jira-work", "write:jira-work", "offline_access"];
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static API_TOKEN_CONNECT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, thiserror::Error)]
pub enum JiraError {
    #[error("Jira OAuth broker is unavailable: {0}")]
    BrokerUnavailable(String),
    #[error("Jira credential vault is unavailable")]
    Vault,
    #[error("advanced Jira API-token authentication is disabled")]
    ApiTokenDisabled,
    #[error("Jira API-token credentials were rejected")]
    ApiTokenRejected,
    #[error("Jira OAuth flow is invalid or expired")]
    Flow,
    #[error("Jira OAuth redemption outcome is ambiguous; reconciliation is required")]
    Ambiguous,
    #[error("Jira OAuth broker returned a malformed response")]
    BrokerResponse,
    #[error("Jira connection or site selection is invalid")]
    Connection,
    #[error("Jira issue key is invalid")]
    IssueKey,
    #[error("Jira issue read failed")]
    IssueRead,
    #[error(transparent)]
    Store(#[from] agentum_store::StoreError),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraOauthStart {
    pub flow_id: String,
    pub revision: i64,
    pub authorization_url: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraConnectionView {
    pub connection_id: String,
    pub display_name: String,
    pub sites: Vec<JiraSite>,
    pub selected_site_id: String,
    pub credential_revision: i64,
    pub auth_kind: String,
    pub granted_scopes: Vec<String>,
    pub delivery_write_authorized: bool,
}

#[derive(Debug, Clone)]
pub struct FetchedJiraIssue {
    pub id: String,
    pub key: String,
    pub title: String,
    pub description: String,
    pub url: String,
    pub updated_at: String,
    pub connection_id: String,
    pub site_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerStartRequest<'a> {
    state: &'a str,
    device_public_key: String,
    scopes: [&'static str; 3],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerStartResponse {
    authorization_url: String,
    redemption_id: String,
    expires_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRedeemRequest<'a> {
    redemption_id: &'a str,
    state: &'a str,
    flow_id: &'a str,
    device_proof: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerTokenResponse {
    account_id: String,
    display_name: String,
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    scopes: Vec<String>,
    sites: Vec<JiraSite>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerRefreshRequest<'a> {
    connection_id: &'a str,
    refresh_token: &'a str,
    credential_revision: i64,
    device_proof: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrokerRefreshResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Debug, Clone)]
struct BrokerConfig {
    base: reqwest::Url,
}

impl BrokerConfig {
    fn from_environment() -> Result<Self, JiraError> {
        let raw = std::env::var(BROKER_ENV)
            .map_err(|_| JiraError::BrokerUnavailable(format!("{BROKER_ENV} is not configured")))?;
        let mut base = reqwest::Url::parse(raw.trim())
            .map_err(|_| JiraError::BrokerUnavailable("broker URL is invalid".into()))?;
        if base.scheme() != "https"
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(JiraError::BrokerUnavailable(
                "broker URL must be credential-free HTTPS".into(),
            ));
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self { base })
    }

    fn endpoint(&self, relative: &str) -> Result<reqwest::Url, JiraError> {
        self.base
            .join(relative)
            .map_err(|_| JiraError::BrokerUnavailable("broker endpoint is invalid".into()))
    }
}

pub fn broker_configured() -> bool {
    BrokerConfig::from_environment().is_ok()
}

pub fn api_token_fallback_enabled() -> bool {
    std::env::var(API_TOKEN_FALLBACK_ENV)
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Advanced local/self-hosted fallback. The API token is sent only to the
/// selected Atlassian tenant for validation and is persisted only in Agentum's
/// secure credential vault. It never passes through the OAuth broker.
pub async fn connect_api_token(
    state: &AppState,
    email: &str,
    api_token: &str,
    site_url: &str,
    risk_acknowledged: bool,
    expected_revision: i64,
) -> Result<JiraConnectionView, JiraError> {
    if !api_token_fallback_enabled() || !risk_acknowledged {
        return Err(JiraError::ApiTokenDisabled);
    }
    if !state.sdd_credentials.status().available {
        return Err(JiraError::Vault);
    }
    validate_api_token_input(email, api_token)?;
    let site_url = validate_site_root(site_url)?;
    let mut myself_url = site_url.clone();
    myself_url.set_path("/rest/api/3/myself");
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| JiraError::ApiTokenRejected)?
        .get(myself_url)
        .basic_auth(email, Some(api_token))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| JiraError::ApiTokenRejected)?;
    if !response.status().is_success() {
        return Err(JiraError::ApiTokenRejected);
    }
    let body = limited_bytes(response, MAX_JIRA_RESPONSE)
        .await
        .map_err(|_| JiraError::ApiTokenRejected)?;
    let myself: Value = serde_json::from_slice(&body).map_err(|_| JiraError::ApiTokenRejected)?;
    let account_id = myself
        .get("accountId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= 256)
        .ok_or(JiraError::ApiTokenRejected)?;
    let display_name = myself
        .get("displayName")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= 512)
        .ok_or(JiraError::ApiTokenRejected)?;
    let hostname = site_url.host_str().ok_or(JiraError::ApiTokenRejected)?;
    let connection_id = format!(
        "jira-local-{}",
        &sha256(format!("{account_id}\n{hostname}").as_bytes())[..24]
    );
    let _guard = API_TOKEN_CONNECT_LOCK.lock().await;
    let existing = state
        .store
        .sdd_integration_connection("jira", &connection_id)
        .await?;
    let current_revision = existing
        .as_ref()
        .map_or(0, |connection| connection.credential_revision);
    if current_revision != expected_revision {
        return Err(agentum_store::StoreError::StaleRevision {
            expected: expected_revision,
            current: current_revision,
        }
        .into());
    }
    let site = JiraSite {
        id: hostname.to_owned(),
        name: hostname.to_owned(),
        url: site_url.as_str().trim_end_matches('/').to_owned(),
    };
    let credential = JiraApiTokenCredential::new(
        connection_id.clone(),
        account_id.to_owned(),
        display_name.to_owned(),
        email.to_owned(),
        api_token.to_owned(),
        site.clone(),
        expected_revision + 1,
    )
    .map_err(|_| JiraError::ApiTokenRejected)?;
    let metadata = JiraConnectionMetadata {
        sites: vec![site.clone()],
        scopes: Vec::new(),
        auth_kind: "api_token".into(),
        site_selection_required: false,
        api_token_risk_acknowledged: true,
    };
    let metadata_json = serde_json::to_string(&metadata).map_err(|_| JiraError::Connection)?;
    let vault = state.sdd_credentials.clone();
    let connection_for_vault = connection_id.clone();
    let credential_bytes =
        serde_json::to_vec(&credential).map_err(|_| JiraError::ApiTokenRejected)?;
    let previous = tokio::task::spawn_blocking(move || {
        let previous =
            get_jira_api_token_credential(vault.as_ref(), Some(connection_for_vault.as_str()))?;
        let credential: JiraApiTokenCredential = serde_json::from_slice(&credential_bytes)
            .map_err(|_| crate::sdd_v2::credentials::VaultError::Unsafe)?;
        put_jira_api_token_credential(vault.as_ref(), &credential, false)?;
        previous
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| crate::sdd_v2::credentials::VaultError::Unsafe)
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    let stored = state
        .store
        .sdd_upsert_integration_connection(
            UpsertSddIntegrationConnection {
                connection_id: &connection_id,
                provider: "jira",
                external_account_id: account_id,
                display_name,
                selected_site_id: Some(&site.id),
                metadata_json: &metadata_json,
                credential_revision: credential.credential_revision,
            },
            expected_revision,
        )
        .await;
    if let Err(error) = stored {
        let vault = state.sdd_credentials.clone();
        let connection = connection_id.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(previous) = previous {
                let previous: JiraApiTokenCredential = serde_json::from_slice(&previous)
                    .map_err(|_| crate::sdd_v2::credentials::VaultError::Unsafe)?;
                put_jira_api_token_credential(vault.as_ref(), &previous, false)
            } else {
                delete_jira_api_token_credential(vault.as_ref(), &connection)
            }
        })
        .await;
        return Err(error.into());
    }
    let vault = state.sdd_credentials.clone();
    let selected_bytes =
        serde_json::to_vec(&credential).map_err(|_| JiraError::ApiTokenRejected)?;
    tokio::task::spawn_blocking(move || {
        let credential: JiraApiTokenCredential = serde_json::from_slice(&selected_bytes)
            .map_err(|_| crate::sdd_v2::credentials::VaultError::Unsafe)?;
        put_jira_api_token_credential(vault.as_ref(), &credential, true)
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    Ok(api_token_connection_view(&credential))
}

pub async fn start_oauth(state: &AppState, request_id: &str) -> Result<JiraOauthStart, JiraError> {
    let broker = BrokerConfig::from_environment()?;
    if let Some(existing) = state
        .store
        .sdd_oauth_flow_by_request("jira", request_id)
        .await?
    {
        return Ok(JiraOauthStart {
            flow_id: existing.flow_id,
            revision: existing.revision,
            authorization_url: existing.authorization_url,
            expires_at: existing.expires_at,
        });
    }
    if !state.sdd_credentials.status().available {
        return Err(JiraError::Vault);
    }
    let flow_id = Uuid::new_v4().to_string();
    let mut state_bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut state_bytes)
        .map_err(|_| JiraError::Flow)?;
    let oauth_state = URL_SAFE_NO_PAD.encode(state_bytes);
    let private_key =
        Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| JiraError::Flow)?;
    let key_pair = Ed25519KeyPair::from_pkcs8(private_key.as_ref()).map_err(|_| JiraError::Flow)?;
    let request = BrokerStartRequest {
        state: &oauth_state,
        device_public_key: URL_SAFE_NO_PAD.encode(key_pair.public_key().as_ref()),
        scopes: REQUIRED_SCOPES,
    };
    let response: BrokerStartResponse = broker_post(
        broker.endpoint("v1/jira/oauth/start")?,
        &request,
        MAX_BROKER_RESPONSE,
    )
    .await?;
    validate_authorization_url(&response.authorization_url)?;
    validate_tokenish(&response.redemption_id, 512)?;
    let expires = OffsetDateTime::parse(&response.expires_at, &Rfc3339)
        .map_err(|_| JiraError::BrokerResponse)?;
    let now = OffsetDateTime::now_utc();
    if expires <= now || expires > now + time::Duration::seconds(FLOW_TTL_SECONDS) {
        return Err(JiraError::BrokerResponse);
    }
    let vault = state.sdd_credentials.clone();
    let flow_for_vault = flow_id.clone();
    let state_for_vault = oauth_state.clone();
    let private_for_vault = private_key.as_ref().to_vec();
    tokio::task::spawn_blocking(move || {
        put_jira_flow_secret(
            vault.as_ref(),
            &flow_for_vault,
            &state_for_vault,
            &private_for_vault,
        )?;
        put_jira_device_private_key(vault.as_ref(), &flow_for_vault, &private_for_vault)
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    if let Err(error) = state
        .store
        .sdd_create_oauth_flow(NewSddOauthFlow {
            flow_id: &flow_id,
            provider: "jira",
            request_id,
            state_hash: &sha256(oauth_state.as_bytes()),
            redemption_id: &response.redemption_id,
            authorization_url: &response.authorization_url,
            device_key_ref: &flow_id,
            expires_at: &response.expires_at,
        })
        .await
    {
        let vault = state.sdd_credentials.clone();
        let cleanup_id = flow_id.clone();
        let _ = tokio::task::spawn_blocking(move || {
            delete_jira_flow_secret(vault.as_ref(), &cleanup_id)
        })
        .await;
        return Err(error.into());
    }
    Ok(JiraOauthStart {
        flow_id,
        revision: 1,
        authorization_url: response.authorization_url,
        expires_at: response.expires_at,
    })
}

pub async fn redeem_oauth(
    state: &AppState,
    flow_id: &str,
    expected_revision: i64,
) -> Result<JiraConnectionView, JiraError> {
    validate_tokenish(flow_id, 256)?;
    let existing = state
        .store
        .sdd_oauth_flow(flow_id)
        .await?
        .ok_or(JiraError::Flow)?;
    if existing.status == "redeemed" {
        let connection_id = existing.connection_id.ok_or(JiraError::Flow)?;
        let selected = state
            .store
            .sdd_integration_connection("jira", &connection_id)
            .await?
            .and_then(|connection| connection.selected_site_id)
            .is_some();
        let vault = state.sdd_credentials.clone();
        let credential = tokio::task::spawn_blocking(move || {
            get_jira_credential(vault.as_ref(), Some(&connection_id))
        })
        .await
        .map_err(|_| JiraError::Vault)?
        .map_err(|_| JiraError::Vault)?
        .ok_or(JiraError::Connection)?;
        return Ok(connection_view(&credential, selected));
    }
    let broker = BrokerConfig::from_environment()?;
    let flow = state
        .store
        .sdd_claim_oauth_redemption(flow_id, expected_revision)
        .await?;
    let vault = state.sdd_credentials.clone();
    let flow_for_vault = flow_id.to_owned();
    let (oauth_state, private_key) =
        tokio::task::spawn_blocking(move || get_jira_flow_secret(vault.as_ref(), &flow_for_vault))
            .await
            .map_err(|_| JiraError::Vault)?
            .map_err(|_| JiraError::Vault)?
            .ok_or(JiraError::Flow)?;
    if sha256(oauth_state.as_bytes()) != flow.state_hash {
        return Err(JiraError::Flow);
    }
    let proof = sign_redemption(
        private_key.expose(),
        flow_id,
        &flow.redemption_id,
        &oauth_state,
    )?;
    let request = BrokerRedeemRequest {
        redemption_id: &flow.redemption_id,
        state: &oauth_state,
        flow_id,
        device_proof: proof,
    };
    let response: BrokerTokenResponse = match broker_post(
        broker.endpoint("v1/jira/oauth/redeem")?,
        &request,
        MAX_BROKER_RESPONSE,
    )
    .await
    {
        Ok(response) => response,
        Err(JiraError::BrokerUnavailable(_)) => {
            state
                .store
                .sdd_mark_oauth_sync_pending(flow_id, flow.revision)
                .await?;
            return Err(JiraError::Ambiguous);
        }
        Err(error) => return Err(error),
    };
    validate_broker_token_response(&response)?;
    let connection_id = format!(
        "jira-{}",
        sha256(response.account_id.as_bytes())[..24].to_owned()
    );
    let selected_site_id = if response.sites.len() == 1 {
        response.sites[0].id.clone()
    } else {
        String::new()
    };
    if selected_site_id.is_empty() {
        // Multi-site grants require an explicit choice. Preserve the tokens in
        // the vault with the first site as a temporary structurally valid value,
        // but do not mark a selected alias until the user chooses.
        return store_unselected_multisite(state, flow, response, connection_id).await;
    }
    finish_redemption(state, flow, response, connection_id, selected_site_id, true).await
}

async fn store_unselected_multisite(
    state: &AppState,
    flow: SddOauthFlowRecord,
    response: BrokerTokenResponse,
    connection_id: String,
) -> Result<JiraConnectionView, JiraError> {
    let temporary_site = response
        .sites
        .first()
        .ok_or(JiraError::BrokerResponse)?
        .id
        .clone();
    finish_redemption(state, flow, response, connection_id, temporary_site, false).await
}

async fn finish_redemption(
    state: &AppState,
    flow: SddOauthFlowRecord,
    response: BrokerTokenResponse,
    connection_id: String,
    selected_site_id: String,
    selected: bool,
) -> Result<JiraConnectionView, JiraError> {
    let credential = JiraCredential::new(
        connection_id.clone(),
        response.account_id.clone(),
        response.display_name.clone(),
        response.access_token,
        response.refresh_token,
        response.scopes,
        OffsetDateTime::now_utc().unix_timestamp() + response.expires_in,
        response.sites,
        selected_site_id,
        1,
        flow.device_key_ref.clone(),
    )
    .map_err(|_| JiraError::BrokerResponse)?;
    let metadata = connection_metadata(&credential, selected);
    let metadata_json = serde_json::to_string(&metadata).map_err(|_| JiraError::BrokerResponse)?;
    let vault = state.sdd_credentials.clone();
    let credential_for_vault = credential_to_bytes(&credential)?;
    let selected_for_vault = selected;
    tokio::task::spawn_blocking(move || {
        let credential: JiraCredential = serde_json::from_slice(&credential_for_vault)
            .map_err(|_| crate::sdd_v2::credentials::VaultError::Unsafe)?;
        put_jira_credential(vault.as_ref(), &credential, selected_for_vault)
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    state
        .store
        .sdd_complete_oauth_redemption(
            &flow.flow_id,
            flow.revision,
            UpsertSddIntegrationConnection {
                connection_id: &credential.connection_id,
                provider: "jira",
                external_account_id: &credential.account_id,
                display_name: &credential.display_name,
                selected_site_id: selected.then_some(credential.selected_site_id.as_str()),
                metadata_json: &metadata_json,
                credential_revision: credential.credential_revision,
            },
        )
        .await?;
    let vault = state.sdd_credentials.clone();
    let flow_id = flow.flow_id.clone();
    let _ = tokio::task::spawn_blocking(move || delete_jira_flow_secret(vault.as_ref(), &flow_id))
        .await;
    Ok(connection_view(&credential, selected))
}

pub async fn select_site(
    state: &AppState,
    connection_id: &str,
    site_id: &str,
    expected_credential_revision: i64,
) -> Result<JiraConnectionView, JiraError> {
    validate_tokenish(connection_id, 256)?;
    validate_tokenish(site_id, 256)?;
    let vault = state.sdd_credentials.clone();
    let connection = connection_id.to_owned();
    let mut credential =
        tokio::task::spawn_blocking(move || get_jira_credential(vault.as_ref(), Some(&connection)))
            .await
            .map_err(|_| JiraError::Vault)?
            .map_err(|_| JiraError::Vault)?
            .ok_or(JiraError::Connection)?;
    if credential.credential_revision != expected_credential_revision {
        return Err(JiraError::Connection);
    }
    credential
        .select_site(site_id)
        .map_err(|_| JiraError::Connection)?;
    let metadata = connection_metadata(&credential, true);
    let metadata_json = serde_json::to_string(&metadata).map_err(|_| JiraError::Connection)?;
    let vault = state.sdd_credentials.clone();
    let credential_for_vault = credential_to_bytes(&credential)?;
    tokio::task::spawn_blocking(move || {
        let credential: JiraCredential = serde_json::from_slice(&credential_for_vault)
            .map_err(|_| crate::sdd_v2::credentials::VaultError::Unsafe)?;
        put_jira_credential(vault.as_ref(), &credential, true)
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    state
        .store
        .sdd_select_integration_site(
            "jira",
            connection_id,
            site_id,
            expected_credential_revision,
            &metadata_json,
        )
        .await?;
    Ok(connection_view(&credential, true))
}

pub async fn selected_connection(
    state: &AppState,
) -> Result<Option<JiraConnectionView>, JiraError> {
    let vault = state.sdd_credentials.clone();
    let credential = tokio::task::spawn_blocking(move || get_jira_credential(vault.as_ref(), None))
        .await
        .map_err(|_| JiraError::Vault)?
        .map_err(|_| JiraError::Vault)?;
    if let Some(credential) = credential {
        let metadata = state
            .store
            .sdd_integration_connection("jira", &credential.connection_id)
            .await?;
        if metadata.as_ref().is_some_and(|metadata| {
            metadata.credential_revision == credential.credential_revision
                && metadata.selected_site_id.as_deref()
                    == Some(credential.selected_site_id.as_str())
        }) {
            return Ok(Some(connection_view(&credential, true)));
        }
        return Err(JiraError::Connection);
    }
    let vault = state.sdd_credentials.clone();
    let credential =
        tokio::task::spawn_blocking(move || get_jira_api_token_credential(vault.as_ref(), None))
            .await
            .map_err(|_| JiraError::Vault)?
            .map_err(|_| JiraError::Vault)?;
    let Some(credential) = credential else {
        return Ok(None);
    };
    let metadata = state
        .store
        .sdd_integration_connection("jira", &credential.connection_id)
        .await?;
    if metadata.as_ref().is_some_and(|metadata| {
        metadata.credential_revision == credential.credential_revision
            && metadata.selected_site_id.as_deref() == Some(credential.site.id.as_str())
    }) {
        Ok(Some(api_token_connection_view(&credential)))
    } else {
        Err(JiraError::Connection)
    }
}

pub async fn connections(state: &AppState) -> Result<Vec<JiraConnectionView>, JiraError> {
    let rows = state.store.sdd_integration_connections("jira").await?;
    rows.into_iter()
        .map(|row| {
            let metadata: JiraConnectionMetadata =
                serde_json::from_str(&row.metadata_json).map_err(|_| JiraError::Connection)?;
            Ok(JiraConnectionView {
                connection_id: row.connection_id,
                display_name: row.display_name,
                sites: metadata.sites,
                selected_site_id: row.selected_site_id.unwrap_or_default(),
                credential_revision: row.credential_revision,
                auth_kind: metadata.auth_kind.clone(),
                delivery_write_authorized: (metadata.auth_kind == "oauth"
                    && REQUIRED_SCOPES
                        .iter()
                        .all(|required| metadata.scopes.iter().any(|scope| scope == required)))
                    || (metadata.auth_kind == "api_token" && metadata.api_token_risk_acknowledged),
                granted_scopes: metadata.scopes,
            })
        })
        .collect()
}

/// Check the encrypted grant, not caller-supplied metadata, before Jira
/// delivery. This is intentionally separate from token access so capability
/// probing can never expose a credential.
pub(crate) async fn delivery_write_authorized(
    state: &AppState,
    connection_id: &str,
) -> Result<bool, JiraError> {
    validate_tokenish(connection_id, 256)?;
    let vault = state.sdd_credentials.clone();
    let connection_for_vault = connection_id.to_owned();
    let credential = tokio::task::spawn_blocking(move || {
        get_jira_credential(vault.as_ref(), Some(&connection_for_vault))
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    if let Some(credential) = credential
        && credential.has_scope("write:jira-work")
        && credential.has_scope("read:jira-work")
        && credential.has_scope("offline_access")
        && credential.selected_site().is_some()
    {
        let metadata = state
            .store
            .sdd_integration_connection("jira", connection_id)
            .await?;
        if metadata.is_some_and(|metadata| {
            metadata.credential_revision == credential.credential_revision
                && serde_json::from_str::<JiraConnectionMetadata>(&metadata.metadata_json)
                    .is_ok_and(|metadata| {
                        metadata.auth_kind == "oauth"
                            && REQUIRED_SCOPES.iter().all(|required| {
                                metadata.scopes.iter().any(|scope| scope == required)
                            })
                    })
        }) {
            return Ok(true);
        }
    }
    let credential = api_token_credential(state, connection_id).await?;
    Ok(credential.is_some())
}

/// Load a warning-gated local API-token credential for the delivery adapter.
/// Callers must keep it in-process and must never serialize or log it.
pub(crate) async fn api_token_credential(
    state: &AppState,
    connection_id: &str,
) -> Result<Option<JiraApiTokenCredential>, JiraError> {
    if !api_token_fallback_enabled() {
        return Ok(None);
    }
    let vault = state.sdd_credentials.clone();
    let connection = connection_id.to_owned();
    let credential = tokio::task::spawn_blocking(move || {
        get_jira_api_token_credential(vault.as_ref(), Some(&connection))
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    let Some(credential) = credential else {
        return Ok(None);
    };
    let metadata = state
        .store
        .sdd_integration_connection("jira", connection_id)
        .await?;
    let valid = metadata.is_some_and(|metadata| {
        metadata.credential_revision == credential.credential_revision
            && metadata.selected_site_id.as_deref() == Some(credential.site.id.as_str())
            && serde_json::from_str::<JiraConnectionMetadata>(&metadata.metadata_json).is_ok_and(
                |metadata| {
                    metadata.auth_kind == "api_token" && metadata.api_token_risk_acknowledged
                },
            )
    });
    Ok(valid.then_some(credential))
}

pub async fn fetch_issue(
    state: &AppState,
    connection_id: &str,
    site_id: &str,
    issue_key: &str,
) -> Result<FetchedJiraIssue, JiraError> {
    validate_issue_key(issue_key)?;
    let vault = state.sdd_credentials.clone();
    let connection = connection_id.to_owned();
    let credential =
        tokio::task::spawn_blocking(move || get_jira_credential(vault.as_ref(), Some(&connection)))
            .await
            .map_err(|_| JiraError::Vault)?
            .map_err(|_| JiraError::Vault)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| JiraError::IssueRead)?;
    let (response, canonical_connection, site) = if let Some(credential) = credential {
        if credential.connection_id != connection_id
            || credential.selected_site_id != site_id
            || credential.selected_site().is_none()
            || !credential.has_scope("read:jira-work")
        {
            return Err(JiraError::Connection);
        }
        let row = state
            .store
            .sdd_integration_connection("jira", connection_id)
            .await?
            .ok_or(JiraError::Connection)?;
        if row.credential_revision != credential.credential_revision
            || row.selected_site_id.as_deref() != Some(site_id)
        {
            return Err(JiraError::Connection);
        }
        let credential = ensure_fresh_credential(state, credential).await?;
        let site = credential
            .selected_site()
            .ok_or(JiraError::Connection)?
            .clone();
        let mut url =
            reqwest::Url::parse("https://api.atlassian.com/").map_err(|_| JiraError::IssueRead)?;
        url.path_segments_mut()
            .map_err(|_| JiraError::IssueRead)?
            .extend([
                "ex", "jira", &site.id, "rest", "api", "3", "issue", issue_key,
            ]);
        url.query_pairs_mut()
            .append_pair("fields", "summary,description,updated");
        let response = client
            .get(url)
            .bearer_auth(credential.access_token())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| JiraError::IssueRead)?;
        (response, credential.connection_id.clone(), site)
    } else {
        let credential = api_token_credential(state, connection_id)
            .await?
            .ok_or(JiraError::Connection)?;
        if credential.site.id != site_id {
            return Err(JiraError::Connection);
        }
        let mut url = validate_site_root(&credential.site.url)?;
        url.set_path(&format!("/rest/api/3/issue/{issue_key}"));
        url.query_pairs_mut()
            .append_pair("fields", "summary,description,updated");
        let response = client
            .get(url)
            .basic_auth(credential.email(), Some(credential.api_token()))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| JiraError::IssueRead)?;
        (
            response,
            credential.connection_id.clone(),
            credential.site.clone(),
        )
    };
    if !response.status().is_success() {
        return Err(JiraError::IssueRead);
    }
    let body = limited_bytes(response, MAX_JIRA_RESPONSE)
        .await
        .map_err(|_| JiraError::IssueRead)?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| JiraError::IssueRead)?;
    parse_issue_response(&value, &canonical_connection, &site, issue_key)
}

pub(crate) async fn ensure_fresh_credential(
    state: &AppState,
    mut credential: JiraCredential,
) -> Result<JiraCredential, JiraError> {
    if credential.expires_at_unix > OffsetDateTime::now_utc().unix_timestamp() + 60 {
        return Ok(credential);
    }
    let _guard = REFRESH_LOCK.lock().await;
    let vault = state.sdd_credentials.clone();
    let connection_id = credential.connection_id.clone();
    if let Some(latest) = tokio::task::spawn_blocking(move || {
        get_jira_credential(vault.as_ref(), Some(&connection_id))
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?
    {
        if latest.credential_revision > credential.credential_revision
            && latest.expires_at_unix > OffsetDateTime::now_utc().unix_timestamp() + 60
        {
            return Ok(latest);
        }
        credential = latest;
    }
    let broker = BrokerConfig::from_environment()?;
    let vault = state.sdd_credentials.clone();
    let device_key_ref = credential.device_key_ref.clone();
    let private_key = tokio::task::spawn_blocking(move || {
        get_jira_device_private_key(vault.as_ref(), &device_key_ref)
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?
    .ok_or(JiraError::Vault)?;
    let proof = sign_refresh(private_key.expose(), &credential)?;
    let request = BrokerRefreshRequest {
        connection_id: &credential.connection_id,
        refresh_token: credential.refresh_token(),
        credential_revision: credential.credential_revision,
        device_proof: proof,
    };
    let response: BrokerRefreshResponse = broker_post(
        broker.endpoint("v1/jira/oauth/refresh")?,
        &request,
        MAX_BROKER_RESPONSE,
    )
    .await?;
    if response.access_token.trim().is_empty()
        || response.refresh_token.trim().is_empty()
        || response.refresh_token == credential.refresh_token()
        || response.expires_in <= 60
        || response.expires_in > 24 * 60 * 60
    {
        return Err(JiraError::BrokerResponse);
    }
    let replacement = JiraCredential::new(
        credential.connection_id.clone(),
        credential.account_id.clone(),
        credential.display_name.clone(),
        response.access_token,
        response.refresh_token,
        credential.granted_scopes().to_vec(),
        OffsetDateTime::now_utc().unix_timestamp() + response.expires_in,
        credential.sites.clone(),
        credential.selected_site_id.clone(),
        credential.credential_revision + 1,
        credential.device_key_ref.clone(),
    )
    .map_err(|_| JiraError::BrokerResponse)?;
    let vault = state.sdd_credentials.clone();
    let bytes = credential_to_bytes(&replacement)?;
    tokio::task::spawn_blocking(move || {
        let replacement: JiraCredential = serde_json::from_slice(&bytes)
            .map_err(|_| crate::sdd_v2::credentials::VaultError::Unsafe)?;
        put_jira_credential(vault.as_ref(), &replacement, true)
    })
    .await
    .map_err(|_| JiraError::Vault)?
    .map_err(|_| JiraError::Vault)?;
    state
        .store
        .sdd_replace_integration_credential_revision(
            "jira",
            &replacement.connection_id,
            credential.credential_revision,
            replacement.credential_revision,
        )
        .await?;
    Ok(replacement)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JiraConnectionMetadata {
    sites: Vec<JiraSite>,
    scopes: Vec<String>,
    auth_kind: String,
    site_selection_required: bool,
    #[serde(default)]
    api_token_risk_acknowledged: bool,
}

fn connection_metadata(credential: &JiraCredential, selected: bool) -> JiraConnectionMetadata {
    JiraConnectionMetadata {
        sites: credential.sites.clone(),
        scopes: credential.granted_scopes().to_vec(),
        auth_kind: "oauth".into(),
        site_selection_required: !selected,
        api_token_risk_acknowledged: false,
    }
}

fn connection_view(credential: &JiraCredential, selected: bool) -> JiraConnectionView {
    JiraConnectionView {
        connection_id: credential.connection_id.clone(),
        display_name: credential.display_name.clone(),
        sites: credential.sites.clone(),
        selected_site_id: if selected {
            credential.selected_site_id.clone()
        } else {
            String::new()
        },
        credential_revision: credential.credential_revision,
        auth_kind: "oauth".into(),
        granted_scopes: credential.granted_scopes().to_vec(),
        delivery_write_authorized: credential.has_scope("write:jira-work")
            && credential.has_scope("read:jira-work")
            && credential.has_scope("offline_access"),
    }
}

fn api_token_connection_view(credential: &JiraApiTokenCredential) -> JiraConnectionView {
    JiraConnectionView {
        connection_id: credential.connection_id.clone(),
        display_name: credential.display_name.clone(),
        sites: vec![credential.site.clone()],
        selected_site_id: credential.site.id.clone(),
        credential_revision: credential.credential_revision,
        auth_kind: "api_token".into(),
        granted_scopes: Vec::new(),
        delivery_write_authorized: true,
    }
}

fn credential_to_bytes(credential: &JiraCredential) -> Result<Vec<u8>, JiraError> {
    serde_json::to_vec(credential).map_err(|_| JiraError::Vault)
}

fn validate_broker_token_response(response: &BrokerTokenResponse) -> Result<(), JiraError> {
    validate_tokenish(&response.account_id, 256)?;
    validate_tokenish(&response.display_name, 512)?;
    if response.access_token.trim().is_empty()
        || response.access_token.len() > 64 * 1024
        || response.refresh_token.trim().is_empty()
        || response.refresh_token.len() > 64 * 1024
        || response.expires_in <= 60
        || response.expires_in > 24 * 60 * 60
        || !REQUIRED_SCOPES
            .iter()
            .all(|required| response.scopes.iter().any(|scope| scope == required))
        || response.scopes.iter().any(|scope| {
            !REQUIRED_SCOPES.contains(&scope.as_str())
                || scope.trim().is_empty()
                || scope.len() > 128
        })
        || response.sites.is_empty()
        || response.sites.len() > 100
    {
        return Err(JiraError::BrokerResponse);
    }
    for site in &response.sites {
        validate_tokenish(&site.id, 256)?;
        validate_tokenish(&site.name, 512)?;
        let url = reqwest::Url::parse(&site.url).map_err(|_| JiraError::BrokerResponse)?;
        if url.scheme() != "https"
            || !url
                .host_str()
                .is_some_and(|host| host.ends_with(".atlassian.net"))
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(JiraError::BrokerResponse);
        }
    }
    Ok(())
}

fn validate_authorization_url(value: &str) -> Result<(), JiraError> {
    let url = reqwest::Url::parse(value).map_err(|_| JiraError::BrokerResponse)?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.atlassian.com")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(JiraError::BrokerResponse);
    }
    Ok(())
}

fn validate_api_token_input(email: &str, api_token: &str) -> Result<(), JiraError> {
    if email.trim().is_empty()
        || email.len() > 512
        || !email.contains('@')
        || email.chars().any(char::is_control)
        || api_token.trim().is_empty()
        || api_token.len() > 64 * 1024
        || api_token.chars().any(char::is_control)
    {
        return Err(JiraError::ApiTokenRejected);
    }
    Ok(())
}

fn validate_site_root(value: &str) -> Result<reqwest::Url, JiraError> {
    let mut url = reqwest::Url::parse(value.trim()).map_err(|_| JiraError::ApiTokenRejected)?;
    let host = url.host_str().ok_or(JiraError::ApiTokenRejected)?;
    if url.scheme() != "https"
        || host == "atlassian.net"
        || !host.ends_with(".atlassian.net")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(JiraError::ApiTokenRejected);
    }
    url.set_path("/");
    Ok(url)
}

fn validate_tokenish(value: &str, maximum: usize) -> Result<(), JiraError> {
    if value.trim().is_empty()
        || value.len() > maximum
        || value.chars().any(|character| character.is_control())
    {
        return Err(JiraError::BrokerResponse);
    }
    Ok(())
}

fn validate_issue_key(value: &str) -> Result<(), JiraError> {
    let Some((project, number)) = value.split_once('-') else {
        return Err(JiraError::IssueKey);
    };
    if value.len() > 64
        || project.is_empty()
        || project.len() > 32
        || !project
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        || !project
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_uppercase)
        || number.starts_with('0')
        || !number.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(JiraError::IssueKey);
    }
    Ok(())
}

fn sign_redemption(
    private_key: &[u8],
    flow_id: &str,
    redemption_id: &str,
    state: &str,
) -> Result<String, JiraError> {
    let key = Ed25519KeyPair::from_pkcs8(private_key).map_err(|_| JiraError::Flow)?;
    let payload = format!("agentum-jira-redemption-v1\n{flow_id}\n{redemption_id}\n{state}");
    Ok(URL_SAFE_NO_PAD.encode(key.sign(payload.as_bytes()).as_ref()))
}

fn sign_refresh(private_key: &[u8], credential: &JiraCredential) -> Result<String, JiraError> {
    let key = Ed25519KeyPair::from_pkcs8(private_key).map_err(|_| JiraError::Vault)?;
    let payload = format!(
        "agentum-jira-refresh-v1\n{}\n{}\n{}",
        credential.connection_id,
        credential.credential_revision,
        sha256(credential.refresh_token().as_bytes())
    );
    Ok(URL_SAFE_NO_PAD.encode(key.sign(payload.as_bytes()).as_ref()))
}

async fn broker_post<T: Serialize + ?Sized, R: for<'de> Deserialize<'de>>(
    url: reqwest::Url,
    body: &T,
    maximum: usize,
) -> Result<R, JiraError> {
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| JiraError::BrokerUnavailable("HTTP client failed".into()))?
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|_| JiraError::BrokerUnavailable("broker request failed".into()))?;
    if !response.status().is_success() {
        return Err(JiraError::BrokerResponse);
    }
    let bytes = limited_bytes(response, maximum)
        .await
        .map_err(|_| JiraError::BrokerResponse)?;
    serde_json::from_slice(&bytes).map_err(|_| JiraError::BrokerResponse)
}

async fn limited_bytes(response: reqwest::Response, maximum: usize) -> Result<Vec<u8>, JiraError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(JiraError::BrokerResponse);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| JiraError::BrokerResponse)?;
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(JiraError::BrokerResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_issue_response(
    value: &Value,
    connection_id: &str,
    site: &JiraSite,
    requested_key: &str,
) -> Result<FetchedJiraIssue, JiraError> {
    let required = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or(JiraError::IssueRead)
    };
    let key = required("/key")?;
    if !key.eq_ignore_ascii_case(requested_key) {
        return Err(JiraError::IssueRead);
    }
    let description = value
        .pointer("/fields/description")
        .map(adf_to_markdown)
        .transpose()?
        .unwrap_or_else(|| "No description was supplied by Jira.\n".into());
    Ok(FetchedJiraIssue {
        id: required("/id")?,
        key,
        title: required("/fields/summary")?,
        description,
        url: format!("{}/browse/{requested_key}", site.url.trim_end_matches('/')),
        updated_at: required("/fields/updated")?,
        connection_id: connection_id.to_owned(),
        site_id: site.id.clone(),
    })
}

fn adf_to_markdown(value: &Value) -> Result<String, JiraError> {
    if value.is_null() {
        return Ok(String::new());
    }
    if value.get("type").and_then(Value::as_str) != Some("doc") {
        return Err(JiraError::IssueRead);
    }
    let mut output = String::new();
    render_adf(value, &mut output, 0)?;
    let normalized = output.trim();
    if normalized.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("{normalized}\n"))
    }
}

fn render_adf(value: &Value, output: &mut String, depth: usize) -> Result<(), JiraError> {
    if depth > 32 || output.len() > MAX_JIRA_RESPONSE {
        return Err(JiraError::IssueRead);
    }
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "text" => {
            let text = value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if text.chars().any(|character| {
                character == '\0'
                    || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            }) {
                return Err(JiraError::IssueRead);
            }
            output.push_str(text);
        }
        "hardBreak" => output.push('\n'),
        "paragraph" => {
            render_children(value, output, depth)?;
            output.push_str("\n\n");
        }
        "heading" => {
            let level = value
                .pointer("/attrs/level")
                .and_then(Value::as_u64)
                .unwrap_or(2)
                .clamp(1, 6);
            output.push_str(&"#".repeat(level as usize));
            output.push(' ');
            render_children(value, output, depth)?;
            output.push_str("\n\n");
        }
        "bulletList" | "orderedList" | "listItem" | "blockquote" | "doc" | "panel" | "expand" => {
            render_children(value, output, depth)?
        }
        "codeBlock" => {
            output.push_str("```\n");
            render_children(value, output, depth)?;
            output.push_str("\n```\n\n");
        }
        "rule" => output.push_str("\n---\n"),
        "mention" => {
            output.push_str(
                value
                    .pointer("/attrs/text")
                    .and_then(Value::as_str)
                    .unwrap_or("@user"),
            );
        }
        "emoji" => output.push_str(
            value
                .pointer("/attrs/text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "inlineCard" | "blockCard" => output.push_str(
            value
                .pointer("/attrs/url")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "media" | "mediaSingle" | "mediaGroup" => output.push_str("[Jira media omitted]"),
        _ => render_children(value, output, depth)?,
    }
    Ok(())
}

fn render_children(value: &Value, output: &mut String, depth: usize) -> Result<(), JiraError> {
    if let Some(children) = value.get("content").and_then(Value::as_array) {
        for child in children {
            if child.get("type").and_then(Value::as_str) == Some("listItem") {
                output.push_str("- ");
            }
            render_adf(child, output, depth + 1)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_issue_key_is_strict() {
        for valid in ["ENG-1", "TEAM_2-42"] {
            assert!(validate_issue_key(valid).is_ok());
        }
        for invalid in ["eng-1", "ENG-0", "ENG-01", "../ENG-1", "ENG-1/x"] {
            assert!(validate_issue_key(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn jira_adf_is_normalized_without_executable_interpretation() {
        let issue = json!({
            "id": "10001",
            "key": "ENG-7",
            "fields": {
                "summary": "Refresh sessions",
                "updated": "2026-07-27T10:00:00.000+0000",
                "description": {
                    "type": "doc", "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{"type":"text","text":"Keep sessions online."}]
                    }]
                }
            }
        });
        let site = JiraSite {
            id: "site-1".into(),
            name: "Example".into(),
            url: "https://example.atlassian.net".into(),
        };
        let parsed = parse_issue_response(&issue, "jira-account", &site, "ENG-7").unwrap();
        assert_eq!(parsed.description, "Keep sessions online.\n");
        assert_eq!(parsed.site_id, "site-1");
        assert_eq!(parsed.url, "https://example.atlassian.net/browse/ENG-7");
    }

    #[test]
    fn authorization_and_site_urls_are_pinned_to_atlassian_https() {
        assert!(validate_authorization_url("https://auth.atlassian.com/authorize?x=1").is_ok());
        assert!(validate_authorization_url("https://evil.example/authorize").is_err());
        let response = BrokerTokenResponse {
            account_id: "account-1".into(),
            display_name: "Example".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_in: 3600,
            scopes: REQUIRED_SCOPES.iter().map(ToString::to_string).collect(),
            sites: vec![JiraSite {
                id: "site-1".into(),
                name: "Example".into(),
                url: "https://example.atlassian.net".into(),
            }],
        };
        assert!(validate_broker_token_response(&response).is_ok());
        assert_eq!(
            REQUIRED_SCOPES,
            ["read:jira-work", "write:jira-work", "offline_access"]
        );
        let mut insufficient = response;
        insufficient
            .scopes
            .retain(|scope| scope != "write:jira-work");
        assert!(validate_broker_token_response(&insufficient).is_err());
    }

    #[test]
    fn api_token_fallback_is_pinned_to_one_atlassian_tenant_root() {
        let url = validate_site_root("https://team.atlassian.net/").unwrap();
        assert_eq!(url.as_str(), "https://team.atlassian.net/");
        for unsafe_url in [
            "http://team.atlassian.net",
            "https://atlassian.net",
            "https://team.atlassian.net.evil.example",
            "https://user@team.atlassian.net",
            "https://team.atlassian.net:8443",
            "https://team.atlassian.net/wiki",
            "https://team.atlassian.net/?redirect=evil",
        ] {
            assert!(validate_site_root(unsafe_url).is_err(), "{unsafe_url}");
        }
        assert!(validate_api_token_input("operator@example.com", "token-value").is_ok());
        assert!(validate_api_token_input("not-an-email", "token-value").is_err());
        assert!(validate_api_token_input("operator@example.com", "line\nbreak").is_err());
    }
}
