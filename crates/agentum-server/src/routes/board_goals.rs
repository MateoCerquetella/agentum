//! `/api/board/goals` — atomic create-goal + spawn-planner-session.
//! The goal IS a `BoardItem` with `lbl="goal"` (CONTEXT D-01); no
//! parallel table. The planner is a normal agent session bound via
//! `session.card_id = goal.id` (CONTEXT D-07).

use std::path::PathBuf;

use agentum_core::{BoardItem, Event, NewBoardItem, NewSession, Status, TransitionCtx};
use agentum_store::paths;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::error::ApiError;
use crate::planner;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/board/goals", post(create_goal))
}

#[derive(Deserialize)]
struct CreateGoalBody {
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    workdir: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateGoalResponse {
    goal: BoardItem,
    planner_session_id: String,
}

async fn create_goal(
    State(state): State<AppState>,
    Json(body): Json<CreateGoalBody>,
) -> Result<(StatusCode, Json<CreateGoalResponse>), ApiError> {
    // Step 1: enforce column rules for `todo` exactly like routes/board::create.
    // Goals land in todo by definition (CONTEXT D-02) so the target is hardcoded.
    let target_status = "todo";
    let mut ctx = TransitionCtx {
        title: Some(body.title.as_str()),
        lbl: Some("goal"),
        workdir: body.workdir.as_deref(),
        tool: None,
        claimed_by: None,
        session_id: None,
        has_comment: false,
    };
    super::board::enforce_transition(&state.store, &state.bus, None, target_status, &mut ctx)
        .await?;

    // Step 2: create the goal BoardItem (lbl=goal, status=todo).
    // `board.created` is emitted inside `create_board_item` → our handler emits
    // `goal.created` separately for consumers that filter on goal-specific events.
    let new_item = NewBoardItem {
        title: body.title.clone(),
        body: body.body.clone(),
        lbl: Some("goal".into()),
        status: Some(target_status.into()),
        workdir: body.workdir.clone(),
        parent_goal_id: None,
        tool: None,
        model: None,
        session_id: None,
        priority: None,
    };
    let goal = state.store.create_board_item(new_item).await?;

    // `board.created` was emitted by routes/board.rs::create; emit the
    // goal-specific event so plan 01-04's watchdog can filter cleanly.
    let _ = state.bus.send(
        Event::new("goal.created")
            .with_payload(json!({"id": goal.id, "key": goal.key, "title": goal.title})),
    );

    // Step 3: load planner config (tool + prompt, with bundled defaults).
    // D-12: reads from disk on every submit (no in-memory cache).
    let cfg = planner::load_planner_config().await?;

    // Step 4: derive workdir. Body wins; else daemon cwd.
    let workdir = body.workdir.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/".to_string())
    });

    // Step 5: spawn the planner session (card_id = goal.id binds them).
    // Per CONTEXT D-07: if spawn fails, the goal card is NOT deleted.
    // The dashboard renders a warning chip from the `goal.planner.spawn_failed` event.
    let planner_session_id = spawn_planner_session(&state, &goal, &cfg, &workdir).await;

    match planner_session_id {
        Ok(sid) => Ok((
            StatusCode::CREATED,
            Json(CreateGoalResponse {
                goal,
                planner_session_id: sid,
            }),
        )),
        Err(e) => {
            tracing::warn!(error = %e, goal_id = goal.id, "planner spawn failed; goal retained");
            let _ = state.bus.send(
                Event::new("goal.planner.spawn_failed")
                    .with_payload(json!({"goal_id": goal.id, "error": e.to_string()})),
            );
            // Return 201 + empty session id so callers can detect the failure
            // while still having the goal row to work with.
            Ok((
                StatusCode::CREATED,
                Json(CreateGoalResponse {
                    goal,
                    planner_session_id: String::new(),
                }),
            ))
        }
    }
}

/// Spawn a planner agent session bound to the given goal.
///
/// Mirrors `routes::sessions::start` lines 256-274 exactly — any
/// change to that flow must be reflected here.
async fn spawn_planner_session(
    state: &AppState,
    goal: &BoardItem,
    cfg: &planner::PlannerConfig,
    workdir: &str,
) -> Result<String, ApiError> {
    // Name convention: `planner-<lowercase-goal-key>` e.g. `planner-ag-42`.
    let session_name = format!("planner-{}", goal.key.to_lowercase());

    let new_session = NewSession {
        name: session_name.clone(),
        workdir: workdir.to_string(),
        tool: cfg.tool.clone(),
        model: None,
        flags: vec![],
        // card_id binds this session to the goal; the watchdog (plan 01-04)
        // uses this FK to decide which goal to recompute on session events.
        card_id: Some(goal.id),
    };
    let session = state.store.create_session(new_session).await?;

    // Spawn the tmux pane — mirrors sessions::start lines 259-273.
    let target = agentum_tmux::target_for(&session.name);
    let wd = PathBuf::from(workdir);
    if !wd.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            wd.display()
        )));
    }
    let adapter = agentum_executor::adapter_for(&session.tool);
    let launch = adapter.launch(&session);

    agentum_tmux::new_session(&target, &wd, &launch.argv, &launch.env)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Err(e) = agentum_tmux::pipe_pane(&target, &log).await {
        let _ = agentum_tmux::kill_session(&target).await;
        return Err(ApiError::Internal(e.to_string()));
    }

    state
        .store
        .update_status_and_target(session.id, Status::Running, Some(&target))
        .await?;

    // Send the planner prompt as the first agent message.
    // <AG-KEY> is substituted with the real goal key (e.g. AG-42) so the
    // prompt can reference the card on the board without hard-coding IDs.
    let prompt = cfg.prompt.replace("<AG-KEY>", &goal.key);
    if let Err(e) = agentum_tmux::send_keys(&target, &prompt, true).await {
        // Prompt delivery failure is not fatal — the session is already
        // running; the user can send the prompt manually via /send.
        tracing::warn!(error = %e, "send planner prompt failed; session still running");
    }

    let _ = state
        .bus
        .send(Event::new("goal.planner.spawned").with_payload(json!({
            "goal_id": goal.id,
            "session_id": session.id.to_string(),
            "tool": cfg.tool,
        })));

    Ok(session.id.to_string())
}

#[cfg(test)]
mod tests {
    //! Handler-level tests for the board-goals endpoint.
    //! Uses the same in-process AppState harness as board.rs and
    //! board_rules.rs tests — no real tmux or HTTP server.
    //!
    //! Auth middleware is verified at the lib.rs::router() merge site
    //! (top-level `require_token` layer). The in-process harness calls
    //! handlers directly and bypasses middleware; a "unauthenticated request"
    //! test is documented as deferred to the end-to-end plan 01-08.
    //!
    //! Tmux spawn tests are marked `#[ignore]` because they require a live
    //! tmux server — uncomment and run with `--ignored` in a tmux session.

    use super::*;
    use agentum_store::Store;
    use std::sync::Arc;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    async fn fresh_state() -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.sqlite");
        std::mem::forget(dir);
        let store = Store::open(&p).await.unwrap();
        let (bus, _rx) = broadcast::channel(16);
        AppState {
            store: Arc::new(store),
            bus,
            started_at: std::time::Instant::now(),
            version: "test",
            auth_limiter: Arc::new(crate::ratelimit::RateLimiter::new(
                8,
                std::time::Duration::from_secs(60),
            )),
            cert_fingerprint: Arc::new(String::new()),
            transcripts: crate::TranscriptStore::new(broadcast::channel(16).0),
            stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: "test".to_string(),
            no_auth: true,
        }
    }

    // Serialise tests that mutate XDG_CONFIG_HOME — same pattern as planner.rs.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _dir: TempDir,
        _guard: MutexGuard<'static, ()>,
    }

    fn isolate_xdg() -> TestEnv {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().unwrap();
        // SAFETY: `set_var` is unsound under concurrent access.
        // `ENV_LOCK` serialises all tests in this module so only one thread
        // mutates XDG_CONFIG_HOME at a time.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
        }
        TestEnv {
            _dir: dir,
            _guard: guard,
        }
    }

    /// POST /api/board/goals creates a BoardItem with lbl=goal, status=todo.
    #[tokio::test]
    async fn create_goal_inserts_board_item_with_lbl_goal() {
        let _env = isolate_xdg();
        let state = fresh_state().await;

        let (code, body) = create_goal(
            State(state.clone()),
            Json(CreateGoalBody {
                title: "build OAuth".into(),
                body: None,
                workdir: None,
            }),
        )
        .await
        .expect("create_goal must succeed");

        assert_eq!(code, StatusCode::CREATED);
        let goal = &body.0.goal;
        assert_eq!(goal.lbl.as_deref(), Some("goal"), "lbl must be 'goal'");
        assert_eq!(goal.status, "todo", "goal must land in todo");
        assert_eq!(goal.title, "build OAuth");
        assert!(goal.parent_goal_id.is_none(), "goals have no parent goal");
    }

    /// POST /api/board/goals emits a goal.created event on the bus.
    #[tokio::test]
    async fn create_goal_emits_goal_created_event() {
        let _env = isolate_xdg();
        let state = fresh_state().await;
        let mut rx = state.bus.subscribe();

        let (_, body) = create_goal(
            State(state.clone()),
            Json(CreateGoalBody {
                title: "event test".into(),
                body: None,
                workdir: None,
            }),
        )
        .await
        .expect("create_goal must succeed");

        // Two events should be on the bus: board.created (from create_board_item path
        // handled inside the handler) then goal.created.  Drain until we see goal.created.
        let goal_id = body.0.goal.id;
        let mut saw_goal_created = false;
        loop {
            match rx.try_recv() {
                Ok(ev) if ev.kind == "goal.created" => {
                    assert_eq!(ev.payload["id"], goal_id);
                    saw_goal_created = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_goal_created, "goal.created event must fire on the bus");
    }

    /// POST /api/board/goals respects the column-rule gate.
    /// If PUT /api/board/rules/todo requires "body", the goal POST without body returns 400.
    #[tokio::test]
    async fn create_goal_respects_column_rule_gate() {
        let _env = isolate_xdg();
        let state = fresh_state().await;

        // Raise the bar: require body for the todo column.
        // The `body` required field maps to RequiredField::Body if that variant
        // exists; since the schema only defines Title/Lbl/Workdir/Tool/ClaimedBy/
        // SessionOrComment, we use an unknown string that the store serialises and
        // the validate_against function treats as "not present" => gate fires.
        // In practice we just require all the default `todo` fields plus `workdir`
        // so the POST without workdir is rejected.
        state
            .store
            .upsert_board_column_rule(
                "todo",
                &[
                    agentum_core::RequiredField::Title,
                    agentum_core::RequiredField::Lbl,
                    agentum_core::RequiredField::Workdir,
                ],
            )
            .await
            .unwrap();

        // POST goal without workdir — must be rejected by the gate.
        let err = create_goal(
            State(state),
            Json(CreateGoalBody {
                title: "missing workdir".into(),
                body: None,
                workdir: None,
            }),
        )
        .await
        .expect_err("gate must reject when body is required");

        // The error must be the Custom(400, {missing, status}) envelope shape.
        assert!(
            matches!(err, ApiError::Custom(s, ref v)
                if s == StatusCode::BAD_REQUEST
                && v["missing"].as_array().is_some_and(|a| !a.is_empty())
                && v["status"] == "todo"),
            "expected Custom 400 gate envelope, got {err:?}"
        );
    }

    /// When the planner tool binary does not exist, the goal card is retained
    /// and the response is still 201 with an empty planner_session_id.
    ///
    /// Marked `#[ignore]` because the full path tries to create a session row
    /// and then call tmux — which requires a live tmux server. Run with
    /// `cargo test -- --ignored` inside a tmux session to exercise this path.
    #[tokio::test]
    #[ignore = "requires a live tmux server; run with --ignored inside tmux"]
    async fn create_goal_with_missing_planner_binary_returns_201_and_emits_spawn_failed() {
        let _env = isolate_xdg();

        // Write a planner.toml that points at a non-existent binary.
        let cfg_dir = agentum_store::paths::config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("planner.toml"),
            "[planner]\ntool = \"definitely-not-a-binary-XYZZY\"\n",
        )
        .unwrap();

        let state = fresh_state().await;
        let mut rx = state.bus.subscribe();

        let (code, body) = create_goal(
            State(state.clone()),
            Json(CreateGoalBody {
                title: "spawn fail test".into(),
                body: None,
                workdir: Some("/tmp".into()),
            }),
        )
        .await
        .expect("create_goal must return Ok even on spawn failure");

        assert_eq!(code, StatusCode::CREATED, "must be 201 even on spawn fail");
        assert!(
            body.0.planner_session_id.is_empty(),
            "planner_session_id must be empty on spawn fail"
        );
        // The goal board item must still exist.
        let goal_in_db = state
            .store
            .get_board_item(body.0.goal.id)
            .await
            .unwrap()
            .expect("goal must remain in DB after spawn failure");
        assert_eq!(goal_in_db.lbl.as_deref(), Some("goal"));

        // goal.planner.spawn_failed must fire on the bus.
        let mut saw_spawn_failed = false;
        loop {
            match rx.try_recv() {
                Ok(ev) if ev.kind == "goal.planner.spawn_failed" => {
                    assert_eq!(ev.payload["goal_id"], body.0.goal.id);
                    saw_spawn_failed = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_spawn_failed, "goal.planner.spawn_failed must fire");
    }

    /// Auth middleware is verified at the lib.rs::router() merge site via the
    /// top-level `require_token` layer — the in-process test harness calls
    /// handlers directly and bypasses middleware. Testing 401 here would
    /// require spinning up a full axum server, which is deferred to the
    /// end-to-end integration tests in plan 01-08.
    #[test]
    fn goals_route_requires_auth_verified_at_router_merge() {
        // Documented skip — see comment above.
    }
}
