use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use agentum_core::{
    EXTERNAL_TMUX_FLAG, Event, Host, HostKind, LOCAL_HOST_ID, NewSession, Session, Status,
    WorktreeSpec,
};
use agentum_store::paths;
use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::time::sleep;
use uuid::Uuid;

use crate::AppState;
use crate::StreamCheckpoint;
use crate::error::ApiError;

// The WS pane-streaming machinery (local + remote `stream_session`) lives in the
// `streaming` child module; the thin `stream` handler below calls the two entry
// points it re-imports here. `streaming` reaches back for shared helpers
// (`save_checkpoint`, …) via its own `use super::*`.
mod streaming;
use streaming::{stream_remote_session, stream_session};

// Pane env + MCP/endpoint provisioning + the shared spawn-into-pane launch path.
// `spawn_agent_into_pane` + `boot_drift_rescan` are re-exported at crate scope to
// preserve `routes::sessions::…` references from harness::drive, board_goals, and
// lib.rs; the rest are used internally by the create/start handlers.
mod provision;
use super::util::parse_uuid;
use provision::{Reprovision, reprovision_session};
pub(crate) use provision::{boot_drift_rescan, boot_revive_dead_sessions, spawn_agent_into_pane};

const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// When a Local session's `workdir` is missing, decide whether it's a desktop
/// git-worktree workspace we can safely recreate. Matches the desktop layout
/// `<repo>/.claude/worktrees/<name>` and returns the parent repo path (when it
/// still exists on disk) so the caller can `git worktree add` the tree back.
/// Returns `None` for any other shape — we never auto-create arbitrary paths.
fn worktree_repo_for_missing(workdir: &std::path::Path) -> Option<PathBuf> {
    let worktrees_dir = workdir.parent()?; // <repo>/.claude/worktrees
    if worktrees_dir.file_name()?.to_str()? != "worktrees" {
        return None;
    }
    let claude_dir = worktrees_dir.parent()?; // <repo>/.claude
    if claude_dir.file_name()?.to_str()? != ".claude" {
        return None;
    }
    let repo = claude_dir.parent()?; // <repo>
    repo.is_dir().then(|| repo.to_path_buf())
}

/// Self-heal a missing **Local** workdir: a no-op when it exists; otherwise, if
/// it's a desktop git-worktree workspace (`<repo>/.claude/worktrees/<name>`)
/// whose tree vanished (pruned out-of-band, removed by hand, or a registry row
/// that outlived its checkout), recreate it from the intact parent repo. Any
/// other missing path stays a hard 400 — a dead workdir breaks branch-compare and
/// every git/terminal op that opens the worktree. Shared by `create` + `start`,
/// which previously inlined this identical block.
async fn self_heal_local_workdir(workdir: &std::path::Path) -> Result<(), ApiError> {
    if workdir.exists() {
        return Ok(());
    }
    match worktree_repo_for_missing(workdir) {
        Some(repo) => crate::git::recreate_worktree(&repo, workdir)
            .await
            .map_err(|e| {
                ApiError::BadRequest(format!(
                    "workdir does not exist and could not be recreated: {} ({e})",
                    workdir.display()
                ))
            }),
        None => Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        ))),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(list).post(create))
        .route(
            "/api/sessions/{id}",
            get(get_one).patch(patch_session).delete(delete),
        )
        .route("/api/sessions/{id}/start", post(start))
        .route("/api/sessions/{id}/stop", post(stop))
        .route("/api/sessions/{id}/kill", post(kill))
        .route("/api/sessions/{id}/send", post(send))
        .route("/api/sessions/{id}/submit", post(submit))
        .route("/api/sessions/{id}/pane", get(pane))
        .route("/api/sessions/{id}/stream", get(stream))
        .route("/api/sessions/{id}/worktree/prune", post(worktree_prune))
        .route(
            "/api/sessions/{id}/worktree/status",
            get(worktree_status_route),
        )
        .route("/api/sessions/{id}/hook", post(hook))
}

/// HTTP body for POST /api/sessions. Wraps `NewSession` to layer the
/// optional `worktree: WorktreeSpec` on top — the spec is consumed
/// server-side by [`crate::git::create_worktree`] before the store
/// ever sees this struct, so `NewSession` itself stays free of any
/// "pending" worktree state.
#[derive(Deserialize)]
struct CreateBody {
    #[serde(flatten)]
    new: NewSession,
    #[serde(default)]
    host_id: Option<Uuid>,
    #[serde(default)]
    worktree: Option<WorktreeSpec>,
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

/// Clamps the `lines=` query param to the allowed 1..=200 range.
/// Default (absent) is 20, matching the D-13 polling cadence spec.
fn clamp_lines(req: Option<u32>) -> usize {
    let n = req.unwrap_or(20);
    n.clamp(1, 200) as usize
}

/// Query extractor for GET /api/sessions/{id}/pane.
#[derive(Deserialize)]
struct PaneQuery {
    lines: Option<u32>,
}

/// Response body for GET /api/sessions/{id}/pane.
/// Contract: UI-SPEC §Component Inventory — `{ lines, captured_at }`.
#[derive(Debug, Serialize)]
struct PaneSnapshot {
    /// Last N plain-text lines of the visible pane viewport, chronological order.
    lines: Vec<String>,
    /// RFC3339 timestamp of the capture.
    captured_at: String,
}

async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Session>>, ApiError> {
    let status = match q.status.as_deref() {
        Some(s) => Some(Status::from_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))?),
        None => None,
    };
    let rows = state.store.list_sessions(status).await?;
    Ok(Json(rows))
}

async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    let CreateBody {
        mut new,
        host_id,
        worktree,
    } = body;
    let host_id = host_id.unwrap_or(LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;

    let workdir = match &host.kind {
        HostKind::Local => {
            let workdir = super::util::expand_workdir(&new.workdir)?;
            self_heal_local_workdir(&workdir).await?;
            new.workdir = workdir.to_string_lossy().into_owned();
            workdir
        }
        HostKind::Ssh { .. } => {
            if worktree.is_some() {
                return Err(ApiError::BadRequest(
                    "worktree isolation is not available on SSH hosts yet".into(),
                ));
            }
            PathBuf::from(new.workdir.trim())
        }
    };

    // Worktree isolation: if the user asked for it, run `git worktree
    // add` *before* persisting so the session row records the resolved
    // path. We do this before the store insert so a partial failure
    // (worktree created, DB insert failed) is the rare path; the more
    // common partial failure (DB insert ok, worktree creation failed)
    // is impossible by construction.
    if let Some(spec) = worktree {
        let resolved = crate::git::create_worktree(
            &workdir,
            &new.name,
            spec.branch.as_deref(),
            spec.base_ref.as_deref(),
        )
        .await
        .map_err(|e| ApiError::BadRequest(format!("worktree: {}", e)))?;
        new.worktree_path = Some(resolved.path.to_string_lossy().into_owned());
        new.worktree_branch = Some(resolved.branch);
        new.worktree_base_ref = Some(resolved.base_ref);
    }

    let s = state
        .store
        .create_session_on_host(new, Some(host_id))
        .await?;
    Ok((StatusCode::CREATED, Json(s)))
}

/// POST /api/sessions/{id}/worktree/prune
///
/// Tears down the worktree associated with this session. Requires the
/// session to be in a non-running state (Stopped/Crashed/Idle without
/// tmux_target) — pruning the cwd of a live tmux pane would yank the
/// rug out from under the agent process.
///
/// Body (all optional):
///   { "force": bool }   — pass through to `git worktree remove --force`,
///                         abandoning uncommitted changes. Defaults to
///                         false; the route preflights with `git status
///                         --porcelain` and refuses on dirty trees unless
///                         this is true.
#[derive(Default, Deserialize)]
struct PruneBody {
    #[serde(default)]
    force: bool,
}

async fn worktree_prune(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<PruneBody>>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let force = body.map(|Json(b)| b.force).unwrap_or(false);

    let session = load(&state, id).await?;

    let wt_path = session
        .worktree_path
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest("session has no worktree to prune".into()))?;
    if matches!(session.status, Status::Running) {
        return Err(ApiError::BadRequest(
            "stop the session before pruning its worktree".into(),
        ));
    }

    let wt_pathbuf = PathBuf::from(wt_path);
    // Preflight dirty check: refuse silently destroying work unless the
    // caller explicitly passed force=true.
    if !force {
        if let Ok(status) = crate::git::worktree_status(&wt_pathbuf).await {
            if !status.is_clean() {
                return Err(ApiError::BadRequest(format!(
                    "worktree has uncommitted changes (staged={}, unstaged={}, untracked={}); pass force=true to discard",
                    status.staged, status.unstaged, status.untracked
                )));
            }
        }
    }

    let repo = PathBuf::from(&session.workdir);
    crate::git::prune_worktree(
        &repo,
        &wt_pathbuf,
        session.worktree_branch.as_deref(),
        force,
    )
    .await
    .map_err(|e| ApiError::Internal(format!("git: {}", e)))?;

    state.store.clear_session_worktree(id).await?;
    let updated = load(&state, id).await?;
    let _ = state.bus.send(
        Event::new("session.worktree.pruned")
            .with_session(updated.id, &updated.name)
            .with_payload(serde_json::json!({"path": wt_path})),
    );
    Ok(Json(updated))
}

/// GET /api/sessions/{id}/worktree/status
///
/// Returns `{ staged, unstaged, untracked }` counts. Used by the
/// dashboard prune confirmation to show "you'll lose N files" before
/// the user clicks through. 404s when the session has no worktree.
async fn worktree_status_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::git::WorktreeStatus>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let wt_path = session
        .worktree_path
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("session has no worktree".into()))?;
    let s = crate::git::worktree_status(&PathBuf::from(wt_path))
        .await
        .map_err(|e| ApiError::Internal(format!("git: {}", e)))?;
    Ok(Json(s))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let s = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(Json(s))
}

#[derive(Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
struct PatchBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    flags: Option<Vec<String>>,
    #[serde(default)]
    model: Option<Option<String>>,
    #[serde(default)]
    pinned: Option<bool>,
}

async fn patch_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let running = matches!(session.status, Status::Running);

    // Rename is allowed even on a running session — it's pure metadata
    // (tmux target stays put because we keyed it by id at start time, not
    // by name).
    let mut current = session;
    if let Some(raw_name) = body.name.as_ref() {
        let name = raw_name.trim();
        if name.is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".into()));
        }
        if name.len() > 64 {
            return Err(ApiError::BadRequest("name too long (max 64 chars)".into()));
        }
        if name != current.name {
            let updated = state.store.patch_session_name(id, name).await?;
            let _ = state
                .bus
                .send(Event::new("session.renamed").with_session(updated.id, &updated.name));
            current = updated;
        }
    }

    // Tool patch is allowed at any time — the watchdog uses this path
    // to keep the chip in sync with the foreground process. UI clients
    // are also free to PATCH it manually if they want to relabel.
    if let Some(raw_tool) = body.tool.as_ref() {
        let tool = raw_tool.trim();
        if tool.is_empty() {
            return Err(ApiError::BadRequest("tool cannot be empty".into()));
        }
        if tool != current.tool {
            let updated = state.store.patch_session_tool(id, tool).await?;
            if current.tool == "claude" && updated.tool != "claude" {
                state.transcripts.stop_observing(id);
            }
            let _ = state.bus.send(
                Event::new("session.tool_changed")
                    .with_session(updated.id, &updated.name)
                    .with_payload(serde_json::json!({"tool": updated.tool})),
            );
            current = updated;
        }
    }

    // Pin toggle is metadata-only — works on running and stopped
    // sessions alike. Always emits `session.pinned` so multi-tab
    // dashboards stay in sync without polling.
    if let Some(pinned) = body.pinned
        && current.pinned != pinned
    {
        let updated = state.store.patch_session_pinned(id, pinned).await?;
        let _ = state.bus.send(
            Event::new("session.pinned")
                .with_session(updated.id, &updated.name)
                .with_payload(serde_json::json!({"pinned": updated.pinned})),
        );
        current = updated;
    }

    // Flags + model still require the session to be stopped — they
    // affect how the agent is launched and rewriting them under a live
    // process would be a footgun.
    if body.flags.is_some() || body.model.is_some() {
        if running {
            return Err(ApiError::BadRequest(
                "cannot patch flags / model on a running session; stop it first".into(),
            ));
        }
        if let Some(flags) = body.flags {
            current = state.store.patch_session_flags(id, &flags).await?;
        }
        if let Some(model) = body.model {
            // Future: patch model — not yet implemented in store
            let _ = model;
        }
    }
    Ok(Json(current))
}

async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;

    // Guard a genuinely-running agent behind ?force=true so it isn't torn
    // down by accident. Any other status (Idle/Crashed/Stopped) deletes freely.
    if matches!(session.status, Status::Running) && !q.force {
        return Err(ApiError::BadRequest(
            "session is running; pass ?force=true to kill and remove".into(),
        ));
    }

    // Deletion owns both observer and parser-state retirement. Do this before
    // best-effort tmux teardown so an unreachable host cannot leak a watcher.
    state.transcripts.forget(id);
    #[cfg(test)]
    state.transcripts.pause_after_early_route_retirement().await;

    // Best-effort tmux teardown, then always remove the record. Two things this
    // must NOT do — both previously surfaced as "can't delete the session":
    //   1. Gate teardown on `Running` only. The recorded status lags reality —
    //      a session resting in `Idle` (an agent awaiting input) still owns a
    //      live pane, so the old `Running`-only gate orphaned it, and a stale
    //      `agentum-<name>` pane then broke recreating a session of that name.
    //      We always tear down now; `kill_session` is idempotent (it no-ops when
    //      the pane is already gone).
    //   2. Let a teardown failure block record removal. An unreachable host, or
    //      a host record that's been deleted, must not pin the session row in the
    //      store forever — removing the local record is always allowed; we just
    //      can't reach a remote pane that may already be dead. Failures are
    //      logged, never propagated.
    match load_host_for_session(&state, &session).await {
        Ok(host) => {
            let target = tmux_target(&session);
            let outcome = if is_external(&session) {
                // Never destroy a user-owned tmux session — just disarm the
                // pipe (and only when no sibling row still streams this pane).
                unpipe_external(&state, &host, &session, &target).await;
                Ok(())
            } else {
                crate::host_runtime::kill_session(&host, &target).await
            };
            if let Err(e) = outcome {
                tracing::warn!(
                    session = %session.id, %target,
                    "tmux teardown failed during delete (removing record anyway): {e}"
                );
            }
        }
        Err(e) => tracing::warn!(
            session = %session.id,
            "host unavailable during delete (removing record anyway): {e}"
        ),
    }

    state.store.delete_session(id).await?;
    // A concurrent read can recreate passive/live transcript state while the
    // durable row still exists during teardown. Successful deletion is the
    // final authority boundary, so clear anything recreated in that window.
    state.transcripts.forget(id);
    Ok(StatusCode::NO_CONTENT)
}

/// Session JSON + a `spawned` flag: `true` when start created a fresh pane
/// (bare shell), `false` when it reattached to a live tmux session. Clients
/// that type a launch command into the pane (the desktop's agent-tab reopen)
/// must skip it on reattach — the command would land inside whatever is
/// already running there (e.g. an agent's composer). Additive field: clients
/// deserializing plain `Session` ignore it.
fn session_with_spawned(session: Session, spawned: bool) -> serde_json::Value {
    let mut value = serde_json::to_value(&session).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("spawned".into(), serde_json::Value::Bool(spawned));
    }
    value
}

async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let host = load_host_for_session(&state, &session).await?;
    // External sessions keep their (non-agentum) target across stop, so
    // start can only ever reattach to it — never derive a fresh
    // `agentum-*` name, which would spawn a parallel session.
    let target = if is_external(&session) {
        session.tmux_target.clone().ok_or_else(|| {
            ApiError::BadRequest("external session has lost its tmux target".into())
        })?
    } else {
        agentum_tmux::target_for(&session.name)
    };

    let already = crate::host_runtime::has_session(&host, &target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if already {
        // The tmux session is still alive — REATTACH to it instead of killing
        // and respawning. This is what makes a session survive an agentum
        // restart (persist): the live pane and the agent running in it are
        // preserved; we just mark the session Running and stream it again. The
        // stored status may be stale (e.g. crashed/idle from before the restart,
        // or never updated because agentum was killed) — but a live tmux session
        // is reattachable regardless, and the watchdog reconciles the real state
        // on its next tick. Previously this branch killed the "orphan", which
        // destroyed exactly the session the user wanted to keep across a restart.
        if !matches!(session.status, Status::Running) {
            state
                .store
                .update_status_and_target(id, Status::Running, Some(&target))
                .await?;
        }
        // R4: the live endpoint may have drifted from what this session was
        // provisioned with (R1+R2 keep it stable across the common restart, but a
        // forced ephemeral rebind still moves it). Rewrite the MCP config + pane
        // env to the current endpoint before returning. Best-effort: discard the
        // result — a manual `/start` IS the user already reconnecting, so we don't
        // set the needs-reconnect flag here.
        if reprovision_session(&state, &session, &host).await == Reprovision::Rewritten
            && matches!(host.kind, HostKind::Local)
        {
            // Refresh the recorded endpoint + clear any stale needs-reconnect flag:
            // a manual reattach IS the reconnect, so the next boot drift scan must
            // not re-flag this already-current session.
            let hash = crate::mcp_provision::token_hash(state.mcp_token.as_str());
            let _ = state
                .store
                .set_session_provisioned(session.id, state.api_base_url.as_deref(), Some(&hash))
                .await;
        }
        return Ok(Json(session_with_spawned(load(&state, id).await?, false)));
    }

    // The underlying tmux session of an external attach is owned by the
    // user, not agentum — if it's gone, there is nothing to respawn (and
    // launching would run TerminalAdapter with the marker flag in argv).
    if is_external(&session) {
        return Err(ApiError::BadRequest(
            "the external tmux session no longer exists on its host".into(),
        ));
    }

    // Older sessions (pre-tilde-expansion fix) may have `~/...` stored
    // in the DB — re-resolve here so they spawn correctly without a
    // migration. New sessions are stored already-expanded by `create`.
    //
    // When the session has an isolated worktree, that takes precedence
    // over `workdir` — `effective_cwd()` encapsulates the precedence
    // so adapters/callers never have to think about it.
    let workdir = match &host.kind {
        HostKind::Local => {
            let workdir = super::util::expand_workdir(session.effective_cwd())?;
            self_heal_local_workdir(&workdir).await?;
            workdir
        }
        HostKind::Ssh { .. } => PathBuf::from(session.effective_cwd()),
    };

    // All launch conventions (YOLO translation, loopback env, Claude hook, MCP
    // wiring, pipe-pane, status flip) live in the shared spawn helper so the
    // harness-engine driver goes through the exact same path.
    if let Err(e) = spawn_agent_into_pane(&state, &session, &host, &target, &workdir).await {
        // Lost a spawn race: the boot revival sweep (or a concurrent /start)
        // can create this pane between the has_session probe above and our
        // spawn, which tmux rejects as a duplicate name. If the pane exists
        // NOW the session is running — report a reattach rather than a 500
        // the client would surface as a dead tab.
        if crate::host_runtime::has_session(&host, &target)
            .await
            .unwrap_or(false)
        {
            return Ok(Json(session_with_spawned(load(&state, id).await?, false)));
        }
        return Err(e);
    }
    Ok(Json(session_with_spawned(load(&state, id).await?, true)))
}

/// Create a session record AND launch its agent into a fresh tmux pane in one
/// call — the programmatic equivalent of `POST /create` then `POST /start`,
/// without the HTTP envelope. Shared with the MCP `agentum_spawn_session` tool
/// so an agent can spawn a *sibling* agent inside agentum through the exact same
/// launch path (YOLO marker translation, loopback `pane_env`, the Claude
/// `--settings` hook, MCP wiring) the interactive routes use — never a parallel
/// reimplementation. Worktree isolation is out of scope here; callers pass an
/// explicit, already-existing `workdir`.
pub(crate) async fn create_and_spawn_session(
    state: &AppState,
    mut new: NewSession,
    host_id: Option<Uuid>,
) -> Result<Session, ApiError> {
    let host_id = host_id.unwrap_or(LOCAL_HOST_ID);
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;

    // Resolve + validate the workdir exactly as `create`/`start` do (a fresh
    // spawn has no worktree to self-heal, so a missing dir is a hard error).
    let workdir = match &host.kind {
        HostKind::Local => {
            let workdir = super::util::expand_workdir(&new.workdir)?;
            if !workdir.exists() {
                return Err(ApiError::BadRequest(format!(
                    "workdir does not exist: {}",
                    workdir.display()
                )));
            }
            new.workdir = workdir.to_string_lossy().into_owned();
            workdir
        }
        HostKind::Ssh { .. } => PathBuf::from(new.workdir.trim()),
    };

    let session = state
        .store
        .create_session_on_host(new, Some(host_id))
        .await?;
    // A freshly-created agentum session always derives its target from the name;
    // there is no pre-existing pane to reattach to (that's the `start` route's job).
    let target = agentum_tmux::target_for(&session.name);
    spawn_agent_into_pane(state, &session, &host, &target, &workdir).await?;
    load(state, session.id).await
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    Ok(Json(stop_session_core(&state, id, false).await?))
}

async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    Ok(Json(stop_session_core(&state, id, true).await?))
}

/// Shared core of the `stop`/`kill` routes AND the MCP `agentum_stop_session`
/// tool — the lifecycle END that `agentum_spawn_session` was missing (#378).
/// `force_kill` picks kill over graceful stop for agentum-owned panes; an
/// external (user-owned) tmux session is only ever detached, never destroyed,
/// regardless of mode.
pub(crate) async fn stop_session_core(
    state: &AppState,
    id: Uuid,
    force_kill: bool,
) -> Result<Session, ApiError> {
    let session = load(state, id).await?;
    // Stop live transcript work before host lookup or graceful tmux shutdown;
    // either can fail or wait, and a later running read can reattach if needed.
    state.transcripts.stop_observing(id);
    #[cfg(test)]
    state.transcripts.pause_after_early_route_retirement().await;
    let host = load_host_for_session(state, &session).await?;
    let target = tmux_target(&session);
    if is_external(&session) {
        // Detach only: the tmux session belongs to the user. Disarm the
        // log pipe and keep the target so a later start can reattach.
        unpipe_external(state, &host, &session, &target).await;
        state
            .store
            .update_status_and_target(id, Status::Stopped, Some(&target))
            .await?;
    } else {
        if force_kill {
            crate::host_runtime::kill_session(&host, &target)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        } else {
            crate::host_runtime::graceful_stop(&host, &target, GRACEFUL_STOP_TIMEOUT)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        }
        state
            .store
            .update_status_and_target(id, Status::Stopped, None)
            .await?;
    }
    // Keep the early retirement above for prompt teardown, then close the
    // window in which a concurrent read could observe the still-Running row
    // and reattach. The durable Stopped commit is authoritative.
    state.transcripts.stop_observing(id);
    state.hook_tokens.lock().unwrap().remove(&id);
    emit_stopped(state, &session, if force_kill { "kill" } else { "stop" }).await;
    load(state, id).await
}

/// Persist + broadcast a `session.stopped` event so the UI gets a benign
/// "stopped" toast instead of letting the watchdog notice the dead pane on
/// its next tick and fire `session.crashed`. The watchdog itself sees the
/// `Stopped` status and bows out silently.
async fn emit_stopped(state: &AppState, session: &Session, reason: &str) {
    let ev = Event::new("session.stopped")
        .with_session(session.id, &session.name)
        .with_payload(serde_json::json!({"reason": reason}));
    if let Err(e) = state.store.insert_event(&ev).await {
        tracing::warn!(error = ?e, "could not persist session.stopped event");
    }
    // send() returns Err only when there are zero subscribers, which is
    // fine — the event is already in the persisted log.
    let _ = state.bus.send(ev);
}

async fn load(state: &AppState, id: Uuid) -> Result<Session, ApiError> {
    state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))
}

pub(crate) async fn load_host_for_session(
    state: &AppState,
    session: &Session,
) -> Result<Host, ApiError> {
    let host_id = session.host_id.unwrap_or(LOCAL_HOST_ID);
    state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("session host is missing: {host_id}")))
}

fn tmux_target(session: &Session) -> String {
    session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name))
}

/// True when this record was created by attaching to a pre-existing
/// (non-agentum) tmux session. The whole lifecycle must be
/// non-destructive for these: never kill the tmux session, never respawn
/// it — only arm/disarm the stream.
fn is_external(session: &Session) -> bool {
    session.flags.iter().any(|f| f == EXTERNAL_TMUX_FLAG)
}

/// True when another session row on the same host resolves to the same tmux
/// target — i.e. the pane's single `pipe-pane` slot belongs to a sibling, not
/// to `session`. External detach paths must then SKIP `unpipe_pane`: a pane
/// has exactly one pipe, so disarming it here silently freezes the sibling's
/// stream (no output, keystrokes never echo) with no self-heal on local hosts.
/// A store error reads as "shared" — wrongly skipping a disarm just leaves a
/// log appending, while wrongly disarming kills a live session.
pub(crate) fn pane_shared_with_sibling(session: &Session, all: &[Session]) -> bool {
    let target = tmux_target(session);
    let host = session.host_id.unwrap_or(LOCAL_HOST_ID);
    all.iter().any(|s| {
        s.id != session.id && s.host_id.unwrap_or(LOCAL_HOST_ID) == host && tmux_target(s) == target
    })
}

/// Disarm the pane→log pipe for an external binding, unless the pane is
/// shared with another session row (see [`pane_shared_with_sibling`]).
async fn unpipe_external(state: &AppState, host: &Host, session: &Session, target: &str) {
    let shared = match state.store.list_sessions(None).await {
        Ok(all) => pane_shared_with_sibling(session, &all),
        Err(e) => {
            tracing::warn!(session = %session.id, error = ?e,
                "sibling check failed; skipping unpipe to protect a possibly-shared pane");
            true
        }
    };
    if !shared {
        let _ = crate::host_runtime::unpipe_pane(host, target).await;
    }
}

// ---------- /send ----------

#[derive(Deserialize)]
struct SendBody {
    /// Free-form text typed into the pane. Conceptually equivalent to a user typing.
    #[serde(default)]
    text: Option<String>,
    /// Raw tmux key spec (e.g. `C-c`, `Enter`, `M-x`). Sent literally.
    #[serde(default)]
    keys: Option<String>,
    /// Append a tmux `Enter` after the payload — useful for chat-style inputs.
    #[serde(default)]
    append_enter: bool,
}

async fn send(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let host = load_host_for_session(&state, &session).await?;
    let target = session
        .tmux_target
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("session is not running".into()))?;

    let payload = body
        .text
        .as_deref()
        .or(body.keys.as_deref())
        .ok_or_else(|| ApiError::BadRequest("must provide `text` or `keys`".into()))?;

    if !crate::host_runtime::has_session(&host, target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::BadRequest(
            "tmux session not active for this session".into(),
        ));
    }

    crate::host_runtime::send_keys(&host, target, payload, body.append_enter)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------- /submit ----------

#[derive(Deserialize)]
struct SubmitBody {
    /// The prompt to deliver to the agent's REPL and submit.
    text: String,
}

/// `POST /api/sessions/{id}/submit` — deliver a prompt to a RUNNING agent the robust
/// way and submit it. Unlike `/send` (a single `send-keys`, which a modern REPL
/// collapses into a swallowed "[Pasted text]" block for multi-line input), this reuses
/// the harness's `inject_prompt`: wait for the REPL to be idle, type the body, then
/// send a SEPARATE Enter after a settle delay so the turn actually executes.
///
/// This is what the browser-annotation "Send to an agent" uses so it reaches ANY
/// session running on a worktree — including tmux/MCP-spawned agents the desktop never
/// opened as terminal tabs. We validate the session up front (404 / 400) then deliver
/// in the BACKGROUND: a busy agent can take tens of seconds to idle, and we must not
/// block the HTTP response on that — the prompt is queued and lands when the REPL frees.
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SubmitBody>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    submit_prompt_core(&state, session, body.text).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Shared core of the `/submit` route AND the MCP `agentum_inject_prompt` tool
/// (#378 — the push channel; the mailbox is pull-only): validate the session is
/// live up front, then deliver in the BACKGROUND. Errors after the checks are
/// logged, not surfaced — a busy agent can take tens of seconds to idle, and
/// neither an HTTP response nor an MCP call should block on that.
pub(crate) async fn submit_prompt_core(
    state: &AppState,
    session: Session,
    text: String,
) -> Result<(), ApiError> {
    let host = load_host_for_session(state, &session).await?;
    let target = session
        .tmux_target
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("session is not running".into()))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::BadRequest("`text` must not be empty".into()));
    }
    if !crate::host_runtime::has_session(&host, target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::BadRequest(
            "tmux session not active for this session".into(),
        ));
    }
    // Robust two-step delivery (see the `/submit` doc comment).
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::harness::inject_prompt(&state, &session, &text).await {
            tracing::warn!(target: "agentum::sessions::submit", error = %e, "submit delivery failed");
        }
    });
    Ok(())
}

// ---------- WS /stream ----------

#[derive(Deserialize, Default)]
struct StreamQuery {
    /// `?resume=true` tells the WS handler the client has cached vt100
    /// parser state for this session and just wants the missed log
    /// delta — not a fresh `capture-pane` snapshot. Travels in the URL
    /// because old daemons silently drop unknown query params; if it
    /// were a wire frame they'd forward it to `tmux send-keys` as a
    /// keystroke (the v0.6.20 regression).
    #[serde(default)]
    resume: bool,
    /// `?redraw=true` asks the handler to force the embedded TUI to fully
    /// REPAINT before snapshotting — not just re-read the current grid.
    /// Needed when foreign bytes were written straight into the pane's
    /// screen, bypassing the agent's own rendering: most notably an OS
    /// `wall` broadcast (systemd's "The system will suspend now!" notice,
    /// sent as root so `mesg n` can't block it) that lands on top of the
    /// input box / footer and stays there, because a ratatui app only
    /// repaints the cells it thinks changed. A plain `capture-pane`
    /// re-snapshot can't heal that — it re-reads the same corrupted grid.
    /// Only the agent can repaint its own cells, and a SIGWINCH is what
    /// makes it clear its buffer and redraw in full; we provoke one with a
    /// momentary resize toggle. Same URL-param rationale as `resume`: old
    /// daemons drop it and fall back to the plain snapshot — safe, just
    /// not self-healing.
    #[serde(default)]
    redraw: bool,
}

async fn stream(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let host = load_host_for_session(&state, &session).await?;
    let target = tmux_target(&session);
    let positions = state.stream_positions.clone();
    let resume = q.resume;
    let redraw = q.redraw;
    Ok(ws.on_upgrade(move |socket| async move {
        if matches!(host.kind, HostKind::Local) {
            stream_session(socket, id, target, positions, resume, redraw).await;
        } else {
            stream_remote_session(socket, host, id, target, redraw).await;
        }
    }))
}

const BACKFILL_BYTES: u64 = 4096;
const READ_CHUNK: usize = 8192;

/// How long the WS handler waits for the client's first `{"resize":...}` text
/// frame before falling back to capturing at tmux's current pane size. Modern
/// clients send this within milliseconds of `onopen` / first frame, so the
/// wait virtually never reaches the timeout. The cap exists so legacy
/// clients (or a stalled connection) don't block the connect indefinitely.
const INITIAL_RESIZE_WAIT: Duration = Duration::from_millis(250);

/// Hard cap on how long we wait for the embedded process's post-SIGWINCH
/// repaint burst to settle before snapshotting. Reached by genuinely
/// active streams (claude code mid-response) — at that point the
/// snapshot inevitably reflects mid-stream content, and the live tail
/// will continue painting fresh bytes after.
const POST_RESIZE_SETTLE_MAX: Duration = Duration::from_millis(400);

/// Polling cadence for the post-resize quiet check. Two consecutive
/// no-growth intervals (≈80 ms total) classifies the embedded TUI as
/// "done repainting".
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(40);

/// If we resized but see no log activity at all within this window, the
/// embedded process probably isn't going to react (size already matched,
/// or it's blocked on something). Bail rather than burning the full
/// max-budget on a no-op wait. Long enough that a slow ratatui app has
/// time to start emitting bytes.
const POST_RESIZE_NO_ACTIVITY_BAIL: Duration = Duration::from_millis(180);

/// Upper bound on a single coalesced WS frame. Caps the work of merging a large
/// backlog and keeps any one `term.write` on the client bounded. Consumed by
/// `coalesce_queued` in the [`streaming`] submodule (via `use super::*`).
const COALESCE_MAX: usize = 256 * 1024;

// ---------- GET /api/sessions/{id}/pane ----------

/// Returns a plain-text snapshot of the last N lines of the session's tmux
/// pane viewport. Contract: UI-SPEC §Component Inventory — polled every 2 s
/// by the Bound-session panel (plan 02-05) to display live agent output.
///
/// Bearer-auth is inherited automatically via the top-level `require_token`
/// middleware merge in `lib.rs`; no entry in `auth::is_public` is added.
async fn pane(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PaneQuery>,
) -> Result<Json<PaneSnapshot>, ApiError> {
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("session {id}")))?;
    let host = load_host_for_session(&state, &session).await?;

    let n = clamp_lines(q.lines);

    let captured_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let lines: Vec<String> = match session.tmux_target.as_deref() {
        Some(target) => {
            let text = crate::host_runtime::capture_pane_visible(&host, target)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            // Lines reversed/truncated/un-reversed so the last N lines preserve chronological order.
            let collected: Vec<&str> = text.lines().rev().take(n).collect();
            collected.into_iter().rev().map(|s| s.to_string()).collect()
        }
        // Idle / stopped session: no tmux pane to capture (UI-SPEC §empty state).
        None => Vec::new(),
    };

    Ok(Json(PaneSnapshot { lines, captured_at }))
}

/// Insert / overwrite the resume checkpoint for `id`. Called at two
/// points: every successful byte-forward (so a concurrent reconnect's
/// resume delta reflects the bytes the client actually received), and
/// on disconnect (final safety net for the rare case where the loop
/// exits between forwards). Stamping during the loop is what closes
/// the original race — switching A→B→A in the TUI used to land the
/// new connection before the old one wrote its only checkpoint, the
/// new task read None, fell through to a fresh RIS + capture-pane
/// snapshot, and wiped the TUI's restored cached parser. Sizes
/// default to 0×0 when the client never sent a resize — the resume
/// path's size-match gate then forces a fresh snapshot, which is the
/// safe choice for legacy clients.
fn save_checkpoint(
    positions: &Arc<std::sync::Mutex<std::collections::HashMap<Uuid, StreamCheckpoint>>>,
    id: Uuid,
    pos: u64,
    size: Option<(u16, u16)>,
) {
    if let Ok(mut map) = positions.lock() {
        let (cols, rows) = size.unwrap_or((0, 0));
        map.insert(id, StreamCheckpoint { pos, cols, rows });
    }
}

// ---------- /hook ----------

#[derive(Deserialize)]
struct HookBody {
    kind: String,
    #[serde(default)]
    payload: serde_json::Value,
}

/// POST /api/sessions/{id}/hook
///
/// Unauthenticated endpoint (no bearer token needed). Agents validate via the
/// `X-Agentum-Hook-Token` header using the ephemeral token injected at launch
/// time. On success, emits an `agent.hook` event on the broadcast bus so the
/// dashboard/TUI can react in real-time.
async fn hook(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<HookBody>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;

    // 404 before revealing whether the token is good or bad.
    let session = state
        .store
        .get_session_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;

    let provided = headers
        .get("x-agentum-hook-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let valid = {
        let map = state.hook_tokens.lock().unwrap();
        map.get(&id).map(|t| t == provided).unwrap_or(false)
    };

    if !valid {
        return Err(ApiError::Unauthorized("invalid hook token".into()));
    }

    let ev = Event::new("agent.hook")
        .with_session(session.id, &session.name)
        .with_payload(serde_json::json!({
            "kind": body.kind,
            "payload": body.payload,
        }));
    let _ = state.bus.send(ev);

    Ok(StatusCode::NO_CONTENT)
}

/// Recognise `{"resize":{"cols":N,"rows":N}}` text frames. Returns the
/// `(cols, rows)` pair on a hit; `None` for any other shape (the caller
/// then treats the frame as raw input bytes for backward compatibility).
fn parse_resize(t: &str) -> Option<(u16, u16)> {
    let trimmed = t.trim();
    if !trimmed.starts_with('{') || !trimmed.contains("resize") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let resize = v.get("resize")?;
    let cols = resize.get("cols")?.as_u64()?;
    let rows = resize.get("rows")?.as_u64()?;
    Some((
        cols.min(u16::MAX as u64) as u16,
        rows.min(u16::MAX as u64) as u16,
    ))
}

/// Recognise `{"refresh":true}` text frames. Clients send this once
/// shortly after WS open (after the xterm fit settles) to request a
/// fresh `ESC c + capture-pane -e` payload — the initial snapshot is
/// sometimes captured mid-repaint or before the client's final size
/// is known, leaving scrollback corruption that only a re-snapshot
/// reliably clears. Older daemons (lacking the `"refresh"` health
/// capability) forward unknown text frames to `tmux send-keys`, so
/// clients MUST capability-gate the send.
fn parse_refresh(t: &str) -> bool {
    let trimmed = t.trim();
    if !trimmed.starts_with('{') || !trimmed.contains("refresh") {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return false;
    };
    v.get("refresh").and_then(|x| x.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{parse_refresh, parse_resize};

    #[test]
    fn parse_resize_recognises_envelope() {
        assert_eq!(
            parse_resize(r#"{"resize":{"cols":120,"rows":40}}"#),
            Some((120, 40))
        );
    }

    #[test]
    fn parse_resize_ignores_other_text() {
        assert_eq!(parse_resize("hello"), None);
        assert_eq!(parse_resize(r#"{"send":"x"}"#), None);
        assert_eq!(parse_resize(""), None);
    }

    #[test]
    fn parse_refresh_recognises_envelope() {
        assert!(parse_refresh(r#"{"refresh":true}"#));
        assert!(parse_refresh(r#"{ "refresh": true }"#));
    }

    #[test]
    fn parse_refresh_rejects_falsy_and_other_text() {
        assert!(!parse_refresh(r#"{"refresh":false}"#));
        assert!(!parse_refresh(r#"{"refresh":"yes"}"#));
        assert!(!parse_refresh("hello"));
        assert!(!parse_refresh(r#"{"resize":{"cols":80,"rows":24}}"#));
        assert!(!parse_refresh(""));
    }

    // ---- shared-pane guard (issue #244) ----

    use super::pane_shared_with_sibling;
    use agentum_core::Session;
    use uuid::Uuid;

    /// Minimal session for the pure guard; built through serde so the helper
    /// stays valid as optional fields are added to the struct.
    fn guard_sess(name: &str, target: Option<&str>, host: Option<&str>) -> Session {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(),
            "name": name,
            "workdir": "/tmp",
            "tool": "terminal",
            "model": null,
            "flags": [],
            "status": "running",
            "tmux_target": target,
            "host_id": host,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "last_activity_at": null,
        }))
        .unwrap()
    }

    #[test]
    fn shared_pane_detected_across_stored_and_derived_targets() {
        // `alpha`'s derived target (`agentum-alpha`) matches the external
        // binding's stored one — the pane is shared, so unpipe must be skipped.
        let owner = guard_sess("alpha", None, None);
        let ext = guard_sess("agentum-alpha-view", Some("agentum-alpha"), None);
        let all = vec![owner, ext.clone()];
        assert!(pane_shared_with_sibling(&ext, &all));
    }

    #[test]
    fn unshared_pane_and_cross_host_pane_are_not_flagged() {
        let owner = guard_sess("alpha", Some("agentum-alpha"), None);
        let solo = guard_sess("my-tmux", Some("my-tmux"), None);
        let other_host = guard_sess(
            "agentum-alpha-remote",
            Some("agentum-alpha"),
            Some("4bfb2ccf-cdd0-4a82-8793-5d87906da5e0"),
        );
        let all = vec![owner, solo.clone(), other_host.clone()];
        assert!(!pane_shared_with_sibling(&solo, &all));
        // Same target string on a different host is a different pane.
        assert!(!pane_shared_with_sibling(&other_host, &all));
    }

    // ---- pane snapshot tests ----

    mod pane_tests {
        use super::super::*;
        use agentum_core::NewSession;
        use agentum_store::Store;
        use axum::extract::{Path, Query, State};
        use std::sync::Arc;
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
                clipboard_pending: Arc::new(
                    std::sync::Mutex::new(std::collections::HashMap::new()),
                ),
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

        async fn create_test_session(state: &AppState) -> agentum_core::Session {
            let dir = tempfile::tempdir().unwrap();
            let workdir = dir.path().to_string_lossy().to_string();
            std::mem::forget(dir);
            state
                .store
                .create_session(NewSession {
                    name: "test-session".into(),
                    workdir,
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

        // Test 2 + 3: pure unit tests for clamp_lines — no HTTP, no DB.
        #[test]
        fn pane_clamp_upper_bound() {
            // lines=500 must clamp to 200 (UI-SPEC §Component Inventory max).
            assert_eq!(clamp_lines(Some(500)), 200);
        }

        #[test]
        fn pane_clamp_lower_bound_and_default() {
            // lines=0 clamps to 1; absent → default 20.
            assert_eq!(clamp_lines(Some(0)), 1);
            assert_eq!(clamp_lines(None), 20);
        }

        // Test 4: idle session (tmux_target = None) → HTTP 200 with empty lines.
        #[tokio::test]
        async fn pane_idle_session_returns_empty_lines() {
            let state = fresh_state().await;
            let sess = create_test_session(&state).await;
            // Session is idle — tmux_target is None.
            assert!(sess.tmux_target.is_none());

            let res = pane(
                State(state),
                Path(sess.id),
                Query(PaneQuery { lines: None }),
            )
            .await;

            let snap = res.expect("idle session must return Ok").0;
            assert!(snap.lines.is_empty(), "idle session: lines must be empty");
            assert!(!snap.captured_at.is_empty(), "captured_at must be set");
            // Must be a valid RFC3339 timestamp.
            assert!(
                snap.captured_at.contains('T'),
                "captured_at must contain T separator"
            );
        }

        // Test 5: non-existent session UUID → 404.
        #[tokio::test]
        async fn pane_nonexistent_session_returns_404() {
            let state = fresh_state().await;
            let missing_id = uuid::Uuid::new_v4();

            let res = pane(
                State(state),
                Path(missing_id),
                Query(PaneQuery { lines: Some(5) }),
            )
            .await;

            match res {
                Err(ApiError::NotFound(_)) => {}
                other => panic!("expected NotFound, got {:?}", other),
            }
        }

        // Test 1 / shape: pane returns the correct JSON shape.
        // This test re-uses the idle path (no tmux in tests) to verify the
        // shape — the lines vec is empty but `lines` + `captured_at` fields
        // are present and well-typed.
        #[tokio::test]
        async fn pane_response_shape_is_correct() {
            let state = fresh_state().await;
            let sess = create_test_session(&state).await;

            let res = pane(
                State(state),
                Path(sess.id),
                Query(PaneQuery { lines: Some(20) }),
            )
            .await;

            let snap = res.expect("must return Ok").0;
            // Shape: Vec<String> + String.
            let _: Vec<String> = snap.lines;
            let _: String = snap.captured_at;
        }
    }

    // ---- hook endpoint tests ----

    mod hook_tests {
        use super::super::*;
        use agentum_core::NewSession;
        use agentum_store::Store;
        use axum::extract::{Path, State};
        use axum::http::HeaderMap;
        use std::sync::Arc;
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
                clipboard_pending: Arc::new(
                    std::sync::Mutex::new(std::collections::HashMap::new()),
                ),
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

        async fn make_session(state: &AppState) -> agentum_core::Session {
            let dir = tempfile::tempdir().unwrap();
            let workdir = dir.path().to_string_lossy().to_string();
            std::mem::forget(dir);
            state
                .store
                .create_session(NewSession {
                    name: "hook-test".into(),
                    workdir,
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
        async fn hook_unknown_session_returns_404() {
            let state = fresh_state().await;
            let unknown_id = uuid::Uuid::new_v4().to_string();
            let mut hdrs = HeaderMap::new();
            hdrs.insert("x-agentum-hook-token", "anytoken".parse().unwrap());
            let res = hook(
                State(state),
                Path(unknown_id),
                hdrs,
                Json(HookBody {
                    kind: "tool_done".into(),
                    payload: serde_json::Value::Null,
                }),
            )
            .await;
            assert!(
                matches!(res, Err(ApiError::NotFound(_))),
                "unknown session must 404"
            );
        }

        #[tokio::test]
        async fn hook_bad_token_returns_401() {
            let state = fresh_state().await;
            let sess = make_session(&state).await;
            // Insert a known token but present a different one.
            state
                .hook_tokens
                .lock()
                .unwrap()
                .insert(sess.id, "correct-token".into());
            let mut hdrs = HeaderMap::new();
            hdrs.insert("x-agentum-hook-token", "wrong-token".parse().unwrap());
            let res = hook(
                State(state),
                Path(sess.id.to_string()),
                hdrs,
                Json(HookBody {
                    kind: "tool_done".into(),
                    payload: serde_json::Value::Null,
                }),
            )
            .await;
            assert!(
                matches!(res, Err(ApiError::Unauthorized(_))),
                "wrong token must 401"
            );
        }

        #[tokio::test]
        async fn hook_good_token_returns_204_and_emits_event() {
            let state = fresh_state().await;
            let mut rx = state.bus.subscribe();
            let sess = make_session(&state).await;
            let token = "valid-token-abc123".to_string();
            state
                .hook_tokens
                .lock()
                .unwrap()
                .insert(sess.id, token.clone());
            let mut hdrs = HeaderMap::new();
            hdrs.insert("x-agentum-hook-token", token.parse().unwrap());
            let res = hook(
                State(state),
                Path(sess.id.to_string()),
                hdrs,
                Json(HookBody {
                    kind: "tool_done".into(),
                    payload: serde_json::json!({"tool": "bash"}),
                }),
            )
            .await;
            assert!(
                matches!(res, Ok(axum::http::StatusCode::NO_CONTENT)),
                "valid token must 204"
            );
            let ev = rx.try_recv().expect("event must be on bus");
            assert_eq!(ev.kind, "agent.hook");
            assert_eq!(ev.session_id, Some(sess.id));
            assert_eq!(ev.payload["kind"], "tool_done");
        }
    }

    mod transcript_lifecycle_tests {
        use super::super::*;
        use agentum_core::NewSession;
        use agentum_store::Store;
        use axum::extract::{Path, Query, State};
        use tokio::sync::broadcast;

        async fn state_with_counting_transcripts() -> (
            tempfile::TempDir,
            AppState,
            crate::transcript_store::ObserverCounts,
        ) {
            let root = tempfile::tempdir().unwrap();
            let store = Store::open(&root.path().join("sessions.sqlite"))
                .await
                .unwrap();
            let (bus, _) = broadcast::channel(32);
            let mut state = AppState::new(store, bus.clone());
            let (transcripts, counts) = crate::TranscriptStore::with_counting_factory(bus);
            state.transcripts = transcripts;
            (root, state, counts)
        }

        async fn observed_claude_session(
            state: &AppState,
            root: &tempfile::TempDir,
            name: &str,
        ) -> Session {
            let workdir = root.path().join(name);
            std::fs::create_dir_all(&workdir).unwrap();
            let session = state
                .store
                .create_session(NewSession {
                    name: name.into(),
                    workdir: workdir.to_string_lossy().into_owned(),
                    tool: "claude".into(),
                    model: None,
                    flags: vec![],
                    card_id: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_base_ref: None,
                })
                .await
                .unwrap();
            state.transcripts.read(
                session.id,
                workdir,
                "claude",
                crate::transcript_store::ObservationMode::Live,
            );
            session
        }

        #[tokio::test]
        async fn listing_500_sessions_creates_zero_transcript_entries_or_observers() {
            let (root, state, counts) = state_with_counting_transcripts().await;
            let workdir = root.path().join("workspace");
            std::fs::create_dir_all(&workdir).unwrap();
            let transcript_dir = agentum_core::transcript::project_dir_for(&workdir).unwrap();
            assert!(!transcript_dir.exists());
            for index in 0..500 {
                state
                    .store
                    .create_session(NewSession {
                        name: format!("history-{index}"),
                        workdir: workdir.to_string_lossy().into_owned(),
                        tool: if index % 2 == 0 { "claude" } else { "codex" }.into(),
                        model: None,
                        flags: vec![],
                        card_id: None,
                        worktree_path: None,
                        worktree_branch: None,
                        worktree_base_ref: None,
                    })
                    .await
                    .unwrap();
            }

            let response = list(State(state.clone()), Query(ListQuery { status: None }))
                .await
                .unwrap();
            assert_eq!(response.0.len(), 500);
            assert_eq!(counts.created(), 0);
            assert_eq!(counts.dropped(), 0);
            assert_eq!(state.transcripts.cache_count(), 0);
            assert!(!transcript_dir.exists());
        }

        #[tokio::test]
        async fn patching_away_from_claude_retires_live_observation() {
            let (root, state, counts) = state_with_counting_transcripts().await;
            let workdir = root.path().join("workspace");
            std::fs::create_dir_all(&workdir).unwrap();
            let session = state
                .store
                .create_session(NewSession {
                    name: "tool-change".into(),
                    workdir: workdir.to_string_lossy().into_owned(),
                    tool: "claude".into(),
                    model: None,
                    flags: vec![],
                    card_id: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_base_ref: None,
                })
                .await
                .unwrap();
            state.transcripts.read(
                session.id,
                workdir,
                "claude",
                crate::transcript_store::ObservationMode::Live,
            );
            assert_eq!(counts.created(), 1);

            let _ = patch_session(
                State(state.clone()),
                Path(session.id.to_string()),
                Json(PatchBody {
                    name: None,
                    tool: Some("codex".into()),
                    flags: None,
                    model: None,
                    pinned: None,
                }),
            )
            .await
            .unwrap();
            assert_eq!(counts.dropped(), 1);
            assert_eq!(state.transcripts.observing_count(), 0);
            if let Some(path) =
                agentum_core::transcript::project_dir_for(std::path::Path::new(&session.workdir))
            {
                let _ = std::fs::remove_dir_all(path);
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn successful_stop_and_kill_finally_retire_observers_reattached_during_teardown() {
            for force_kill in [false, true] {
                let (root, state, counts) = state_with_counting_transcripts().await;
                let session = observed_claude_session(
                    &state,
                    &root,
                    if force_kill {
                        "route-kill"
                    } else {
                        "route-stop"
                    },
                )
                .await;
                state
                    .store
                    .update_status(session.id, Status::Running)
                    .await
                    .unwrap();
                let mut events = state.bus.subscribe();
                let gate = state.transcripts.park_next_route_retirement();
                let route_state = state.clone();
                let route = tokio::spawn(async move {
                    stop_session_core(&route_state, session.id, force_kill).await
                });

                tokio::time::timeout(std::time::Duration::from_secs(1), gate.wait_until_arrived())
                    .await
                    .expect("route reaches the controlled teardown window");
                assert_eq!(counts.dropped(), 1, "early retirement must remain prompt");
                assert_eq!(state.transcripts.observing_count(), 0);
                let still_running = state
                    .store
                    .get_session_by_id(session.id)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(still_running.status, Status::Running);

                state.transcripts.read(
                    session.id,
                    std::path::PathBuf::from(&still_running.workdir),
                    &still_running.tool,
                    crate::transcript_store::ObservationMode::Live,
                );
                assert_eq!(counts.created(), 2, "live read reattaches during teardown");
                assert_eq!(state.transcripts.observing_count(), 1);
                while events.try_recv().is_ok() {}

                gate.release();
                let stopped = tokio::time::timeout(std::time::Duration::from_secs(2), route)
                    .await
                    .expect("successful teardown completes")
                    .unwrap()
                    .unwrap();
                assert_eq!(stopped.status, Status::Stopped);
                assert_eq!(counts.dropped(), 2, "final retirement drops replacement");
                assert_eq!(state.transcripts.observing_count(), 0);
                assert_eq!(state.transcripts.cache_count(), 1, "stop retains snapshot");

                while events.try_recv().is_ok() {}
                counts.notify(1, 1);
                assert!(
                    events.try_recv().is_err(),
                    "retired callback emitted an event"
                );
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn forced_running_delete_finally_forgets_state_recreated_during_teardown() {
            let (root, state, counts) = state_with_counting_transcripts().await;
            let session = observed_claude_session(&state, &root, "route-force-delete").await;
            state
                .store
                .update_status(session.id, Status::Running)
                .await
                .unwrap();
            let mut events = state.bus.subscribe();
            let gate = state.transcripts.park_next_route_retirement();
            let route_state = state.clone();
            let id = session.id;
            let route = tokio::spawn(async move {
                delete(
                    State(route_state),
                    Path(id.to_string()),
                    Query(DeleteQuery { force: true }),
                )
                .await
            });

            tokio::time::timeout(std::time::Duration::from_secs(1), gate.wait_until_arrived())
                .await
                .expect("delete reaches the controlled teardown window");
            assert_eq!(counts.dropped(), 1, "early forget remains prompt");
            assert_eq!(state.transcripts.cache_count(), 0);
            let still_running = state.store.get_session_by_id(id).await.unwrap().unwrap();
            assert_eq!(still_running.status, Status::Running);

            state.transcripts.read(
                id,
                std::path::PathBuf::from(&still_running.workdir),
                &still_running.tool,
                crate::transcript_store::ObservationMode::Live,
            );
            assert_eq!(
                counts.created(),
                2,
                "live read recreates state during teardown"
            );
            assert_eq!(state.transcripts.cache_count(), 1);
            assert_eq!(state.transcripts.observing_count(), 1);
            while events.try_recv().is_ok() {}

            gate.release();
            let status = tokio::time::timeout(std::time::Duration::from_secs(2), route)
                .await
                .expect("forced delete completes")
                .unwrap()
                .unwrap();
            assert_eq!(status, StatusCode::NO_CONTENT);
            assert_eq!(counts.dropped(), 2, "final forget drops replacement");
            assert_eq!(state.transcripts.observing_count(), 0);
            assert_eq!(state.transcripts.cache_count(), 0);
            assert!(state.store.get_session_by_id(id).await.unwrap().is_none());

            counts.notify(1, 1);
            assert!(
                events.try_recv().is_err(),
                "retired callback emitted an event"
            );
        }
    }
}
