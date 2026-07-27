//! Agentum's independently deployable Jira Cloud OAuth broker.
//!
//! The service owns Atlassian's confidential-client exchange. Pending OAuth
//! codes and replay responses live only in bounded process memory. Its durable
//! database contains public device keys, credential revisions, and one-way
//! refresh-token hashes; it never contains OAuth tokens or Jira issue data.

mod atlassian;
mod config;
mod database;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use atlassian::{AtlassianClient, AtlassianError, AtlassianResource, TokenGrant};
use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, PRAGMA};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use database::{Database, DatabaseError, DeviceBinding};
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom as _, SystemRandom};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::Zeroize as _;

pub use config::{BrokerConfig, ConfigError};

pub const REQUIRED_SCOPES: [&str; 3] = ["read:jira-work", "write:jira-work", "offline_access"];
const FLOW_TTL_SECONDS: i64 = 15 * 60;
const REDEEM_REPLAY_TTL_SECONDS: i64 = 5 * 60;
const REFRESH_REPLAY_TTL_SECONDS: i64 = 10 * 60;
const MAX_ACTIVE_FLOWS: usize = 10_000;
const MAX_REFRESH_REPLAYS: usize = 10_000;
const MAX_REQUEST_BODY: usize = 96 * 1024;

#[derive(Clone)]
pub struct Broker {
    inner: Arc<BrokerInner>,
}

struct BrokerInner {
    atlassian: AtlassianClient,
    database: Database,
    flows: Mutex<FlowStore>,
    refresh_replays: Mutex<HashMap<RefreshKey, CachedRefresh>>,
    refresh_gate: Mutex<()>,
}

impl Broker {
    pub fn new(config: BrokerConfig) -> Result<Self, StartupError> {
        let database = Database::open(&config.database_path)?;
        let atlassian = AtlassianClient::new(
            config.client_id,
            config.client_secret,
            config.callback_url,
            config.endpoints,
        )?;
        Ok(Self {
            inner: Arc::new(BrokerInner {
                atlassian,
                database,
                flows: Mutex::new(FlowStore::default()),
                refresh_replays: Mutex::new(HashMap::new()),
                refresh_gate: Mutex::new(()),
            }),
        })
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/healthz", get(health))
            .route("/v1/jira/oauth/start", post(start))
            .route("/v1/jira/oauth/callback", get(callback))
            .route("/v1/jira/oauth/redeem", post(redeem))
            .route("/v1/jira/oauth/refresh", post(refresh))
            .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY))
            .with_state(self)
    }
}

#[derive(Default)]
struct FlowStore {
    by_redemption: HashMap<String, PendingFlow>,
    by_state: HashMap<String, String>,
}

impl FlowStore {
    fn remove_expired(&mut self, now: i64) {
        self.by_redemption.retain(|_, flow| {
            flow.expires_at_unix > now || matches!(flow.status, FlowStatus::Redeeming)
        });
        self.by_state
            .retain(|_, redemption_id| self.by_redemption.contains_key(redemption_id));
    }
}

struct PendingFlow {
    state: String,
    device_public_key: [u8; 32],
    expires_at_unix: i64,
    status: FlowStatus,
}

enum FlowStatus {
    AwaitingCallback,
    Authorized {
        code: SecretString,
    },
    Redeeming,
    Redeemed {
        request_fingerprint: String,
        response: Arc<BrokerTokenResponse>,
    },
}

#[derive(Clone)]
struct SecretString(String);

impl SecretString {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartRequest {
    state: String,
    device_public_key: String,
    scopes: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartResponse {
    authorization_url: String,
    redemption_id: String,
    expires_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackQuery {
    state: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RedeemRequest {
    redemption_id: String,
    state: String,
    flow_id: String,
    device_proof: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrokerTokenResponse {
    account_id: String,
    display_name: String,
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    scopes: Vec<String>,
    sites: Vec<JiraSite>,
}

impl Drop for BrokerTokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JiraSite {
    id: String,
    name: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RefreshRequest {
    connection_id: String,
    refresh_token: String,
    credential_revision: i64,
    device_proof: String,
}

impl Drop for RefreshRequest {
    fn drop(&mut self) {
        self.refresh_token.zeroize();
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

impl Drop for RefreshResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct RefreshKey {
    connection_id: String,
    credential_revision: i64,
    refresh_token_hash: String,
}

struct CachedRefresh {
    response: Arc<RefreshResponse>,
    expires_at_unix: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
    schema_version: u32,
}

async fn health() -> impl IntoResponse {
    no_store_json(Json(HealthResponse {
        status: "ok",
        service: "agentum-jira-oauth-broker",
        version: env!("CARGO_PKG_VERSION"),
        schema_version: 1,
    }))
}

async fn start(
    State(broker): State<Broker>,
    request: Result<Json<StartRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let state_bytes = decode_exact::<32>(&request.state).ok_or(ApiError::InvalidRequest)?;
    let device_public_key =
        decode_exact::<32>(&request.device_public_key).ok_or(ApiError::InvalidRequest)?;
    if !exact_required_scopes(&request.scopes) {
        return Err(ApiError::InvalidScopes);
    }

    let now = now_unix();
    let expires_at_unix = now + FLOW_TTL_SECONDS;
    let redemption_id = random_redemption_id()?;
    let authorization_url = broker
        .inner
        .atlassian
        .authorization_url(&request.state, &REQUIRED_SCOPES);
    let expires_at = OffsetDateTime::from_unix_timestamp(expires_at_unix)
        .map_err(|_| ApiError::Unavailable)?
        .format(&Rfc3339)
        .map_err(|_| ApiError::Unavailable)?;

    let mut flows = broker.inner.flows.lock().await;
    flows.remove_expired(now);
    if flows.by_redemption.len() >= MAX_ACTIVE_FLOWS {
        return Err(ApiError::Capacity);
    }
    let state_key = URL_SAFE_NO_PAD.encode(state_bytes);
    if state_key != request.state || flows.by_state.contains_key(&request.state) {
        return Err(ApiError::Conflict);
    }
    flows
        .by_state
        .insert(request.state.clone(), redemption_id.clone());
    flows.by_redemption.insert(
        redemption_id.clone(),
        PendingFlow {
            state: request.state,
            device_public_key,
            expires_at_unix,
            status: FlowStatus::AwaitingCallback,
        },
    );
    drop(flows);

    Ok(no_store_json(Json(StartResponse {
        authorization_url: authorization_url.into(),
        redemption_id,
        expires_at,
    })))
}

async fn callback(
    State(broker): State<Broker>,
    query: Result<Query<CallbackQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return callback_page(StatusCode::BAD_REQUEST, false);
    };
    if decode_exact::<32>(&query.state).is_none() {
        return callback_page(StatusCode::BAD_REQUEST, false);
    }

    let now = now_unix();
    let mut flows = broker.inner.flows.lock().await;
    flows.remove_expired(now);
    let Some(redemption_id) = flows.by_state.get(&query.state).cloned() else {
        return callback_page(StatusCode::GONE, false);
    };
    if query.error.is_some() {
        // Never echo provider error text: it is untrusted callback input.
        let _ = query.error_description;
        flows.by_state.remove(&query.state);
        flows.by_redemption.remove(&redemption_id);
        return callback_page(StatusCode::BAD_REQUEST, false);
    }
    let Some(code) = query.code else {
        return callback_page(StatusCode::BAD_REQUEST, false);
    };
    if code.trim().is_empty() || code.len() > 4096 || code.chars().any(char::is_control) {
        return callback_page(StatusCode::BAD_REQUEST, false);
    }
    let Some(flow) = flows.by_redemption.get_mut(&redemption_id) else {
        return callback_page(StatusCode::GONE, false);
    };
    if !matches!(flow.status, FlowStatus::AwaitingCallback) {
        return callback_page(StatusCode::CONFLICT, false);
    }
    flow.status = FlowStatus::Authorized {
        code: SecretString(code),
    };
    callback_page(StatusCode::OK, true)
}

async fn redeem(
    State(broker): State<Broker>,
    request: Result<Json<RedeemRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    validate_redeem_request(&request)?;
    let request_fingerprint = sha256_hex(
        format!(
            "{}\n{}\n{}\n{}",
            request.flow_id, request.redemption_id, request.state, request.device_proof
        )
        .as_bytes(),
    );

    let now = now_unix();
    let (device_public_key, code) = {
        let mut flows = broker.inner.flows.lock().await;
        flows.remove_expired(now);
        let flow = flows
            .by_redemption
            .get_mut(&request.redemption_id)
            .ok_or(ApiError::FlowExpired)?;
        if flow.state != request.state {
            return Err(ApiError::Forbidden);
        }
        verify_redemption_proof(&request, &flow.device_public_key)?;
        match &flow.status {
            FlowStatus::Redeemed {
                request_fingerprint: expected,
                response,
            } => {
                if expected != &request_fingerprint {
                    return Err(ApiError::Replay);
                }
                return Ok(no_store_json(Json(response.as_ref().clone())));
            }
            FlowStatus::AwaitingCallback => return Err(ApiError::AuthorizationPending),
            FlowStatus::Redeeming => return Err(ApiError::Conflict),
            FlowStatus::Authorized { code } => {
                let code = code.clone();
                let key = flow.device_public_key;
                flow.expires_at_unix = now + 2 * 60;
                flow.status = FlowStatus::Redeeming;
                (key, code)
            }
        }
    };

    let result = complete_redemption(&broker, &device_public_key, code.expose()).await;
    match result {
        Ok(response) => {
            let response = Arc::new(response);
            let mut flows = broker.inner.flows.lock().await;
            let flow = flows
                .by_redemption
                .get_mut(&request.redemption_id)
                .ok_or(ApiError::FlowExpired)?;
            flow.expires_at_unix = now_unix() + REDEEM_REPLAY_TTL_SECONDS;
            flow.status = FlowStatus::Redeemed {
                request_fingerprint,
                response: response.clone(),
            };
            Ok(no_store_json(Json(response.as_ref().clone())))
        }
        Err(error) => {
            let mut flows = broker.inner.flows.lock().await;
            if let Some(flow) = flows.by_redemption.get_mut(&request.redemption_id)
                && matches!(flow.status, FlowStatus::Redeeming)
            {
                flow.status = FlowStatus::Authorized { code };
            }
            Err(error)
        }
    }
}

async fn complete_redemption(
    broker: &Broker,
    device_public_key: &[u8; 32],
    code: &str,
) -> Result<BrokerTokenResponse, ApiError> {
    let grant = broker.inner.atlassian.exchange_code(code).await?;
    let scopes = validate_grant_scopes(&grant, true)?;
    let resources = broker
        .inner
        .atlassian
        .accessible_resources(&grant.access_token)
        .await?;
    let sites = sanitize_jira_sites(resources)?;

    // The exact minimal Jira scopes intentionally omit Atlassian's separate
    // `read:me` identity scope. Use a device-bound, broker-local grant identity
    // instead of collecting a profile or pretending a site ID is a user ID.
    let account_id = format!("agentum-grant-{}", sha256_hex(device_public_key));
    let connection_id = format!("jira-{}", &sha256_hex(account_id.as_bytes())[..24]);
    let display_name = if sites.len() == 1 {
        format!("Jira — {}", sites[0].name)
    } else {
        format!("Jira — {} sites", sites.len())
    };
    let response = BrokerTokenResponse {
        account_id,
        display_name,
        access_token: grant.access_token.clone(),
        refresh_token: grant.refresh_token.clone(),
        expires_in: grant.expires_in,
        scopes,
        sites,
    };
    broker.inner.database.replace_binding(
        &connection_id,
        device_public_key,
        &sha256_hex(grant.refresh_token.as_bytes()),
        now_unix(),
    )?;
    Ok(response)
}

async fn refresh(
    State(broker): State<Broker>,
    request: Result<Json<RefreshRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    validate_refresh_request(&request)?;
    let refresh_token_hash = sha256_hex(request.refresh_token.as_bytes());
    let replay_key = RefreshKey {
        connection_id: request.connection_id.clone(),
        credential_revision: request.credential_revision,
        refresh_token_hash: refresh_token_hash.clone(),
    };

    let _gate = broker.inner.refresh_gate.lock().await;
    let binding = broker
        .inner
        .database
        .binding(&request.connection_id)?
        .ok_or(ApiError::Forbidden)?;
    verify_refresh_proof(&request, &refresh_token_hash, &binding)?;
    let now = now_unix();
    {
        let mut replays = broker.inner.refresh_replays.lock().await;
        replays.retain(|_, cached| cached.expires_at_unix > now);
        if let Some(cached) = replays.get(&replay_key) {
            return Ok(no_store_json(Json(cached.response.as_ref().clone())));
        }
        if replays.len() >= MAX_REFRESH_REPLAYS {
            return Err(ApiError::Capacity);
        }
    }
    let current_matches = binding.credential_revision == request.credential_revision
        && bool::from(
            binding
                .refresh_token_hash
                .as_bytes()
                .ct_eq(refresh_token_hash.as_bytes()),
        );
    let recoverable_previous = binding.previous_credential_revision
        == Some(request.credential_revision)
        && binding
            .previous_valid_until_unix
            .is_some_and(|until| until > now)
        && binding
            .previous_refresh_token_hash
            .as_deref()
            .is_some_and(|previous| {
                bool::from(previous.as_bytes().ct_eq(refresh_token_hash.as_bytes()))
            });
    if !current_matches && !recoverable_previous {
        return Err(ApiError::StaleCredential);
    }

    let grant = broker
        .inner
        .atlassian
        .refresh(&request.refresh_token)
        .await?;
    let _ = validate_grant_scopes(&grant, false)?;
    if grant.refresh_token == request.refresh_token {
        return Err(ApiError::MalformedUpstream);
    }
    let replacement_hash = sha256_hex(grant.refresh_token.as_bytes());
    let stored = if current_matches {
        broker.inner.database.replace_refresh_hash(
            &request.connection_id,
            request.credential_revision,
            &refresh_token_hash,
            &replacement_hash,
            now,
            now + REFRESH_REPLAY_TTL_SECONDS,
        )?
    } else {
        broker.inner.database.recover_refresh_hash(
            &request.connection_id,
            request.credential_revision,
            &refresh_token_hash,
            &replacement_hash,
            now,
        )?
    };
    if !stored {
        return Err(ApiError::StaleCredential);
    }
    let response = Arc::new(RefreshResponse {
        access_token: grant.access_token.clone(),
        refresh_token: grant.refresh_token.clone(),
        expires_in: grant.expires_in,
    });
    let mut replays = broker.inner.refresh_replays.lock().await;
    replays.retain(|_, cached| cached.expires_at_unix > now);
    replays.insert(
        replay_key,
        CachedRefresh {
            response: response.clone(),
            expires_at_unix: now + REFRESH_REPLAY_TTL_SECONDS,
        },
    );
    Ok(no_store_json(Json(response.as_ref().clone())))
}

fn validate_redeem_request(request: &RedeemRequest) -> Result<(), ApiError> {
    if !valid_tokenish(&request.redemption_id, 512)
        || decode_exact::<32>(&request.state).is_none()
        || Uuid::parse_str(&request.flow_id).is_err()
        || decode_exact::<64>(&request.device_proof).is_none()
    {
        return Err(ApiError::InvalidRequest);
    }
    Ok(())
}

fn validate_refresh_request(request: &RefreshRequest) -> Result<(), ApiError> {
    if !valid_connection_id(&request.connection_id)
        || request.refresh_token.trim().is_empty()
        || request.refresh_token.len() > 64 * 1024
        || request.refresh_token.chars().any(char::is_control)
        || request.credential_revision <= 0
        || request.credential_revision > i64::MAX - 1
        || decode_exact::<64>(&request.device_proof).is_none()
    {
        return Err(ApiError::InvalidRequest);
    }
    Ok(())
}

fn verify_redemption_proof(
    request: &RedeemRequest,
    device_public_key: &[u8; 32],
) -> Result<(), ApiError> {
    let signature = decode_exact::<64>(&request.device_proof).ok_or(ApiError::InvalidRequest)?;
    let payload = format!(
        "agentum-jira-redemption-v1\n{}\n{}\n{}",
        request.flow_id, request.redemption_id, request.state
    );
    UnparsedPublicKey::new(&ED25519, device_public_key)
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| ApiError::Forbidden)
}

fn verify_refresh_proof(
    request: &RefreshRequest,
    refresh_token_hash: &str,
    binding: &DeviceBinding,
) -> Result<(), ApiError> {
    if binding.connection_id != request.connection_id || binding.device_public_key.len() != 32 {
        return Err(ApiError::Forbidden);
    }
    let signature = decode_exact::<64>(&request.device_proof).ok_or(ApiError::InvalidRequest)?;
    let payload = format!(
        "agentum-jira-refresh-v1\n{}\n{}\n{}",
        request.connection_id, request.credential_revision, refresh_token_hash
    );
    UnparsedPublicKey::new(&ED25519, &binding.device_public_key)
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| ApiError::Forbidden)
}

fn validate_grant_scopes(grant: &TokenGrant, required: bool) -> Result<Vec<String>, ApiError> {
    let Some(scope) = grant.scope.as_deref() else {
        if required {
            return Err(ApiError::MalformedUpstream);
        }
        return Ok(REQUIRED_SCOPES.iter().map(ToString::to_string).collect());
    };
    let scopes: Vec<String> = scope.split_ascii_whitespace().map(str::to_owned).collect();
    if !exact_required_scopes(&scopes) {
        return Err(ApiError::MalformedUpstream);
    }
    Ok(REQUIRED_SCOPES.iter().map(ToString::to_string).collect())
}

fn exact_required_scopes(scopes: &[String]) -> bool {
    if scopes.len() != REQUIRED_SCOPES.len() {
        return false;
    }
    let actual: HashSet<&str> = scopes.iter().map(String::as_str).collect();
    actual.len() == REQUIRED_SCOPES.len()
        && REQUIRED_SCOPES
            .iter()
            .all(|required| actual.contains(required))
}

fn sanitize_jira_sites(resources: Vec<AtlassianResource>) -> Result<Vec<JiraSite>, ApiError> {
    let mut sites = Vec::new();
    let mut ids = HashSet::new();
    for resource in resources {
        let has_read = resource
            .scopes
            .iter()
            .any(|scope| scope == "read:jira-work");
        let has_write = resource
            .scopes
            .iter()
            .any(|scope| scope == "write:jira-work");
        if !has_read && !has_write {
            continue;
        }
        if !has_read || !has_write || sites.len() >= 100 || !ids.insert(resource.id.clone()) {
            return Err(ApiError::MalformedUpstream);
        }
        if !valid_site_text(&resource.id, 256) || !valid_site_text(&resource.name, 512) {
            return Err(ApiError::MalformedUpstream);
        }
        let mut url =
            reqwest::Url::parse(&resource.url).map_err(|_| ApiError::MalformedUpstream)?;
        let host = url
            .host_str()
            .map(str::to_ascii_lowercase)
            .ok_or(ApiError::MalformedUpstream)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || host == "atlassian.net"
            || !host.ends_with(".atlassian.net")
        {
            return Err(ApiError::MalformedUpstream);
        }
        url.set_path("");
        sites.push(JiraSite {
            id: resource.id,
            name: resource.name,
            url: url.as_str().trim_end_matches('/').to_owned(),
        });
    }
    if sites.is_empty() {
        return Err(ApiError::MalformedUpstream);
    }
    sites.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(sites)
}

fn valid_site_text(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn valid_tokenish(value: &str, maximum: usize) -> bool {
    !value.trim().is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn valid_connection_id(value: &str) -> bool {
    value.len() == 29
        && value.starts_with("jira-")
        && value[5..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_exact<const LENGTH: usize>(encoded: &str) -> Option<[u8; LENGTH]> {
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    decoded.try_into().ok()
}

fn random_redemption_id() -> Result<String, ApiError> {
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ApiError::Unavailable)?;
    Ok(format!("rdm_{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = digest(&SHA256, value);
    let mut encoded = String::with_capacity(64);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn no_store_json<T: Serialize>(json: Json<T>) -> Response {
    let mut response = json.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn callback_page(status: StatusCode, success: bool) -> Response {
    let body = if success {
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Jira connected</title></head><body><main><h1>Authorization received</h1><p>You can return to Agentum to finish connecting Jira.</p></main></body></html>"
    } else {
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Jira connection not completed</title></head><body><main><h1>Authorization was not completed</h1><p>Return to Agentum and start the Jira connection again.</p></main></body></html>"
    };
    let mut response = (status, Html(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'none'; img-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("the broker database could not be initialized securely")]
    Database,
    #[error("the Atlassian OAuth client could not be initialized")]
    Atlassian,
}

impl From<DatabaseError> for StartupError {
    fn from(_: DatabaseError) -> Self {
        Self::Database
    }
}

impl From<AtlassianError> for StartupError {
    fn from(_: AtlassianError) -> Self {
        Self::Atlassian
    }
}

enum ApiError {
    InvalidRequest,
    InvalidScopes,
    AuthorizationPending,
    FlowExpired,
    Forbidden,
    Replay,
    Conflict,
    StaleCredential,
    Capacity,
    MalformedUpstream,
    Upstream,
    Unavailable,
}

impl From<DatabaseError> for ApiError {
    fn from(_: DatabaseError) -> Self {
        Self::Unavailable
    }
}

impl From<AtlassianError> for ApiError {
    fn from(error: AtlassianError) -> Self {
        match error {
            AtlassianError::Malformed => Self::MalformedUpstream,
            AtlassianError::Unavailable | AtlassianError::Rejected => Self::Upstream,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The request is invalid.",
            ),
            Self::InvalidScopes => (
                StatusCode::BAD_REQUEST,
                "invalid_scopes",
                "The exact supported Jira scopes are required.",
            ),
            Self::AuthorizationPending => (
                StatusCode::CONFLICT,
                "authorization_pending",
                "The Atlassian authorization callback has not completed.",
            ),
            Self::FlowExpired => (
                StatusCode::GONE,
                "flow_expired",
                "The OAuth flow is unknown or expired.",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "invalid_device_proof",
                "The device-bound proof is invalid.",
            ),
            Self::Replay => (
                StatusCode::CONFLICT,
                "replay_rejected",
                "The one-time request does not match the completed redemption.",
            ),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "request_conflict",
                "A conflicting operation is already in progress.",
            ),
            Self::StaleCredential => (
                StatusCode::PRECONDITION_FAILED,
                "stale_credential_revision",
                "The refresh token or credential revision is stale.",
            ),
            Self::Capacity => (
                StatusCode::TOO_MANY_REQUESTS,
                "capacity_reached",
                "The broker is at its bounded in-memory capacity.",
            ),
            Self::MalformedUpstream => (
                StatusCode::BAD_GATEWAY,
                "malformed_upstream_response",
                "Atlassian returned an invalid OAuth response.",
            ),
            Self::Upstream => (
                StatusCode::BAD_GATEWAY,
                "upstream_unavailable",
                "The Atlassian OAuth request did not complete.",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "broker_unavailable",
                "The OAuth broker is temporarily unavailable.",
            ),
        };
        #[derive(Serialize)]
        struct ErrorBody<'a> {
            code: &'a str,
            message: &'a str,
        }
        let mut response = (status, Json(ErrorBody { code, message })).into_response();
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
            .headers_mut()
            .insert(PRAGMA, HeaderValue::from_static("no-cache"));
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        response
    }
}

/// Serve until SIGINT or SIGTERM. TLS must terminate at the configured reverse
/// proxy; the public callback URL is always required to be HTTPS.
pub async fn serve(config: BrokerConfig) -> anyhow::Result<()> {
    let bind = config.bind;
    let broker = Broker::new(config)?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "Agentum Jira OAuth broker listening");
    axum::serve(listener, broker.router())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests;
