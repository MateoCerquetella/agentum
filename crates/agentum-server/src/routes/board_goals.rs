//! `/api/board/goals` — atomic create-goal + spawn-planner-session.
//! The goal IS a `BoardItem` with `lbl="goal"` (CONTEXT D-01); no
//! parallel table. The planner is a normal agent session bound via
//! `session.card_id = goal.id` (CONTEXT D-07).

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

/// Spawn a tool session bound to the given card and atomically dual-write
/// the binding via `Store::claim_card`. Mirrors `spawn_planner_session`
/// but takes a card directly instead of a goal + planner config.
///
/// CONTEXT D-01: PATCH→doing auto-spawn fires this from board.rs::patch
/// after `enforce_transition` passes.
/// CONTEXT D-02: tool defaults to "claude"; workdir falls through to
/// parent_goal.workdir, then HTTP 400.
/// CONTEXT D-03 (superseded): the ticket title+body is sent as the first
/// prompt so the agent starts with context instead of a blank pane. The
/// send is fire-and-forget on a tokio task after a short delay so the
/// agent's splash/trust dialog is past before keystrokes arrive.
/// CLAUDE.md YOLO rule: push the canonical YOLO marker into flags; let
/// translate_yolo_marker in the adapter substitute per-tool.
/// Plan-checker iter-1 W-3: does NOT emit `session.started` — the
/// watchdog's per-session loop already emits that event when status
/// flips to Running (watchdog/src/lib.rs:147).
pub(crate) async fn spawn_card_session(
    state: &AppState,
    card: &BoardItem,
) -> Result<String, ApiError> {
    // 1. Resolve tool: card.tool → parent_goal.tool → "claude" (CONTEXT D-02).
    let tool = match card.tool.as_deref() {
        Some(t) => t.to_string(),
        None => {
            if let Some(pg_id) = card.parent_goal_id {
                state
                    .store
                    .get_board_item(pg_id)
                    .await?
                    .and_then(|pg| pg.tool)
                    .unwrap_or_else(|| "claude".to_string())
            } else {
                "claude".to_string()
            }
        }
    };

    // 2. Resolve workdir: card.workdir → parent_goal.workdir → 400 (CONTEXT D-02).
    let workdir = match card.workdir.as_deref() {
        Some(w) => w.to_string(),
        None => {
            let from_parent = if let Some(pg_id) = card.parent_goal_id {
                state
                    .store
                    .get_board_item(pg_id)
                    .await?
                    .and_then(|pg| pg.workdir)
            } else {
                None
            };
            from_parent.ok_or_else(|| {
                ApiError::Custom(
                    axum::http::StatusCode::BAD_REQUEST,
                    serde_json::json!({"missing": ["workdir"], "status": "doing"}),
                )
            })?
        }
    };

    // 3. Verify workdir exists on disk (mirrors spawn_planner_session :155-160).
    let wd = super::util::expand_workdir(&workdir)?;
    if !wd.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            wd.display()
        )));
    }
    let workdir = wd.to_string_lossy().into_owned();

    // 4. Build NewSession with the canonical YOLO marker pushed verbatim —
    //    adapters call translate_yolo_marker on launch (CLAUDE.md YOLO rule).
    //    agentum_executor::YOLO_MARKER = "--dangerously-skip-permissions".
    let new_session = NewSession {
        name: format!("card-{}", card.key.to_lowercase()),
        workdir: workdir.clone(),
        tool: tool.clone(),
        model: None,
        flags: vec![agentum_executor::YOLO_MARKER.to_string()],
        // card_id is overwritten unconditionally by claim_card — set it
        // here for clarity but claim_card will enforce it.
        card_id: Some(card.id),
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };

    // 5. Atomic dual-write: INSERT session row + UPDATE card.session_id in one tx.
    //    claim_card returns AlreadyExists (→ HTTP 409) if the card is already bound.
    let (_card_after, session) = state
        .store
        .claim_card(card.id, new_session)
        .await
        .map_err(ApiError::from)?;

    // 6. Spawn tmux pane (mirrors spawn_planner_session :152-173).
    let target = agentum_tmux::target_for(&session.name);
    let adapter = agentum_executor::adapter_for(&session.tool);
    let launch = adapter.launch(&session);

    if let Err(e) = agentum_tmux::new_session(&target, &wd, &launch.argv, &launch.env).await {
        return Err(ApiError::Internal(e.to_string()));
    }

    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Err(e) = agentum_tmux::pipe_pane(&target, &log).await {
        let _ = agentum_tmux::kill_session(&target).await;
        return Err(ApiError::Internal(e.to_string()));
    }

    // 7. Mark session Running so the watchdog reconciler picks it up.
    //    The watchdog's watch_session loop emits `session.started` on the bus
    //    once it observes Status::Running — this route does NOT emit that event
    //    itself (plan-checker iter-1 W-3: avoid duplicate signal on the bus).
    state
        .store
        .update_status_and_target(session.id, Status::Running, Some(&target))
        .await?;

    // 8. Send the ticket title+body as the first prompt so the agent
    //    starts with context. Fire-and-forget on a tokio task because
    //    Claude's splash + trust dialog can swallow keystrokes that
    //    arrive too early — wait ~1.5s before send_keys. The HTTP
    //    response doesn't block on this; if the send fails the user
    //    can paste the prompt manually.
    if let Some(prompt) = build_card_prompt(card) {
        let target_for_prompt = target.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            if let Err(e) = agentum_tmux::send_keys(&target_for_prompt, &prompt, true).await {
                tracing::warn!(error = %e, "send card prompt failed; session still running");
            }
        });
    }

    Ok(session.id.to_string())
}

/// Compose the opening prompt sent to a freshly-spawned card session.
/// Returns `None` when the card has neither body nor a non-empty title
/// (defensive — title is required by the schema but we don't want to
/// send an empty Enter to the agent on edge cases).
fn build_card_prompt(card: &BoardItem) -> Option<String> {
    let title = card.title.trim();
    let body = card.body.as_deref().map(str::trim).unwrap_or("");
    match (title.is_empty(), body.is_empty()) {
        (true, true) => None,
        (false, true) => Some(format!("Working on {key}: {title}", key = card.key)),
        (true, false) => Some(body.to_string()),
        (false, false) => Some(format!(
            "Working on {key}: {title}\n\n{body}",
            key = card.key
        )),
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

    // Expand `~`/`~/x` once so the stored session row and the tmux spawn
    // both see the same canonical absolute path.
    let wd = super::util::expand_workdir(workdir)?;
    if !wd.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            wd.display()
        )));
    }
    let workdir_resolved = wd.to_string_lossy().into_owned();

    let new_session = NewSession {
        name: session_name.clone(),
        workdir: workdir_resolved,
        tool: cfg.tool.clone(),
        model: None,
        flags: vec![],
        // card_id binds this session to the goal; the watchdog (plan 01-04)
        // uses this FK to decide which goal to recompute on session events.
        card_id: Some(goal.id),
        worktree_path: None,
        worktree_branch: None,
        worktree_base_ref: None,
    };
    let session = state.store.create_session(new_session).await?;

    // Spawn the tmux pane — mirrors sessions::start lines 259-273.
    let target = agentum_tmux::target_for(&session.name);
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
    use std::sync::MutexGuard;
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
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus: broadcast::channel(64).0,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            mcp_token: Arc::new(String::from("test-mcp-token")),
            api_base_url: None,
            desktop_bridge: None,
            harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
        }
    }

    struct TestEnv {
        _dir: TempDir,
        _guard: MutexGuard<'static, ()>,
    }

    fn isolate_xdg() -> TestEnv {
        // Shared crate-wide lock: AGENTUM_HOME is process-global, so serialise
        // against profiles/planner too (a per-module lock would not).
        let guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().unwrap();
        // SAFETY: `set_var` is unsound under concurrent access.
        // `ENV_LOCK` serialises all tests in this module so only one thread
        // mutates the env at a time. AGENTUM_HOME isolates on every platform
        // (XDG_CONFIG_HOME is a no-op on macOS).
        unsafe {
            std::env::set_var("AGENTUM_HOME", dir.path());
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

    // --- spawn_card_session tests (plan 02-03) ---

    /// spawn_card_session returns HTTP 400 with the canonical missing-workdir
    /// envelope when the card has no workdir and no parent_goal.
    ///
    /// This test exercises the CONTEXT D-02 fallthrough path without needing
    /// a live tmux server. The tmux-requiring happy-path is deferred to plan
    /// 02-06 e2e.
    #[tokio::test]
    async fn spawn_card_session_missing_workdir_returns_400() {
        let state = fresh_state().await;

        // Create a card with no workdir and no parent_goal_id.
        let card = state
            .store
            .create_board_item(agentum_core::NewBoardItem {
                title: "no workdir card".into(),
                body: None,
                status: Some("todo".into()),
                lbl: Some("feat".into()),
                tool: Some("claude".into()),
                workdir: None,
                model: None,
                session_id: None,
                priority: None,
                parent_goal_id: None,
            })
            .await
            .unwrap();

        let err = spawn_card_session(&state, &card)
            .await
            .expect_err("spawn_card_session must fail when workdir is absent");

        // Must be ApiError::Custom(400, {missing: ["workdir"], status: "doing"}).
        assert!(
            matches!(&err, ApiError::Custom(s, v)
                if *s == axum::http::StatusCode::BAD_REQUEST
                && v["missing"].as_array().is_some_and(|a| a.iter().any(|x| x == "workdir"))
                && v["status"] == "doing"),
            "expected Custom 400 missing-workdir envelope, got {err:?}"
        );
    }

    /// spawn_card_session with a live tmux requires a running tmux server.
    /// The full happy-path (workdir resolved, session spawned, dual-write committed,
    /// board.updated carries session_id) is covered by plan 02-06 e2e.
    ///
    /// Marked `#[ignore]` — run with `--ignored` inside a tmux session to exercise
    /// the live tmux path.
    #[tokio::test]
    #[ignore = "requires a live tmux server; covered by plan 02-06 e2e"]
    async fn spawn_card_session_happy_path_requires_live_tmux() {
        // Deferred to plan 02-06 end-to-end integration tests.
    }

    fn card_with(title: &str, body: Option<&str>) -> BoardItem {
        BoardItem {
            id: 1,
            key: "AG-1".into(),
            title: title.into(),
            body: body.map(str::to_string),
            status: "doing".into(),
            claimed_by: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
            lbl: None,
            tool: None,
            workdir: None,
            model: None,
            session_id: None,
            priority: 0,
            parent_goal_id: None,
        }
    }

    #[test]
    fn build_card_prompt_combines_title_and_body() {
        let card = card_with("Wire dashboard", Some("Use Svelte 5 runes"));
        let p = build_card_prompt(&card).expect("title+body must produce a prompt");
        assert!(p.contains("AG-1"), "prompt must include the card key");
        assert!(p.contains("Wire dashboard"), "prompt must include title");
        assert!(p.contains("Use Svelte 5 runes"), "prompt must include body");
    }

    #[test]
    fn build_card_prompt_title_only_includes_key() {
        let card = card_with("Wire dashboard", None);
        let p = build_card_prompt(&card).expect("title-only must produce a prompt");
        assert!(p.contains("AG-1"));
        assert!(p.contains("Wire dashboard"));
    }

    #[test]
    fn build_card_prompt_body_only_is_verbatim() {
        let card = card_with("", Some("Investigate the panic"));
        let p = build_card_prompt(&card).expect("body-only must produce a prompt");
        assert_eq!(p, "Investigate the panic");
    }

    #[test]
    fn build_card_prompt_empty_returns_none() {
        let card = card_with("   ", Some("   "));
        assert!(build_card_prompt(&card).is_none());
    }
}
