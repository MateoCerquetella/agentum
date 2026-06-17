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
    let host = load_host_for_session(&state, &session).await?;
    if matches!(session.status, Status::Running) {
        if !q.force {
            return Err(ApiError::BadRequest(
                "session is running; pass ?force=true to kill and remove".into(),
            ));
        }
        let target = tmux_target(&session);
        if is_external(&session) {
            // Removing the record must leave the user's tmux session
            // running; just stop piping its output to our log.
            let _ = crate::host_runtime::unpipe_pane(&host, &target).await;
        } else {
            crate::host_runtime::kill_session(&host, &target)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        }
    }
    state.store.delete_session(id).await?;
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

/// Spawn the agent process for a freshly-(re)started session into a tmux pane
/// on `host`, arm the output pipe, and mark it `Running`. Shared by the `start`
/// HTTP handler and the harness-engine driver ([`crate::harness`]) so both go
/// through the *one* launch path — YOLO marker translation, loopback `pane_env`,
/// the Claude `--settings` PostToolUse hook, and MCP wiring all stay centralized
/// here. `workdir` must already be resolved + validated by the caller (the
/// reattach / external / worktree-heal decisions differ per caller and stay
/// out of this helper). On a pipe failure the half-spawned pane is killed so we
/// never leave an orphan behind.
pub(crate) async fn spawn_agent_into_pane(
    state: &AppState,
    session: &Session,
    host: &Host,
    target: &str,
    workdir: &std::path::Path,
) -> Result<(), ApiError> {
    let adapter = agentum_executor::adapter_for(&session.tool);
    let mut launch = adapter.launch(session);

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
        let base = state
            .api_base_url
            .as_deref()
            .unwrap_or("http://127.0.0.1:8822");
        let agentum_mcp_url = format!("{base}/mcp");
        if let Some(p) =
            crate::mcp_provision::provision(state, &session.tool, &agentum_mcp_url).await
        {
            launch.argv.extend(adapter.mcp_args(&p));
        }
        // File-based agents (Cursor/Gemini/OpenCode) load MCP from a config file
        // in the workdir — write it (no-op for claude/codex).
        crate::mcp_provision::write_agent_project_config(
            state,
            host,
            &workdir.to_string_lossy(),
            &session.tool,
            &agentum_mcp_url,
        )
        .await;
    } else if matches!(host.kind, HostKind::Ssh { .. }) {
        // Remote MCP parity: the agentum MCP lives on the Mac. Reverse-tunnel it
        // to the host (token-guarded, loopback-bound), then wire each agent at
        // the tunnel URL. Best-effort: a tunnel failure logs and launches the
        // agent without the MCP rather than blocking.
        match crate::mcp_provision::local_mcp_port(state) {
            Some(mac_port) => match crate::host_runtime::ensure_reverse_tunnel(host, mac_port).await
            {
                Ok(host_port) => {
                    // The remote agent needs its own orchestration handle and an
                    // AGENTUM_API_URL pointing at the tunnel.
                    launch
                        .env
                        .push(("AGENTUM_TERMINAL_HANDLE".into(), session.name.clone()));
                    launch.env.push((
                        "AGENTUM_API_URL".into(),
                        format!("http://127.0.0.1:{host_port}"),
                    ));
                    let agentum_mcp_url = format!("http://127.0.0.1:{host_port}/mcp");
                    let servers =
                        vec![crate::mcp_provision::agentum_server(state, &agentum_mcp_url)];
                    let provision = if session.tool == "claude" {
                        // Claude needs the --mcp-config FILE on the HOST.
                        let host_cfg = format!("/tmp/agentum-mcp-{}.json", session.id);
                        let json = crate::mcp_provision::config_json(&servers);
                        match crate::host_runtime::write_remote_file(host, &host_cfg, &json).await {
                            Ok(()) => Some(agentum_executor::McpProvision {
                                servers,
                                config_file: PathBuf::from(host_cfg),
                            }),
                            Err(e) => {
                                tracing::warn!(session = %session.id, "could not write remote MCP config to host: {e}");
                                None
                            }
                        }
                    } else {
                        // Codex injects MCP inline via `-c` — no host file needed.
                        Some(agentum_executor::McpProvision {
                            servers,
                            config_file: PathBuf::new(),
                        })
                    };
                    if let Some(p) = provision {
                        launch.argv.extend(adapter.mcp_args(&p));
                    }
                    // File-based agents: write the config on the HOST in the workdir.
                    crate::mcp_provision::write_agent_project_config(
                        state,
                        host,
                        &workdir.to_string_lossy(),
                        &session.tool,
                        &agentum_mcp_url,
                    )
                    .await;
                }
                Err(e) => tracing::warn!(
                    session = %session.id,
                    "reverse MCP tunnel to host failed; launching remote agent without agentum MCP: {e}"
                ),
            },
            None => tracing::warn!(
                "no embedded api_base_url; cannot reverse-tunnel the agentum MCP to an SSH host"
            ),
        }
    }

    crate::host_runtime::new_session(host, target, workdir, &launch.argv, &launch.env)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Err(e) = crate::host_runtime::pipe_pane(host, target, &log).await {
        let _ = crate::host_runtime::kill_session(host, target).await;
        return Err(ApiError::Internal(e.to_string()));
    }

    state
        .store
        .update_status_and_target(session.id, Status::Running, Some(target))
        .await?;
    Ok(())
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
    spawn_agent_into_pane(&state, &session, &host, &target, &workdir).await?;
    Ok(Json(session_with_spawned(load(&state, id).await?, true)))
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let host = load_host_for_session(&state, &session).await?;
    let target = tmux_target(&session);
    if is_external(&session) {
        // Detach only: the tmux session belongs to the user. Disarm the
        // log pipe and keep the target so a later start can reattach.
        let _ = crate::host_runtime::unpipe_pane(&host, &target).await;
        state
            .store
            .update_status_and_target(id, Status::Stopped, Some(&target))
            .await?;
    } else {
        crate::host_runtime::graceful_stop(&host, &target, GRACEFUL_STOP_TIMEOUT)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        state
            .store
            .update_status_and_target(id, Status::Stopped, None)
            .await?;
    }
    state.hook_tokens.lock().unwrap().remove(&id);
    emit_stopped(&state, &session, "stop").await;
    Ok(Json(load(&state, id).await?))
}

async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Session>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = load(&state, id).await?;
    let host = load_host_for_session(&state, &session).await?;
    let target = tmux_target(&session);
    if is_external(&session) {
        // Even a kill must not destroy a user-owned tmux session —
        // detach (disarm the pipe) and keep the target for reattach.
        let _ = crate::host_runtime::unpipe_pane(&host, &target).await;
        state
            .store
            .update_status_and_target(id, Status::Stopped, Some(&target))
            .await?;
    } else {
        crate::host_runtime::kill_session(&host, &target)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        state
            .store
            .update_status_and_target(id, Status::Stopped, None)
            .await?;
    }
    state.hook_tokens.lock().unwrap().remove(&id);
    emit_stopped(&state, &session, "kill").await;
    Ok(Json(load(&state, id).await?))
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
    Ok(ws.on_upgrade(move |socket| async move {
        if matches!(host.kind, HostKind::Local) {
            stream_session(socket, id, target, positions, resume).await;
        } else {
            stream_remote_session(socket, host, id, target).await;
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

async fn stream_session(
    mut socket: WebSocket,
    id: Uuid,
    target: String,
    positions: Arc<std::sync::Mutex<std::collections::HashMap<Uuid, StreamCheckpoint>>>,
    resume_requested: bool,
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
            // No activity within the bail window: probably a no-op resize
            // (size already matched, no SIGWINCH propagated). Don't burn
            // the full max budget waiting for a wake-up that won't come.
            if !activity_seen && now >= start + POST_RESIZE_NO_ACTIVITY_BAIL {
                break;
            }
            // Hard cap so an actively-streaming agent can't hold connect
            // open indefinitely.
            if now >= max_deadline {
                break;
            }
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
    if let (true, Some(cp), true) = (resume_requested, saved_checkpoint, resume_size_matches) {
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
async fn stream_remote_session(mut socket: WebSocket, host: Host, id: Uuid, target: String) {
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
    // Capture the snapshot, RETRYING while it comes back empty. A just-launched
    // agent paints its first frame slowly — Claude Code spins up node and draws
    // a "trust this folder?" prompt, which measured >3s to appear — and that
    // first frame is often a STATIC screen (it waits for input, emitting nothing
    // more), so the live tail has nothing to stream and the snapshot is the only
    // way to paint it. A single capture at connect therefore returned a blank
    // grid and the pane sat BLANK ("opened an agent, no response") until a manual
    // refresh. Re-capturing every ~300ms until the pane has drawn something (or a
    // 12s budget elapses — generous enough for a cold node/agent boot) makes a
    // freshly-opened agent paint as soon as it renders. The loop breaks the
    // instant a frame is non-empty, so a fast pane isn't delayed; each retry
    // re-samples the offset so the tail still resumes exactly past what we paint.
    const SNAPSHOT_RETRY_BUDGET: Duration = Duration::from_millis(12_000);
    const SNAPSHOT_RETRY_GAP: Duration = Duration::from_millis(300);
    let snap_deadline = tokio::time::Instant::now() + SNAPSHOT_RETRY_BUDGET;
    let mut log_offset: Option<u64> = None;
    loop {
        let out_of_budget = tokio::time::Instant::now() >= snap_deadline;
        match crate::host_runtime::capture_pane_with_log_offset(&host, &target, &log).await {
            Ok((offset, snap)) => {
                log_offset = Some(offset);
                if !snap.is_empty() {
                    let mut payload = Vec::with_capacity(snap.len() + 2);
                    payload.extend_from_slice(b"\x1bc");
                    payload.extend_from_slice(&snap);
                    if socket
                        .send(Message::Binary(Bytes::from(payload)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    break;
                }
                // Empty grid: the agent hasn't painted yet. Keep retrying.
                if out_of_budget {
                    break;
                }
                sleep(SNAPSHOT_RETRY_GAP).await;
            }
            // A capture error early in a session's life is usually transient —
            // the tmux pane is still being set up, or the streaming SSH channel
            // is cold. RETRY within the budget instead of bailing to a blank
            // pane; only give up (fall back to tail-from-EOF) once time's up.
            Err(_) => {
                if out_of_budget {
                    break;
                }
                sleep(SNAPSHOT_RETRY_GAP).await;
            }
        }
    }

    // Persistent `ssh tail -f` of the remote pane log. Its stdout is pumped
    // through an mpsc so the select loop below multiplexes output against
    // keystrokes — a chatty pane never starves input, and vice versa.
    let mut child = match crate::host_runtime::spawn_remote_pane_tail(&host, &log, log_offset) {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[remote tail error: {e}]").into()))
                .await;
            return;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill().await;
        return;
    };
    // Drain + log the tail's stderr so a transport failure that ends the stream
    // (e.g. the remote sshd refusing the channel under MaxSessions pressure:
    // "Session open refused by peer") is recorded instead of silently surfacing
    // as a bare "[session stream closed]". Also keeps the pipe from filling.
    if let Some(mut stderr) = child.stderr.take() {
        let label = log
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if stderr.read_to_end(&mut buf).await.is_ok() && !buf.is_empty() {
                tracing::warn!(
                    session = %label,
                    "remote pane tail ended: {}",
                    String::from_utf8_lossy(&buf).trim()
                );
            }
        });
    }
    let (tail_tx, mut tail_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    let tail_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match stdout.read(&mut buf).await {
                // 0 = ssh/tail exited (channel/host dropped). Unlike a local file
                // tail this never means "caught up"; end the task so the loop
                // tears the connection down.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tail_tx
                        .send(Bytes::copy_from_slice(&buf[..n]))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

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
    // in flight (fast typing, paste) into one write, and delivery failures are
    // logged rather than echoed — the pane not echoing already signals a drop.
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let input_handle = {
        let host = host.clone();
        let target = target.clone();
        tokio::spawn(async move {
            // Persistent keystroke channel + its stdin; None → use per-exec fallback.
            let mut writer = crate::host_runtime::spawn_remote_input_writer(&host, &target).ok();
            let mut stdin = writer.as_mut().and_then(|c| c.stdin.take());
            while let Some(first) = input_rx.recv().await {
                let mut buf = first;
                // Drain whatever queued while the previous write was in flight,
                // up to the send-keys 4 KB chunk size.
                while buf.len() < 4096 {
                    match input_rx.try_recv() {
                        Ok(more) => buf.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }
                let mut delivered = false;
                if let Some(si) = stdin.as_mut() {
                    let line = crate::host_runtime::encode_input_hex_line(&buf);
                    if si.write_all(&line).await.is_ok() && si.flush().await.is_ok() {
                        delivered = true;
                    } else {
                        // Persistent channel broke — drop the child (kills the
                        // dead ssh) and fall back to per-exec for this and every
                        // subsequent keystroke.
                        stdin = None;
                        drop(writer.take());
                    }
                }
                if !delivered
                    && let Err(e) = crate::host_runtime::send_bytes(&host, &target, &buf).await
                {
                    tracing::warn!(target = %target, error = ?e, "remote input send failed");
                }
            }
        })
    };

    // Why: like the local stream, the remote pane's OSC title is consumed by tmux
    // on the host and never crosses the pane byte stream — so the desktop's
    // title-derived agent status would be blank for SSH sessions. Poll the remote
    // pane_title and re-inject it as a synthetic OSC title on change. Each poll is
    // a round-trip SSH exec over the *shared* ControlMaster, and that master is
    // also what carries keystroke `send_keys` — so a too-fast cadence across many
    // open sessions churns the master's limited channels (remote MaxSessions) and
    // can starve input. 2.5 s keeps agent-status lag imperceptible while leaving
    // the master headroom.
    let mut title_ticker = tokio::time::interval(Duration::from_millis(2500));
    let mut last_pane_title = String::new();
    loop {
        tokio::select! {
            _ = title_ticker.tick() => {
                if let Ok(title) = crate::host_runtime::pane_title(&host, &target).await
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
                    // Coalesce a backlog of small SSH-tail reads into one frame
                    // (no added latency) so a weak client isn't woken once per
                    // tiny chunk of a chatty remote agent.
                    let frame = coalesce_queued(bytes, &mut tail_rx);
                    if socket.send(Message::Binary(frame)).await.is_err() {
                        break;
                    }
                }
                None => break, // tail task ended (ssh died / host unreachable)
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) if !b.is_empty() => {
                    // Channel-full (writer wedged on a dead host for 256
                    // frames) drops the key; the 12 s ssh timeout unwedges
                    // the writer long before that backlog accrues.
                    let _ = input_tx.try_send(b.to_vec());
                }
                Some(Ok(Message::Text(t))) => {
                    if let Some((cols, rows)) = parse_resize(&t) {
                        if let Err(e) = crate::host_runtime::resize_window(&host, &target, cols, rows).await
                            && socket.send(Message::Text(format!("[resize dropped: {e}]").into())).await.is_err()
                        {
                            break;
                        }
                    } else if parse_refresh(&t) {
                        // Re-paint the current screen on demand (same shape as the
                        // initial snapshot). Heals any bytes missed at connect.
                        if let Ok(snap) = crate::host_runtime::capture_pane_ansi(&host, &target).await
                            && !snap.is_empty()
                        {
                            let mut payload = Vec::with_capacity(snap.len() + 2);
                            payload.extend_from_slice(b"\x1bc");
                            payload.extend_from_slice(&snap);
                            if socket.send(Message::Binary(Bytes::from(payload))).await.is_err() {
                                break;
                            }
                        }
                    } else {
                        let _ = input_tx.try_send(t.as_bytes().to_vec());
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    tail_handle.abort();
    input_handle.abort();
    let _ = child.kill().await;
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
    use super::{coalesce_queued, pane_env, parse_refresh, parse_resize};
    use bytes::Bytes;

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
