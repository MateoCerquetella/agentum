//! End-to-end integration test for Phase 2 (card ↔ session binding).
//!
//! Exercises the full Phase 2 happy-path through the in-process axum router:
//!   claim_card atomic dual-write (store layer) → HTTP GET confirmation
//!   → pane-snapshot route shape → unbind/rebind via PATCH → bus event
//!   → comment bridge → daemon-restart persistence.
//!
//! No tmux fixture: the auto-spawn path is exercised at the STORE level via
//! `Store::claim_card` directly, bypassing the tmux launch in `spawn_card_session`.
//! The PATCH → doing branch that calls tmux is covered separately by the
//! `patch_doing_autospawn_happy_path_via_store` test which probes the store
//! side-effects even when the HTTP response is 500 (tmux absent → Internal
//! error, but binding was atomically committed). Live-tmux observation is
//! deferred to the UAT step at
//! `.planning/phases/02-card-session-binding/02-UAT.md`, matching the
//! Phase 1 precedent from `01-08-SUMMARY.md` §"Manual UAT — Deferred".
//!
//! Thread-safety: `make_state` mutates XDG env vars. All tests in this file
//! serialise through ENV_LOCK (same pattern as `goal_cards_end_to_end.rs`)
//! to prevent races under parallel `cargo test`.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use agentum_core::{Event, NewSession};
use agentum_server::AppState;
use agentum_store::Store;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tower::ServiceExt;
use uuid::Uuid;

// Serialise env mutations — same pattern as `goal_cards_end_to_end.rs` and
// `board_goals::tests` inside the server crate.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TestEnv {
    _config_dir: TempDir,
    _data_dir: TempDir,
    _guard: MutexGuard<'static, ()>,
}

fn isolate_xdg() -> TestEnv {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config_dir = TempDir::new().unwrap();
    let data_dir = TempDir::new().unwrap();
    // SAFETY: ENV_LOCK serialises all tests in this module so only one
    // thread mutates the XDG env vars at a time.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", config_dir.path());
        std::env::set_var("XDG_DATA_HOME", data_dir.path());
    }
    TestEnv {
        _config_dir: config_dir,
        _data_dir: data_dir,
        _guard: guard,
    }
}

/// Build an in-process AppState backed by a fresh SQLite database at `dir/test.sqlite`.
///
/// Mirrors the file-private helper in `goal_cards_end_to_end.rs` exactly.
/// The `no_auth: true` flag bypasses the `require_token` middleware so
/// requests don't need a real bearer token in these tests.
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
        api_base_url: None,
        desktop_bridge: None,
    }
}

fn post_json(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::post(path)
        .header("authorization", "Bearer test-token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn patch_json(path: &str, body: serde_json::Value) -> Request<Body> {
    Request::patch(path)
        .header("authorization", "Bearer test-token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
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

/// Phase 2 full happy-path integration test.
///
/// 14 scenarios cover all six Phase 2 requirements (BIND-01..BIND-06) and
/// the five ROADMAP §Phase 2 success criteria, without requiring a tmux server.
///
/// The auto-spawn STORE contract (BIND-01, D-11) is proven via `claim_card`
/// directly; the PATCH → doing HTTP branch is also exercised (scenario 2b)
/// and the store side-effect is confirmed even when tmux is absent and the
/// HTTP returns 500 (per CONTEXT D-12: binding survives even a spawn failure).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn card_session_binding_full_happy_path() -> anyhow::Result<()> {
    let _env = isolate_xdg();

    // Outer tempdir lives for the entire test so the SQLite file persists
    // across the daemon-restart simulation in scenario 14.
    let dir = TempDir::new().unwrap();

    // Outer-scope variables populated in the inner scope and read in scenario 14.
    let bound_card_id: i64;
    let bound_session_id: Uuid;
    let card_x_id: i64;
    let session_b_id: Uuid;

    {
        // === Scenario 1: Harness bootstrap ===
        let state = make_state(dir.path()).await;

        // Spawn the watchdog → comment bridge. In production `serve()` does
        // this; the test drives the router directly without calling `serve()`.
        {
            let store = state.store.clone();
            let bus = state.bus.clone();
            tokio::spawn(async move {
                agentum_watchdog::run_session_comment_bridge(store, bus).await;
            });
        }

        // Subscribe BEFORE any HTTP call so no events are missed.
        let mut bus_rx = state.bus.subscribe();

        // Yield to the scheduler so the bridge task subscribes to the bus
        // before the first `bus.send` call in scenario 7.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        let app: Router = agentum_server::router(state.clone());

        // === Scenario 2a: Auto-spawn STORE-level dual-write (BIND-01) ===
        //
        // `claim_card` atomically inserts a session row + binds the card
        // in a single SQLite transaction. This is the BIND-01 contract.
        // Folds in the pending todo:
        //   .planning/todos/pending/2026-05-20-board-doing-create-test.md
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": "auto-spawn UAT card",
                    "lbl": "feat",
                    "status": "todo",
                    "workdir": "/tmp",
                    "tool": "claude",
                }),
            ))
            .await?;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let card_body = read_json(resp).await;
        let card_id = card_body["id"].as_i64().expect("card id must be integer");

        // Call claim_card directly (bypass HTTP PATCH to avoid tmux dependency).
        // The HTTP PATCH → doing path is tested in scenario 2b.
        let (bound_card, bound_session) = state
            .store
            .claim_card(
                card_id,
                NewSession {
                    name: format!("card-auto-spawn-{}", Uuid::new_v4()),
                    workdir: "/tmp".to_string(),
                    tool: "claude".to_string(),
                    model: None,
                    flags: vec![],
                    card_id: None, // overwritten unconditionally by claim_card
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_base_ref: None,
                },
            )
            .await
            .expect("claim_card must succeed for scenario 2a");
        assert_eq!(
            bound_card.session_id.as_deref(),
            Some(bound_session.id.to_string().as_str()),
            "claim_card must set card.session_id in the same transaction"
        );
        assert_eq!(
            bound_session.card_id,
            Some(card_id),
            "claim_card must set session.card_id in the same transaction (BIND-01 dual-write)"
        );

        // Reload via HTTP GET to confirm the committed state is readable.
        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/board/{card_id}")))
            .await?;
        assert_eq!(resp.status(), StatusCode::OK);
        let card_row = read_json(resp).await;
        assert_eq!(
            card_row["session_id"].as_str(),
            Some(bound_session.id.to_string().as_str()),
            "card session_id must persist and be readable via GET"
        );

        // Confirm the session tool defaulted to "claude" (D-02).
        assert_eq!(
            bound_session.tool, "claude",
            "auto-spawned session must default to claude (CONTEXT D-02)"
        );

        bound_card_id = card_id;
        bound_session_id = bound_session.id;

        // === Scenario 2b: PATCH → doing triggers auto-spawn path + board.updated event ===
        //
        // Create a SEPARATE card to test the HTTP PATCH path. Without a live tmux
        // server, spawn_card_session's claim_card step commits atomically, but the
        // subsequent tmux launch returns an error → HTTP 500. CONTEXT D-12 specifies
        // the binding persists even on spawn failure ("card stays in doing with
        // session_id set; user navigates to the dead pane").
        //
        // We verify: (1) the gate passes and claim_card's store effects commit;
        // (2) board.updated event fires before the tmux call (the event is emitted
        //     AFTER the re-fetch that shows session_id set); in the success path,
        //     the event includes session_id. Without tmux the HTTP returns 500 before
        //     reaching the event emit, so we check store state via DB instead.
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": "http-patch-spawn card",
                    "lbl": "feat",
                    "status": "todo",
                    "workdir": "/tmp",
                    "tool": "claude",
                }),
            ))
            .await?;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let http_card_body = read_json(resp).await;
        let http_card_id = http_card_body["id"].as_i64().expect("http card id");

        // Claim ownership so the doing-gate's ClaimedBy requirement is satisfied.
        let claim_resp = app
            .clone()
            .oneshot(post_json(
                &format!("/api/board/{http_card_id}/claim"),
                json!({"claimed_by": "test-agent"}),
            ))
            .await?;
        assert_eq!(claim_resp.status(), StatusCode::OK, "claim must succeed");

        // Issue PATCH → doing. Either 200 (tmux available) or 500 (tmux absent
        // but claim_card committed). Both are valid outcomes; the key assertion
        // is the store side-effect (below).
        let patch_resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{http_card_id}"),
                json!({"status": "doing"}),
            ))
            .await?;
        let patch_status = patch_resp.status();
        // Either 200 (tmux available, full success) or 500 (tmux absent, binding
        // committed per D-12). 400/409 would indicate a gate or conflict error
        // (a real failure, not just missing tmux).
        assert!(
            patch_status == StatusCode::OK || patch_status == StatusCode::INTERNAL_SERVER_ERROR,
            "PATCH → doing must be 200 (tmux available) or 500 (tmux absent + binding committed); got {patch_status}"
        );

        // Regardless of HTTP status: claim_card atomically committed the binding
        // before the tmux call. Verify via the store.
        let http_session = state
            .store
            .get_session_by_card_id(http_card_id)
            .await?
            .expect("session must exist in DB after PATCH → doing (D-12: binding persists)");
        assert_eq!(
            http_session.card_id,
            Some(http_card_id),
            "session.card_id must be set after auto-spawn PATCH (BIND-01 dual-write)"
        );

        // Check the board.updated bus event only if PATCH returned 200.
        if patch_status == StatusCode::OK {
            let deadline = tokio::time::sleep(Duration::from_millis(1000));
            tokio::pin!(deadline);
            let saw_board_updated = loop {
                tokio::select! {
                    result = bus_rx.recv() => {
                        match result {
                            Ok(ev) if ev.kind == "board.updated" => {
                                assert!(
                                    ev.payload["session_id"].is_string(),
                                    "board.updated payload must include session_id"
                                );
                                break true;
                            }
                            Ok(_) => continue,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                eprintln!("bus lagged {n} in scenario 2b");
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                panic!("bus closed in scenario 2b");
                            }
                        }
                    }
                    _ = &mut deadline => break false,
                }
            };
            assert!(
                saw_board_updated,
                "board.updated event must fire within 1000ms when tmux is available"
            );
        }

        // === Scenario 3: Auto-spawn missing-workdir → 400 (BIND-01 gate) ===
        //
        // No workdir → the doing gate fires before spawn_card_session is called.
        // The gate returns 400 before claim_card is ever invoked; no session row
        // should exist.
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": "no-workdir card",
                    "lbl": "feat",
                    "status": "todo",
                }),
            ))
            .await?;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let no_wd_card = read_json(resp).await;
        let no_wd_id = no_wd_card["id"].as_i64().expect("card id must be integer");

        // Claim so gate doesn't fail on claimed_by (we want the workdir failure).
        let _ = app
            .clone()
            .oneshot(post_json(
                &format!("/api/board/{no_wd_id}/claim"),
                json!({"claimed_by": "test-agent"}),
            ))
            .await?;

        let resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{no_wd_id}"),
                json!({"status": "doing"}),
            ))
            .await?;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "PATCH with no workdir must return 400"
        );
        let err_body = read_json(resp).await;
        // workdir is always in the missing list; tool may also be missing since
        // we didn't set it. Assert workdir is present; accept additional missing fields.
        assert_eq!(
            err_body["status"].as_str(),
            Some("doing"),
            "400 body must carry status: doing"
        );
        let missing = err_body["missing"]
            .as_array()
            .expect("400 body must have missing array");
        assert!(
            missing.iter().any(|v| v.as_str() == Some("workdir")),
            "missing array must include 'workdir'"
        );

        // Gate fires BEFORE claim_card → no session row should exist.
        let no_session = state.store.get_session_by_card_id(no_wd_id).await?;
        assert!(
            no_session.is_none(),
            "no session row should exist for the no-workdir card (gate fires before store mutation)"
        );

        // Card's status is still todo (gate fired BEFORE patch_board_item).
        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/board/{no_wd_id}")))
            .await?;
        let no_wd_row = read_json(resp).await;
        assert_eq!(
            no_wd_row["status"].as_str(),
            Some("todo"),
            "card status must remain todo after failed gate"
        );
        assert!(
            no_wd_row["session_id"].is_null(),
            "card session_id must remain null after failed gate"
        );

        // === Scenario 4: Pane-snapshot route happy path (BIND-02) ===
        //
        // GET /api/sessions/{sid}/pane?lines=5 against the session from scenario 2a.
        // No live tmux pane → lines is empty (correct for Idle sessions per UI-SPEC §empty state).
        let resp = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/sessions/{bound_session_id}/pane?lines=5"
            )))
            .await?;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET /api/sessions/{bound_session_id}/pane must return 200"
        );
        let pane_body = read_json(resp).await;
        assert!(
            pane_body["lines"].is_array(),
            "pane response must have 'lines' array"
        );
        let captured_at = pane_body["captured_at"]
            .as_str()
            .expect("pane response must have 'captured_at' string");
        // Verify captured_at parses as RFC3339.
        time::OffsetDateTime::parse(captured_at, &time::format_description::well_known::Rfc3339)
            .expect("captured_at must be a valid RFC3339 timestamp");

        // Response keys must be exactly {lines, captured_at}.
        let keys: std::collections::HashSet<&str> = pane_body
            .as_object()
            .expect("pane body must be an object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            keys,
            ["lines", "captured_at"].into_iter().collect(),
            "pane response must have exactly two keys: lines and captured_at"
        );

        // === Scenario 5: Pane-snapshot clamping ===
        //
        // lines=0 and lines=500 both return 200 (server clamps, doesn't 400).
        for lines_param in ["0", "500"] {
            let resp = app
                .clone()
                .oneshot(get_req(&format!(
                    "/api/sessions/{bound_session_id}/pane?lines={lines_param}"
                )))
                .await?;
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "pane route must clamp lines={lines_param}, not reject with 400"
            );
            let clamped_body = read_json(resp).await;
            assert!(
                clamped_body["lines"].is_array(),
                "clamped pane response must still have lines array"
            );
        }

        // === Scenario 6: Pane-snapshot bearer-auth static proof (BIND-02) ===
        //
        // Assert that `/api/sessions/{id}/pane` is NOT in the is_public allow-list
        // by reading the auth.rs source at compile time. Static proof that the route
        // inherits bearer-auth middleware; runtime enforcement is covered by the
        // plan 02-02 unit tests inside the server crate.
        //
        // Hook endpoints (`/api/sessions/{id}/hook`) are intentionally public for
        // the per-session ephemeral-token flow — agent CLIs don't know the user's
        // bearer token. The runtime matcher requires BOTH a `/api/sessions/` prefix
        // AND a `/hook` suffix, so pane/stream/etc. can't slip through. We assert
        // the narrower invariant: no `/pane` literal in the allow-list.
        const AUTH_RS: &str = include_str!("../src/auth.rs");
        let is_public_fn = {
            let start = AUTH_RS
                .find("fn is_public(")
                .expect("auth.rs must contain fn is_public(");
            let end_marker = AUTH_RS[start..]
                .find("\nfn ")
                .map(|off| start + off)
                .unwrap_or(AUTH_RS.len());
            &AUTH_RS[start..end_marker]
        };
        assert!(
            !is_public_fn.contains("/pane"),
            "is_public must NOT grant public access to /api/sessions/{{id}}/pane"
        );
        assert!(
            !is_public_fn.contains("/stream"),
            "is_public must NOT grant public access to /api/sessions/{{id}}/stream"
        );

        // === Scenario 7: Comment bridge — agent.finished inserts [system] comment (BIND-04 + D-06) ===
        let session_name = state
            .store
            .get_session_by_id(bound_session_id)
            .await?
            .expect("bound session must exist")
            .name
            .clone();

        let ev = Event::new("agent.finished").with_session(bound_session_id, &session_name);
        state.bus.send(ev).expect("bus.send must succeed");

        // Poll-loop: bridge may take up to 2 seconds on a loaded CI runner.
        let comment_appeared = {
            let mut appeared = false;
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let comments = state.store.list_board_comments(bound_card_id).await?;
                if !comments.is_empty() {
                    let latest = comments.last().unwrap();
                    assert_eq!(
                        latest.author, "system",
                        "system comment author must be 'system'"
                    );
                    assert_eq!(
                        latest.body, "[system] agent finished",
                        "agent.finished comment body must match D-06 template exactly"
                    );
                    appeared = true;
                    break;
                }
            }
            appeared
        };
        assert!(
            comment_appeared,
            "bridge must insert [system] agent finished comment within 2000ms"
        );
        let comment_count_after_7 = state.store.list_board_comments(bound_card_id).await?.len();

        // === Scenario 8: Comment bridge dedupe (D-07) ===
        //
        // Same session_id + same event kind → bridge's HashMap<Uuid, &str> skips it.
        let ev = Event::new("agent.finished").with_session(bound_session_id, &session_name);
        state.bus.send(ev).expect("bus.send for dedupe test");

        tokio::time::sleep(Duration::from_millis(300)).await;

        let comment_count_after_8 = state.store.list_board_comments(bound_card_id).await?.len();
        assert_eq!(
            comment_count_after_8, comment_count_after_7,
            "bridge must deduplicate back-to-back identical agent.finished events (D-07)"
        );

        // === Scenario 9: Comment bridge — session.crashed with signature + unknown fallback (D-06) ===
        //
        // Part A: crash with explicit signature field.
        let ev = Event::new("session.crashed")
            .with_session(bound_session_id, &session_name)
            .with_payload(json!({"signature": "SIGSEGV"}));
        state.bus.send(ev).expect("bus.send for crash+sig");

        let crash_sig_appeared = {
            let mut appeared = false;
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let comments = state.store.list_board_comments(bound_card_id).await?;
                if comments.len() > comment_count_after_8 {
                    let latest = comments.last().unwrap();
                    assert_eq!(
                        latest.body, "[system] session crashed: SIGSEGV",
                        "crash comment with signature must match D-06 template exactly"
                    );
                    appeared = true;
                    break;
                }
            }
            appeared
        };
        assert!(
            crash_sig_appeared,
            "bridge must insert [system] session crashed: SIGSEGV"
        );
        let _comment_count_after_9a = state.store.list_board_comments(bound_card_id).await?.len();

        // Part B: crash with empty payload → "unknown" substitution per D-06.
        // Use a FRESH card + session for this assertion because the bridge's
        // in-memory dedupe (D-07) skips a second "crashed" event for the same
        // session (last_kind maps session_id → "crashed" from part A). A fresh
        // session has no prior "crashed" entry in last_kind, so the event is
        // processed and the "unknown" fallback fires.
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": "crash-unknown card",
                    "lbl": "feat",
                    "status": "todo",
                    "workdir": "/tmp",
                }),
            ))
            .await?;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let cu_card_body = read_json(resp).await;
        let cu_card_id = cu_card_body["id"].as_i64().expect("crash-unknown card id");

        let (_, cu_session) = state
            .store
            .claim_card(
                cu_card_id,
                NewSession {
                    name: format!("cu-session-{}", Uuid::new_v4()),
                    workdir: "/tmp".to_string(),
                    tool: "claude".to_string(),
                    model: None,
                    flags: vec![],
                    card_id: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_base_ref: None,
                },
            )
            .await
            .expect("claim_card for crash-unknown session must succeed");

        let ev = Event::new("session.crashed")
            .with_session(cu_session.id, &cu_session.name)
            .with_payload(json!({}));
        state.bus.send(ev).expect("bus.send for crash+unknown");

        let crash_unknown_appeared = {
            let mut appeared = false;
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let comments = state.store.list_board_comments(cu_card_id).await?;
                if !comments.is_empty() {
                    let latest = comments.last().unwrap();
                    assert_eq!(
                        latest.body, "[system] session crashed: unknown",
                        "crash comment without signature must use 'unknown' fallback (D-06)"
                    );
                    appeared = true;
                    break;
                }
            }
            appeared
        };
        assert!(
            crash_unknown_appeared,
            "bridge must insert [system] session crashed: unknown for empty-payload crash event"
        );

        // === Scenario 10: Goal-card filter (D-08) ===
        //
        // Events for sessions bound to goal-lbl cards must not produce comments.
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": "test goal card",
                    "lbl": "goal",
                    "status": "todo",
                    "workdir": "/tmp",
                }),
            ))
            .await?;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let goal_card_body = read_json(resp).await;
        let goal_card_id = goal_card_body["id"].as_i64().expect("goal card id");

        // Bind a session directly via claim_card (the planner-spawn path).
        let (_, goal_session) = state
            .store
            .claim_card(
                goal_card_id,
                NewSession {
                    name: format!("goal-session-{}", Uuid::new_v4()),
                    workdir: "/tmp".to_string(),
                    tool: "claude".to_string(),
                    model: None,
                    flags: vec![],
                    card_id: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_base_ref: None,
                },
            )
            .await
            .expect("claim_card for goal-card must succeed");

        // Emit agent.finished for the goal card's session.
        let ev = Event::new("agent.finished").with_session(goal_session.id, &goal_session.name);
        state.bus.send(ev).expect("bus.send for goal-filter test");

        tokio::time::sleep(Duration::from_millis(500)).await;

        let goal_comments = state.store.list_board_comments(goal_card_id).await?;
        assert_eq!(
            goal_comments.len(),
            0,
            "bridge must NOT insert comments on goal-card sessions (D-08 filter)"
        );

        // === Scenario 11: Unbind via PATCH (BIND-06) ===
        //
        // PATCH { session_id: null } clears both card.session_id and session.card_id.
        // [system] comments from scenarios 7 and 9 must survive (history retained).
        let comment_count_before_unbind =
            state.store.list_board_comments(bound_card_id).await?.len();

        let resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{bound_card_id}"),
                json!({"session_id": null}),
            ))
            .await?;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "PATCH session_id=null must return 200"
        );

        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/board/{bound_card_id}")))
            .await?;
        let unbound_card = read_json(resp).await;
        assert!(
            unbound_card["session_id"].is_null(),
            "card session_id must be null after unbind"
        );

        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/sessions/{bound_session_id}")))
            .await?;
        let unbound_session = read_json(resp).await;
        assert!(
            unbound_session["card_id"].is_null(),
            "session card_id must be null after unbind"
        );

        // History preserved (UI-SPEC §Unbind: "render-only, never edit/delete").
        let comments_after_unbind = state.store.list_board_comments(bound_card_id).await?;
        assert_eq!(
            comments_after_unbind.len(),
            comment_count_before_unbind,
            "unbind must NOT delete [system] comment history"
        );

        // === Scenario 12: Rebind via PATCH (BIND-06 + D-11 atomic 3-row transfer) ===
        //
        // Fresh card X + sessions A and B. Bind X to A, then rebind X to B via PATCH.
        // All three rows (card X, session A, session B) must reflect the transfer atomically.
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": "rebind card X",
                    "lbl": "feat",
                    "status": "todo",
                    "workdir": "/tmp",
                }),
            ))
            .await?;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let card_x_body = read_json(resp).await;
        let cx_id = card_x_body["id"].as_i64().expect("card X id");

        let session_a = state
            .store
            .create_session(NewSession {
                name: format!("session-a-{}", Uuid::new_v4()),
                workdir: "/tmp".to_string(),
                tool: "claude".to_string(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await?;
        let session_a_id = session_a.id;

        let session_b = state
            .store
            .create_session(NewSession {
                name: format!("session-b-{}", Uuid::new_v4()),
                workdir: "/tmp".to_string(),
                tool: "claude".to_string(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await?;
        let sb_id = session_b.id;

        // Bind X → A via transfer_card_binding.
        state
            .store
            .transfer_card_binding(cx_id, Some(session_a_id))
            .await
            .expect("binding X to A must succeed");

        // Rebind X → B via PATCH.
        let resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{cx_id}"),
                json!({"session_id": sb_id.to_string()}),
            ))
            .await?;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "rebind PATCH must return 200"
        );

        // Card X: bound to B.
        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/board/{cx_id}")))
            .await?;
        let rx_body = read_json(resp).await;
        assert_eq!(
            rx_body["session_id"].as_str(),
            Some(sb_id.to_string().as_str()),
            "card X must be bound to session B after rebind"
        );

        // Session A: card_id cleared.
        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/sessions/{session_a_id}")))
            .await?;
        let ra_body = read_json(resp).await;
        assert!(
            ra_body["card_id"].is_null(),
            "session A card_id must be null after rebind to B"
        );

        // Session B: card_id = X.
        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/sessions/{sb_id}")))
            .await?;
        let rb_body = read_json(resp).await;
        assert_eq!(
            rb_body["card_id"].as_i64(),
            Some(cx_id),
            "session B card_id must be X after rebind"
        );

        // Save for daemon-restart scenario.
        card_x_id = cx_id;
        session_b_id = sb_id;

        // === Scenario 13: Rebind conflict → 409 + rollback (D-11) ===
        //
        // Card Y bound to session C. PATCH card X to bind to C → 409.
        // All four rows (X, Y, A, B, C) must be unchanged after the conflict.
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": "conflict card Y",
                    "lbl": "feat",
                    "status": "todo",
                    "workdir": "/tmp",
                }),
            ))
            .await?;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let card_y_body = read_json(resp).await;
        let card_y_id = card_y_body["id"].as_i64().expect("card Y id");

        let session_c = state
            .store
            .create_session(NewSession {
                name: format!("session-c-{}", Uuid::new_v4()),
                workdir: "/tmp".to_string(),
                tool: "claude".to_string(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await?;
        let session_c_id = session_c.id;

        state
            .store
            .transfer_card_binding(card_y_id, Some(session_c_id))
            .await
            .expect("binding Y to C must succeed");

        // Try to rebind card X to session C (already bound to Y) → 409.
        let resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{cx_id}"),
                json!({"session_id": session_c_id.to_string()}),
            ))
            .await?;
        assert_eq!(
            resp.status(),
            StatusCode::CONFLICT,
            "rebind conflict must return 409"
        );

        // All rows must be unchanged after the conflict rollback.
        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/board/{cx_id}")))
            .await?;
        let rx_after = read_json(resp).await;
        assert_eq!(
            rx_after["session_id"].as_str(),
            Some(sb_id.to_string().as_str()),
            "card X must still be bound to B after conflict"
        );

        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/board/{card_y_id}")))
            .await?;
        let ry_after = read_json(resp).await;
        assert_eq!(
            ry_after["session_id"].as_str(),
            Some(session_c_id.to_string().as_str()),
            "card Y must still be bound to C after conflict"
        );

        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/sessions/{sb_id}")))
            .await?;
        let rb_after = read_json(resp).await;
        assert_eq!(
            rb_after["card_id"].as_i64(),
            Some(cx_id),
            "session B card_id must still be X after conflict"
        );

        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/sessions/{session_c_id}")))
            .await?;
        let rc_after = read_json(resp).await;
        assert_eq!(
            rc_after["card_id"].as_i64(),
            Some(card_y_id),
            "session C card_id must still be Y after conflict"
        );

        let _ = (session_a_id, session_c_id, card_y_id);
    } // end inner scope — "daemon restart" simulation

    // === Scenario 14: Daemon restart preserves binding (ROADMAP SC #5) ===
    //
    // Reopen the Store from the SAME tempfile DB path with a fresh AppState.
    // The card X + session B binding from scenario 12 must survive.
    let state2 = make_state(dir.path()).await;
    let app2: Router = agentum_server::router(state2.clone());

    let resp = app2
        .clone()
        .oneshot(get_req(&format!("/api/board/{card_x_id}")))
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let cx_after_restart = read_json(resp).await;
    assert_eq!(
        cx_after_restart["session_id"].as_str(),
        Some(session_b_id.to_string().as_str()),
        "card X binding must survive daemon restart (ROADMAP SC #5)"
    );

    let resp = app2
        .clone()
        .oneshot(get_req(&format!("/api/sessions/{session_b_id}")))
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let sb_after_restart = read_json(resp).await;
    assert_eq!(
        sb_after_restart["card_id"].as_i64(),
        Some(card_x_id),
        "session B card_id must survive daemon restart (ROADMAP SC #5)"
    );

    // === Scenario 15: Pre-existing card backwards-compat ===
    //
    // Create a card via POST with no session_id field (the shape of every card
    // that existed before Phase 2's auto-spawn path). Verify it round-trips
    // cleanly through the Phase 2 PATCH handler (title-only update, no status change).
    let resp = app2
        .clone()
        .oneshot(post_json(
            "/api/board",
            json!({
                "title": "pre-existing card",
                "lbl": "feat",
                "status": "todo",
                // No workdir, no session_id — old-shape card.
            }),
        ))
        .await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let old_card_body = read_json(resp).await;
    let row_id = old_card_body["id"].as_i64().expect("pre-existing card id");

    // GET: session_id must be null.
    let resp = app2
        .clone()
        .oneshot(get_req(&format!("/api/board/{row_id}")))
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let old_card = read_json(resp).await;
    assert!(
        old_card["session_id"].is_null(),
        "pre-existing card must have null session_id"
    );

    // PATCH title only (no status change): must round-trip cleanly through
    // the Phase 2 PATCH handler without triggering any auto-spawn branch.
    let resp = app2
        .clone()
        .oneshot(patch_json(
            &format!("/api/board/{row_id}"),
            json!({"title": "renamed pre-existing"}),
        ))
        .await?;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "PATCH title on pre-existing card must return 200"
    );
    let renamed = read_json(resp).await;
    assert_eq!(
        renamed["title"].as_str(),
        Some("renamed pre-existing"),
        "pre-existing card must round-trip through the Phase 2 PATCH handler"
    );
    assert!(
        renamed["session_id"].is_null(),
        "title-only PATCH must NOT set session_id on a pre-existing card"
    );

    Ok(())
}
