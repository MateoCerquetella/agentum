//! End-to-end integration test for Phase 1 (goal-cards-planner-slice).
//!
//! Exercises the full happy path through the in-process axum router:
//!   goal submit → simulated children → dependency link → watchdog
//!   goal-status reconciler → bus events fan out.
//!
//! No tmux fixture: the planner-spawn branch fails because there is no
//! tmux server in CI. The route handler emits `goal.planner.spawn_failed`
//! and retains the goal row (CONTEXT D-07). The test asserts this
//! error-resilience branch works as designed.
//!
//! After the spawn failure the session row exists in the DB at `Idle`
//! status. The test manually promotes it to `Running` so that when the
//! first child card arrives the reconciler can fire
//! `goal.planner.first_child` and call `graceful_stop` (which harmlessly
//! fails against the non-existent tmux pane). This is the correct way to
//! exercise the reconciler's planner-auto-stop path without a real tmux
//! server.
//!
//! Thread-safety: `load_planner_config` reads `XDG_CONFIG_HOME`. All tests
//! in this file serialise through ENV_LOCK (same pattern as
//! routes::board_goals::tests) to prevent races under parallel `cargo test`.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use agentum_core::{Event, NewSession, Status};
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

// Serialise env mutations — same pattern used by board_goals::tests and
// board_rules::tests inside the server crate.
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

/// Build an in-process AppState backed by a fresh tempdir SQLite database.
///
/// Mirrors `routes::board::tests::fresh_state()` exactly, lifted here so
/// integration tests outside the crate can use the same harness. The
/// `no_auth: true` flag bypasses the `require_token` middleware so requests
/// don't need a real bearer token.
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
    }
}

/// Build a JSON POST request with the `Authorization: Bearer` header set to
/// an empty token (bypassed by `no_auth: true`).
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

/// Drain the response body into a `serde_json::Value`.
async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        // Print raw body for debugging on parse failure.
        let raw = String::from_utf8_lossy(&bytes);
        panic!("failed to parse response as JSON: {raw}");
    })
}

/// Receive events from `rx` until one with `kind == target_kind` arrives
/// or the timeout elapses. Returns the matching `Event`.
///
/// Non-matching events are silently discarded so that unrelated events
/// (e.g. `board.transition.rejected` on an intermediate gate check) don't
/// block the loop. If no matching event arrives within `ms` milliseconds
/// the function panics with a descriptive message.
async fn expect_event(rx: &mut broadcast::Receiver<Event>, target_kind: &str, ms: u64) -> Event {
    let deadline = tokio::time::sleep(Duration::from_millis(ms));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(ev) if ev.kind == target_kind => return ev,
                    Ok(ev) => {
                        eprintln!("[bus] saw {}: {:?}", ev.kind, ev.payload);
                        continue;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // The reconciler or other tasks sent many events; we
                        // may have missed some. Log and continue — the
                        // missing event will be emitted again on the next
                        // reconcile tick (T-04-02 convergence guarantee).
                        eprintln!("bus lagged by {n}; some intermediate events dropped");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        panic!("bus closed while waiting for event '{target_kind}'");
                    }
                }
            }
            _ = &mut deadline => {
                panic!("timed out after {ms}ms waiting for event '{target_kind}'");
            }
        }
    }
}

/// Full Phase 1 happy-path integration test.
///
/// Sequence:
/// 1. POST /api/board/goals — creates the goal card + attempts planner spawn.
/// 2. Bus: `goal.created` → `goal.planner.spawn_failed` (no tmux in CI).
/// 3. Manually promote the orphaned planner session to Running so the
///    reconciler can fire `goal.planner.first_child` when the first child
///    arrives (the D-07 auto-stop branch).
/// 4. POST /api/board ×3 — simulate planner output (3 child cards).
/// 5. POST /api/board/links — add a `blocks` edge (b blocks a).
/// 6. Bus: `board.created` ×3, `goal.planner.first_child`, `board.link.created`.
/// 7. PATCH child_a → doing: bus emits `goal.status.changed {todo→doing}`.
/// 8. PATCH all children → done (step-by-step): bus emits
///    `goal.status.changed {doing→done}` when child_a (the first to reach
///    done) tips the max rank above doing.
/// 9. Reverse: PATCH child_b + child_c → doing, then child_a → doing:
///    bus emits `goal.status.changed {done→doing}` when the last done
///    child drops below done.
/// 10. DELETE all children: goal drops back to todo (max-of-empty per D-03).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goal_cards_full_happy_path() {
    let _env = isolate_xdg();
    let dir = TempDir::new().unwrap();
    let state = make_state(dir.path()).await;

    // Spawn the goal-status reconciler manually — `serve()` owns this
    // wiring in production; the test drives the router directly without
    // calling `serve()`.
    {
        let store = state.store.clone();
        let bus = state.bus.clone();
        tokio::spawn(async move {
            agentum_watchdog::run_goal_reconciler(store, bus).await;
        });
    }

    // Subscribe BEFORE any HTTP call so no events are missed (the bus is a
    // broadcast channel; events sent before `subscribe()` are lost to us).
    let mut bus_rx = state.bus.subscribe();

    // Yield twice to the tokio scheduler so the reconciler task actually
    // starts and calls `bus.subscribe()` before we emit any events. In a
    // 2-worker-thread runtime the spawned task runs on the other thread,
    // but `yield_now` + a brief sleep ensures scheduling has happened.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Build the in-process app. `router(state)` already calls
    // `.with_state(state)` internally — no second `.with_state` needed.
    let app: Router = agentum_server::router(state.clone());

    // ── Step 1: POST /api/board/goals ────────────────────────────────────
    //
    // The planner spawn will fail because there is no tmux server in CI.
    // Per D-07 (CONTEXT), the goal card is retained and a 201 is returned
    // with `planner_session_id: ""`.  The route emits `goal.planner.spawn_failed`.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/board/goals",
            json!({
                "title": "deliver feature",
                // workdir must point to a path that exists on the test
                // machine so the spawn path reaches the tmux call before
                // bailing on a missing directory. "/tmp" is always present.
                "workdir": "/tmp"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "POST /api/board/goals must return 201"
    );
    let goal_body = read_json(resp).await;
    let goal_id = goal_body["goal"]["id"]
        .as_i64()
        .expect("goal.id must be an integer");
    let goal_key = goal_body["goal"]["key"]
        .as_str()
        .expect("goal.key must be a string")
        .to_string();
    assert_eq!(goal_body["goal"]["lbl"], "goal", "goal lbl must be 'goal'");
    assert_eq!(
        goal_body["goal"]["status"], "todo",
        "goal must start in todo"
    );

    // ── Step 2: observe goal.created + planner outcome ────────────────────
    //
    // Two paths:
    //  a) tmux NOT available (CI): `goal.planner.spawn_failed` fires.
    //     The goal card is retained per D-07.
    //  b) tmux available (local dev): `goal.planner.spawned` fires.
    //     The planner pane is running and will be auto-stopped on first child.
    //
    // The test handles both by consuming events until we see one of the two
    // outcomes. Either way the goal.created event fires first.
    let goal_created = expect_event(&mut bus_rx, "goal.created", 2000).await;
    assert_eq!(
        goal_created.payload["id"], goal_id,
        "goal.created must carry the new goal id"
    );

    // Drain until we see either the spawn success or failure event.
    let spawn_succeeded = {
        let deadline = tokio::time::sleep(Duration::from_millis(2000));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                result = bus_rx.recv() => {
                    match result {
                        Ok(ev) => match ev.kind.as_str() {
                            "goal.planner.spawned" => break true,
                            "goal.planner.spawn_failed" => {
                                assert_eq!(
                                    ev.payload["goal_id"], goal_id,
                                    "spawn_failed must reference the goal"
                                );
                                break false;
                            }
                            _ => continue,
                        },
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("lagged {n} waiting for planner outcome");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            panic!("bus closed waiting for planner outcome");
                        }
                    }
                }
                _ = &mut deadline => {
                    panic!("timed out waiting for goal.planner.spawned or goal.planner.spawn_failed");
                }
            }
        }
    };

    // ── Step 3: ensure planner session is in Running state ───────────────
    //
    // The reconciler only fires `goal.planner.first_child` when the
    // planner session is in Running status. We need to:
    //  - If spawn failed (no tmux): session is at Idle; promote to Running.
    //    The reconciler will call graceful_stop on a non-existent pane, which
    //    returns immediately (`has_session` → false → Ok immediately).
    //  - If spawn succeeded (tmux available): the real pane is running.
    //    Kill the tmux pane directly and then mark the session as Running in
    //    the DB. The reconciler will call graceful_stop on the now-dead pane,
    //    which also returns immediately (`has_session` → false → Ok).
    //    This prevents the 5-second SIGTERM wait from blocking the reconciler
    //    task and causing timeouts in subsequent steps.
    let planner_session = state
        .store
        .get_session_by_card_id(goal_id)
        .await
        .expect("DB lookup must succeed")
        .expect("planner session row must exist after spawn attempt");

    if spawn_succeeded {
        // Kill the real tmux pane so graceful_stop returns immediately later.
        let target = agentum_tmux::target_for(&planner_session.name);
        let _ = agentum_tmux::kill_session(&target).await;
    }

    // Ensure the DB row shows Running so the reconciler emits first_child.
    state
        .store
        .update_status_and_target(planner_session.id, Status::Running, None)
        .await
        .expect("setting planner session to Running must succeed");

    // ── Step 4: simulate planner output — POST 3 child cards ─────────────
    //
    // Bodies follow the `key: <key>\n\n<rest>` convention (plan 01-05) so
    // that the symbolic-key resolution in POST /api/board/links works.
    //
    // The `doing` column gate requires Title + Lbl + Workdir + Tool +
    // ClaimedBy (board_schema.rs). We set workdir + tool here and use
    // /claim to set claimed_by before each status transition.
    let mut child_ids: Vec<i64> = Vec::with_capacity(3);
    for key in ["a", "b", "c"] {
        // Real session row per child — `create_board_item` validates that
        // session_id references an existing sessions row (the dual-write
        // landed in v0.8.3 / e081d89). A dummy UUID is rejected with 404.
        let child_session = state
            .store
            .create_session(NewSession {
                name: format!("child-{key}-{}", uuid::Uuid::new_v4()),
                workdir: "/tmp".to_string(),
                tool: "bash".to_string(),
                model: None,
                flags: vec![],
                card_id: None,
            })
            .await
            .expect("create child session must succeed");

        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/board",
                json!({
                    "title": format!("Step {key}"),
                    "body": format!("key: {key}\n\nDo the {key} part."),
                    "lbl": "feat",
                    "status": "todo",
                    "workdir": "/tmp",
                    "tool": "bash",
                    // Real session_id satisfies the `done` gate
                    // (SessionOrComment OR-clause) AND the store's dual-write
                    // existence check.
                    "session_id": child_session.id.to_string(),
                    "parent_goal_id": goal_id,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "child card POST must return 201"
        );
        let b = read_json(resp).await;
        let child_id = b["id"].as_i64().expect("child id must be integer");

        // Claim the card so the doing-gate's ClaimedBy requirement is met.
        let claim_resp = app
            .clone()
            .oneshot(post_json(
                &format!("/api/board/{child_id}/claim"),
                json!({"claimed_by": "test-agent"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            claim_resp.status(),
            StatusCode::OK,
            "claim must succeed for child {key}"
        );

        child_ids.push(child_id);
    }

    // ── Step 5: add a blocks link  b → a (b blocks a) ────────────────────
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/board/links",
            json!({
                "parent_goal_id": goal_id,
                "from_key": "b",
                "to_key": "a",
                "kind": "blocks",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "POST /api/board/links must return 201"
    );

    // ── Step 6: observe bus events for children + first-child ────────────
    //
    // Drain the bus in any order until all expected events have arrived.
    // The reconciler runs on its own task; event ordering relative to the
    // HTTP responses is non-deterministic at fine granularity but all
    // events must appear within 2 s.
    let mut saw_board_created: u8 = 0;
    let mut saw_first_child = false;
    let mut saw_link_created = false;

    // We need 3 × board.created + 1 × goal.planner.first_child + 1 × board.link.created = 5 events.
    let deadline = tokio::time::sleep(Duration::from_millis(2000));
    tokio::pin!(deadline);
    loop {
        if saw_board_created >= 3 && saw_first_child && saw_link_created {
            break;
        }
        tokio::select! {
            result = bus_rx.recv() => {
                match result {
                    Ok(ev) => match ev.kind.as_str() {
                        "board.created" => {
                            if ev.payload["parent_goal_id"].as_i64() == Some(goal_id) {
                                saw_board_created += 1;
                            }
                        }
                        "goal.planner.first_child" => {
                            assert_eq!(
                                ev.payload["goal_id"], goal_id,
                                "first_child event must reference the correct goal"
                            );
                            saw_first_child = true;
                        }
                        "board.link.created" => {
                            saw_link_created = true;
                        }
                        _ => {}
                    },
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("bus lagged {n} in step 6; some events may be missed");
                    }
                    Err(broadcast::error::RecvError::Closed) => panic!("bus closed in step 6"),
                }
            }
            _ = &mut deadline => {
                panic!(
                    "timed out in step 6; saw_board_created={saw_board_created} \
                     saw_first_child={saw_first_child} saw_link_created={saw_link_created}"
                );
            }
        }
    }

    // ── Step 7: PATCH child_a → doing; goal flips to doing ───────────────
    //
    // Subscribe a fresh receiver AFTER the step-6 drain loop so that we
    // have a clean view of everything emitted from this point forward. The
    // step-6 loop uses `bus_rx` and may have consumed events that arrived
    // concurrently; `bus_rx2` is unaffected because it starts here.
    let mut bus_rx2 = state.bus.subscribe();

    let resp = app
        .clone()
        .oneshot(patch_json(
            &format!("/api/board/{}", child_ids[0]),
            json!({"status": "doing"}),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "PATCH child_a → doing must return 200"
    );

    let ev = expect_event(&mut bus_rx2, "goal.status.changed", 3000).await;
    assert_eq!(
        ev.payload["goal_id"], goal_id,
        "goal_id must match in todo→doing event"
    );
    assert_eq!(
        ev.payload["from"], "todo",
        "status must transition from todo"
    );
    assert_eq!(ev.payload["to"], "doing", "status must transition to doing");

    // Verify via GET that the goal row reflects the new status.
    let resp = app
        .clone()
        .oneshot(get_req(&format!("/api/board/{goal_id}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let goal_row = read_json(resp).await;
    assert_eq!(
        goal_row["status"], "doing",
        "goal status must be doing after first child moves to doing"
    );

    // ── Step 8: escalate goal to done ────────────────────────────────────
    //
    // Move child_b and child_c to doing first. max(doing, doing, doing) = doing
    // → goal is already doing, no event.
    for &child_id in &child_ids[1..] {
        let resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{child_id}"),
                json!({"status": "doing"}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Move child_a → done. max(done, doing, doing) = done > doing → goal
    // flips to done. The reconciler emits the event as soon as this PATCH is
    // processed; child_b and child_c are still at doing.
    let resp = app
        .clone()
        .oneshot(patch_json(
            &format!("/api/board/{}", child_ids[0]),
            json!({"status": "done"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ev = expect_event(&mut bus_rx2, "goal.status.changed", 2000).await;
    assert_eq!(ev.payload["goal_id"], goal_id, "goal_id must match");
    assert_eq!(ev.payload["from"], "doing", "must transition from doing");
    assert_eq!(ev.payload["to"], "done", "must transition to done");

    // Move child_b and child_c → done as well (max stays done, no extra event).
    for &child_id in &child_ids[1..] {
        let resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{child_id}"),
                json!({"status": "done"}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Step 9: reverse — move child_a back to doing; goal drops to doing ─
    //
    // With child_b and child_c still at done, moving child_a → doing makes
    // max(doing, done, done) = done → goal stays done.  We need ALL children
    // below done to see the reversal. Move child_a back to doing first, then
    // child_b and child_c as well so max = doing.
    //
    // Only the final PATCH (child_c → doing) drives the status change because
    // until that point max(doing, done, doing) or max(doing, doing, done) = done.
    //
    // Simpler: move child_b and child_c to doing first (goal stays done because
    // child_a is still done), then child_a → doing makes max(doing, doing, doing)
    // = doing, and goal flips from done → doing.
    for &child_id in &child_ids[1..] {
        let resp = app
            .clone()
            .oneshot(patch_json(
                &format!("/api/board/{child_id}"),
                json!({"status": "doing"}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    // child_b and child_c are now doing; child_a is still done → max = done.
    // Move child_a → doing: max(doing, doing, doing) = doing → goal flips.
    let resp = app
        .clone()
        .oneshot(patch_json(
            &format!("/api/board/{}", child_ids[0]),
            json!({"status": "doing"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ev = expect_event(&mut bus_rx2, "goal.status.changed", 2000).await;
    assert_eq!(ev.payload["goal_id"], goal_id);
    assert_eq!(ev.payload["from"], "done");
    assert_eq!(ev.payload["to"], "doing");

    // ── Step 10: DELETE all children; goal drops to todo ──────────────────
    //
    // The routes/board::delete handler emits `board.deleted` with
    // `parent_goal_id` in the payload (added in plan 01-03). The reconciler
    // recomputes max(child ranks) = max-of-empty = todo (D-03).
    for &child_id in &child_ids {
        let resp = app
            .clone()
            .oneshot(
                Request::delete(format!("/api/board/{child_id}"))
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // The reconciler may emit goal.status.changed once or twice here
    // (doing → todo when the first child is deleted). Wait for the status
    // to reach "todo" via GET rather than asserting exact event count —
    // the ordering of two rapid deletes can race.
    let deadline = tokio::time::sleep(Duration::from_millis(2000));
    tokio::pin!(deadline);
    let final_status = loop {
        let resp = app
            .clone()
            .oneshot(get_req(&format!("/api/board/{goal_id}")))
            .await
            .unwrap();
        let row = read_json(resp).await;
        let s = row["status"].as_str().unwrap_or("").to_string();
        if s == "todo" {
            break s;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            _ = &mut deadline => {
                break s; // let the assert below fail with a clear message
            }
        }
    };
    assert_eq!(
        final_status, "todo",
        "goal must return to todo after all children are deleted (D-03 max-of-empty)"
    );

    // Confirm the goal key matches what was returned at creation time —
    // verifies the `key` field is stable (e.g. "AG-1" or the configured prefix).
    let resp = app
        .clone()
        .oneshot(get_req(&format!("/api/board/{goal_id}")))
        .await
        .unwrap();
    let final_goal = read_json(resp).await;
    assert_eq!(
        final_goal["key"].as_str().unwrap_or(""),
        &goal_key,
        "goal key must not change across the test"
    );
}
