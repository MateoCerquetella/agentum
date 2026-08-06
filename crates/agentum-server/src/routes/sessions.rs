use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use agentum_core::{
    EXTERNAL_TMUX_FLAG, Event, Host, HostKind, LOCAL_HOST_ID, NewSession, Session, Status,
    WorktreeSpec,
};
use agentum_store::paths;
use agentum_tmux::ssh::SshMux;
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

const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_STREAM_CANCELLATION_TIMEOUT: Duration = Duration::from_secs(5);

type RemoteStreamCleanup = Result<(), String>;

/// One long-lived remote WebSocket stream registered against its host. Host
/// PUT/DELETE signals `cancel`; `cleanup` is published only after the stream's
/// SSH tail and input-writer children have both been explicitly killed and
/// reaped. Weak registry entries make normal disconnect cleanup automatic.
struct RemoteStreamControl {
    cancel: tokio::sync::watch::Sender<bool>,
    cleanup: tokio::sync::watch::Sender<Option<RemoteStreamCleanup>>,
}

struct RemoteStreamRegistration {
    control: Arc<RemoteStreamControl>,
    finished: bool,
}

impl RemoteStreamRegistration {
    fn finish(mut self, result: RemoteStreamCleanup) {
        self.control.cleanup.send_replace(Some(result));
        self.finished = true;
    }
}

impl Drop for RemoteStreamRegistration {
    fn drop(&mut self) {
        if !self.finished {
            self.control.cleanup.send_replace(Some(Err(
                "remote stream ended without explicit child cleanup".into(),
            )));
        }
    }
}

fn remote_stream_registry()
-> &'static std::sync::Mutex<HashMap<Uuid, Vec<Weak<RemoteStreamControl>>>> {
    static STREAMS: OnceLock<std::sync::Mutex<HashMap<Uuid, Vec<Weak<RemoteStreamControl>>>>> =
        OnceLock::new();
    STREAMS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Register while the caller holds this host's shared lifecycle lease and
/// before spawning any long-lived SSH child. That ordering guarantees a host
/// PUT can either precede the spawn or observe, cancel and await it; there is no
/// interval where an old-revision child can appear after invalidation unseen.
fn register_remote_stream(
    host_id: Uuid,
) -> (RemoteStreamRegistration, tokio::sync::watch::Receiver<bool>) {
    let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
    let (cleanup, _cleanup_rx) = tokio::sync::watch::channel(None);
    let control = Arc::new(RemoteStreamControl { cancel, cleanup });

    let registry = remote_stream_registry();
    let mut registry = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.retain(|_, streams| {
        streams.retain(|stream| stream.strong_count() > 0);
        !streams.is_empty()
    });
    registry
        .entry(host_id)
        .or_default()
        .push(Arc::downgrade(&control));

    (
        RemoteStreamRegistration {
            control,
            finished: false,
        },
        cancel_rx,
    )
}

/// Cancel every persistent remote stream for `host_id` and wait for explicit
/// child cleanup acknowledgment. The caller MUST already hold the shared host
/// lifecycle lease. Host routes use this before ControlMaster invalidation, so
/// no pre-PUT child can attach late and recreate the retired namespace.
pub(crate) async fn cancel_remote_streams_for_host(host_id: Uuid) -> Result<(), ApiError> {
    let controls: Vec<Arc<RemoteStreamControl>> = {
        let registry = remote_stream_registry();
        let mut registry = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let controls = registry
            .get_mut(&host_id)
            .map(|streams| {
                streams.retain(|stream| stream.strong_count() > 0);
                streams.iter().filter_map(Weak::upgrade).collect()
            })
            .unwrap_or_default();
        registry.retain(|_, streams| !streams.is_empty());
        controls
    };

    for control in &controls {
        control.cancel.send_replace(true);
    }

    let deadline = tokio::time::Instant::now() + REMOTE_STREAM_CANCELLATION_TIMEOUT;
    for control in controls {
        let mut cleanup = control.cleanup.subscribe();
        let result = tokio::time::timeout_at(deadline, async {
            loop {
                if let Some(result) = cleanup.borrow().clone() {
                    return result;
                }
                cleanup.changed().await.map_err(|_| {
                    "remote stream cleanup channel closed before acknowledgment".to_string()
                })?;
            }
        })
        .await
        .map_err(|_| {
            ApiError::Conflict(format!(
                "timed out cleaning up active SSH streams for host {host_id}; host mutation cancelled"
            ))
        })?;
        result.map_err(|error| {
            ApiError::Conflict(format!(
                "could not clean up an active SSH stream for host {host_id}: {error}; host mutation cancelled"
            ))
        })?;
    }
    Ok(())
}

/// Serialize every lifecycle mutation for one session. The lock covers the
/// complete store read → tmux mutation → store write transaction for start,
/// startup resume, stop, kill and delete. Weak entries keep this process-wide
/// registry from growing with deleted sessions while avoiding an `AppState`
/// field that every test fixture would need to duplicate.
fn session_lifecycle_lock(id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<Uuid, Weak<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let locks = LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(id, Arc::downgrade(&lock));
    lock
}

/// Short global hand-off gate used only while a caller resolves/acquires its
/// per-session lock. `create_and_spawn_session` holds it across the database
/// insert (before the UUID exists) and hands off directly to the new session's
/// lock. Other lifecycle routes take the gate before their per-session lock, so
/// none can slip into the tiny insert → lock-registration window. The gate is
/// dropped before any tmux/network work, preserving concurrency across sessions.
fn lifecycle_lock_handoff() -> &'static tokio::sync::Mutex<()> {
    static HANDOFF: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    HANDOFF.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub(crate) async fn acquire_session_lifecycle(id: Uuid) -> tokio::sync::OwnedMutexGuard<()> {
    let handoff = lifecycle_lock_handoff().lock().await;
    let lock = session_lifecycle_lock(id);
    drop(handoff);
    lock.lock_owned().await
}

pub(crate) async fn acquire_host_lifecycle(id: Uuid) -> tokio::sync::OwnedMutexGuard<()> {
    agentum_tmux::ssh::acquire_host_lifecycle(id).await
}

/// Resolve the immutable host binding before taking leases, then acquire them
/// in the process-wide canonical host → session order. The session is always
/// reloaded by the locked implementation, so a concurrent delete between the
/// preliminary lookup and lease acquisition resolves to a normal 404.
pub(crate) async fn acquire_host_and_session_lifecycle(
    state: &AppState,
    id: Uuid,
) -> Result<
    (
        tokio::sync::OwnedMutexGuard<()>,
        tokio::sync::OwnedMutexGuard<()>,
    ),
    ApiError,
> {
    let session = load(state, id).await?;
    let host_id = session.host_id.unwrap_or(LOCAL_HOST_ID);
    let host_guard = acquire_host_lifecycle(host_id).await;
    let session_guard = acquire_session_lifecycle(id).await;
    Ok((host_guard, session_guard))
}

fn tool_consumes_agentum_mcp(tool: &str) -> bool {
    crate::mcp_provision::tool_supports_mcp(tool)
        || crate::mcp_provision::agent_mcp_file(tool).is_some()
}

pub(crate) fn managed_session_consumes_agentum_mcp(session: &Session) -> bool {
    !is_external(session) && tool_consumes_agentum_mcp(&session.tool)
}

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
    // Lazy-start a transcript watcher per known session so plan/todo
    // updates stream live. `ensure_started` is idempotent — calling it
    // for sessions that already have a watcher is cheap.
    for s in &rows {
        state
            .transcripts
            .ensure_started(s.id, PathBuf::from(&s.workdir), &s.tool);
    }
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
    let _host_guard = acquire_host_lifecycle(host_id).await;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;

    let workdir = match &host.kind {
        HostKind::Local => {
            let workdir = super::util::expand_workdir(&new.workdir)?;
            if !workdir.exists() {
                // Self-heal a desktop git-worktree workspace
                // (`<repo>/.claude/worktrees/<name>`) whose directory went
                // missing (pruned out-of-band, removed by hand, or a registry
                // row that outlived its checkout). A hard 400 here breaks
                // branch-compare and every git/terminal op that opens the
                // worktree, so recreate it from the intact parent repo instead.
                // Only the worktrees layout is auto-created; any other missing
                // path stays a hard error.
                match worktree_repo_for_missing(&workdir) {
                    Some(repo) => {
                        crate::git::recreate_worktree(&repo, &workdir)
                            .await
                            .map_err(|e| {
                                ApiError::BadRequest(format!(
                                    "workdir does not exist and could not be recreated: {} ({e})",
                                    workdir.display()
                                ))
                            })?;
                    }
                    None => {
                        return Err(ApiError::BadRequest(format!(
                            "workdir does not exist: {}",
                            workdir.display()
                        )));
                    }
                }
            }
            new.workdir = workdir.to_string_lossy().into_owned();
            workdir
        }
        HostKind::Ssh { .. } => {
            if worktree.is_some() {
                return Err(ApiError::BadRequest(
                    "worktree isolation is not available on SSH hosts yet".into(),
                ));
            }
            // Resolve `~`/relative paths and validate the selected tool against
            // the actual SSH target before persisting. Session creation has no
            // UUID yet, so the nil id is used only for the irrelevant Claude
            // transcript probe; start performs the same preflight with the real
            // id before any tunnel/tmux mutation.
            let requested = PathBuf::from(new.workdir.trim());
            let preflight = crate::host_runtime::preflight_remote_launch(
                &host,
                &requested,
                &new.tool,
                Uuid::nil(),
            )
            .await
            .map_err(|e| ApiError::from_host_runtime(&host, e))?;
            new.workdir = preflight.workdir.to_string_lossy().into_owned();
            preflight.workdir
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
    let _guard = acquire_session_lifecycle(id).await;
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
    delete_session_by_id_with_force(&state, id, q.force).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Canonical force-delete used by internal rollback paths. It shares the same
/// host/session leases, remote ownership checks and credential cleanup as the
/// HTTP route.
pub(crate) async fn delete_session_by_id(state: &AppState, id: Uuid) -> Result<(), ApiError> {
    delete_session_by_id_with_force(state, id, true).await
}

async fn delete_session_by_id_with_force(
    state: &AppState,
    id: Uuid,
    force: bool,
) -> Result<(), ApiError> {
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(state, id).await?;
    delete_session_locked(state, id, force).await
}

async fn delete_session_locked(state: &AppState, id: Uuid, force: bool) -> Result<(), ApiError> {
    let session = load(state, id).await?;

    // Guard a genuinely-running agent behind ?force=true so it isn't torn
    // down by accident. Any other status (Idle/Crashed/Stopped) deletes freely.
    if matches!(session.status, Status::Running) && !force {
        return Err(ApiError::BadRequest(
            "session is running; pass ?force=true to kill and remove".into(),
        ));
    }

    // Best-effort tmux teardown, then always remove the record. Two things this
    // must NOT do — both previously surfaced as "can't delete the session":
    //   1. Gate teardown on `Running` only. The recorded status lags reality —
    //      a session resting in `Idle` (an agent awaiting input) still owns a
    //      live pane, so the old `Running`-only gate orphaned it. We tear down
    //      any still-owned target now; `kill_session` is idempotent when gone.
    //   2. Let an incidental teardown failure block record removal. An
    //      unreachable/deleted host must not pin the row forever. A live SSH
    //      ownership conflict is the deliberate exception: keep the exact DB
    //      binding and return 409 rather than orphaning a pane we refused to
    //      touch.
    match load_host_for_session(state, &session).await {
        Ok(host) => {
            let target = tmux_target(&session);
            let mut control_target = target.clone();
            let may_teardown = if !is_external(&session)
                && matches!(host.kind, HostKind::Ssh { .. })
            {
                match crate::host_runtime::has_session(&host, &target).await {
                    // Do not issue a name-based idempotent kill: if the exact
                    // pane disappears and a prefix sibling appears between
                    // calls, tmux could otherwise resolve the sibling.
                    Ok(false) => false,
                    Ok(true) => {
                        control_target =
                            migrate_or_verify_remote_tmux_owner(state, &host, &session, &target)
                                .await?;
                        true
                    }
                    Err(error) => {
                        tracing::warn!(session = %session.id, %target, %error, "could not verify tmux ownership during delete");
                        false
                    }
                }
            } else {
                true
            };
            let outcome = if !may_teardown {
                Ok(())
            } else if is_external(&session) {
                // Never destroy a user-owned tmux session — just disarm the pipe.
                crate::host_runtime::unpipe_pane(&host, &target).await
            } else {
                crate::host_runtime::kill_session(&host, &control_target).await
            };
            if let Err(e) = outcome {
                tracing::warn!(
                    session = %session.id, %target,
                    "tmux teardown failed during delete (removing record anyway): {e}"
                );
            }
            if session.tool == "claude"
                && let Err(error) = remove_session_claude_mcp_config(&host, id).await
            {
                tracing::warn!(
                    session_id = %id,
                    %error,
                    "could not remove Claude MCP config during delete"
                );
            }
        }
        Err(e) => tracing::warn!(
            session = %session.id,
            "host unavailable during delete (removing record anyway): {e}"
        ),
    }

    state.store.delete_session(id).await?;
    Ok(())
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

/// The loopback env every LOCAL pane is launched with. `AGENTUM_API_URL` lets an
/// `agentum` CLI run inside the pane find THIS server (the embedded desktop server
/// binds an ephemeral port, so a hardcoded guess would miss it); the hook URL/token
/// let the agent curl lifecycle events back. `api_base` is the embedded server's own
/// URL when known, else the standalone daemon's conventional `127.0.0.1:8822`. The
/// hook URL is DERIVED from the same base — never a separate hardcoded port, which
/// previously pointed every hook at 8822 regardless of where the server actually was.
fn pane_env(
    api_base: Option<&str>,
    session_id: Uuid,
    session_name: &str,
    hook_token: &str,
) -> Vec<(String, String)> {
    let base = api_base.unwrap_or("http://127.0.0.1:8822");
    vec![
        ("AGENTUM_API_URL".to_string(), base.to_string()),
        // The orchestration handle for an agent running in this pane: its session
        // name. `agentum orchestration send/check` default `--from`/`--terminal`
        // to this, so an agent can mail siblings without knowing its own name.
        (
            "AGENTUM_TERMINAL_HANDLE".to_string(),
            session_name.to_string(),
        ),
        (
            "AGENTUM_HOOK_URL".to_string(),
            format!("{base}/api/sessions/{session_id}/hook"),
        ),
        ("AGENTUM_HOOK_TOKEN".to_string(), hook_token.to_string()),
    ]
}

fn required_remote_mcp_port(state: &AppState) -> Result<u16, ApiError> {
    crate::mcp_provision::local_mcp_port(state).ok_or_else(|| {
        ApiError::BadGateway(
            "remote launch requires Agentum's dedicated MCP listener; restart the Agentum TUI or desktop app"
                .into(),
        )
    })
}

fn remote_claude_mcp_config_path(home: &str, session_id: Uuid) -> PathBuf {
    std::path::Path::new(home)
        .join(".agentum/runtime")
        .join(format!("mcp-{session_id}.json"))
}

fn agent_mcp_config_matches(
    content: &str,
    file: crate::mcp_provision::AgentMcpFile,
    expected_url: &str,
    expected_auth_env: &str,
    forbidden_secret: &str,
) -> bool {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(content) else {
        return false;
    };
    let entry = &root[file.servers_key]["agentum"];
    let expected_authorization = file.auth_header_value(expected_auth_env);
    entry[file.url_field].as_str() == Some(expected_url)
        && entry["headers"]["Authorization"].as_str() == Some(expected_authorization.as_str())
        && file
            .extra
            .iter()
            .all(|(key, value)| entry[*key].as_str() == Some(*value))
        && (forbidden_secret.is_empty() || !content.contains(forbidden_secret))
}

async fn verify_remote_agent_mcp_config(
    host: &Host,
    workdir: &str,
    tool: &str,
    expected_url: &str,
    expected_auth_env: &str,
    forbidden_secret: &str,
) -> Result<(), ApiError> {
    let Some(file) = crate::mcp_provision::agent_mcp_file(tool) else {
        return Ok(());
    };
    let abs = format!("{}/{}", workdir.trim_end_matches('/'), file.rel_path);
    let content = crate::host_runtime::read_remote_file(host, &abs)
        .await
        .map_err(|error| ApiError::from_host_runtime(host, error))?
        .ok_or_else(|| {
            ApiError::BadGateway(format!("remote {tool} MCP config was not created at {abs}"))
        })?;
    if agent_mcp_config_matches(
        &content,
        file,
        expected_url,
        expected_auth_env,
        forbidden_secret,
    ) {
        Ok(())
    } else {
        Err(ApiError::BadGateway(format!(
            "remote {tool} MCP config at {abs} does not contain the current Agentum endpoint and credential reference"
        )))
    }
}

fn quote_remote_shell(inner: &str) -> Result<String, ApiError> {
    shlex::try_quote(inner)
        .map(|quoted| format!("sh -c {quoted}"))
        .map_err(|_| ApiError::BadRequest("remote path could not be shell-quoted".into()))
}

/// Ensure the directory that temporarily carries Claude's bearer-token config
/// is owner-only before writing the file. `write_remote_file` makes the file
/// itself 0600 atomically; this also repairs an older, overly-permissive runtime
/// directory before the credential enters it.
async fn prepare_remote_runtime_dir(host: &Host, home: &str) -> Result<(), ApiError> {
    let dir = std::path::Path::new(home).join(".agentum/runtime");
    let dir = dir
        .to_str()
        .ok_or_else(|| ApiError::BadRequest("remote HOME is not valid UTF-8".into()))?;
    let dir = shlex::try_quote(dir)
        .map_err(|_| ApiError::BadRequest("remote runtime path could not be quoted".into()))?;
    let script = quote_remote_shell(&format!("umask 077; mkdir -p {dir} && chmod 700 {dir}"))?;
    crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map(|_| ())
        .map_err(|error| ApiError::from_host_runtime(host, error))
}

async fn remove_remote_claude_mcp_config(host: &Host, session_id: Uuid) -> Result<(), ApiError> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Ok(());
    }
    // `$HOME` is intentionally expanded by the remote POSIX shell. The only
    // interpolated portion is a UUID, so this cleanup command carries no
    // user-controlled shell syntax.
    let inner = format!(
        "rm -f \"$HOME/.agentum/runtime/mcp-{session_id}.json\" \
         \"/tmp/agentum-mcp-{session_id}.json\""
    );
    let script = quote_remote_shell(&inner)?;
    crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map(|_| ())
        .map_err(|error| ApiError::from_host_runtime(host, error))
}

async fn remove_session_claude_mcp_config(host: &Host, session_id: Uuid) -> Result<(), ApiError> {
    match &host.kind {
        HostKind::Local => {
            crate::mcp_provision::remove_combined_config(session_id).map_err(|error| {
                ApiError::Internal(format!("remove local Claude MCP config: {error:#}"))
            })
        }
        HostKind::Ssh { .. } => remove_remote_claude_mcp_config(host, session_id).await,
    }
}

const TMUX_OWNER_OPTION: &str = "@agentum_session_id";

fn quote_remote_tmux_target(target: &str) -> Result<String, ApiError> {
    // tmux 3.7b does not accept the `=name` exact-target spelling. A quoted
    // target is safe to pass through the remote shell, and tmux resolves an
    // exact session name before considering a unique prefix.
    shlex::try_quote(target)
        .map(|quoted| quoted.into_owned())
        .map_err(|_| ApiError::BadRequest("tmux target could not be quoted".into()))
}

#[derive(Debug)]
struct RemoteTmuxIdentity {
    name: String,
    id: String,
    owner: Option<Uuid>,
}

async fn remote_tmux_identity(host: &Host, selector: &str) -> Result<RemoteTmuxIdentity, ApiError> {
    let quoted_target = quote_remote_tmux_target(selector)?;
    let script = quote_remote_shell(&format!(
        "sep=$(printf '\\037'); \
         tmux display-message -p -t {quoted_target} \
         \"AGENTUM_TMUX_IDENTITY${{sep}}#{{session_name}}${{sep}}#{{session_id}}${{sep}}#{{{TMUX_OWNER_OPTION}}}\" \
         2>/dev/null || true"
    ))?;
    let output = crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map_err(|error| ApiError::from_host_runtime(host, error))?;
    parse_remote_tmux_identity_output(&output, selector)
}

fn parse_remote_tmux_identity_output(
    output: &str,
    selector: &str,
) -> Result<RemoteTmuxIdentity, ApiError> {
    let record = output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("AGENTUM_TMUX_IDENTITY\u{1f}"))
        .ok_or_else(|| {
            ApiError::BadGateway("remote tmux identity probe returned an invalid response".into())
        })?;
    let mut fields = record.split('\u{1f}');
    let resolved = fields.next().unwrap_or_default().trim();
    let session_id = fields.next().unwrap_or_default().trim();
    let value = fields.next().unwrap_or_default().trim();
    if fields.next().is_some() {
        return Err(ApiError::BadGateway(
            "remote tmux identity probe returned an invalid field count".into(),
        ));
    }
    if resolved.is_empty()
        || !session_id.strip_prefix('$').is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(ApiError::Conflict(format!(
            "tmux selector `{selector}` does not resolve to a live session identity"
        )));
    }
    let owner = if value.is_empty() {
        None
    } else {
        Uuid::parse_str(value).map(Some).map_err(|_| {
            ApiError::Conflict(format!(
                "tmux target `{resolved}` has an invalid Agentum ownership marker"
            ))
        })?
    };
    Ok(RemoteTmuxIdentity {
        name: resolved.to_string(),
        id: session_id.to_string(),
        owner,
    })
}

async fn exact_remote_tmux_identity(
    host: &Host,
    target: &str,
) -> Result<RemoteTmuxIdentity, ApiError> {
    let identity = remote_tmux_identity(host, target).await?;
    validate_exact_tmux_resolution(target, &identity.name).map_err(ApiError::Conflict)?;
    Ok(identity)
}

fn validate_exact_tmux_resolution(requested: &str, resolved: &str) -> Result<(), String> {
    if requested == resolved {
        Ok(())
    } else {
        Err(format!(
            "tmux target `{requested}` resolves by prefix to `{resolved}`; refusing non-exact ownership"
        ))
    }
}

async fn mark_remote_tmux_owner(
    host: &Host,
    expected_name: &str,
    exact_selector: &str,
    id: Uuid,
) -> Result<(), ApiError> {
    mark_remote_tmux_owner_if_absent(host, exact_selector, id).await?;
    let identity = remote_tmux_identity(host, exact_selector).await?;
    if identity.name != expected_name {
        return Err(ApiError::Conflict(format!(
            "tmux identity `{exact_selector}` changed from `{expected_name}` to `{}`",
            identity.name
        )));
    }
    match identity.owner {
        Some(owner) if owner == id => Ok(()),
        Some(owner) => Err(ApiError::Conflict(format!(
            "tmux target `{expected_name}` belongs to Agentum session {owner}, not {id}"
        ))),
        None => Err(ApiError::Conflict(format!(
            "tmux target `{expected_name}` remained unmarked after ownership claim"
        ))),
    }
}

fn validate_legacy_owner_migration_binding(
    session_id: Uuid,
    external: bool,
    persisted_host_id: Option<Uuid>,
    persisted_target: Option<&str>,
    live_host_id: Uuid,
    live_target: &str,
    exact_binding_ids: &[Uuid],
) -> Result<(), String> {
    if external {
        return Err("external tmux sessions are user-owned".into());
    }
    if persisted_host_id != Some(live_host_id) || persisted_target != Some(live_target) {
        return Err("the live host/target is not the session's exact persisted binding".into());
    }
    if live_target
        .strip_prefix("agentum-")
        .is_none_or(|suffix| suffix.is_empty())
    {
        return Err("the target is not a historical `agentum-` managed target".into());
    }
    match exact_binding_ids {
        [only] if *only == session_id => Ok(()),
        [only] => Err(format!(
            "the sole persisted binding belongs to Agentum session {only}"
        )),
        bindings => Err(format!(
            "found {} persisted session bindings for the host/target pair",
            bindings.len()
        )),
    }
}

/// Claim an unmarked legacy target without overwriting a marker another actor
/// may have installed after our initial probe. tmux's `-o` flag is an
/// option-level compare-and-set: it writes only while the option is absent.
async fn mark_remote_tmux_owner_if_absent(
    host: &Host,
    exact_selector: &str,
    id: Uuid,
) -> Result<(), ApiError> {
    let quoted_target = quote_remote_tmux_target(exact_selector)?;
    let id_string = id.to_string();
    let quoted_id = shlex::try_quote(&id_string)
        .map_err(|_| ApiError::BadRequest("session id could not be quoted".into()))?;
    let script = quote_remote_shell(&format!(
        "tmux set-option -o -t {quoted_target} {TMUX_OWNER_OPTION} {quoted_id} >/dev/null 2>&1 || true"
    ))?;
    crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map(|_| ())
        .map_err(|error| ApiError::from_host_runtime(host, error))
}

/// Verify a live managed SSH target before reattaching, controlling or
/// destroying it. An unmarked pane may be migrated exactly once, but only when
/// it is an historical `agentum-` target and the store proves there is exactly
/// one persisted `(host_id, tmux_target)` binding, owned by this session.
/// Existing mismatched (or invalid) markers are never overwritten.
async fn migrate_or_verify_remote_tmux_owner(
    state: &AppState,
    host: &Host,
    session: &Session,
    target: &str,
) -> Result<String, ApiError> {
    let identity = exact_remote_tmux_identity(host, target).await?;
    match identity.owner {
        Some(owner) if owner != session.id => {
            return Err(ApiError::Conflict(format!(
                "tmux target `{target}` belongs to Agentum session {owner}, not {}",
                session.id
            )));
        }
        Some(_) => return Ok(identity.id),
        None => {}
    }

    let exact_binding_ids: Vec<Uuid> = state
        .store
        .list_sessions(None)
        .await?
        .into_iter()
        .filter(|bound| {
            bound.host_id == Some(host.id) && bound.tmux_target.as_deref() == Some(target)
        })
        .map(|bound| bound.id)
        .collect();
    validate_legacy_owner_migration_binding(
        session.id,
        is_external(session),
        session.host_id,
        session.tmux_target.as_deref(),
        host.id,
        target,
        &exact_binding_ids,
    )
    .map_err(|reason| {
        ApiError::Conflict(format!(
            "tmux target `{target}` has no Agentum ownership marker; refusing legacy ownership migration: {reason}"
        ))
    })?;

    mark_remote_tmux_owner_if_absent(host, &identity.id, session.id).await?;
    let verified = remote_tmux_identity(host, &identity.id).await?;
    if verified.name != target || verified.id != identity.id {
        return Err(ApiError::Conflict(format!(
            "tmux identity `{}` changed while migrating `{target}`",
            identity.id
        )));
    }
    match verified.owner {
        Some(owner) if owner == session.id => Ok(identity.id),
        Some(owner) => Err(ApiError::Conflict(format!(
            "tmux target `{target}` belongs to Agentum session {owner}, not {}",
            session.id
        ))),
        None => Err(ApiError::Conflict(format!(
            "tmux target `{target}` remained unmarked after legacy ownership migration"
        ))),
    }
}

/// Ownership gate for callers that already hold the canonical host → session
/// leases and are about to operate on a live tmux target. External/user-owned
/// panes and local sessions deliberately bypass Agentum's marker namespace.
pub(crate) async fn guard_managed_ssh_tmux_io(
    state: &AppState,
    host: &Host,
    session: &Session,
    target: &str,
) -> Result<String, ApiError> {
    if !is_external(session) && matches!(host.kind, HostKind::Ssh { .. }) {
        migrate_or_verify_remote_tmux_owner(state, host, session, target).await
    } else {
        Ok(target.to_string())
    }
}

const TMUX_MCP_GENERATION_OPTION: &str = "@agentum_mcp_generation";

fn mcp_token_generation(token: &str) -> String {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;

    let mut input = b"session-scoped-mcp-v1\0".to_vec();
    input.extend_from_slice(token.as_bytes());
    let digest = Sha256::digest(&input);
    let mut generation = String::with_capacity(24);
    for byte in &digest[..12] {
        write!(&mut generation, "{byte:02x}").expect("writing to String cannot fail");
    }
    generation
}

async fn remote_tmux_mcp_generation(host: &Host, target: &str) -> Result<Option<String>, ApiError> {
    let quoted_target = quote_remote_tmux_target(target)?;
    let script = quote_remote_shell(&format!(
        "value=$(tmux show-options -v -t {quoted_target} {TMUX_MCP_GENERATION_OPTION} 2>/dev/null || true); \
         printf 'AGENTUM_MCP_GENERATION\\t%s\\n' \"$value\""
    ))?;
    let output = crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map_err(|error| ApiError::from_host_runtime(host, error))?;
    let value = output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("AGENTUM_MCP_GENERATION\t"))
        .ok_or_else(|| {
            ApiError::BadGateway(
                "remote tmux MCP generation probe returned an invalid response".into(),
            )
        })?
        .trim();
    Ok((!value.is_empty()).then(|| value.to_string()))
}

async fn mark_remote_tmux_mcp_generation(
    host: &Host,
    target: &str,
    generation: &str,
) -> Result<(), ApiError> {
    let quoted_target = quote_remote_tmux_target(target)?;
    let quoted_generation = shlex::try_quote(generation)
        .map_err(|_| ApiError::BadRequest("MCP generation could not be quoted".into()))?;
    let script = quote_remote_shell(&format!(
        "tmux set-option -t {quoted_target} {TMUX_MCP_GENERATION_OPTION} {quoted_generation}"
    ))?;
    crate::host_runtime::ssh_stdout(host, &script)
        .await
        .map(|_| ())
        .map_err(|error| ApiError::from_host_runtime(host, error))
}

/// Spawn implementation for callers that already hold this session's
/// lifecycle lock. Keeping the public crate seam locked lets harness drivers
/// participate in the same transaction without recursively locking start/create.
async fn spawn_agent_into_pane_locked(
    state: &AppState,
    session: &Session,
    host: &Host,
    target: &str,
    workdir: &std::path::Path,
) -> Result<(), ApiError> {
    let adapter = agentum_executor::adapter_for(&session.tool);
    let remote_preflight = if matches!(host.kind, HostKind::Ssh { .. }) {
        Some(
            crate::host_runtime::preflight_remote_launch(host, workdir, &session.tool, session.id)
                .await
                .map_err(|e| ApiError::from_host_runtime(host, e))?,
        )
    } else {
        None
    };
    let resolved_workdir = remote_preflight
        .as_ref()
        .map(|preflight| preflight.workdir.as_path())
        .unwrap_or(workdir);
    let mut launch = match remote_preflight.as_ref() {
        Some(preflight) => adapter.launch_remote(
            session,
            &agentum_executor::RemoteLaunchContext {
                shell: Cow::Borrowed(preflight.shell.as_str()),
                claude_transcript_exists: preflight.claude_transcript_exists,
            },
        ),
        None => adapter.launch(session),
    };
    if let Some(preflight) = remote_preflight.as_ref() {
        let program = launch.argv.first_mut().ok_or_else(|| {
            ApiError::BadRequest(format!(
                "tool adapter `{}` produced an empty launch command",
                session.tool
            ))
        })?;
        // A long-lived remote tmux server can retain a stale PATH. Always exec
        // the absolute binary selected by the fresh host preflight.
        *program = preflight.executable.clone();
    }

    if let Some(preflight) = remote_preflight.as_ref() {
        // The SSH login/preflight sees the user's current PATH, whereas a
        // long-lived tmux server may retain a stale snapshot. Replace any
        // adapter-provided PATH with the freshly probed value so the selected
        // CLI and all of its child processes see the same toolchain.
        launch.env.retain(|(key, _)| key != "PATH");
        launch
            .env
            .push(("PATH".into(), preflight.fresh_path.clone()));
    }

    let session_mcp_secret =
        crate::mcp_provision::session_mcp_token(state.mcp_token.as_str(), session.id);
    // Resolve the local pane-log path before staging any remote credential so
    // a local path failure cannot strand a Claude MCP config on the host.
    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;

    if matches!(host.kind, HostKind::Local) {
        // Loopback hook URLs only work for local panes. SSH-hosted agents
        // run on another machine, where 127.0.0.1 points at the VPS.
        let hook_token = crate::auth::new_token();
        for kv in pane_env(
            state.api_base_url.as_deref(),
            session.id,
            &session.name,
            &hook_token,
        ) {
            launch.env.push(kv);
        }

        if session.tool == "claude" {
            // Claude Code has no `--hook-post-tool-use` flag; hooks are
            // registered through settings. Inject a PostToolUse command hook
            // via `--settings` (which *adds* to the user's settings rather than
            // replacing them). The AGENTUM_HOOK_* refs resolve from the pane env
            // exported above, so they must stay unexpanded here.
            let hook_cmd = "curl -s -X POST \"$AGENTUM_HOOK_URL\" \
                 -H \"X-Agentum-Hook-Token: $AGENTUM_HOOK_TOKEN\" \
                 -H \"Content-Type: application/json\" \
                 -d '{\"kind\":\"tool_done\",\"payload\":{}}'";
            let settings = serde_json::json!({
                "hooks": {
                    "PostToolUse": [
                        {
                            "matcher": "*",
                            "hooks": [
                                { "type": "command", "command": hook_cmd }
                            ]
                        }
                    ]
                }
            });
            launch.argv.push("--settings".into());
            launch.argv.push(settings.to_string());
        }

        // Scope the lock so the (non-Send) MutexGuard is dropped before the
        // provisioning await below — holding it across `.await` would make the
        // caller's future non-Send and break the axum Handler bound.
        {
            let mut map = state.hook_tokens.lock().unwrap();
            map.insert(session.id, hook_token);
        }

        // Wire the agentum MCP into agents that take it via a launch arg
        // (Claude --mcp-config, Codex -c); local agents reach it on the Mac
        // loopback. Best-effort — never blocks a launch.
        // Real boots always provide the dedicated MCP-only listener. Keep the
        // API/default fallback solely for direct unit-test AppState values and
        // older embedding callers; SSH tunnelling uses `local_mcp_port`, which
        // intentionally has no such fallback.
        let base = state
            .mcp_base_url
            .as_deref()
            .or(state.api_base_url.as_deref())
            .unwrap_or("http://127.0.0.1:8822");
        let agentum_mcp_url = format!("{base}/mcp");
        if let Some(p) =
            crate::mcp_provision::provision(state, session.id, &session.tool, &agentum_mcp_url)
                .await
        {
            adapter.apply_mcp(&mut launch, &p);
        }
        // File-based agents (Cursor/Gemini/OpenCode) load MCP from a config file
        // in the workdir — write it (no-op for claude/codex).
        match crate::mcp_provision::write_agent_project_config(
            state,
            session.id,
            host,
            &resolved_workdir.to_string_lossy(),
            &session.tool,
            &agentum_mcp_url,
        )
        .await
        {
            Ok(Some(env)) => launch.env.push(env),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    session_id = %session.id,
                    tool = %session.tool,
                    %error,
                    "could not provision local project MCP config; launching without it"
                );
            }
        }
    } else if matches!(host.kind, HostKind::Ssh { .. }) && tool_consumes_agentum_mcp(&session.tool)
    {
        // Remote MCP parity: the agentum MCP lives on the Mac. Reverse-tunnel it
        // to the host (token-guarded, loopback-bound), then wire each agent at
        // the tunnel URL. This is required, not best-effort: reporting a remote
        // agent as Running while silently omitting Agentum's MCP leaves it
        // deceptively half-functional.
        let mac_port = required_remote_mcp_port(state)?;
        let host_port = crate::host_runtime::ensure_reverse_tunnel(host, mac_port)
            .await
            .map_err(|e| ApiError::from_host_runtime(host, e))?;

        // The remote agent keeps its orchestration identity, but the reverse
        // tunnel terminates at an MCP-only listener. Never advertise that port
        // as AGENTUM_API_URL: doing so would either be misleading (REST is 404)
        // or tempt a future change to expose the no-auth embedded REST router.
        // Remote orchestration is available through the scoped Agentum MCP.
        launch
            .env
            .push(("AGENTUM_TERMINAL_HANDLE".into(), session.name.clone()));
        let agentum_mcp_url = format!("http://127.0.0.1:{host_port}/mcp");
        let servers = vec![crate::mcp_provision::agentum_server(
            state,
            session.id,
            &agentum_mcp_url,
        )];
        let provision = if session.tool == "claude" {
            // Claude needs the --mcp-config FILE on the HOST. Failing this write
            // is a launch failure; do not start an agent missing its MCP.
            let preflight = remote_preflight
                .as_ref()
                .expect("SSH launch always has a remote preflight");
            prepare_remote_runtime_dir(host, &preflight.home).await?;
            let host_cfg = remote_claude_mcp_config_path(&preflight.home, session.id);
            let host_cfg_str = host_cfg.to_str().ok_or_else(|| {
                ApiError::BadRequest("remote Claude MCP config path is not valid UTF-8".into())
            })?;
            let json = crate::mcp_provision::config_json(&servers);
            crate::host_runtime::write_remote_file(host, host_cfg_str, &json)
                .await
                .map_err(|e| ApiError::from_host_runtime(host, e))?;
            agentum_executor::McpProvision {
                servers,
                config_file: host_cfg,
            }
        } else {
            // Codex injects MCP inline via `-c` — no host file needed.
            agentum_executor::McpProvision {
                servers,
                config_file: PathBuf::new(),
            }
        };
        adapter.apply_mcp(&mut launch, &provision);
        // File-based agents: write the config on the HOST in the workdir.
        let project_auth_env = crate::mcp_provision::write_agent_project_config(
            state,
            session.id,
            host,
            &resolved_workdir.to_string_lossy(),
            &session.tool,
            &agentum_mcp_url,
        )
        .await
        .map_err(|error| {
            ApiError::BadGateway(format!(
                "could not provision remote {} MCP project config: {error}",
                session.tool
            ))
        })?;
        match project_auth_env {
            Some((env_name, env_value)) => {
                launch.env.push((env_name.clone(), env_value));
                verify_remote_agent_mcp_config(
                    host,
                    &resolved_workdir.to_string_lossy(),
                    &session.tool,
                    &agentum_mcp_url,
                    &env_name,
                    session_mcp_secret.as_str(),
                )
                .await?;
            }
            None if crate::mcp_provision::agent_mcp_file(&session.tool).is_some() => {
                return Err(ApiError::BadGateway(format!(
                    "remote {} MCP project config did not produce a launch credential",
                    session.tool
                )));
            }
            None => {}
        }
    }

    let mut launch_result = match &host.kind {
        HostKind::Local => match crate::host_runtime::new_session(
            host,
            target,
            resolved_workdir,
            &launch.argv,
            &launch.env,
        )
        .await
        {
            Err(error) => Err(ApiError::from_host_runtime(host, error)),
            Ok(()) => {
                if let Err(error) = crate::host_runtime::pipe_pane(host, target, &log).await {
                    let _ = crate::host_runtime::kill_session(host, target).await;
                    Err(ApiError::from_host_runtime(host, error))
                } else {
                    Ok(())
                }
            }
        },
        HostKind::Ssh { .. } => crate::host_runtime::launch_remote_session(
            host,
            target,
            resolved_workdir,
            &launch.argv,
            &launch.env,
            &[state.mcp_token.as_str(), session_mcp_secret.as_str()],
            &log,
        )
        .await
        .map_err(|e| ApiError::from_host_runtime(host, e)),
    };

    let mut remote_control_target = None;
    if launch_result.is_ok() && matches!(host.kind, HostKind::Ssh { .. }) {
        match exact_remote_tmux_identity(host, target).await {
            Ok(identity) => {
                remote_control_target = Some(identity.id.clone());
                if let Err(marker_error) =
                    mark_remote_tmux_owner(host, target, &identity.id, session.id).await
                {
                    // A new pane is not committed until its durable ownership
                    // marker is in place. Roll back through tmux's immutable
                    // `$N` session id so a disappearing exact name can never
                    // prefix-resolve to a different pane.
                    let _ = crate::host_runtime::kill_session(host, &identity.id).await;
                    launch_result = Err(marker_error);
                }
            }
            Err(marker_error) => {
                launch_result = Err(marker_error);
            }
        }
    }
    if launch_result.is_ok()
        && matches!(host.kind, HostKind::Ssh { .. })
        && tool_consumes_agentum_mcp(&session.tool)
    {
        let generation = mcp_token_generation(state.mcp_token.as_str());
        let control_target = remote_control_target.as_deref().unwrap_or(target);
        if let Err(marker_error) =
            mark_remote_tmux_mcp_generation(host, control_target, &generation).await
        {
            let _ = crate::host_runtime::kill_session(host, control_target).await;
            launch_result = Err(marker_error);
        }
    }

    // Keep Claude's owner-only config for the lifetime of a successful pane:
    // the CLI is allowed to re-read it after startup. Failed launches have no
    // consumer, so remove their credential immediately; successful panes clean
    // it on stop/kill/delete (and before generation reprovisioning).
    if session.tool == "claude"
        && launch_result.is_err()
        && let Err(cleanup_error) = remove_session_claude_mcp_config(host, session.id).await
    {
        tracing::warn!(session_id = %session.id, %cleanup_error, "could not remove Claude MCP config after failed launch");
    }
    if let Err(error) = launch_result {
        state.hook_tokens.lock().unwrap().remove(&session.id);
        return Err(error);
    }

    if let Err(error) = state
        .store
        .update_status_and_target(session.id, Status::Running, Some(target))
        .await
    {
        // A pane without its persisted identity/status is an orphan and can be
        // stolen by a later name-derived start. Roll the just-created pane back
        // before surfacing the database error.
        let control_target = remote_control_target.as_deref().unwrap_or(target);
        let _ = crate::host_runtime::kill_session(host, control_target).await;
        if session.tool == "claude"
            && let Err(cleanup_error) = remove_session_claude_mcp_config(host, session.id).await
        {
            tracing::warn!(session_id = %session.id, %cleanup_error, "could not remove Claude MCP config after persistence failure");
        }
        state.hook_tokens.lock().unwrap().remove(&session.id);
        return Err(error.into());
    }
    Ok(())
}

async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_uuid(&id)?;
    let (session, spawned) = start_session_by_id(&state, id).await?;
    Ok(Json(session_with_spawned(session, spawned)))
}

/// Host-aware start operation shared by the HTTP route and boot-time idle
/// resume. Keeping host lookup, reattach, remote validation and launch behind
/// this one callable prevents a persisted SSH-owned row from ever falling back
/// to local filesystem/tmux behavior.
pub(crate) async fn start_session_by_id(
    state: &AppState,
    id: Uuid,
) -> Result<(Session, bool), ApiError> {
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(state, id).await?;
    start_session_locked(state, id).await
}

/// Boot-time resume entry point. The initial `list_sessions(Idle)` is only a
/// snapshot; a concurrent explicit stop can win while the sweep waits for this
/// session's lock. Re-read under the lifecycle lock and skip unless it is still
/// `Idle` so startup can never undo the user's stop.
pub(crate) async fn resume_idle_session_by_id(
    state: &AppState,
    id: Uuid,
) -> Result<Option<(Session, bool)>, ApiError> {
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(state, id).await?;
    let session = load(state, id).await?;
    if !matches!(session.status, Status::Idle) {
        return Ok(None);
    }
    start_session_locked(state, id).await.map(Some)
}

/// Reconcile a preserved Running SSH MCP pane at boot. Normal boots verify its
/// stable credential-generation marker, re-arm the stable host listener and
/// reattach without killing it. A missing/stale generation (first upgrade or
/// explicit token rotation) triggers the one necessary restart. Plain terminals,
/// local panes and external/user-owned tmux sessions are excluded.
pub(crate) async fn reprovision_running_remote_mcp_session_by_id(
    state: &AppState,
    id: Uuid,
) -> Result<Option<(Session, bool)>, ApiError> {
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(state, id).await?;
    let session = load(state, id).await?;
    if !matches!(session.status, Status::Running)
        || is_external(&session)
        || !tool_consumes_agentum_mcp(&session.tool)
    {
        return Ok(None);
    }
    let host = load_host_for_session(state, &session).await?;
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Ok(None);
    }
    start_session_locked(state, id).await.map(Some)
}

async fn start_session_locked(state: &AppState, id: Uuid) -> Result<(Session, bool), ApiError> {
    let session = load(state, id).await?;
    let host = load_host_for_session(state, &session).await?;
    // External sessions keep their (non-agentum) target across stop, so
    // start can only ever reattach to it — never derive a fresh
    // `agentum-*` name, which would spawn a parallel session.
    let target = start_target(&session)?;

    let mut already = crate::host_runtime::has_session(&host, &target)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?;
    let mut remote_control_target = None;

    if already && !is_external(&session) && matches!(host.kind, HostKind::Ssh { .. }) {
        let exact_target =
            migrate_or_verify_remote_tmux_owner(state, &host, &session, &target).await?;
        remote_control_target = Some(exact_target.clone());
        if tool_consumes_agentum_mcp(&session.tool) {
            let expected = mcp_token_generation(state.mcp_token.as_str());
            let current = remote_tmux_mcp_generation(&host, &exact_target).await?;
            if current.as_deref() != Some(expected.as_str()) {
                // One-time upgrade/credential rotation: this verified managed
                // pane has a missing/stale credential generation. Restart it
                // exactly once; subsequent boots keep the stable token/host
                // port and reattach without sacrificing conversation state.
                crate::host_runtime::kill_session(&host, &exact_target)
                    .await
                    .map_err(|e| ApiError::from_host_runtime(&host, e))?;
                state
                    .store
                    .update_status_and_target(id, Status::Idle, None)
                    .await?;
                already = false;
            } else {
                // The credential and agent-facing host port are stable, but the
                // Mac endpoint is ephemeral. Ensure boot has re-armed the stable
                // reverse listener to this server before reporting the pane live.
                let mac_port = required_remote_mcp_port(state)?;
                crate::host_runtime::ensure_reverse_tunnel(&host, mac_port)
                    .await
                    .map_err(|error| ApiError::from_host_runtime(&host, error))?;
            }
        }
    }

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
        // Stop explicitly disarms an external pane's pipe. Re-arm every live
        // reattach (the operation is idempotent) before reporting Running so a
        // reattached local or remote pane immediately resumes streaming.
        let log = paths::pane_log(&session.id.to_string())
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let control_target = remote_control_target.as_deref().unwrap_or(&target);
        crate::host_runtime::pipe_pane(&host, control_target, &log)
            .await
            .map_err(|e| ApiError::from_host_runtime(&host, e))?;
        if !matches!(session.status, Status::Running)
            || session.tmux_target.as_deref() != Some(target.as_str())
        {
            state
                .store
                .update_status_and_target(id, Status::Running, Some(&target))
                .await?;
        }
        return Ok((load(state, id).await?, false));
    }

    if !is_external(&session) && session.tool == "claude" {
        // Remove a credential file left by a crashed daemon/pane before the
        // next preflight. The successful launch below writes the current
        // generation atomically at this same private path.
        remove_session_claude_mcp_config(&host, id).await?;
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
            if !workdir.exists() {
                // Same self-heal as create: a started session whose isolated
                // worktree directory vanished should be recoverable, not a dead
                // 400. Recreate the `<repo>/.claude/worktrees/<name>` tree from
                // the parent repo; any other missing path is still a hard error.
                match worktree_repo_for_missing(&workdir) {
                    Some(repo) => {
                        crate::git::recreate_worktree(&repo, &workdir)
                            .await
                            .map_err(|e| {
                                ApiError::BadRequest(format!(
                                    "workdir does not exist and could not be recreated: {} ({e})",
                                    workdir.display()
                                ))
                            })?;
                    }
                    None => {
                        return Err(ApiError::BadRequest(format!(
                            "workdir does not exist: {}",
                            workdir.display()
                        )));
                    }
                }
            }
            workdir
        }
        HostKind::Ssh { .. } => PathBuf::from(session.effective_cwd()),
    };

    // All launch conventions (YOLO translation, loopback env, Claude hook, MCP
    // wiring, pipe-pane, status flip) live in the shared spawn helper so the
    // harness-engine driver goes through the exact same path.
    spawn_agent_into_pane_locked(state, &session, &host, &target, &workdir).await?;
    Ok((load(state, id).await?, true))
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
    let _host_guard = acquire_host_lifecycle(host_id).await;
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
        HostKind::Ssh { .. } => {
            let requested = PathBuf::from(new.workdir.trim());
            let preflight = crate::host_runtime::preflight_remote_launch(
                &host,
                &requested,
                &new.tool,
                Uuid::nil(),
            )
            .await
            .map_err(|e| ApiError::from_host_runtime(&host, e))?;
            new.workdir = preflight.workdir.to_string_lossy().into_owned();
            preflight.workdir
        }
    };

    let handoff = lifecycle_lock_handoff().lock().await;
    let session = state
        .store
        .create_session_on_host(new, Some(host_id))
        .await?;
    // Hand the insert directly to the new per-session lock before allowing any
    // lifecycle route through the registry gate. This closes the only interval
    // where the row exists but its generated UUID was not known to this caller.
    let _guard = session_lifecycle_lock(session.id)
        .try_lock_owned()
        .expect("a newly inserted session lock cannot already be held");
    drop(handoff);
    let target = managed_tmux_target(&session);
    spawn_agent_into_pane_locked(state, &session, &host, &target, &workdir).await?;
    load(state, session.id).await
}

/// Atomically claim a board card and launch its canonical managed session.
/// `Store::claim_card` currently creates local-host rows (its schema-era
/// contract predates selectable hosts), so reject a non-local request rather
/// than launching remotely while persisting a misleading local binding.
pub(crate) async fn claim_card_and_spawn_session(
    state: &AppState,
    card_id: i64,
    mut new: NewSession,
    host_id: Option<Uuid>,
) -> Result<Session, ApiError> {
    let host_id = host_id.unwrap_or(LOCAL_HOST_ID);
    if host_id != LOCAL_HOST_ID {
        return Err(ApiError::BadRequest(
            "board card sessions can only be claimed on the local host".into(),
        ));
    }

    let _host_guard = acquire_host_lifecycle(host_id).await;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("unknown host: {host_id}")))?;
    let workdir = super::util::expand_workdir(&new.workdir)?;
    if !workdir.exists() {
        return Err(ApiError::BadRequest(format!(
            "workdir does not exist: {}",
            workdir.display()
        )));
    }
    new.workdir = workdir.to_string_lossy().into_owned();

    // Keep the UUID hand-off atomic with respect to every route that resolves a
    // per-session lease, exactly like `create_and_spawn_session`.
    let handoff = lifecycle_lock_handoff().lock().await;
    let (_card, session) = state.store.claim_card(card_id, new).await?;
    let _session_guard = session_lifecycle_lock(session.id)
        .try_lock_owned()
        .expect("a newly claimed session lock cannot already be held");
    drop(handoff);

    let target = managed_tmux_target(&session);
    spawn_agent_into_pane_locked(state, &session, &host, &target, &workdir).await?;
    load(state, session.id).await
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(&state, id).await?;
    Ok(Json(stop_session_locked(&state, id).await?))
}

async fn stop_session_locked(state: &AppState, id: Uuid) -> Result<Session, ApiError> {
    let session = load(state, id).await?;
    let host = load_host_for_session(state, &session).await?;
    let target = tmux_target(&session);
    let managed_remote = !is_external(&session) && matches!(host.kind, HostKind::Ssh { .. });
    let live = if managed_remote {
        crate::host_runtime::has_session(&host, &target)
            .await
            .map_err(|error| ApiError::from_host_runtime(&host, error))?
    } else {
        true
    };
    let control_target = if managed_remote && live {
        migrate_or_verify_remote_tmux_owner(state, &host, &session, &target).await?
    } else {
        target.clone()
    };
    if is_external(&session) {
        // Detach only: the tmux session belongs to the user. Disarm the
        // log pipe and keep the target so a later start can reattach.
        let _ = crate::host_runtime::unpipe_pane(&host, &target).await;
        state
            .store
            .update_status_and_target(id, Status::Stopped, Some(&target))
            .await?;
    } else {
        if live {
            crate::host_runtime::graceful_stop(&host, &control_target, GRACEFUL_STOP_TIMEOUT)
                .await
                .map_err(|e| ApiError::from_host_runtime(&host, e))?;
        }
        state
            .store
            .update_status_and_target(id, Status::Stopped, None)
            .await?;
    }
    if session.tool == "claude"
        && let Err(error) = remove_session_claude_mcp_config(&host, id).await
    {
        tracing::warn!(session_id = %id, %error, "could not remove Claude MCP config during stop");
    }
    state.hook_tokens.lock().unwrap().remove(&id);
    emit_stopped(state, &session, "stop").await;
    load(state, id).await
}

async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    Ok(Json(kill_session_by_id(&state, id).await?))
}

/// Canonical hard stop used by HTTP, board and harness paths.
pub(crate) async fn kill_session_by_id(state: &AppState, id: Uuid) -> Result<Session, ApiError> {
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(state, id).await?;
    kill_session_locked(state, id).await
}

async fn kill_session_locked(state: &AppState, id: Uuid) -> Result<Session, ApiError> {
    let session = load(state, id).await?;
    let host = load_host_for_session(state, &session).await?;
    let target = tmux_target(&session);
    let managed_remote = !is_external(&session) && matches!(host.kind, HostKind::Ssh { .. });
    let live = if managed_remote {
        crate::host_runtime::has_session(&host, &target)
            .await
            .map_err(|error| ApiError::from_host_runtime(&host, error))?
    } else {
        true
    };
    let control_target = if managed_remote && live {
        migrate_or_verify_remote_tmux_owner(state, &host, &session, &target).await?
    } else {
        target.clone()
    };
    if is_external(&session) {
        // Even a kill must not destroy a user-owned tmux session —
        // detach (disarm the pipe) and keep the target for reattach.
        let _ = crate::host_runtime::unpipe_pane(&host, &target).await;
        state
            .store
            .update_status_and_target(id, Status::Stopped, Some(&target))
            .await?;
    } else {
        if live {
            crate::host_runtime::kill_session(&host, &control_target)
                .await
                .map_err(|e| ApiError::from_host_runtime(&host, e))?;
        }
        state
            .store
            .update_status_and_target(id, Status::Stopped, None)
            .await?;
    }
    if session.tool == "claude"
        && let Err(error) = remove_session_claude_mcp_config(&host, id).await
    {
        tracing::warn!(session_id = %id, %error, "could not remove Claude MCP config during kill");
    }
    state.hook_tokens.lock().unwrap().remove(&id);
    emit_stopped(state, &session, "kill").await;
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

async fn load_host_for_session(state: &AppState, session: &Session) -> Result<Host, ApiError> {
    let host_id = session.host_id.unwrap_or(LOCAL_HOST_ID);
    state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("session host is missing: {host_id}")))
}

/// Acquire the process-wide host lifecycle lease and reload the persisted SSH
/// host while that lease is held. Long-lived WebSocket tasks must never retain
/// a [`Host`] across a host PUT: the PUT closes the old ControlMaster before it
/// commits the new credential revision, and reusing a pre-PUT value afterwards
/// could recreate the retired master. Callers keep the returned guard only for
/// the single SSH/tmux operation (or process spawn) they are about to perform.
async fn acquire_current_stream_host(
    state: &AppState,
    host_id: Uuid,
) -> Result<(tokio::sync::OwnedMutexGuard<()>, Host), ApiError> {
    let guard = acquire_host_lifecycle(host_id).await;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("session host is missing: {host_id}")))?;
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(ApiError::BadRequest(format!(
            "session host is no longer an SSH host: {host_id}"
        )));
    }
    Ok((guard, host))
}

/// Cancellation-aware form used after a persistent stream has registered.
/// Without this select, a stream could wait for the host lease held by PUT
/// while PUT simultaneously waits for the stream's cleanup acknowledgment.
async fn acquire_current_stream_host_unless_cancelled(
    state: &AppState,
    host_id: Uuid,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Option<(tokio::sync::OwnedMutexGuard<()>, Host)>, ApiError> {
    if *cancel.borrow() {
        return Ok(None);
    }
    tokio::select! {
        biased;
        _ = cancel.changed() => Ok(None),
        current = acquire_current_stream_host(state, host_id) => current.map(Some),
    }
}

/// Send a frame without letting a backpressured/non-reading WebSocket block a
/// host mutation from reaching child cleanup. Once cancellation is signaled,
/// the frame is abandoned and the stream loop proceeds directly to reap/ACK.
async fn send_remote_stream_message_unless_cancelled(
    socket: &mut WebSocket,
    message: Message,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    if *cancel.borrow() {
        return false;
    }
    tokio::select! {
        biased;
        _ = cancel.changed() => false,
        result = socket.send(message) => result.is_ok(),
    }
}

fn tmux_target(session: &Session) -> String {
    session
        .tmux_target
        .clone()
        .unwrap_or_else(|| managed_tmux_target(session))
}

/// A newly managed pane includes its immutable session UUID. Names are display
/// labels and are not unique, so a name-only target can collide with another DB
/// row (or a user's unrelated tmux session) and make `has-session` look like a
/// valid reattach. Persisted legacy targets remain honored, while every new
/// target is collision-safe without relying on mutable pane contents.
fn managed_tmux_target(session: &Session) -> String {
    agentum_tmux::target_for(&format!("{}-{}", session.name, session.id.simple()))
}

/// Target selection for a start is deliberately persistence-first. A running
/// pane keeps its immutable tmux identity even when the user renames the
/// session; only rows that have never owned a target derive a collision-safe
/// target from the display name plus immutable UUID. External records cannot be
/// recreated when that binding is missing.
fn start_target(session: &Session) -> Result<String, ApiError> {
    match session.tmux_target.clone() {
        Some(target) => Ok(target),
        None if is_external(session) => Err(ApiError::BadRequest(
            "external session has lost its tmux target".into(),
        )),
        None => Ok(managed_tmux_target(session)),
    }
}

/// True when this record was created by attaching to a pre-existing
/// (non-agentum) tmux session. The whole lifecycle must be
/// non-destructive for these: never kill the tmux session, never respawn
/// it — only arm/disarm the stream.
fn is_external(session: &Session) -> bool {
    session.flags.iter().any(|f| f == EXTERNAL_TMUX_FLAG)
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
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(&state, id).await?;
    let session = load(&state, id).await?;
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
        .map_err(|e| ApiError::from_host_runtime(&host, e))?
    {
        return Err(ApiError::BadRequest(
            "tmux session not active for this session".into(),
        ));
    }
    let control_target = guard_managed_ssh_tmux_io(&state, &host, &session, target).await?;

    crate::host_runtime::send_keys(&host, &control_target, payload, body.append_enter)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?;

    Ok(StatusCode::NO_CONTENT)
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
    let (host_guard, session_guard) = acquire_host_and_session_lifecycle(&state, id).await?;
    let session = load(&state, id).await?;
    let host = load_host_for_session(&state, &session).await?;
    let host_id = session.host_id.unwrap_or(LOCAL_HOST_ID);
    let mut target = session
        .tmux_target
        .clone()
        .ok_or_else(|| ApiError::BadRequest("session is not running".into()))?;
    if !is_external(&session) && matches!(host.kind, HostKind::Ssh { .. }) {
        // The ownership guard resolves the target and proves that the exact
        // tmux identity is live. A separate `has-session` here duplicated that
        // SSH round trip on every attach, directly delaying the WebSocket
        // upgrade on high-latency hosts.
        target = guard_managed_ssh_tmux_io(&state, &host, &session, &target).await?;
    }
    let positions = state.stream_positions.clone();
    let resume = q.resume;
    let redraw = q.redraw;
    let remote = matches!(host.kind, HostKind::Ssh { .. });
    // The upgraded socket is long-lived. Release the handshake transaction
    // before returning it; the remote stream reacquires the shared host lease
    // around each SSH operation instead of pinning host edits for its lifetime.
    drop(session_guard);
    drop(host_guard);
    Ok(ws.on_upgrade(move |socket| async move {
        if !remote {
            stream_session(socket, id, target, positions, resume, redraw).await;
        } else {
            stream_remote_session(socket, state, host_id, id, target, redraw).await;
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
/// backlog and keeps any one `term.write` on the client bounded.
const COALESCE_MAX: usize = 256 * 1024;

/// Merge any pane chunks *already queued* in `rx` into `first`, producing one WS
/// frame instead of many. This adds **no latency** — it only drains what's
/// instantly available via `try_recv` — so a client keeping up still sees one
/// frame per chunk, while a client falling behind (a weak laptop, a slow link)
/// gets fewer, larger frames. That directly cuts the per-frame cost the new
/// push stream would otherwise pile on a slow client: each frame is an
/// `onmessage` dispatch + `Uint8Array` alloc + `term.write` + OSC-title scan, so
/// collapsing a burst of tiny tmux writes into one frame is a large win exactly
/// when the client is the bottleneck. The single-chunk path returns `first`
/// untouched (no copy).
fn coalesce_queued(first: Bytes, rx: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    use tokio::sync::mpsc::error::TryRecvError;
    match rx.try_recv() {
        // Nothing else waiting → forward the lone chunk as-is (zero-copy).
        Err(_) => first,
        Ok(second) => {
            let mut buf = Vec::with_capacity(first.len() + second.len());
            buf.extend_from_slice(&first);
            buf.extend_from_slice(&second);
            while buf.len() < COALESCE_MAX {
                match rx.try_recv() {
                    Ok(more) => buf.extend_from_slice(&more),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            Bytes::from(buf)
        }
    }
}

/// Block until the embedded process's post-SIGWINCH repaint burst has
/// settled, so a `capture-pane` taken afterwards reflects a complete frame
/// rather than a half-painted one. The pane log file (the pipe-pane sink)
/// is a cheap activity probe: bytes the process emits are appended in real
/// time, so file-size growth is direct evidence of repaint work. Wait for
/// activity to start, then for it to quiet (two no-growth polls). Bail
/// early when no activity ever shows (a no-op resize that propagated no
/// SIGWINCH) and hard-cap so an actively-streaming agent can't pin the
/// connect open. Shared by the resize-settle and redraw-nudge paths.
async fn settle_repaint_burst(file: &tokio::fs::File) {
    let mut last_size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut activity_seen = false;
    let mut quiet_streak: u32 = 0;
    let start = tokio::time::Instant::now();
    let max_deadline = start + POST_RESIZE_SETTLE_MAX;
    loop {
        sleep(SETTLE_POLL_INTERVAL).await;
        let now_size = file.metadata().await.map(|m| m.len()).unwrap_or(last_size);
        if now_size != last_size {
            activity_seen = true;
            quiet_streak = 0;
            last_size = now_size;
        } else {
            quiet_streak = quiet_streak.saturating_add(1);
        }
        let now = tokio::time::Instant::now();
        // Activity → quiet: repaint burst is over, capture is safe.
        if activity_seen && quiet_streak >= 2 {
            break;
        }
        // No activity within the bail window: nothing is going to repaint.
        if !activity_seen && now >= start + POST_RESIZE_NO_ACTIVITY_BAIL {
            break;
        }
        // Hard cap against an agent that never goes quiet.
        if now >= max_deadline {
            break;
        }
    }
}

async fn stream_session(
    mut socket: WebSocket,
    id: Uuid,
    target: String,
    positions: Arc<std::sync::Mutex<std::collections::HashMap<Uuid, StreamCheckpoint>>>,
    resume_requested: bool,
    redraw_requested: bool,
) {
    let log_path = match paths::pane_log(&id.to_string()) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[path error: {e}]").into()))
                .await;
            return;
        }
    };

    // Wait briefly for pipe-pane to create the file (it appears milliseconds
    // after `agentum up` returns).
    let mut waited = 0;
    while !log_path.exists() && waited < 50 {
        sleep(Duration::from_millis(100)).await;
        waited += 1;
    }
    if !log_path.exists() {
        let _ = socket
            .send(Message::Text("[no pane log — session not running]".into()))
            .await;
        return;
    }

    let mut file = match tokio::fs::File::open(&log_path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[open error: {e}]").into()))
                .await;
            return;
        }
    };

    // Resize tmux to match the client's viewport BEFORE we snapshot. Without
    // this we capture-pane at tmux's stale pane size (80×24 default for fresh
    // detached sessions, or whatever the previous client sat at), the embedded
    // TUI keeps emitting cursor-position escapes against that size, and the
    // client's vt100 parser — sized to the actual viewport — places the
    // characters in the wrong cells. Symptom: status-line text like "esc to
    // interrupt" overpaints scrollback content and you end up with artefacts
    // like `okterrupt` permanently baked into the scrollback buffer.
    //
    // Modern clients (TUI ≥ 0.6.7, dashboard ≥ 0.6.7) push a `{"resize":...}`
    // text frame within milliseconds of WS open, so this wait almost never
    // reaches the timeout in practice. Old clients fall through to the
    // existing capture-at-current-size path.
    let mut early_input: Vec<Bytes> = Vec::new();
    let mut got_resize = false;
    // Captured on the first resize frame so the resume-replay path can
    // bail out when the client's viewport changed during a disconnect
    // — replaying bytes emitted at a different grid produces visible
    // layout corruption (cursor moves target stale cells).
    let mut current_size: Option<(u16, u16)> = None;
    let resize_deadline = tokio::time::Instant::now() + INITIAL_RESIZE_WAIT;
    loop {
        let remaining = resize_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                if let Some((cols, rows)) = parse_resize(&t) {
                    let _ = agentum_tmux::resize_window(&target, cols, rows).await;
                    got_resize = true;
                    current_size = Some((cols, rows));
                    break;
                }
                // Non-resize text frame — preserve the legacy "treat as raw
                // input" behaviour by buffering for replay after the snapshot.
                early_input.push(Bytes::copy_from_slice(t.as_bytes()));
            }
            Ok(Some(Ok(Message::Binary(b)))) if !b.is_empty() => {
                early_input.push(b);
            }
            Ok(Some(Ok(_))) => {}                  // ping/pong/etc.
            Ok(Some(Err(_))) | Ok(None) => return, // socket already gone
            Err(_) => break,                       // timeout — fall through with no resize
        }
    }
    if got_resize {
        // Wait for the embedded process's post-SIGWINCH repaint burst to
        // settle before snapshotting. Fixed sleeps don't work: idle panes
        // are quiet immediately, but a ratatui-based agent (claude code,
        // codex, opencode) reacting to a real size change can take well
        // over 100 ms to start emitting its full repaint, then several
        // dozen ms more to finish it. Capturing during that window
        // returned a half-painted frame — tool indicator drawn but
        // input box / footer missing, or status-line characters
        // overpainting scrollback content because cursor moves still
        // referenced the old grid.
        //
        // The pane log file (pipe-pane sink) gives us a cheap activity
        // probe: bytes the embedded process emits are appended in real
        // time, so file-size growth is direct evidence of repaint
        // activity. Wait for activity to start, then for it to quiet.
        // Fall back to a "no activity" bail-out if the resize was a
        // no-op (size already matched), so connect doesn't pay the full
        // budget for a settle that will never come.
        settle_repaint_burst(&file).await;
    }

    // Redraw heal: force the agent to repaint EVERY cell before we snapshot.
    // The reconnect after a system suspend (or any foreign write into the
    // pane grid — an OS `wall` broadcast lands on top of the input box and
    // footer) leaves stale bytes a ratatui app won't overpaint on its own,
    // and a same-size reconnect emits no SIGWINCH so the agent never
    // repaints. We provoke one with a momentary 1-row shrink-then-restore:
    // two SIGWINCHs, each making the agent clear its buffer and redraw in
    // full, netting to the original geometry. The intermediate settle gives
    // the app time to observe the smaller size and start repainting before
    // we restore — two resizes delivered too close together can collapse
    // into a single read of the final (unchanged) size and skip the redraw.
    // The post-restore settle lets the clean frame land in the log before
    // the snapshot below captures it. No-op when we never learned the
    // client's size or the pane is too short to shrink.
    if redraw_requested && let Some((cols, rows)) = current_size {
        let shrunk = rows.saturating_sub(1);
        if shrunk >= 1 && shrunk != rows {
            let _ = agentum_tmux::resize_window(&target, cols, shrunk).await;
            settle_repaint_burst(&file).await;
            let _ = agentum_tmux::resize_window(&target, cols, rows).await;
            settle_repaint_burst(&file).await;
        }
    }

    // Replay path. Two modes:
    //
    // 1. RESUME: client has cached parser state and just wants the bytes
    //    it missed during the WS gap. Look up the saved log position and
    //    forward `log[saved..end]` as binary. The client's parser was
    //    not reset, so playing back those bytes brings it from the
    //    pre-disconnect state to the live tail without clobbering
    //    anything. Without this, switching agents and switching back
    //    used to wipe all visible chat history because we'd send a
    //    `capture-pane` snapshot reflecting whatever the embedded TUI's
    //    UI looks like *now* — which after a task completes can be
    //    almost empty.
    //
    // 2. FRESH SNAPSHOT (default): client has no cached state (or its
    //    cached state is invalid because the pane size changed during
    //    the disconnect), so we `capture-pane -e` the current visible
    //    grid and ship it after an `ESC c` (RIS) full parser reset.
    let mut snapshot_sent = false;
    // Resume only if we have a saved checkpoint AND its pane size matches
    // the current viewport. Two guards:
    //
    //  1. `unwrap_or(0)` after a daemon restart wiped `stream_positions`
    //     (in-memory only) made the daemon ship the ENTIRE log as
    //     "delta" on top of the client's existing parser state —
    //     duplicate footer/content baked into scrollback every time
    //     the daemon was bounced. v0.6.26 fixed this by falling through
    //     to the fresh snapshot path when no checkpoint exists.
    //
    //  2. Replaying bytes emitted at a stale grid size (e.g., user
    //     dragged their tmux window during the disconnect) places
    //     cursor moves and line wraps against the wrong cells, so the
    //     visible layout ends up corrupted in ways that survive in the
    //     vt100 parser's history. Mismatch → fall through to a fresh
    //     snapshot at the new size and let the client's parser reset.
    let saved_checkpoint: Option<StreamCheckpoint> =
        positions.lock().ok().and_then(|map| map.get(&id).copied());
    let resume_size_matches = match (saved_checkpoint, current_size) {
        (Some(cp), Some((cols, rows))) => cp.cols == cols && cp.rows == rows,
        // No first-resize frame from the client, or no checkpoint — let
        // the existing "resume only with checkpoint" gate handle it.
        (Some(_), None) => true,
        _ => false,
    };
    // A redraw heal always wants the fresh snapshot of the repainted grid,
    // never a delta replay (which would just re-feed the corrupting bytes).
    // Clients pair `redraw` with omitting `resume`, but gate here too so a
    // client sending both still heals.
    if let (true, Some(cp), true) = (
        resume_requested && !redraw_requested,
        saved_checkpoint,
        resume_size_matches,
    ) {
        if let Ok(end) = file.seek(std::io::SeekFrom::End(0)).await
            && end >= cp.pos
        {
            let delta = end - cp.pos;
            if delta > 0 && file.seek(std::io::SeekFrom::Start(cp.pos)).await.is_ok() {
                let mut buf = vec![0u8; delta as usize];
                if file.read_exact(&mut buf).await.is_ok()
                    && socket
                        .send(Message::Binary(Bytes::from(buf)))
                        .await
                        .is_err()
                {
                    return;
                }
            }
            // Position file at end so tail picks up only post-delta bytes.
            let _ = file.seek(std::io::SeekFrom::End(0)).await;
            snapshot_sent = true;
        }
    }
    if !snapshot_sent
        && let Ok(snap) = agentum_tmux::capture_pane_ansi(&target).await
        && !snap.is_empty()
    {
        // Pin the tail's replay point AFTER capturing, BEFORE sending: the
        // snapshot reflects pane state at capture time, so the tail must resume
        // just past it. Bytes emitted during the (possibly slow) socket send
        // land after this offset and stream through the tail exactly once. The
        // earlier order pinned End BEFORE the capture, replaying the
        // capture-window bytes on top of the snapshot — harmless for an
        // alt-screen app, but for a normal-screen agent that redraws with
        // RELATIVE cursor motion (cursor-agent: ESC[1A + ESC[2K) that duplicate
        // desynced the cursor and stacked spinner lines ("Composing…
        // Composing…"). The trade is a sub-ms gap (bytes emitted *during*
        // capture-pane), self-healed by the agent's next redraw — far cheaper
        // than permanent stacking.
        let _ = file.seek(std::io::SeekFrom::End(0)).await;
        // Reset the client parser before painting the snapshot so EVERY
        // bit of stale state from the previous session is discarded —
        // not just the visible grid.
        //
        //   ESC c (RIS, "Reset to Initial State")
        //
        // This is more thorough than the previous `\x1b[2J\x1b[H`
        // (clear-screen + cursor-home), which left SGR colors, saved
        // cursor positions, scroll regions, alternate-screen state,
        // application keypad/cursor mode, and mouse-tracking modes
        // intact. Carrying any of those across a session-switch (or
        // a crash-and-resume on a different agent type) showed up as
        // hard-to-pin-down corruption: text in the wrong color long
        // after the agent stopped emitting that SGR, scroll regions
        // clipping vt100-parser updates to a strip of the screen,
        // mouse events firing on a session that never asked for them.
        let mut payload = Vec::with_capacity(snap.len() + 4);
        payload.extend_from_slice(b"\x1bc");
        payload.extend_from_slice(&snap);
        if socket
            .send(Message::Binary(Bytes::from(payload)))
            .await
            .is_err()
        {
            return;
        }
        snapshot_sent = true;
        // The file cursor was pinned at the post-capture end above; no re-seek —
        // anything appended since then replays through the tail exactly once.
    }

    // Fallback: if capture-pane didn't yield anything (early in session
    // life, before tmux has rendered, or for non-tmux sessions), keep the
    // old 4 KB tail behaviour so users still see *something* on connect.
    if !snapshot_sent && let Ok(end) = file.seek(std::io::SeekFrom::End(0)).await {
        let backfill = end.min(BACKFILL_BYTES);
        if backfill > 0
            && file
                .seek(std::io::SeekFrom::End(-(backfill as i64)))
                .await
                .is_ok()
        {
            let mut backfill_buf = vec![0u8; backfill as usize];
            if file.read_exact(&mut backfill_buf).await.is_ok()
                && socket
                    .send(Message::Binary(Bytes::from(backfill_buf)))
                    .await
                    .is_err()
            {
                return;
            }
        }
    }

    // Replay any non-resize input that arrived during the resize-wait window.
    // Rare in practice (a fast typer connecting and hammering keys before the
    // first frame fires), but preserves the previous "every byte forwarded"
    // contract so we never silently drop a keystroke at connect time.
    for chunk in early_input {
        let _ = agentum_tmux::send_bytes(&target, &chunk).await;
    }

    // Tail the pane log on a dedicated task and pipe chunks through an mpsc.
    // The main loop multiplexes `tail_rx` (output) and `socket.recv()` (input)
    // so a chatty pane never starves keystrokes — and vice versa.
    //
    // We also remember the log position the tail starts from so the outer
    // loop can save where the client left off on disconnect — that's what
    // makes `{"resume":true}` reconnects deliver only the missed delta.
    let tail_start_pos = file.stream_position().await.unwrap_or(0);
    let (tail_tx, mut tail_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    let tail_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match file.read(&mut buf).await {
                Ok(0) => sleep(Duration::from_millis(80)).await,
                Ok(n) => {
                    if tail_tx
                        .send(Bytes::copy_from_slice(&buf[..n]))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut bytes_forwarded: u64 = 0;
    // Why: tmux consumes the agent's OSC title (set-titles off) so it never
    // reaches the client through the pane byte stream — the desktop's
    // title-derived agent-status (working/idle/needs-attention) had no input.
    // Poll the captured pane_title and re-inject it as a synthetic OSC title
    // whenever it changes. Invisible to the terminal grid; the desktop's
    // title pipeline extracts it. ~400ms keeps the sidebar dot responsive
    // without hammering tmux.
    let mut title_ticker = tokio::time::interval(Duration::from_millis(400));
    let mut last_pane_title = String::new();
    loop {
        tokio::select! {
            _ = title_ticker.tick() => {
                if let Ok(title) = agentum_tmux::pane_title(&target).await
                    && !title.is_empty()
                    && title != last_pane_title
                {
                    last_pane_title = title.clone();
                    let mut osc = Vec::with_capacity(title.len() + 5);
                    osc.extend_from_slice(b"\x1b]0;");
                    osc.extend_from_slice(title.as_bytes());
                    osc.push(0x07);
                    if socket.send(Message::Binary(Bytes::from(osc))).await.is_err() {
                        break;
                    }
                }
            }
            chunk = tail_rx.recv() => match chunk {
                Some(bytes) => {
                    // Coalesce any backlog into one frame (no added latency).
                    // Byte total is unchanged, so the checkpoint stays accurate.
                    let frame = coalesce_queued(bytes, &mut tail_rx);
                    let len = frame.len() as u64;
                    if socket.send(Message::Binary(frame)).await.is_err() {
                        break;
                    }
                    bytes_forwarded += len;
                    // Keep the checkpoint live so a concurrent reconnect
                    // can take the (cheap) delta path instead of the
                    // (destructive) snapshot path. Without this, the
                    // checkpoint only updates at disconnect — and the
                    // disconnect write loses the race against any
                    // reconnect that arrives in the same millisecond.
                    save_checkpoint(
                        &positions,
                        id,
                        tail_start_pos.saturating_add(bytes_forwarded),
                        current_size,
                    );
                }
                None => break, // tail task ended (file error / eof on dead pane)
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) if !b.is_empty() => {
                    if let Err(e) = agentum_tmux::send_bytes(&target, &b).await
                        && socket
                            .send(Message::Text(format!("[input dropped: {e}]").into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    // Text frames double as a side-channel for control
                    // messages — `{"resize":{"cols":N,"rows":N}}` and
                    // `{"refresh":true}`. Anything that isn't a recognised
                    // JSON envelope is forwarded as raw input bytes
                    // (preserves the old behaviour for clients that send
                    // keystrokes as text).
                    if parse_refresh(&t) {
                        // Client asked for a clean re-snapshot. Same
                        // payload shape as the initial fresh-snapshot
                        // path: parser reset (RIS) + current visible
                        // grid. Cheap and side-effect-free on tmux.
                        if let Ok(snap) = agentum_tmux::capture_pane_ansi(&target).await
                            && !snap.is_empty()
                        {
                            let mut payload = Vec::with_capacity(snap.len() + 4);
                            payload.extend_from_slice(b"\x1bc");
                            payload.extend_from_slice(&snap);
                            if socket
                                .send(Message::Binary(Bytes::from(payload)))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    } else if let Some((cols, rows)) = parse_resize(&t) {
                        // Track every successful resize so the disconnect
                        // checkpoint records the size at the time the
                        // client actually left, not the size we captured
                        // in the early-input window.
                        current_size = Some((cols, rows));
                        // Refresh the live checkpoint with the new size
                        // so a concurrent reconnect's size-match gate
                        // doesn't fall back to a fresh snapshot just
                        // because the saved size is stale.
                        save_checkpoint(
                            &positions,
                            id,
                            tail_start_pos.saturating_add(bytes_forwarded),
                            current_size,
                        );
                        if let Err(e) = agentum_tmux::resize_window(&target, cols, rows).await
                            && socket
                                .send(Message::Text(format!("[resize dropped: {e}]").into()))
                                .await
                                .is_err()
                        {
                            break;
                        }
                    } else if let Err(e) = agentum_tmux::send_bytes(&target, t.as_bytes()).await
                        && socket
                            .send(Message::Text(format!("[input dropped: {e}]").into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    tail_handle.abort();
    // Final save on disconnect — typically redundant now that we stamp
    // the checkpoint live during the forward loop, but keeps the
    // invariant that the last byte forwarded is reflected even if the
    // very last `save_checkpoint` call lost a race with the abort.
    save_checkpoint(
        &positions,
        id,
        tail_start_pos.saturating_add(bytes_forwarded),
        current_size,
    );
}

/// Terminate a long-lived SSH child and explicitly reap it. A bounded failure
/// is propagated to the host mutation barrier, which then fails closed instead
/// of committing a new host revision while an old-revision child may survive.
async fn kill_and_reap_remote_stream_child(
    child: &mut tokio::process::Child,
    label: &str,
) -> RemoteStreamCleanup {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(error) => return Err(format!("could not inspect {label}: {error}")),
    }

    let kill_error = child.start_kill().err();
    match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(format!("could not reap {label}: {error}")),
        Err(_) => {
            let suffix = kill_error
                .map(|error| format!(" (kill request also failed: {error})"))
                .unwrap_or_default();
            Err(format!("timed out reaping {label}{suffix}"))
        }
    }
}

const TAIL_RESPAWN_ATTEMPTS: u32 = 6;
const TAIL_POOLED_ATTEMPTS: u32 = 3;
const INPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const REMOTE_TITLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const REMOTE_TITLE_POLL_IDLE_TICKS: u32 = 2;
const REMOTE_TAIL_LAG_CONFIRMATIONS: u8 = 2;

/// Durable progress comparison for one remote `tail -f`. A single observation
/// of remote bytes ahead of locally consumed bytes can be an ordinary race;
/// only a persistent discrepancy proves the live SSH child stopped forwarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteTailProgress {
    consumed_offset: Option<u64>,
    lag_observations: u8,
}

impl RemoteTailProgress {
    fn new(offset: Option<u64>) -> Self {
        Self {
            consumed_offset: offset,
            lag_observations: 0,
        }
    }

    fn received(&mut self, bytes: usize) {
        self.consumed_offset = self
            .consumed_offset
            .map(|offset| offset.saturating_add(bytes as u64));
        self.lag_observations = 0;
    }

    fn reset(&mut self, offset: u64) {
        self.consumed_offset = Some(offset);
        self.lag_observations = 0;
    }

    /// Returns true only after the remote log has remained ahead across the
    /// configured number of probes with no intervening tail delivery.
    fn observe(&mut self, remote_size: u64) -> bool {
        let Some(consumed) = self.consumed_offset else {
            self.reset(remote_size);
            return false;
        };
        if remote_size <= consumed {
            if remote_size < consumed {
                // Log replacement/truncation starts a new monotonic baseline.
                self.consumed_offset = Some(remote_size);
            }
            self.lag_observations = 0;
            return false;
        }
        self.lag_observations = self.lag_observations.saturating_add(1);
        self.lag_observations >= REMOTE_TAIL_LAG_CONFIRMATIONS
    }
}

fn tail_respawn_backoff(attempt: u32) -> Duration {
    let millis = 250u64.saturating_mul(1u64 << attempt.min(4));
    Duration::from_millis(millis.min(3_000))
}

/// First try the pooled streaming master. If repeated attempts cannot open a
/// usable tail, evict it exactly once and keep the rest of this recovery cycle
/// on fresh connections so a dead/saturated pool cannot trap every retry.
fn tail_reconnect_plan(attempt: u32) -> (SshMux, bool) {
    if attempt < TAIL_POOLED_ATTEMPTS {
        (SshMux::Streaming, false)
    } else {
        (SshMux::Off, attempt == TAIL_POOLED_ATTEMPTS)
    }
}

fn remote_title_poll_due(bytes_since_poll: bool, ticks_since_poll: u32) -> bool {
    bytes_since_poll || ticks_since_poll >= REMOTE_TITLE_POLL_IDLE_TICKS
}

async fn write_remote_input_with_timeout<W>(
    writer: &mut W,
    bytes: &[u8],
    deadline: Duration,
) -> bool
where
    W: tokio::io::AsyncWrite + Unpin,
{
    matches!(
        tokio::time::timeout(deadline, async {
            writer.write_all(bytes).await?;
            writer.flush().await
        })
        .await,
        Ok(Ok(()))
    )
}

async fn queue_remote_input_unless_cancelled(
    sender: &tokio::sync::mpsc::Sender<Vec<u8>>,
    bytes: Vec<u8>,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    if *cancel.borrow() {
        return false;
    }
    tokio::select! {
        biased;
        _ = cancel.changed() => false,
        result = sender.send(bytes) => result.is_ok(),
    }
}

/// One long-lived remote tail plus the tasks that drain both of its pipes. The
/// stream registry cannot acknowledge host mutation until `shutdown` explicitly
/// kills/reaps the child and ends these tasks.
struct RemoteTailPump {
    child: tokio::process::Child,
    receiver: tokio::sync::mpsc::Receiver<Bytes>,
    stdout_handle: tokio::task::JoinHandle<()>,
    stderr_handle: Option<tokio::task::JoinHandle<()>>,
}

impl RemoteTailPump {
    async fn spawn(
        host: &Host,
        log: &std::path::Path,
        offset: Option<u64>,
        mux: SshMux,
    ) -> Result<Self, String> {
        let mut child = crate::host_runtime::spawn_remote_pane_tail(host, log, offset, mux)
            .map_err(|error| error.to_string())?;
        let Some(mut stdout) = child.stdout.take() else {
            let cleanup = kill_and_reap_remote_stream_child(&mut child, "remote pane tail").await;
            return Err(match cleanup {
                Ok(()) => "remote pane tail stdout was unavailable".into(),
                Err(error) => format!("remote pane tail stdout was unavailable; {error}"),
            });
        };
        let stderr_handle = child.stderr.take().map(|mut stderr| {
            let label = log
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_string();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                if stderr.read_to_end(&mut bytes).await.is_ok() && !bytes.is_empty() {
                    tracing::warn!(
                        session = %label,
                        "remote pane tail ended: {}",
                        String::from_utf8_lossy(&bytes).trim()
                    );
                }
            })
        });
        let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(64);
        let stdout_handle = tokio::spawn(async move {
            let mut buffer = vec![0u8; READ_CHUNK];
            loop {
                match stdout.read(&mut buffer).await {
                    // EOF means the SSH channel died; closing the sender wakes
                    // the owning WebSocket loop so it can recover in place.
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender
                            .send(Bytes::copy_from_slice(&buffer[..read]))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            receiver,
            stdout_handle,
            stderr_handle,
        })
    }

    async fn shutdown(mut self) -> RemoteStreamCleanup {
        let cleanup = kill_and_reap_remote_stream_child(&mut self.child, "remote pane tail").await;
        self.stdout_handle.abort();
        let _ = self.stdout_handle.await;
        if let Some(mut stderr_handle) = self.stderr_handle
            && tokio::time::timeout(Duration::from_millis(500), &mut stderr_handle)
                .await
                .is_err()
        {
            stderr_handle.abort();
            let _ = stderr_handle.await;
        }
        cleanup
    }
}

/// Recover a tail without tearing down the client WebSocket. Every attempt
/// reloads the saved host under its lifecycle lease, samples the snapshot and
/// log offset together, then starts the replacement tail before repainting.
async fn reestablish_remote_tail(
    state: &AppState,
    host_id: Uuid,
    id: Uuid,
    target: &str,
    log: &std::path::Path,
    socket: &mut WebSocket,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Option<(RemoteTailPump, u64)> {
    for attempt in 0..TAIL_RESPAWN_ATTEMPTS {
        if *cancel.borrow() {
            return None;
        }
        tokio::select! {
            biased;
            _ = cancel.changed() => return None,
            _ = sleep(tail_respawn_backoff(attempt)) => {}
        }

        let (host_guard, host) = match acquire_current_stream_host_unless_cancelled(
            state, host_id, cancel,
        )
        .await
        {
            Ok(Some(current)) => current,
            Ok(None) => return None,
            Err(error) => {
                tracing::debug!(session = %id, %host_id, %error, "remote tail recovery could not reload host");
                continue;
            }
        };

        let (mux, evict_first) = tail_reconnect_plan(attempt);
        if evict_first {
            crate::host_runtime::evict_ssh_master(&host, SshMux::Streaming).await;
        }
        let capture = crate::host_runtime::capture_pane_with_log_offset(&host, target, log).await;
        let (offset, snapshot) = match capture {
            Ok(capture) => capture,
            Err(error) => {
                drop(host_guard);
                tracing::debug!(session = %id, %host_id, %error, "remote tail recovery snapshot failed");
                continue;
            }
        };
        let replacement = RemoteTailPump::spawn(&host, log, Some(offset), mux).await;
        drop(host_guard);
        let replacement = match replacement {
            Ok(replacement) => replacement,
            Err(error) => {
                tracing::debug!(session = %id, %host_id, %error, "remote tail recovery spawn failed");
                continue;
            }
        };

        if !snapshot.is_empty() {
            let mut payload = Vec::with_capacity(snapshot.len() + 2);
            payload.extend_from_slice(b"\x1bc");
            payload.extend_from_slice(&snapshot);
            if !send_remote_stream_message_unless_cancelled(
                socket,
                Message::Binary(Bytes::from(payload)),
                cancel,
            )
            .await
            {
                let _ = replacement.shutdown().await;
                return None;
            }
        }
        return Some((replacement, offset));
    }

    let _ = send_remote_stream_message_unless_cancelled(
        socket,
        Message::Text("[remote stream interrupted — reconnecting]".into()),
        cancel,
    )
    .await;
    None
}

/// Remote (SSH) session stream — the push-based mirror of [`stream_session`].
///
/// Previously this polled `capture-pane` over SSH every 700 ms and re-sent a
/// full-screen snapshot (RIS + whole pane) on any change, which made remote
/// terminals lag up to 700 ms behind and flicker as the client cleared and
/// repainted on every tick. Now we follow the remote pane log incrementally:
/// `pipe_pane` (armed at session start, re-armed here for safety) appends raw
/// pane bytes to a per-session log on the host, and a single persistent
/// `ssh tail -f` streams those bytes as they appear — the same incremental
/// model as the local file tail, just sourced over one long-lived SSH channel.
async fn stream_remote_session(
    mut socket: WebSocket,
    state: AppState,
    host_id: Uuid,
    id: Uuid,
    target: String,
    redraw_requested: bool,
) {
    let log = match paths::pane_log(&id.to_string()) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[path error: {e}]").into()))
                .await;
            return;
        }
    };

    // Current screen state: pipe-pane only carries output produced *after* it was
    // armed, so a fresh connect (or an idle pane) needs one snapshot to paint
    // what's already there. RIS (`\x1bc`) resets the client parser first — same
    // payload shape as a fresh local connect.
    //
    // `capture_pane_with_log_offset` ALSO re-arms pipe-pane (idempotent `-o`) in
    // the same remote exec, so the old separate arm round-trip is gone — one
    // SSH call at connect instead of two, halving the time-to-first-paint on a
    // distant host.
    //
    // The log's byte size is sampled in the SAME remote exec as the snapshot, and
    // the tail below replays from that offset. Previously the tail started at EOF
    // *at attach time*, and its (deliberately unmultiplexed) SSH connection takes
    // a full handshake to attach — every byte a busy agent emitted in that
    // multi-second window was silently dropped, and a chunk lost mid-escape-
    // sequence left the terminal corrupted until a manual refresh.
    // Capture exactly once, even when the pane is still blank. The same command
    // arms pipe-pane and returns the byte offset paired with that snapshot, so
    // the live tail below will deliver the first frame as soon as the process
    // draws it. Retrying an empty capture delayed tail startup by as much as 12
    // seconds and made a valid idle shell look disconnected.
    // Redraw heal (see the local path and the `redraw` query doc): force the
    // remote agent to fully repaint before we snapshot, so a corrupted grid
    // (e.g. an OS `wall` broadcast written over the pane on the host) is
    // overpainted rather than re-captured. We don't learn the pane's absolute
    // size at connect here, so provoke the SIGWINCH with a RELATIVE 1-row
    // shrink-then-restore that nets to the original geometry. No file probe on
    // a remote host, so use a fixed inter-resize pause long enough for the
    // agent to observe the smaller size and start repainting; the snapshot
    // below then captures the clean frame.
    if redraw_requested {
        const REMOTE_NUDGE_PAUSE: Duration = Duration::from_millis(150);
        if let Ok((_host_guard, host)) = acquire_current_stream_host(&state, host_id).await {
            let _ = crate::host_runtime::resize_window_relative(&host, &target, -1).await;
        }
        sleep(REMOTE_NUDGE_PAUSE).await;
        if let Ok((_host_guard, host)) = acquire_current_stream_host(&state, host_id).await {
            let _ = crate::host_runtime::resize_window_relative(&host, &target, 1).await;
        }
        sleep(REMOTE_NUDGE_PAUSE).await;
    }

    let mut log_offset: Option<u64> = None;
    match acquire_current_stream_host(&state, host_id).await {
        Ok((_host_guard, host)) => {
            match crate::host_runtime::capture_pane_with_log_offset(&host, &target, &log).await {
                Ok((offset, snapshot)) => {
                    log_offset = Some(offset);
                    if !snapshot.is_empty() {
                        let mut payload = Vec::with_capacity(snapshot.len() + 2);
                        payload.extend_from_slice(b"\x1bc");
                        payload.extend_from_slice(&snapshot);
                        if socket
                            .send(Message::Binary(Bytes::from(payload)))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!(session = %id, %host_id, %error, "remote pane snapshot failed; starting live tail immediately");
                }
            }
        }
        Err(error) => {
            tracing::debug!(session = %id, %host_id, %error, "remote pane snapshot could not reload host; starting live tail immediately");
        }
    }

    // Persistent `ssh tail -f` of the remote pane log. Its stdout is pumped
    // through a bounded channel so the select loop multiplexes output and
    // keystrokes. Initial channel refusal uses the same bounded recovery as a
    // mid-stream failure instead of opening a blank pane and immediately
    // tearing down the WebSocket.
    let (host_guard, host) = match acquire_current_stream_host(&state, host_id).await {
        Ok(current) => current,
        Err(error) => {
            let _ = socket
                .send(Message::Text(
                    format!("[remote tail error: {error}]").into(),
                ))
                .await;
            return;
        }
    };
    // Registration and spawn are one host-lease transaction. A PUT cannot
    // cross this point without seeing the child in the cancellation registry.
    let (registration, mut cancel_rx) = register_remote_stream(host_id);
    let tail_spawn = RemoteTailPump::spawn(&host, &log, log_offset, SshMux::Streaming).await;
    drop(host_guard);
    let (mut tail, initial_tail_offset) = match tail_spawn {
        Ok(tail) => (Some(tail), log_offset),
        Err(error) => {
            tracing::debug!(session = %id, %host_id, %error, "initial remote tail spawn failed");
            match reestablish_remote_tail(
                &state,
                host_id,
                id,
                &target,
                &log,
                &mut socket,
                &mut cancel_rx,
            )
            .await
            {
                Some((tail, offset)) => (Some(tail), Some(offset)),
                None => {
                    registration.finish(Ok(()));
                    return;
                }
            }
        }
    };
    let mut tail_progress = RemoteTailProgress::new(initial_tail_offset);

    // Input writer: keystrokes leave the select loop through an mpsc and a
    // dedicated task delivers them. The fast path is a PERSISTENT SSH channel
    // ([`spawn_remote_input_writer`]): each keystroke is a one-way write down an
    // already-open stream (~1 RTT, ~150 ms to a distant host) instead of the old
    // exec-per-keystroke (a fresh `tmux send-keys` channel open + round-trip,
    // ~450 ms measured — which made typing into a remote agent unusable). If the
    // persistent writer can't spawn (or dies), we fall back to the per-exec
    // `send_bytes` so input still works, just slower.
    //
    // Either way the loop coalesces whatever queued while the previous write was
    // in flight (fast typing, paste) into one write. The WebSocket task applies
    // backpressure at the bounded queue instead of silently dropping accepted
    // input, while cancellation can still wake it immediately.
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let mut input_handle = {
        let state = state.clone();
        let target = target.clone();
        let mut input_cancel = cancel_rx.clone();
        tokio::spawn(async move {
            // Pre-open the writer while the streaming master is hot so the
            // first keystroke is a one-way write, not a channel-open round trip.
            let mut writer = match acquire_current_stream_host_unless_cancelled(
                &state,
                host_id,
                &mut input_cancel,
            )
            .await
            {
                Ok(Some((_host_guard, host))) => {
                    crate::host_runtime::spawn_remote_input_writer(&host, &target).ok()
                }
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!(%host_id, %error, "remote input writer could not reload host");
                    None
                }
            };
            let mut stdin = writer.as_mut().and_then(|child| child.stdin.take());
            let mut cleanup_errors = Vec::new();
            'input: loop {
                let first = tokio::select! {
                    biased;
                    _ = input_cancel.changed() => break 'input,
                    first = input_rx.recv() => match first {
                        Some(first) => first,
                        None => break 'input,
                    },
                };
                let mut buf = first;
                // Drain whatever queued while the previous write was in flight.
                // The encoder independently splits an arbitrarily large first
                // frame at the shared tmux-safe boundary.
                while buf.len() < 4096 {
                    match input_rx.try_recv() {
                        Ok(more) => buf.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }

                // Restore the fast persistent path after an initial spawn
                // failure or a prior broken/wedged child. Reload the saved host
                // while holding its lifecycle lease so old credentials cannot
                // reappear after a PUT.
                if stdin.is_none() {
                    if let Some(mut stale) = writer.take()
                        && let Err(error) =
                            kill_and_reap_remote_stream_child(&mut stale, "remote input writer")
                                .await
                    {
                        cleanup_errors.push(error);
                    }
                    match acquire_current_stream_host_unless_cancelled(
                        &state,
                        host_id,
                        &mut input_cancel,
                    )
                    .await
                    {
                        Ok(Some((_host_guard, host))) => {
                            if let Ok(mut child) =
                                crate::host_runtime::spawn_remote_input_writer(&host, &target)
                            {
                                stdin = child.stdin.take();
                                writer = Some(child);
                            }
                        }
                        Ok(None) => break 'input,
                        Err(error) => {
                            tracing::warn!(%host_id, %error, "remote input writer could not reload host");
                        }
                    }
                }

                let mut delivered = false;
                if let Some(si) = stdin.as_mut() {
                    // This is the exact framing used by the working Agentum
                    // server: newline-framed base64 decoded into tmux's own
                    // paste buffer. This preserves arbitrary bytes and lets
                    // tmux drain large pastes without overflowing the pane PTY.
                    let frames = crate::host_runtime::encode_remote_input_lines(&buf);
                    let wrote = tokio::select! {
                        biased;
                        _ = input_cancel.changed() => break 'input,
                        result = write_remote_input_with_timeout(si, &frames, INPUT_WRITE_TIMEOUT) => result,
                    };
                    if wrote {
                        delivered = true;
                    } else {
                        // A broken pipe or a master whose far end stopped
                        // draining must never freeze every later keystroke.
                        stdin = None;
                        if let Some(mut failed) = writer.take()
                            && let Err(error) = kill_and_reap_remote_stream_child(
                                &mut failed,
                                "remote input writer",
                            )
                            .await
                        {
                            cleanup_errors.push(error);
                        }
                    }
                }
                if !delivered {
                    // A failed persistent channel usually means a host PUT
                    // closed its old ControlMaster. Reload under the shared
                    // lease so fallback input cannot recreate that old
                    // credential revision.
                    let send_result = match acquire_current_stream_host_unless_cancelled(
                        &state,
                        host_id,
                        &mut input_cancel,
                    )
                    .await
                    {
                        Ok(Some((_host_guard, host))) => {
                            crate::host_runtime::send_bytes(&host, &target, &buf)
                                .await
                                .map_err(|error| error.to_string())
                        }
                        Ok(None) => break 'input,
                        Err(error) => Err(error.to_string()),
                    };
                    if let Err(error) = send_result {
                        tracing::warn!(target = %target, %error, "remote input send failed");
                    }
                }
            }

            if let Some(mut writer) = writer
                && let Err(error) =
                    kill_and_reap_remote_stream_child(&mut writer, "remote input writer").await
            {
                cleanup_errors.push(error);
            }
            if cleanup_errors.is_empty() {
                Ok(())
            } else {
                Err(cleanup_errors.join("; "))
            }
        })
    };

    // Pane title and tail liveness share one background metadata probe on the
    // observer master. Never await its SSH round trip inside the ticker branch:
    // a slow probe must not pause keyboard or pane-byte dispatch. The remote log
    // size lets us distinguish a healthy idle pane from a live local ssh child
    // that silently stopped forwarding newly appended bytes.
    // The observer pool makes this cheap tick independent of both keystrokes
    // and pane bytes. Idle sessions still skip every other tick, while a silent
    // tail with new remote bytes is confirmed and repaired in about 2-3s
    // instead of remaining frozen until another user event.
    let mut title_ticker = tokio::time::interval(REMOTE_TITLE_POLL_INTERVAL);
    let mut last_pane_title = String::new();
    let mut bytes_since_title_poll = true;
    let mut ticks_since_title_poll = 0u32;
    let mut pane_state_probe: Option<
        tokio::task::JoinHandle<Result<crate::host_runtime::RemotePaneStreamState, String>>,
    > = None;
    let mut tail_cleanup_errors = Vec::new();
    loop {
        // Deliberately fair. A ratatui/agent pane can keep the tail receiver
        // continuously ready; output-first `biased` selection then prevents
        // `socket.recv()` from ever accepting keyboard bytes. The working
        // Agentum stream uses Tokio's fair selection for this exact mux.
        tokio::select! {
            _ = cancel_rx.changed() => break,
            _ = title_ticker.tick() => {
                ticks_since_title_poll = ticks_since_title_poll.saturating_add(1);
                if !remote_title_poll_due(bytes_since_title_poll, ticks_since_title_poll) {
                    continue;
                }
                if pane_state_probe.is_some() {
                    continue;
                }
                bytes_since_title_poll = false;
                ticks_since_title_poll = 0;
                let probe_state = state.clone();
                let probe_target = target.clone();
                let probe_log = log.clone();
                let mut probe_cancel = cancel_rx.clone();
                pane_state_probe = Some(tokio::spawn(async move {
                    let Some((_host_guard, host)) = acquire_current_stream_host_unless_cancelled(
                        &probe_state,
                        host_id,
                        &mut probe_cancel,
                    )
                    .await
                    .map_err(|error| error.to_string())?
                    else {
                        return Err("remote pane-state probe cancelled".into());
                    };
                    tokio::select! {
                        biased;
                        _ = probe_cancel.changed() => Err("remote pane-state probe cancelled".into()),
                        result = crate::host_runtime::remote_pane_stream_state(
                            &host,
                            &probe_target,
                            &probe_log,
                        ) => result.map_err(|error| error.to_string()),
                    }
                }));
            }
            probe_result = async {
                pane_state_probe
                    .as_mut()
                    .expect("guarded pane-state probe")
                    .await
            }, if pane_state_probe.is_some() => {
                let _finished_probe = pane_state_probe.take();
                let pane_state = match probe_result {
                    Ok(Ok(pane_state)) => pane_state,
                    Ok(Err(error)) => {
                        tracing::debug!(session = %id, %host_id, %error, "remote pane-state probe failed");
                        continue;
                    }
                    Err(error) => {
                        tracing::debug!(session = %id, %host_id, %error, "remote pane-state probe task failed");
                        continue;
                    }
                };

                if tail_progress.observe(pane_state.log_size) {
                    tracing::warn!(
                        session = %id,
                        %host_id,
                        remote_log_size = pane_state.log_size,
                        consumed_offset = ?tail_progress.consumed_offset,
                        "remote pane tail stopped forwarding; recovering without user input"
                    );
                    let wedged = tail.take().expect("remote tail is present");
                    if let Err(error) = wedged.shutdown().await {
                        tail_cleanup_errors.push(error);
                        break;
                    }
                    match acquire_current_stream_host_unless_cancelled(
                        &state,
                        host_id,
                        &mut cancel_rx,
                    )
                    .await
                    {
                        Ok(Some((_host_guard, host))) => {
                            if let Err(error) = crate::host_runtime::repair_ssh_master_role(
                                &host,
                                SshMux::Streaming,
                            )
                            .await
                            {
                                tracing::debug!(session = %id, %host_id, %error, "streaming master repair deferred to tail reconnect");
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::debug!(session = %id, %host_id, %error, "streaming master repair could not reload host");
                        }
                    }
                    match reestablish_remote_tail(
                        &state,
                        host_id,
                        id,
                        &target,
                        &log,
                        &mut socket,
                        &mut cancel_rx,
                    )
                    .await
                    {
                        Some((replacement, offset)) => {
                            tail = Some(replacement);
                            tail_progress.reset(offset);
                        }
                        None => break,
                    }
                }

                let title = pane_state.title;
                if !title.is_empty() && title != last_pane_title {
                    last_pane_title = title.clone();
                    let mut osc = Vec::with_capacity(title.len() + 5);
                    osc.extend_from_slice(b"\x1b]0;");
                    osc.extend_from_slice(title.as_bytes());
                    osc.push(0x07);
                    if !send_remote_stream_message_unless_cancelled(
                        &mut socket,
                        Message::Binary(Bytes::from(osc)),
                        &mut cancel_rx,
                    )
                    .await
                    {
                        break;
                    }
                }
            }
            chunk = tail.as_mut().expect("remote tail is present").receiver.recv() => match chunk {
                Some(bytes) => {
                    bytes_since_title_poll = true;
                    // Coalesce a backlog of small SSH-tail reads into one frame
                    // (no added latency) so a weak client isn't woken once per
                    // tiny chunk of a chatty remote agent.
                    let frame = coalesce_queued(
                        bytes,
                        &mut tail.as_mut().expect("remote tail is present").receiver,
                    );
                    tail_progress.received(frame.len());
                    if !send_remote_stream_message_unless_cancelled(
                        &mut socket,
                        Message::Binary(frame),
                        &mut cancel_rx,
                    )
                    .await
                    {
                        break;
                    }
                }
                None => {
                    let ended = tail.take().expect("remote tail is present");
                    if let Err(error) = ended.shutdown().await {
                        tail_cleanup_errors.push(error);
                        break;
                    }
                    match reestablish_remote_tail(
                        &state,
                        host_id,
                        id,
                        &target,
                        &log,
                        &mut socket,
                        &mut cancel_rx,
                    )
                    .await
                    {
                        Some((replacement, offset)) => {
                            tail = Some(replacement);
                            tail_progress.reset(offset);
                        }
                        None => break,
                    }
                }
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) if !b.is_empty() => {
                    if !queue_remote_input_unless_cancelled(
                        &input_tx,
                        b.to_vec(),
                        &mut cancel_rx,
                    )
                    .await
                    {
                        break;
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Some((cols, rows)) = parse_resize(&t) {
                        let resize_result = match acquire_current_stream_host_unless_cancelled(
                            &state,
                            host_id,
                            &mut cancel_rx,
                        )
                        .await
                        {
                            Ok(Some((_host_guard, host))) => {
                                crate::host_runtime::resize_window(&host, &target, cols, rows)
                                    .await
                                    .map_err(|error| (error.is_tmux_target_missing(), error.to_string()))
                            }
                            Ok(None) => break,
                            Err(error) => Err((false, error.to_string())),
                        };
                        match resize_result {
                            Ok(()) => {}
                            // Teardown can win the race with a resize already
                            // queued by the selected client. The event/session
                            // refresh path owns recovery; reporting this as a
                            // terminal error only opens a fatal-looking modal
                            // for expected lifecycle churn.
                            Err((true, error)) => {
                                tracing::debug!(
                                    target = %target,
                                    %error,
                                    "ignored resize for vanished remote tmux target"
                                );
                            }
                            Err((false, error)) => {
                                if !send_remote_stream_message_unless_cancelled(
                                    &mut socket,
                                    Message::Text(format!("[resize dropped: {error}]").into()),
                                    &mut cancel_rx,
                                )
                                .await
                                {
                                    break;
                                }
                            }
                        }
                    } else if parse_refresh(&t) {
                        // Re-paint the current screen on demand (same shape as the
                        // initial snapshot). Heals any bytes missed at connect.
                        let snapshot = match acquire_current_stream_host_unless_cancelled(
                            &state,
                            host_id,
                            &mut cancel_rx,
                        )
                        .await
                        {
                            Ok(Some((_host_guard, host))) => {
                                crate::host_runtime::capture_pane_ansi(&host, &target).await.ok()
                            }
                            Ok(None) => break,
                            Err(error) => {
                                tracing::debug!(session = %id, %host_id, %error, "remote refresh could not reload host");
                                None
                            }
                        };
                        if let Some(snap) = snapshot
                            && !snap.is_empty()
                        {
                            let mut payload = Vec::with_capacity(snap.len() + 2);
                            payload.extend_from_slice(b"\x1bc");
                            payload.extend_from_slice(&snap);
                            if !send_remote_stream_message_unless_cancelled(
                                &mut socket,
                                Message::Binary(Bytes::from(payload)),
                                &mut cancel_rx,
                            )
                            .await
                            {
                                break;
                            }
                        }
                    } else if !queue_remote_input_unless_cancelled(
                        &input_tx,
                        t.as_bytes().to_vec(),
                        &mut cancel_rx,
                    )
                    .await
                    {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    // Normal WebSocket disconnects use the same cancellation path as host
    // mutation so an input worker waiting on the host lease or a blocked stdin
    // write wakes promptly before cleanup.
    registration.control.cancel.send_replace(true);
    drop(input_tx);
    if let Some(probe) = pane_state_probe.take() {
        probe.abort();
        let _ = probe.await;
    }

    // Kill and reap both long-lived SSH children before acknowledging host
    // cancellation. Run the independent cleanup paths concurrently so the
    // host mutation barrier stays comfortably inside its bounded deadline.
    let tail_cleanup = async {
        if let Some(tail) = tail.take()
            && let Err(error) = tail.shutdown().await
        {
            tail_cleanup_errors.push(error);
        }
        if tail_cleanup_errors.is_empty() {
            Ok(())
        } else {
            Err(tail_cleanup_errors.join("; "))
        }
    };
    let input_cleanup = async {
        match tokio::time::timeout(Duration::from_secs(3), &mut input_handle).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(format!("remote input writer task failed: {error}")),
            Err(_) => {
                input_handle.abort();
                let _ = input_handle.await;
                Err("timed out cleaning up remote input writer".into())
            }
        }
    };
    let (tail_cleanup, input_cleanup) = tokio::join!(tail_cleanup, input_cleanup);

    let cleanup = match (tail_cleanup, input_cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(tail), Ok(())) => Err(tail),
        (Ok(()), Err(input)) => Err(input),
        (Err(tail), Err(input)) => Err(format!("{tail}; {input}")),
    };
    registration.finish(cleanup);
}

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
    let (_host_guard, _session_guard) = acquire_host_and_session_lifecycle(&state, id).await?;
    let session = load(&state, id).await?;
    let host = load_host_for_session(&state, &session).await?;

    let n = clamp_lines(q.lines);

    let captured_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let lines: Vec<String> = match session.tmux_target.as_deref() {
        Some(target) => {
            let mut control_target = target.to_string();
            if !is_external(&session) && matches!(host.kind, HostKind::Ssh { .. }) {
                if !crate::host_runtime::has_session(&host, target)
                    .await
                    .map_err(|error| ApiError::from_host_runtime(&host, error))?
                {
                    return Err(ApiError::BadRequest(
                        "tmux session not active for this session".into(),
                    ));
                }
                control_target = guard_managed_ssh_tmux_io(&state, &host, &session, target).await?;
            }
            let text = crate::host_runtime::capture_pane_visible(&host, &control_target)
                .await
                .map_err(|e| ApiError::from_host_runtime(&host, e))?;
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

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|e| ApiError::BadRequest(e.to_string()))
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
    use super::{
        RemoteTailProgress, TAIL_POOLED_ATTEMPTS, acquire_session_lifecycle,
        agent_mcp_config_matches, coalesce_queued, mcp_token_generation, pane_env, parse_refresh,
        parse_remote_tmux_identity_output, parse_resize, queue_remote_input_unless_cancelled,
        quote_remote_tmux_target, remote_claude_mcp_config_path, remote_title_poll_due,
        session_lifecycle_lock, tail_reconnect_plan, tail_respawn_backoff,
        tool_consumes_agentum_mcp, validate_exact_tmux_resolution,
        validate_legacy_owner_migration_binding, write_remote_input_with_timeout,
    };
    use bytes::Bytes;

    #[test]
    fn only_mcp_consumers_require_remote_forwarding() {
        for tool in ["claude", "codex", "cursor", "agent", "gemini", "opencode"] {
            assert!(tool_consumes_agentum_mcp(tool), "{tool}");
        }
        for tool in ["terminal", "aider", "unknown"] {
            assert!(!tool_consumes_agentum_mcp(tool), "{tool}");
        }
    }

    #[test]
    fn remote_claude_config_lives_in_private_runtime_namespace() {
        let id = uuid::Uuid::nil();
        assert_eq!(
            remote_claude_mcp_config_path("/home/alice", id),
            std::path::PathBuf::from(format!("/home/alice/.agentum/runtime/mcp-{id}.json"))
        );
    }

    #[test]
    fn remote_tmux_target_uses_supported_quoted_name_without_exact_prefix() {
        assert_eq!(
            quote_remote_tmux_target("agentum-test3").unwrap(),
            "agentum-test3"
        );
        let quoted = quote_remote_tmux_target("agentum test3").unwrap();
        assert!(!quoted.contains("=agentum"));
        assert_eq!(quoted, "'agentum test3'");
        assert_eq!(quote_remote_tmux_target("$12").unwrap(), "'$12'");
    }

    #[test]
    fn remote_tmux_resolution_rejects_prefix_collisions() {
        validate_exact_tmux_resolution("agentum-test3", "agentum-test3").unwrap();
        let prefix = validate_exact_tmux_resolution("agentum-test", "agentum-test3").unwrap_err();
        assert!(prefix.contains("resolves by prefix"));
    }

    #[test]
    fn remote_tmux_identity_parser_uses_one_tagged_record_and_ignores_banners() {
        let owner = uuid::Uuid::new_v4();
        let output = format!(
            "login banner\nAGENTUM_TMUX_IDENTITY\u{1f}agentum-test3\u{1f}$7\u{1f}{owner}\n"
        );
        let identity = parse_remote_tmux_identity_output(&output, "agentum-test3").unwrap();
        assert_eq!(identity.name, "agentum-test3");
        assert_eq!(identity.id, "$7");
        assert_eq!(identity.owner, Some(owner));
    }

    #[test]
    fn mcp_generation_is_stable_non_secret_token_fingerprint() {
        let token = "abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE";
        let generation = mcp_token_generation(token);
        assert_eq!(generation, mcp_token_generation(token));
        assert_ne!(generation, mcp_token_generation("different-token"));
        assert_eq!(generation.len(), 24);
        assert!(!generation.contains(token));
    }

    #[test]
    fn file_agent_config_verification_requires_current_url_and_token() {
        let file = crate::mcp_provision::agent_mcp_file("cursor").unwrap();
        let auth_env = "AGENTUM_MCP_AUTH_TEST";
        let server = agentum_executor::McpServer {
            name: "agentum".into(),
            url: "http://127.0.0.1:9001/mcp".into(),
            auth_token: Some("current-token".into()),
            auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_TEST".into()),
        };
        let config =
            crate::mcp_provision::merge_agent_config(None, &file, &server, auth_env).unwrap();
        assert!(agent_mcp_config_matches(
            &config,
            file,
            "http://127.0.0.1:9001/mcp",
            auth_env,
            "current-token",
        ));
        assert!(!agent_mcp_config_matches(
            &config,
            file,
            "http://127.0.0.1:9002/mcp",
            auth_env,
            "current-token",
        ));
        assert!(!config.contains("current-token"));
    }

    #[test]
    fn file_agent_config_verification_requires_client_specific_fields() {
        let file = crate::mcp_provision::agent_mcp_file("opencode").unwrap();
        let auth_env = "AGENTUM_MCP_AUTH_TEST";
        let server = agentum_executor::McpServer {
            name: "agentum".into(),
            url: "http://127.0.0.1:9001/mcp".into(),
            auth_token: Some("current-token".into()),
            auth_env_var: Some("AGENTUM_CODEX_MCP_AUTH_TEST".into()),
        };
        let config =
            crate::mcp_provision::merge_agent_config(None, &file, &server, auth_env).unwrap();
        assert!(agent_mcp_config_matches(
            &config,
            file,
            "http://127.0.0.1:9001/mcp",
            auth_env,
            "current-token",
        ));

        let mut without_type: serde_json::Value = serde_json::from_str(&config).unwrap();
        without_type[file.servers_key]["agentum"]
            .as_object_mut()
            .unwrap()
            .remove("type");
        let without_type = serde_json::to_string(&without_type).unwrap();
        assert!(!agent_mcp_config_matches(
            &without_type,
            file,
            "http://127.0.0.1:9001/mcp",
            auth_env,
            "current-token",
        ));
    }

    #[test]
    fn lifecycle_lock_is_shared_while_a_mutation_holds_it() {
        let id = uuid::Uuid::new_v4();
        let first = session_lifecycle_lock(id);
        let second = session_lifecycle_lock(id);
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn blocked_session_lease_does_not_hold_the_global_handoff_gate() {
        let blocked_id = uuid::Uuid::new_v4();
        let other_id = uuid::Uuid::new_v4();
        let held = session_lifecycle_lock(blocked_id).lock_owned().await;
        let waiter = tokio::spawn(acquire_session_lifecycle(blocked_id));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let other = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            acquire_session_lifecycle(other_id),
        )
        .await
        .expect("a blocked session must not convoy unrelated session leases");
        drop(other);
        drop(held);
        waiter.await.unwrap();
    }

    #[test]
    fn unmarked_legacy_owner_migration_requires_one_exact_binding() {
        let session_id = uuid::Uuid::new_v4();
        let host_id = uuid::Uuid::new_v4();
        validate_legacy_owner_migration_binding(
            session_id,
            false,
            Some(host_id),
            Some("agentum-legacy"),
            host_id,
            "agentum-legacy",
            &[session_id],
        )
        .unwrap();
    }

    #[test]
    fn unmarked_legacy_owner_migration_rejects_duplicate_bindings() {
        let session_id = uuid::Uuid::new_v4();
        let host_id = uuid::Uuid::new_v4();
        let other_id = uuid::Uuid::new_v4();
        let error = validate_legacy_owner_migration_binding(
            session_id,
            false,
            Some(host_id),
            Some("agentum-legacy"),
            host_id,
            "agentum-legacy",
            &[session_id, other_id],
        )
        .unwrap_err();
        assert!(error.contains("2 persisted session bindings"));
    }

    #[test]
    fn unmarked_legacy_owner_migration_rejects_unpersisted_or_foreign_targets() {
        let session_id = uuid::Uuid::new_v4();
        let host_id = uuid::Uuid::new_v4();
        let missing = validate_legacy_owner_migration_binding(
            session_id,
            false,
            Some(host_id),
            None,
            host_id,
            "agentum-legacy",
            &[session_id],
        )
        .unwrap_err();
        assert!(missing.contains("exact persisted binding"));

        let foreign = validate_legacy_owner_migration_binding(
            session_id,
            false,
            Some(host_id),
            Some("user-pane"),
            host_id,
            "user-pane",
            &[session_id],
        )
        .unwrap_err();
        assert!(foreign.contains("historical `agentum-`"));
    }

    #[test]
    fn pane_env_publishes_api_url_and_derives_hook_from_it() {
        let sid = uuid::Uuid::nil();
        let env = pane_env(Some("http://127.0.0.1:5544"), sid, "build-agent", "tok");
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("AGENTUM_API_URL"), Some("http://127.0.0.1:5544"));
        // Hook URL is anchored to the SAME base — never a separate hardcoded 8822.
        assert_eq!(
            get("AGENTUM_HOOK_URL"),
            Some(format!("http://127.0.0.1:5544/api/sessions/{sid}/hook").as_str())
        );
        assert_eq!(get("AGENTUM_HOOK_TOKEN"), Some("tok"));
        // The orchestration handle is the session name.
        assert_eq!(get("AGENTUM_TERMINAL_HANDLE"), Some("build-agent"));
    }

    #[test]
    fn pane_env_falls_back_to_8822_when_base_unknown() {
        // A standalone daemon (no embedded api_base_url) keeps the conventional port.
        let env = pane_env(None, uuid::Uuid::nil(), "sh", "tok");
        let url = env.iter().find(|(k, _)| k == "AGENTUM_API_URL").unwrap();
        assert_eq!(url.1, "http://127.0.0.1:8822");
    }

    #[tokio::test]
    async fn coalesce_queued_forwards_lone_chunk_unchanged() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        drop(tx); // empty + closed: nothing to drain
        let out = coalesce_queued(Bytes::from_static(b"abc"), &mut rx);
        assert_eq!(&out[..], b"abc");
    }

    #[tokio::test]
    async fn coalesce_queued_merges_backlog_into_one_frame() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        // Simulate a weak-client backlog: several tiny chunks already queued.
        tx.send(Bytes::from_static(b"two")).await.unwrap();
        tx.send(Bytes::from_static(b"three")).await.unwrap();
        let out = coalesce_queued(Bytes::from_static(b"one"), &mut rx);
        // Byte total is preserved and ordered — only the framing changes.
        assert_eq!(&out[..], b"onetwothree");
        // Drained everything that was waiting.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn tail_reconnect_is_bounded_and_escapes_the_pool_once() {
        use agentum_tmux::ssh::SshMux;

        assert_eq!(
            tail_respawn_backoff(0),
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            tail_respawn_backoff(1),
            std::time::Duration::from_millis(500)
        );
        assert_eq!(tail_respawn_backoff(2), std::time::Duration::from_secs(1));
        assert_eq!(tail_respawn_backoff(3), std::time::Duration::from_secs(2));
        assert_eq!(tail_respawn_backoff(4), std::time::Duration::from_secs(3));
        assert_eq!(
            tail_respawn_backoff(u32::MAX),
            std::time::Duration::from_secs(3)
        );

        for attempt in 0..TAIL_POOLED_ATTEMPTS {
            assert_eq!(tail_reconnect_plan(attempt), (SshMux::Streaming, false));
        }
        assert_eq!(
            tail_reconnect_plan(TAIL_POOLED_ATTEMPTS),
            (SshMux::Off, true)
        );
        assert_eq!(
            tail_reconnect_plan(TAIL_POOLED_ATTEMPTS + 1),
            (SshMux::Off, false)
        );
    }

    #[test]
    fn remote_title_poll_skips_idle_ticks_but_keeps_activity_and_safety_checks() {
        assert!(
            remote_title_poll_due(true, 1),
            "output re-arms the next poll"
        );
        assert!(
            !remote_title_poll_due(false, 1),
            "first idle tick is skipped"
        );
        assert!(
            remote_title_poll_due(false, 2),
            "second idle tick is the safety poll"
        );
        assert!(remote_title_poll_due(false, u32::MAX));
    }

    #[test]
    fn remote_tail_progress_requires_persistent_lag_and_resets_on_delivery() {
        let mut progress = RemoteTailProgress::new(Some(100));
        assert!(
            !progress.observe(120),
            "one racing observation is tolerated"
        );
        progress.received(20);
        assert!(!progress.observe(120), "caught-up tail is healthy");
        assert!(!progress.observe(140), "new lag starts a fresh candidate");
        assert!(
            progress.observe(140),
            "unchanged lag confirms a silent tail"
        );
    }

    #[test]
    fn remote_tail_progress_distinguishes_idle_unknown_and_truncated_logs() {
        let mut unknown = RemoteTailProgress::new(None);
        assert!(!unknown.observe(50));
        assert_eq!(unknown.consumed_offset, Some(50));
        assert!(!unknown.observe(50), "healthy idle does not reconnect");

        let mut truncated = RemoteTailProgress::new(Some(200));
        assert!(!truncated.observe(10));
        assert_eq!(truncated.consumed_offset, Some(10));
        assert!(!truncated.observe(10));
    }

    #[tokio::test]
    async fn persistent_input_write_succeeds_fast_or_times_out() {
        let mut sink = tokio::io::sink();
        assert!(
            write_remote_input_with_timeout(
                &mut sink,
                b"61\n",
                std::time::Duration::from_millis(20),
            )
            .await
        );

        // Keep the reader alive but never drain it. The one-byte duplex buffer
        // fills immediately, reproducing a writer whose remote end stopped
        // consuming without depending on a live SSH host.
        let (mut blocked, _reader) = tokio::io::duplex(1);
        assert!(
            !write_remote_input_with_timeout(
                &mut blocked,
                b"61 62 63\n",
                std::time::Duration::from_millis(20),
            )
            .await
        );
    }

    #[tokio::test]
    async fn remote_input_queue_backpressures_without_dropping() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        sender.send(vec![1]).await.unwrap();
        let (_cancel_sender, mut cancel) = tokio::sync::watch::channel(false);

        let enqueue = queue_remote_input_unless_cancelled(&sender, vec![2], &mut cancel);
        tokio::pin!(enqueue);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut enqueue)
                .await
                .is_err(),
            "a full input queue must apply backpressure"
        );
        assert_eq!(receiver.recv().await, Some(vec![1]));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut enqueue)
                .await
                .expect("enqueue should resume when queue capacity returns")
        );
        assert_eq!(receiver.recv().await, Some(vec![2]));
    }

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
                hostname: "test".to_string(),
                no_auth: true,
                clipboard_pending: Arc::new(
                    std::sync::Mutex::new(std::collections::HashMap::new()),
                ),
                clipboard_request_bus: broadcast::channel(64).0,
                hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
                mcp_token: Arc::new(String::from("test-mcp-token")),
                api_base_url: None,
                mcp_base_url: None,
                desktop_bridge: None,
                harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
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

        #[tokio::test]
        async fn remote_stream_host_reload_waits_for_and_observes_latest_revision() {
            use agentum_core::{HostKind, NewHost, SshAuth};

            let state = fresh_state().await;
            let host = state
                .store
                .create_host(NewHost {
                    name: "before-put".into(),
                    kind: HostKind::Ssh {
                        user: "alice".into(),
                        hostname: "example.test".into(),
                        port: 22,
                        auth: SshAuth::Agent,
                    },
                })
                .await
                .unwrap();
            let host_id = host.id;
            let host_kind = host.kind.clone();

            // Model the host PUT transaction: its shared lease stays held
            // through the store commit. A stream reload must wait, then read
            // the post-PUT row rather than retaining/recreating the old SSH
            // connection revision.
            let put_guard = acquire_host_lifecycle(host_id).await;
            let stream_state = state.clone();
            let mut waiter = tokio::spawn(async move {
                let (_stream_guard, current) = acquire_current_stream_host(&stream_state, host_id)
                    .await
                    .unwrap();
                current
            });
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(25), &mut waiter)
                    .await
                    .is_err(),
                "the stream reload must serialize behind an in-flight host PUT"
            );

            state
                .store
                .update_host(
                    host_id,
                    NewHost {
                        name: "after-put".into(),
                        kind: host_kind,
                    },
                )
                .await
                .unwrap();
            drop(put_guard);

            let current = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("stream reload should resume after PUT releases its lease")
                .unwrap();
            assert_eq!(current.name, "after-put");
        }

        #[tokio::test]
        async fn host_stream_cancellation_waits_for_explicit_cleanup_ack() {
            let host_id = Uuid::new_v4();
            let _host_guard = acquire_host_lifecycle(host_id).await;
            let (registration, mut cancel_rx) = register_remote_stream(host_id);

            let mut cancellation =
                tokio::spawn(async move { cancel_remote_streams_for_host(host_id).await });
            tokio::time::timeout(std::time::Duration::from_secs(1), cancel_rx.changed())
                .await
                .expect("host mutation should signal the registered stream")
                .unwrap();
            assert!(*cancel_rx.borrow());
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(25), &mut cancellation,)
                    .await
                    .is_err(),
                "host mutation must not pass the barrier before child cleanup ACK"
            );

            registration.finish(Ok(()));
            tokio::time::timeout(std::time::Duration::from_secs(1), cancellation)
                .await
                .expect("host mutation should resume after cleanup ACK")
                .unwrap()
                .unwrap();
        }

        #[tokio::test]
        async fn remote_stream_child_cleanup_explicitly_kills_and_reaps() {
            let mut child = tokio::process::Command::new("sh")
                .args(["-c", "exec sleep 30"])
                .kill_on_drop(true)
                .spawn()
                .unwrap();

            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                kill_and_reap_remote_stream_child(&mut child, "test stream child"),
            )
            .await
            .expect("cleanup must be bounded")
            .unwrap();
            assert!(
                child.try_wait().unwrap().is_some(),
                "cleanup must reap the child, not merely request termination"
            );
        }

        #[tokio::test]
        async fn remote_launch_requires_a_tunnelable_embedded_api_port() {
            let mut state = fresh_state().await;
            assert!(matches!(
                required_remote_mcp_port(&state),
                Err(ApiError::BadGateway(message)) if message.contains("dedicated MCP listener")
            ));

            // The full loopback REST API is never a valid reverse-tunnel
            // destination; remote agents may reach only the dedicated,
            // bearer-protected MCP listener.
            state.api_base_url = Some("http://127.0.0.1:5544".into());
            assert!(required_remote_mcp_port(&state).is_err());
            state.mcp_base_url = Some("http://127.0.0.1:5544".into());
            assert_eq!(required_remote_mcp_port(&state).unwrap(), 5544);
        }

        #[tokio::test]
        async fn start_prefers_persisted_tmux_target_after_rename() {
            let state = fresh_state().await;
            let session = create_test_session(&state).await;
            state
                .store
                .update_status_and_target(session.id, Status::Idle, Some("agentum-original"))
                .await
                .unwrap();
            let renamed = state
                .store
                .patch_session_name(session.id, "renamed-display-label")
                .await
                .unwrap();

            assert_eq!(start_target(&renamed).unwrap(), "agentum-original");
        }

        #[tokio::test]
        async fn new_managed_targets_are_unique_even_when_names_match() {
            let state = fresh_state().await;
            let first = create_test_session(&state).await;
            let mut second = first.clone();
            second.id = Uuid::new_v4();

            let first_target = managed_tmux_target(&first);
            let second_target = managed_tmux_target(&second);
            assert_ne!(first_target, second_target);
            assert!(first_target.contains(&first.id.simple().to_string()));
            assert!(second_target.contains(&second.id.simple().to_string()));
        }

        #[tokio::test]
        async fn startup_resume_rechecks_idle_status_before_mutating_tmux() {
            let state = fresh_state().await;
            let session = create_test_session(&state).await;
            state
                .store
                .update_status_and_target(session.id, Status::Stopped, None)
                .await
                .unwrap();

            let resumed = resume_idle_session_by_id(&state, session.id).await.unwrap();
            assert!(resumed.is_none());
            let stored = state
                .store
                .get_session_by_id(session.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.status, Status::Stopped);
        }

        #[tokio::test]
        async fn external_start_requires_its_persisted_binding() {
            let state = fresh_state().await;
            let mut session = create_test_session(&state).await;
            session.flags.push(EXTERNAL_TMUX_FLAG.into());
            session.tmux_target = None;

            assert!(matches!(
                start_target(&session),
                Err(ApiError::BadRequest(message)) if message.contains("lost its tmux target")
            ));
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
                hostname: "test".to_string(),
                no_auth: true,
                clipboard_pending: Arc::new(
                    std::sync::Mutex::new(std::collections::HashMap::new()),
                ),
                clipboard_request_bus: broadcast::channel(64).0,
                hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
                mcp_token: Arc::new(String::from("test-mcp-token")),
                api_base_url: None,
                mcp_base_url: None,
                desktop_bridge: None,
                harness: std::sync::Arc::new(crate::harness::HarnessEngine::new()),
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
}
