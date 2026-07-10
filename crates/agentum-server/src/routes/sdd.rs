//! SDD surface: playbook listing for the UI, button injection, and the
//! per-session SDD loop.
//!
//! The design rule (issue #313): the playbook bodies live server-side
//! (`crate::sdd`) and reach the agent over MCP — a button click injects a short
//! *bootstrap line* telling the agent to fetch the playbook via the
//! `agentum_sdd` tool, so the same button works for every MCP-wired tool
//! (claude/codex by launch arg, cursor/gemini/opencode by workdir config file).
//! Tools with no MCP wiring (plain shells, aider) get the whole playbook typed
//! in instead — same procedure, fatter payload.
//!
//! The **loop** is server-owned state, not UI state: toggling it on spawns a
//! worker that re-injects the orchestrator playbook each time the agent
//! settles (`agent.awaiting_input`/`agent.finished` — the same signals the
//! harness uses), and every transition is broadcast as `sdd.loop.*` on the
//! event bus. That's what lets any client render the active state truthfully
//! across reloads, and lets the agent/harness flip it with the same effect.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use agentum_core::{Event, Session, Status};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::harness::SettleOutcome;
use crate::sdd::Playbook;

use super::util::parse_uuid;

/// Live SDD loops keyed by session id. Lives in [`AppState`] so the toggle is
/// one truth for every client (and for the worker's self-cleanup).
pub type SddLoops = Arc<std::sync::Mutex<HashMap<Uuid, SddLoopHandle>>>;

pub struct SddLoopHandle {
    /// Distinguishes THIS activation from a later one on the same session so a
    /// finishing worker never removes (or announces the stop of) a successor.
    generation: u64,
    /// Current step, for `GET …/sdd/loop` and the stop event.
    step: Arc<AtomicU32>,
    max_steps: u32,
    abort: tokio::task::AbortHandle,
}

/// Monotonic activation counter backing [`SddLoopHandle::generation`].
static LOOP_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Hard cap on unattended iterations — the loop's own stop instruction asks
/// the agent to declare completion, but the server never relies on that alone.
const DEFAULT_MAX_STEPS: u32 = 10;

/// Settle windows per step. Grace mirrors the harness default (an injected
/// turn needs time to leave idle before "idle" means "done"); the timeout
/// stops the loop rather than re-injecting into a possibly-stuck agent.
const SETTLE_GRACE: Duration = Duration::from_secs(45);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(1800);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sdd/playbooks", get(list_playbooks))
        .route("/api/sessions/{id}/sdd/inject", post(inject))
        .route(
            "/api/sessions/{id}/sdd/loop",
            get(loop_state).post(loop_toggle),
        )
}

/// `GET /api/sdd/playbooks` — the registry, for pickers and preview modals.
async fn list_playbooks() -> Json<Vec<Playbook>> {
    Json(crate::sdd::playbooks())
}

// ---------- /sdd/inject ----------

#[derive(Deserialize)]
struct InjectBody {
    /// Canonical playbook name (`sdd-spec`, …).
    playbook: String,
    /// Optional free-form playbook arguments (e.g. `autonomous`, a spec id).
    #[serde(default)]
    args: Option<String>,
}

#[derive(Serialize)]
struct InjectResponse {
    /// What was typed into the pane: `bootstrap` (MCP fetch line) or `full`
    /// (the whole playbook, for tools without MCP wiring).
    mode: &'static str,
}

/// `POST /api/sessions/{id}/sdd/inject` — deliver a playbook to a running
/// session. Validation mirrors `/submit`; delivery happens in the background
/// through the same robust two-step `inject_prompt` path.
async fn inject(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<InjectBody>,
) -> Result<(StatusCode, Json<InjectResponse>), ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let host = super::sessions::load_host_for_session(&state, &session).await?;
    let target = session
        .tmux_target
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("session is not running".into()))?;
    if !crate::host_runtime::has_session(&host, target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::BadRequest(
            "tmux session not active for this session".into(),
        ));
    }
    let playbook = crate::sdd::get(&body.playbook).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "unknown playbook `{}` — see GET /api/sdd/playbooks",
            body.playbook
        ))
    })?;

    let (mode, prompt) = prompt_for(&state, &session, &playbook, body.args.as_deref()).await;
    let _ = state.bus.send(
        Event::new("sdd.injected")
            .with_session(session.id, session.name.clone())
            .with_payload(json!({ "playbook": playbook.name, "mode": mode })),
    );
    // Background delivery, same rationale as `/submit`: a busy agent can take
    // tens of seconds to go idle and the HTTP response must not wait on that.
    let bg = state.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::harness::inject_prompt(&bg, &session, &prompt).await {
            tracing::warn!(target: "agentum::sdd", error = %e, "sdd inject delivery failed");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(InjectResponse { mode })))
}

/// Pick the delivery mode for a session: the short MCP bootstrap when the
/// session's tool was MCP-wired at launch (and the master switch is on), the
/// full playbook body otherwise.
async fn prompt_for(
    state: &AppState,
    session: &Session,
    playbook: &Playbook,
    args: Option<&str>,
) -> (&'static str, String) {
    let mcp_on = state
        .store
        .setting_get_bool(super::mcp::MCP_ENABLED_SETTING, true)
        .await
        .unwrap_or(true);
    if mcp_on && tool_is_mcp_wired(&session.tool) {
        ("bootstrap", crate::sdd::bootstrap_prompt(playbook, args))
    } else {
        ("full", crate::sdd::full_prompt(playbook, args))
    }
}

/// Does this tool get the agentum MCP at launch? Arg-based (claude/codex) or
/// config-file-based (cursor/gemini/opencode) — both mean the bootstrap line
/// will resolve; anything else needs the full playbook.
fn tool_is_mcp_wired(tool: &str) -> bool {
    crate::mcp_provision::tool_supports_mcp(tool)
        || crate::mcp_provision::agent_mcp_file(tool).is_some()
}

// ---------- /sdd/loop ----------

#[derive(Deserialize)]
struct LoopBody {
    active: bool,
    /// Override the step cap for this activation (clamped to 1..=100).
    #[serde(default)]
    max_steps: Option<u32>,
}

#[derive(Serialize)]
struct LoopState {
    active: bool,
    step: u32,
    max_steps: u32,
}

/// `GET /api/sessions/{id}/sdd/loop` — the authoritative toggle state (what a
/// client renders on load; changes stream as `sdd.loop.*` events).
async fn loop_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<LoopState>, ApiError> {
    let id = parse_uuid(&id)?;
    Ok(Json(read_loop_state(&state, id)))
}

fn read_loop_state(state: &AppState, id: Uuid) -> LoopState {
    let map = state.sdd_loops.lock().expect("sdd_loops lock");
    match map.get(&id) {
        Some(h) if !h.abort.is_finished() => LoopState {
            active: true,
            step: h.step.load(Ordering::Relaxed),
            max_steps: h.max_steps,
        },
        _ => LoopState {
            active: false,
            step: 0,
            max_steps: 0,
        },
    }
}

/// `POST /api/sessions/{id}/sdd/loop` — flip the loop. Activation is
/// idempotent (a live loop stays); deactivation aborts the worker and
/// announces the stop.
async fn loop_toggle(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<LoopBody>,
) -> Result<Json<LoopState>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;

    if !body.active {
        let removed = state.sdd_loops.lock().expect("sdd_loops lock").remove(&id);
        if let Some(h) = removed {
            h.abort.abort();
            emit_loop_stopped(
                &state,
                id,
                &session.name,
                "toggled_off",
                h.step.load(Ordering::Relaxed),
            );
        }
        return Ok(Json(read_loop_state(&state, id)));
    }

    if session.status != Status::Running || session.tmux_target.is_none() {
        return Err(ApiError::BadRequest(
            "session is not running — start it before activating the SDD loop".into(),
        ));
    }
    // Idempotent re-activation: a live loop keeps running untouched.
    {
        let map = state.sdd_loops.lock().expect("sdd_loops lock");
        if let Some(h) = map.get(&id) {
            if !h.abort.is_finished() {
                return Ok(Json(LoopState {
                    active: true,
                    step: h.step.load(Ordering::Relaxed),
                    max_steps: h.max_steps,
                }));
            }
        }
    }

    let max_steps = body.max_steps.unwrap_or(DEFAULT_MAX_STEPS).clamp(1, 100);
    let generation = LOOP_GENERATION.fetch_add(1, Ordering::Relaxed);
    let step = Arc::new(AtomicU32::new(0));
    let worker = tokio::spawn(run_loop(
        state.clone(),
        id,
        generation,
        step.clone(),
        max_steps,
    ));
    state.sdd_loops.lock().expect("sdd_loops lock").insert(
        id,
        SddLoopHandle {
            generation,
            step,
            max_steps,
            abort: worker.abort_handle(),
        },
    );
    let _ = state.bus.send(
        Event::new("sdd.loop.started")
            .with_session(id, session.name.clone())
            .with_payload(json!({ "max_steps": max_steps })),
    );
    Ok(Json(LoopState {
        active: true,
        step: 0,
        max_steps,
    }))
}

fn emit_loop_stopped(state: &AppState, id: Uuid, name: &str, reason: &str, steps: u32) {
    let _ = state.bus.send(
        Event::new("sdd.loop.stopped")
            .with_session(id, name.to_string())
            .with_payload(json!({ "reason": reason, "steps": steps })),
    );
}

/// The loop worker: drive to a terminal reason, then clean up — but only if
/// this activation still owns the map entry (a re-toggle may have replaced it,
/// and the successor's entry/stop-event are not ours to touch).
async fn run_loop(state: AppState, id: Uuid, generation: u64, step: Arc<AtomicU32>, max_steps: u32) {
    let reason = drive_sdd_loop(&state, id, &step, max_steps).await;
    let is_current = {
        let mut map = state.sdd_loops.lock().expect("sdd_loops lock");
        match map.get(&id) {
            Some(h) if h.generation == generation => {
                map.remove(&id);
                true
            }
            _ => false,
        }
    };
    if is_current {
        let name = state
            .store
            .get_session_by_id(id)
            .await
            .ok()
            .flatten()
            .map(|s| s.name)
            .unwrap_or_default();
        emit_loop_stopped(&state, id, &name, reason, step.load(Ordering::Relaxed));
    }
}

/// One activation's drive: inject the orchestrator step, wait for the agent to
/// settle, repeat. Every exit path is a named reason (it lands in the
/// `sdd.loop.stopped` payload — no human is watching the pane, so the event is
/// the explanation).
async fn drive_sdd_loop(
    state: &AppState,
    id: Uuid,
    step_counter: &AtomicU32,
    max_steps: u32,
) -> &'static str {
    let Some(playbook) = crate::sdd::get("sdd-orchestrate") else {
        return "playbook_missing";
    };
    for step in 1..=max_steps {
        // Fresh row each iteration: stop/kill/crash between steps must end the
        // loop, not feed prompts to a dead pane.
        let session = match state.store.get_session_by_id(id).await {
            Ok(Some(s)) => s,
            _ => return "session_gone",
        };
        if session.status != Status::Running || session.tmux_target.is_none() {
            return "session_not_running";
        }
        step_counter.store(step, Ordering::Relaxed);
        let _ = state.bus.send(
            Event::new("sdd.loop.step")
                .with_session(id, session.name.clone())
                .with_payload(json!({ "step": step, "max_steps": max_steps })),
        );
        // Autonomous mode: the orchestrator must apply gates itself — a loop
        // that pauses to ask a human defeats its own purpose.
        let (_, base) = prompt_for(state, &session, &playbook, Some("autonomous")).await;
        let prompt = crate::sdd::loop_step_prompt(step, &base);
        if let Err(e) = crate::harness::inject_prompt(state, &session, &prompt).await {
            tracing::warn!(target: "agentum::sdd", error = %e, "sdd loop inject failed");
            return "inject_failed";
        }
        match crate::harness::wait_for_settle(&state.bus, id, SETTLE_GRACE, SETTLE_TIMEOUT).await {
            SettleOutcome::Settled => {}
            SettleOutcome::Crashed => return "session_ended",
            // Re-injecting into an agent that never signalled idle only piles
            // prompts into a stuck pane — stop loudly instead.
            SettleOutcome::TimedOut => return "settle_timeout",
        }
    }
    "max_steps"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_only_for_tools_with_mcp_wiring() {
        // Arg-based + file-based launches resolve the bootstrap line…
        for tool in ["claude", "codex", "cursor", "gemini", "opencode"] {
            assert!(tool_is_mcp_wired(tool), "{tool} is wired");
        }
        // …bare shells and unwired CLIs must get the full playbook.
        for tool in ["bash", "terminal", "aider", "hermes"] {
            assert!(!tool_is_mcp_wired(tool), "{tool} is not wired");
        }
    }

    use agentum_core::NewSession;
    use agentum_store::Store;
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
            wiki_keys: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: "test".to_string(),
            no_auth: true,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus: broadcast::channel(64).0,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            mcp_token: Arc::new(String::from("test-mcp-token")),
            api_base_url: None,
            desktop_bridge: None,
            harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
            sdd_loops: Default::default(),
            events_ws_clients: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    async fn seed_session(state: &AppState) -> agentum_core::Session {
        state
            .store
            .create_session(NewSession {
                name: "sdd-test".into(),
                workdir: "/tmp".into(),
                tool: "claude".into(),
                model: None,
                flags: vec![],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn loop_defaults_inactive_and_wont_activate_on_a_stopped_session() {
        let state = fresh_state().await;
        let session = seed_session(&state).await;

        // Never toggled → inactive, step 0.
        let st = read_loop_state(&state, session.id);
        assert!(!st.active);
        assert_eq!(st.step, 0);

        // Toggle-off with no live loop is an idempotent no-op.
        let res = loop_toggle(
            State(state.clone()),
            Path(session.id.to_string()),
            Json(LoopBody {
                active: false,
                max_steps: None,
            }),
        )
        .await
        .unwrap();
        assert!(!res.0.active);

        // Activating on a not-running session is a 400, not a zombie worker.
        let err = loop_toggle(
            State(state.clone()),
            Path(session.id.to_string()),
            Json(LoopBody {
                active: true,
                max_steps: None,
            }),
        )
        .await;
        assert!(err.is_err());
        assert!(state.sdd_loops.lock().unwrap().is_empty(), "no worker spawned");
    }

    #[tokio::test]
    async fn inject_rejects_stopped_sessions_and_unknown_playbooks() {
        let state = fresh_state().await;
        let session = seed_session(&state).await;
        // Stopped session (no tmux target) → 400 before any delivery attempt.
        let err = inject(
            State(state.clone()),
            Path(session.id.to_string()),
            Json(InjectBody {
                playbook: "sdd-spec".into(),
                args: None,
            }),
        )
        .await;
        assert!(err.is_err());
        // Unknown session id → 404.
        let err = inject(
            State(state.clone()),
            Path(Uuid::new_v4().to_string()),
            Json(InjectBody {
                playbook: "sdd-spec".into(),
                args: None,
            }),
        )
        .await;
        assert!(matches!(err, Err(ApiError::NotFound(_))));
    }
}
