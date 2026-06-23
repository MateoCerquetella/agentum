//! Acceptance-criteria integration tests for spec 016a (server-side board ←
//! GitHub pull + durable tracker bindings), exercised through the **full
//! in-process axum router** (`agentum_server::router`) so the route merge and
//! the #58 contract are validated together.
//!
//! Two ACs are covered here:
//!   1. **#58 regression** — `POST /api/board/sync {items:[…]}` (the shipped
//!      client-supplied mirror) still succeeds unchanged after 016a merges its
//!      own `/api/board/bindings*` routes.
//!   2. **Fails-loud → zero mutation** — a server pull
//!      (`POST /api/board/bindings/{id}/sync`) against a no-token GitHub returns
//!      a non-success status AND leaves the board card count + contents
//!      unchanged.
//!
//! Both run offline (no network, no `gh`): the fails-loud case forces the
//! no-token path by isolating the forge-token store under an empty
//! `AGENTUM_HOME`. Env mutations are serialised behind `ENV_LOCK`.

use std::sync::{Mutex, MutexGuard};
use std::sync::Arc;
use std::time::Duration;

use agentum_core::Event;
use agentum_server::AppState;
use agentum_store::Store;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tower::ServiceExt;

// Serialise env mutations — same pattern used across the server crate's tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Build an in-process AppState backed by a fresh tempdir SQLite database.
/// Mirrors `routes::board::tests::fresh_state()`. `no_auth: true` bypasses the
/// `require_token` middleware so requests don't need a real bearer token.
async fn make_state(dir: &std::path::Path) -> AppState {
    let db_path = dir.join("test.sqlite");
    let store = Store::open(&db_path).await.unwrap();
    let (bus, _rx) = broadcast::channel(1024);
    AppState {
        store: Arc::new(store),
        bus,
        started_at: std::time::Instant::now(),
        version: "test",
        auth_limiter: Arc::new(agentum_server::ratelimit::RateLimiter::new(
            8,
            Duration::from_secs(60),
        )),
        cert_fingerprint: Arc::new(String::new()),
        transcripts: agentum_server::TranscriptStore::new(broadcast::channel(16).0),
        stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        hostname: "test".to_string(),
        no_auth: true,
        clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        clipboard_request_bus: broadcast::channel(64).0,
        hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        mcp_token: Arc::new(String::from("test-mcp-token")),
        api_base_url: None,
        desktop_bridge: None,
        harness: Arc::new(agentum_server::harness::HarnessEngine::new()),
    }
}

fn post_json(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(path)
        .header("authorization", "Bearer test-token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn post_empty(path: &str) -> Request<Body> {
    Request::post(path)
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap()
}

fn get_req(path: &str) -> Request<Body> {
    Request::get(path)
        .header("authorization", "Bearer test-token")
        .body(Body::empty())
        .unwrap()
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        let raw = String::from_utf8_lossy(&bytes);
        panic!("failed to parse response as JSON: {raw}");
    })
}

/// Count board cards via `GET /api/board` (grouped columns → flat count).
async fn board_card_count(app: &Router, _bus: &broadcast::Sender<Event>) -> usize {
    let resp = app.clone().oneshot(get_req("/api/board")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET /api/board must be 200");
    let body = read_json(resp).await;
    body.get("columns")
        .and_then(|c| c.as_object())
        .map(|cols| {
            cols.values()
                .map(|v| v.as_array().map(|a| a.len()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// **AC: the #58 client-push path still succeeds unchanged.**
/// `POST /api/board/sync {items:[…]}` through the full router (with 016a's
/// `board_sync::router()` merged) creates the card and re-syncing is idempotent.
#[tokio::test]
async fn post_board_sync_items_still_works_after_016a_merge() {
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path()).await;
    let bus = state.bus.clone();
    let app = agentum_server::router(state);

    let item = json!({
        "items": [{
            "external_url": "https://github.com/o/r/issues/12",
            "external_provider": "github",
            "title": "Fix the thing",
            "body": "repro steps",
            "status": "todo",
            "lbl": "github"
        }]
    });

    // First sync creates exactly one card.
    let resp = app
        .clone()
        .oneshot(post_json("/api/board/sync", item.clone()))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "#58 POST /api/board/sync must still return 200"
    );
    let body = read_json(resp).await;
    let synced = body.get("synced").and_then(|s| s.as_array()).unwrap();
    assert_eq!(synced.len(), 1, "one issue → one synced card");
    assert_eq!(
        synced[0].get("external_url").and_then(|u| u.as_str()),
        Some("https://github.com/o/r/issues/12")
    );
    let first_id = synced[0].get("id").and_then(|i| i.as_i64()).unwrap();

    // Re-sync of the same issue updates in place — board still has one card.
    let resp2 = app
        .clone()
        .oneshot(post_json("/api/board/sync", item))
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = read_json(resp2).await;
    let second_id = body2.get("synced").and_then(|s| s.as_array()).unwrap()[0]
        .get("id")
        .and_then(|i| i.as_i64())
        .unwrap();
    assert_eq!(second_id, first_id, "re-sync hits the same card");

    assert_eq!(
        board_card_count(&app, &bus).await,
        1,
        "the #58 mirror must not duplicate cards"
    );
}

/// **AC: fails-loud sync makes zero board changes.**
/// A server pull against a no-token GitHub returns a non-success status, and
/// the board's card count + contents are unchanged (the pre-existing card and
/// no new external card). Deterministic & offline via an empty `AGENTUM_HOME`.
#[tokio::test]
async fn server_sync_with_no_token_fails_loud_and_writes_nothing() {
    let _guard: MutexGuard<'_, ()> = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("AGENTUM_HOME");
    let empty_home = TempDir::new().unwrap();
    // SAFETY: env access is serialised by ENV_LOCK for the env-touching tests.
    unsafe {
        std::env::set_var("AGENTUM_HOME", empty_home.path());
    }

    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path()).await;
    let bus = state.bus.clone();
    let app = agentum_server::router(state);

    // Seed one pre-existing card via the #58 path so we can prove non-mutation.
    let seed = post_json(
        "/api/board/sync",
        json!({
            "items": [{
                "external_url": "https://github.com/o/r/issues/99",
                "external_provider": "github",
                "title": "pre-existing card",
                "body": "untouched",
                "status": "todo",
                "lbl": "github"
            }]
        }),
    );
    let seed_resp = app.clone().oneshot(seed).await.unwrap();
    assert_eq!(seed_resp.status(), StatusCode::OK);
    assert_eq!(board_card_count(&app, &bus).await, 1, "seeded one card");

    // Create a github binding.
    let bind_resp = app
        .clone()
        .oneshot(post_json(
            "/api/board/bindings",
            json!({ "provider": "github", "project": "o/r" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        bind_resp.status(),
        StatusCode::CREATED,
        "github binding must be created"
    );
    let binding = read_json(bind_resp).await;
    let binding_id = binding.get("id").and_then(|i| i.as_i64()).unwrap();

    // Trigger the server pull — the empty token store makes token_for(Github)
    // fail BEFORE any forge call or store write.
    let sync_resp = app
        .clone()
        .oneshot(post_empty(&format!(
            "/api/board/bindings/{binding_id}/sync"
        )))
        .await
        .unwrap();
    assert!(
        !sync_resp.status().is_success(),
        "sync with no token must return a non-success status, got {}",
        sync_resp.status()
    );

    // Zero board mutation: the count is still 1 and the seeded card is intact.
    assert_eq!(
        board_card_count(&app, &bus).await,
        1,
        "a failed sync must not change the board card count"
    );
    let board = read_json(app.clone().oneshot(get_req("/api/board")).await.unwrap()).await;
    let todo = board
        .get("columns")
        .and_then(|c| c.get("todo"))
        .and_then(|v| v.as_array())
        .expect("a todo column with the seeded card");
    assert_eq!(todo.len(), 1);
    assert_eq!(
        todo[0].get("title").and_then(|t| t.as_str()),
        Some("pre-existing card"),
        "the pre-existing card's contents are untouched after a failed sync"
    );

    // Restore the prior env so other tests are unaffected.
    // SAFETY: still under ENV_LOCK.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("AGENTUM_HOME", v),
            None => std::env::remove_var("AGENTUM_HOME"),
        }
    }
}
