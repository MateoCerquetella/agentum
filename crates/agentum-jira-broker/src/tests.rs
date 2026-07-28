use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http_body_util::BodyExt as _;
use reqwest::Url;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tower::ServiceExt as _;

use super::*;
use crate::config::AtlassianEndpoints;

#[derive(Clone, Copy)]
enum MockMode {
    Valid,
    ExtraTokenScope,
    InvalidSite,
    NoRotatedRefresh,
}

#[derive(Clone)]
struct MockState {
    mode: MockMode,
    code_exchanges: Arc<AtomicUsize>,
    refreshes: Arc<AtomicUsize>,
    resource_reads: Arc<AtomicUsize>,
}

struct TestBroker {
    _directory: TempDir,
    database_path: std::path::PathBuf,
    broker: Broker,
    app: Router,
    upstream: MockState,
    upstream_task: JoinHandle<()>,
}

impl Drop for TestBroker {
    fn drop(&mut self) {
        self.upstream_task.abort();
    }
}

async fn test_broker(mode: MockMode) -> TestBroker {
    let upstream = MockState {
        mode,
        code_exchanges: Arc::new(AtomicUsize::new(0)),
        refreshes: Arc::new(AtomicUsize::new(0)),
        resource_reads: Arc::new(AtomicUsize::new(0)),
    };
    let upstream_app = Router::new()
        .route("/oauth/token", post(mock_token))
        .route("/resources", get(mock_resources))
        .with_state(upstream.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        axum::serve(listener, upstream_app).await.unwrap();
    });
    let base = Url::parse(&format!("http://{address}/")).unwrap();
    let endpoints = AtlassianEndpoints {
        authorization: Url::parse("https://auth.atlassian.com/authorize").unwrap(),
        token: base.join("oauth/token").unwrap(),
        accessible_resources: base.join("resources").unwrap(),
    };
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("broker.sqlite3");
    let config = BrokerConfig::for_test(
        database_path.clone(),
        Url::parse("https://broker.agentum.example/").unwrap(),
        endpoints,
    );
    let broker = Broker::new(config).unwrap();
    let app = broker.clone().router();
    TestBroker {
        _directory: directory,
        database_path,
        broker,
        app,
        upstream,
        upstream_task,
    }
}

async fn mock_token(
    State(state): State<MockState>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if body.get("client_id") != Some(&json!("test-client"))
        || body.get("client_secret") != Some(&json!("test-secret"))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        );
    }
    match body.get("grant_type").and_then(Value::as_str) {
        Some("authorization_code") if body.get("code") == Some(&json!("auth-code")) => {
            state.code_exchanges.fetch_add(1, Ordering::SeqCst);
            let scope = match state.mode {
                MockMode::ExtraTokenScope => {
                    "offline_access read:jira-work write:jira-work read:me"
                }
                _ => "offline_access write:jira-work read:jira-work",
            };
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": "access-token-one",
                    "refresh_token": "refresh-token-one",
                    "expires_in": 3600,
                    "token_type": "Bearer",
                    "scope": scope
                })),
            )
        }
        Some("refresh_token") if body.get("refresh_token") == Some(&json!("refresh-token-one")) => {
            state.refreshes.fetch_add(1, Ordering::SeqCst);
            let refresh = if matches!(state.mode, MockMode::NoRotatedRefresh) {
                "refresh-token-one"
            } else {
                "refresh-token-two"
            };
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": "access-token-two",
                    "refresh_token": refresh,
                    "expires_in": 3600,
                    "token_type": "Bearer"
                })),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        ),
    }
}

async fn mock_resources(
    State(state): State<MockState>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer access-token-one")
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        );
    }
    state.resource_reads.fetch_add(1, Ordering::SeqCst);
    let first_url = if matches!(state.mode, MockMode::InvalidSite) {
        "https://attacker.example"
    } else {
        "https://zeta.atlassian.net"
    };
    (
        StatusCode::OK,
        Json(json!([
            {
                "id": "site-z",
                "name": "Zeta",
                "url": first_url,
                "scopes": ["write:jira-work", "read:jira-work"]
            },
            {
                "id": "site-a",
                "name": "Alpha",
                "url": "https://alpha.atlassian.net",
                "scopes": ["read:jira-work", "write:jira-work"]
            },
            {
                "id": "not-jira",
                "name": "Confluence",
                "url": "https://wiki.atlassian.net",
                "scopes": ["read:confluence-content.summary"]
            }
        ])),
    )
}

struct Device {
    key_pair: Ed25519KeyPair,
}

impl Device {
    fn new() -> Self {
        let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Self {
            key_pair: Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap(),
        }
    }

    fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key_pair.public_key().as_ref())
    }

    fn sign(&self, payload: &str) -> String {
        URL_SAFE_NO_PAD.encode(self.key_pair.sign(payload.as_bytes()).as_ref())
    }
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn start_flow(app: &Router, device: &Device, state: &str) -> Value {
    let response = call(
        app,
        Method::POST,
        "/v1/jira/oauth/start",
        Some(json!({
            "state": state,
            "devicePublicKey": device.public_key(),
            "scopes": REQUIRED_SCOPES
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    response_json(response).await
}

async fn authorize(app: &Router, state: &str) -> Response {
    call(
        app,
        Method::GET,
        &format!("/v1/jira/oauth/callback?state={state}&code=auth-code"),
        None,
    )
    .await
}

fn redeem_body(device: &Device, flow: &Value, state: &str, flow_id: &str) -> Value {
    let redemption_id = flow["redemptionId"].as_str().unwrap();
    let payload = format!("agentum-jira-redemption-v1\n{flow_id}\n{redemption_id}\n{state}");
    json!({
        "redemptionId": redemption_id,
        "state": state,
        "flowId": flow_id,
        "deviceProof": device.sign(&payload)
    })
}

async fn completed_connection(test: &TestBroker, device: &Device) -> (Value, Value, String) {
    let state = URL_SAFE_NO_PAD.encode([9_u8; 32]);
    let flow = start_flow(&test.app, device, &state).await;
    let flow_id = "4b95a344-6db4-4fc5-8280-2cf10d9cbe9f";
    let body = redeem_body(device, &flow, &state, flow_id);
    let pending = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/redeem",
        Some(body.clone()),
    )
    .await;
    assert_eq!(pending.status(), StatusCode::CONFLICT);

    let callback = authorize(&test.app, &state).await;
    assert_eq!(callback.status(), StatusCode::OK);
    assert_eq!(callback.headers().get("cache-control").unwrap(), "no-store");
    assert!(callback.headers().contains_key("content-security-policy"));

    let response = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/redeem",
        Some(body.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let connection = response_json(response).await;
    let account_id = connection["accountId"].as_str().unwrap();
    let connection_id = format!("jira-{}", &sha256_hex(account_id.as_bytes())[..24]);
    (connection, body, connection_id)
}

#[tokio::test]
async fn complete_flow_is_device_bound_multisite_and_replay_safe() {
    let test = test_broker(MockMode::Valid).await;
    let device = Device::new();
    let state = URL_SAFE_NO_PAD.encode([9_u8; 32]);
    let flow = start_flow(&test.app, &device, &state).await;
    let authorization = Url::parse(flow["authorizationUrl"].as_str().unwrap()).unwrap();
    assert_eq!(
        authorization.origin().ascii_serialization(),
        "https://auth.atlassian.com"
    );
    let parameters: HashMap<_, _> = authorization.query_pairs().into_owned().collect();
    assert_eq!(parameters.get("audience").unwrap(), "api.atlassian.com");
    assert_eq!(parameters.get("state").unwrap(), &state);
    assert_eq!(
        parameters.get("scope").unwrap(),
        "read:jira-work write:jira-work offline_access"
    );
    assert_eq!(
        parameters.get("redirect_uri").unwrap(),
        "https://broker.agentum.example/v1/jira/oauth/callback"
    );

    let flow_id = "4b95a344-6db4-4fc5-8280-2cf10d9cbe9f";
    let redemption = redeem_body(&device, &flow, &state, flow_id);
    assert_eq!(authorize(&test.app, &state).await.status(), StatusCode::OK);
    assert_eq!(
        authorize(&test.app, &state).await.status(),
        StatusCode::CONFLICT
    );

    let response = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/redeem",
        Some(redemption.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = response_json(response).await;
    assert!(
        token["accountId"]
            .as_str()
            .unwrap()
            .starts_with("agentum-grant-")
    );
    assert_eq!(token["displayName"], "Jira — 2 sites");
    assert_eq!(token["accessToken"], "access-token-one");
    assert_eq!(token["refreshToken"], "refresh-token-one");
    assert_eq!(token["scopes"], json!(REQUIRED_SCOPES));
    assert_eq!(token["sites"][0]["id"], "site-a");
    assert_eq!(token["sites"][1]["id"], "site-z");

    let replay = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/redeem",
        Some(redemption),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json(replay).await, token);
    assert_eq!(test.upstream.code_exchanges.load(Ordering::SeqCst), 1);
    assert_eq!(test.upstream.resource_reads.load(Ordering::SeqCst), 1);

    let database = std::fs::read(&test.database_path).unwrap();
    let database_text = String::from_utf8_lossy(&database);
    assert!(!database_text.contains("access-token-one"));
    assert!(!database_text.contains("refresh-token-one"));
}

#[tokio::test]
async fn rotating_refresh_is_revision_bound_and_idempotent() {
    let test = test_broker(MockMode::Valid).await;
    let device = Device::new();
    let (_connection, _redemption, connection_id) = completed_connection(&test, &device).await;
    let old_hash = sha256_hex(b"refresh-token-one");
    let payload = format!("agentum-jira-refresh-v1\n{connection_id}\n1\n{old_hash}");
    let refresh = json!({
        "connectionId": connection_id,
        "refreshToken": "refresh-token-one",
        "credentialRevision": 1,
        "deviceProof": device.sign(&payload)
    });
    let response = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/refresh",
        Some(refresh.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let token = response_json(response).await;
    assert_eq!(token["accessToken"], "access-token-two");
    assert_eq!(token["refreshToken"], "refresh-token-two");

    let replay = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/refresh",
        Some(refresh.clone()),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json(replay).await, token);
    assert_eq!(test.upstream.refreshes.load(Ordering::SeqCst), 1);

    // Losing the in-memory response (for example on a broker restart) is
    // recoverable during Atlassian's rotating-token reuse interval using only
    // the durable previous token hash and the token resupplied by the client.
    test.broker.inner.refresh_replays.lock().await.clear();
    let recovered = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/refresh",
        Some(refresh),
    )
    .await;
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(response_json(recovered).await, token);
    assert_eq!(test.upstream.refreshes.load(Ordering::SeqCst), 2);

    let wrong_hash = sha256_hex(b"stale-token");
    let wrong_payload = format!("agentum-jira-refresh-v1\n{connection_id}\n1\n{wrong_hash}");
    let stale = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/refresh",
        Some(json!({
            "connectionId": connection_id,
            "refreshToken": "stale-token",
            "credentialRevision": 1,
            "deviceProof": device.sign(&wrong_payload)
        })),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn rejects_scope_expansion_site_spoofing_and_nonrotating_tokens() {
    for (mode, expected_stage) in [
        (MockMode::ExtraTokenScope, "redeem"),
        (MockMode::InvalidSite, "redeem"),
        (MockMode::NoRotatedRefresh, "refresh"),
    ] {
        let test = test_broker(mode).await;
        let device = Device::new();
        if expected_stage == "redeem" {
            let state = URL_SAFE_NO_PAD.encode([4_u8; 32]);
            let flow = start_flow(&test.app, &device, &state).await;
            assert_eq!(authorize(&test.app, &state).await.status(), StatusCode::OK);
            let response = call(
                &test.app,
                Method::POST,
                "/v1/jira/oauth/redeem",
                Some(redeem_body(
                    &device,
                    &flow,
                    &state,
                    "bb4937c7-f5a7-40f9-9135-fb7cd939c225",
                )),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        } else {
            let (_connection, _redemption, connection_id) =
                completed_connection(&test, &device).await;
            let old_hash = sha256_hex(b"refresh-token-one");
            let payload = format!("agentum-jira-refresh-v1\n{connection_id}\n1\n{old_hash}");
            let response = call(
                &test.app,
                Method::POST,
                "/v1/jira/oauth/refresh",
                Some(json!({
                    "connectionId": connection_id,
                    "refreshToken": "refresh-token-one",
                    "credentialRevision": 1,
                    "deviceProof": device.sign(&payload)
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        }
    }
}

#[tokio::test]
async fn rejects_extra_requested_scope_bad_proof_and_expired_flow() {
    let test = test_broker(MockMode::Valid).await;
    let device = Device::new();
    let state = URL_SAFE_NO_PAD.encode([5_u8; 32]);
    let extra = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/start",
        Some(json!({
            "state": state,
            "devicePublicKey": device.public_key(),
            "scopes": ["read:jira-work", "write:jira-work", "offline_access", "read:me"]
        })),
    )
    .await;
    assert_eq!(extra.status(), StatusCode::BAD_REQUEST);

    let state = URL_SAFE_NO_PAD.encode([6_u8; 32]);
    let flow = start_flow(&test.app, &device, &state).await;
    assert_eq!(authorize(&test.app, &state).await.status(), StatusCode::OK);
    let other_device = Device::new();
    let bad = call(
        &test.app,
        Method::POST,
        "/v1/jira/oauth/redeem",
        Some(redeem_body(
            &other_device,
            &flow,
            &state,
            "bb4937c7-f5a7-40f9-9135-fb7cd939c225",
        )),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);

    let state = URL_SAFE_NO_PAD.encode([7_u8; 32]);
    let _flow = start_flow(&test.app, &device, &state).await;
    {
        let mut flows = test.broker.inner.flows.lock().await;
        let redemption_id = flows.by_state.get(&state).unwrap().clone();
        flows
            .by_redemption
            .get_mut(&redemption_id)
            .unwrap()
            .expires_at_unix = now_unix() - 1;
    }
    let expired = authorize(&test.app, &state).await;
    assert_eq!(expired.status(), StatusCode::GONE);
}

#[tokio::test]
async fn health_is_small_and_non_cacheable() {
    let test = test_broker(MockMode::Valid).await;
    let response = call(&test.app, Method::GET, "/healthz", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    let health = response_json(response).await;
    assert_eq!(health["status"], "ok");
    assert_eq!(health["schemaVersion"], 1);
    assert!(health.get("clientSecret").is_none());
}
