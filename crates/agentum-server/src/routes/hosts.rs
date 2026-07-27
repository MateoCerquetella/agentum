//! `/api/hosts` — machines controlled directly by this daemon.

use agentum_core::{
    EXTERNAL_TMUX_FLAG, Host, HostKind, HostReadiness, LOCAL_HOST_ID, NewHost, NewSession, Session,
    SshAuth, Status,
};
use agentum_store::paths;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

use super::util::parse_uuid;
use crate::AppState;
use crate::error::ApiError;
use crate::host_runtime::{DiscoveredTmuxSession, HostProbe};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/hosts", get(list).post(create))
        .route("/api/hosts/{id}", get(get_one).put(update).delete(remove))
        .route("/api/hosts/{id}/test", post(test))
        .route("/api/hosts/{id}/readiness", get(readiness))
        .route("/api/hosts/{id}/bootstrap", post(bootstrap))
        .route("/api/hosts/{id}/install-agent", post(install_agent))
        .route("/api/hosts/{id}/provision-skills", post(provision_skills))
        .route("/api/hosts/{id}/tmux-sessions", get(tmux_sessions))
        .route(
            "/api/hosts/{id}/tmux-sessions/{name}/attach",
            post(attach_tmux_session),
        )
        .route(
            "/api/hosts/{id}/tmux-sessions/{name}",
            axum::routing::delete(kill_tmux_session_route),
        )
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<Host>>, ApiError> {
    Ok(Json(state.store.list_hosts().await?))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Host>, ApiError> {
    let id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(Json(host))
}

/// Validate + normalise an incoming host payload (shared by create and
/// update). Trims the name/user/hostname, rejects empty required fields and
/// out-of-range ports, collapses a blank key path to ssh-agent, and refuses
/// `HostKind::Local` (the local host is a singleton, never created/edited).
/// Passwords are kept verbatim (they may carry meaningful whitespace); only
/// an entirely empty one is rejected.
fn validate_host(new: NewHost) -> Result<NewHost, ApiError> {
    let name = new.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::BadRequest("host name is required".into()));
    }
    let kind = match new.kind {
        HostKind::Local => {
            return Err(ApiError::BadRequest(
                "additional local hosts are not supported".into(),
            ));
        }
        HostKind::Ssh {
            user,
            hostname,
            port,
            auth,
        } => {
            let user = user.trim().to_string();
            let hostname = hostname.trim().to_string();
            if user.is_empty() {
                return Err(ApiError::BadRequest("ssh user is required".into()));
            }
            if hostname.is_empty() {
                return Err(ApiError::BadRequest("ssh hostname is required".into()));
            }
            if port == 0 {
                return Err(ApiError::BadRequest(
                    "ssh port must be between 1 and 65535".into(),
                ));
            }
            let auth = match auth {
                SshAuth::Key { path } if path.trim().is_empty() => SshAuth::Agent,
                SshAuth::Key { path } => SshAuth::Key {
                    path: path.trim().to_string(),
                },
                SshAuth::Agent => SshAuth::Agent,
                SshAuth::Password { password } if password.is_empty() => {
                    return Err(ApiError::BadRequest("ssh password is required".into()));
                }
                SshAuth::Password { password } => SshAuth::Password { password },
            };
            HostKind::Ssh {
                user,
                hostname,
                port,
                auth,
            }
        }
    };
    Ok(NewHost { name, kind })
}

async fn create(
    State(state): State<AppState>,
    Json(new): Json<NewHost>,
) -> Result<(StatusCode, Json<Host>), ApiError> {
    let new = validate_host(new)?;
    let host = state.store.create_host(new).await?;
    Ok((StatusCode::CREATED, Json(host)))
}

/// `PUT /api/hosts/{id}` — edit an existing SSH host's connection settings.
/// Same validation as create; the store rewrites the row in place (keeping
/// the id so sessions stay attached) and returns the refreshed host.
async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(new): Json<NewHost>,
) -> Result<Json<Host>, ApiError> {
    let id = parse_uuid(&id)?;
    let new = validate_host(new)?;
    let host = state.store.update_host(id, new).await?;
    Ok(Json(host))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    if state.store.delete_host(id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(id.to_string()))
    }
}

async fn test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<HostProbe>, ApiError> {
    let id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let probe = crate::host_runtime::probe(&host).await;
    if probe.ok {
        let _ = state.store.update_host_seen(id).await;
    }
    Ok(Json(probe))
}

/// `GET /api/hosts/{id}/readiness` — full structured readiness report
/// (required deps + agent CLIs + package manager + install hints) from a
/// single preflight. The TUI hosts overlay and New Session form, plus
/// `agentum hosts readiness`, consume this. Marks the host as seen when
/// reachable so the sidebar dot reflects the last successful contact.
async fn readiness(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<HostReadiness>, ApiError> {
    let id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let report = crate::host_runtime::readiness(&host).await;
    if report.ok {
        let _ = state.store.update_host_seen(id).await;
    }
    Ok(Json(report))
}

/// Body for `POST /api/hosts/{id}/bootstrap`. `confirm` must be `true`
/// and every item must be in `BOOTSTRAPABLE` — both enforced below so a
/// client can never trigger a `sudo` install without explicit intent or
/// install something other than `tmux`/`git`.
#[derive(serde::Deserialize)]
struct BootstrapRequest {
    #[serde(default)]
    items: Vec<String>,
    #[serde(default)]
    confirm: bool,
}

/// `POST /api/hosts/{id}/bootstrap` — install `tmux`/`git` via the host's
/// package manager after explicit confirmation (phase 2). Returns the
/// re-probed [`HostReadiness`]. Rejects `confirm != true` and any item
/// outside `BOOTSTRAPABLE`. PRD §7.3 + §12.
async fn bootstrap(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<BootstrapRequest>,
) -> Result<Json<HostReadiness>, ApiError> {
    let id = parse_uuid(&id)?;
    if !req.confirm {
        return Err(ApiError::BadRequest(
            "bootstrap requires explicit confirm: true".into(),
        ));
    }
    if req.items.is_empty() {
        return Err(ApiError::BadRequest("no items to bootstrap".into()));
    }
    for item in &req.items {
        if !crate::host_install_hints::BOOTSTRAPABLE.contains(&item.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "`{item}` is not bootstrapable (only: {})",
                crate::host_install_hints::BOOTSTRAPABLE.join(", ")
            )));
        }
    }
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    // A failed remote install (sudo password required, no network, etc.)
    // surfaces as Internal with the remote stderr in the message.
    let report = crate::host_runtime::bootstrap(&host, &req.items)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if report.ok {
        let _ = state.store.update_host_seen(id).await;
    }
    Ok(Json(report))
}

/// Body for `POST /api/hosts/{id}/install-agent`. `confirm` must be `true`
/// and every tool must have a verified installer (rejected otherwise so
/// we never run an arbitrary command on the remote).
#[derive(serde::Deserialize)]
struct InstallAgentRequest {
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    confirm: bool,
}

/// `POST /api/hosts/{id}/install-agent` — install one or more agent CLIs
/// on the host by running their official installers over SSH, after
/// explicit confirmation (phase 3). Returns the re-probed readiness.
/// Agent CLIs never gate `ok` (only tmux/git do), so this is purely
/// additive convenience.
async fn install_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<InstallAgentRequest>,
) -> Result<Json<HostReadiness>, ApiError> {
    let id = parse_uuid(&id)?;
    if !req.confirm {
        return Err(ApiError::BadRequest(
            "install-agent requires explicit confirm: true".into(),
        ));
    }
    if req.tools.is_empty() {
        return Err(ApiError::BadRequest("no tools to install".into()));
    }
    for tool in &req.tools {
        if crate::host_install_hints::agent_install_command(tool).is_none() {
            return Err(ApiError::BadRequest(format!(
                "`{tool}` has no verified installer"
            )));
        }
    }
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let report = crate::host_runtime::install_agents(&host, &req.tools)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if report.ok {
        let _ = state.store.update_host_seen(id).await;
    }
    Ok(Json(report))
}

/// Body for `POST /api/hosts/{id}/provision-skills`. `confirm` must be `true`;
/// `skills` are agentum skill ids validated against this daemon's installed
/// skills in `host_runtime::provision_skills`. File-copy only — never runs an
/// arbitrary command on the remote.
#[derive(serde::Deserialize)]
struct ProvisionSkillsRequest {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    confirm: bool,
}

/// `POST /api/hosts/{id}/provision-skills` — copy agentum skills (by id) from
/// this daemon's `~/.claude/skills` to the host's `~/.claude/skills`, then
/// return the re-probed [`HostReadiness`]. Skills are opt-in per host and never
/// gate `ok` (purely additive). Rejects `confirm != true`.
async fn provision_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ProvisionSkillsRequest>,
) -> Result<Json<HostReadiness>, ApiError> {
    let id = parse_uuid(&id)?;
    if !req.confirm {
        return Err(ApiError::BadRequest(
            "provision-skills requires explicit confirm: true".into(),
        ));
    }
    if req.skills.is_empty() {
        return Err(ApiError::BadRequest("no skills to provision".into()));
    }
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let report = crate::host_runtime::provision_skills(&host, &req.skills)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if report.ok {
        let _ = state.store.update_host_seen(id).await;
    }
    Ok(Json(report))
}

/// Query for `GET /api/hosts/{id}/tmux-sessions`. `path` is the project
/// root used only to compute the per-session `related` flag — never a
/// server-side filter, so the UI can render "related first, show all"
/// from one response.
#[derive(serde::Deserialize)]
struct TmuxSessionsQuery {
    #[serde(default)]
    path: Option<String>,
    /// When true, return ALL sessions (external + agentum-managed). The
    /// per-repo RemoteTmuxRepoCard leaves this false; the host-level modal sets it.
    #[serde(default)]
    all: bool,
}

/// A discovered session plus its relation to the queried project path.
#[derive(serde::Serialize)]
struct DiscoveredSessionItem {
    #[serde(flatten)]
    session: DiscoveredTmuxSession,
    /// True when any pane's cwd is at or under the queried `path`.
    related: bool,
}

/// `GET /api/hosts/{id}/tmux-sessions?path=…` — list tmux sessions on the
/// host that agentum does not manage (anything not `agentum-*`). One SSH
/// round trip; "tmux missing / no server" reads as an empty list.
async fn tmux_sessions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<TmuxSessionsQuery>,
) -> Result<Json<Vec<DiscoveredSessionItem>>, ApiError> {
    let id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let found = if q.all {
        crate::host_runtime::list_all_tmux_sessions(&host).await
    } else {
        crate::host_runtime::list_tmux_sessions(&host).await
    }
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let _ = state.store.update_host_seen(id).await;
    let root = q
        .path
        .as_deref()
        .map(|p| p.trim_end_matches('/'))
        .filter(|p| !p.is_empty());
    let items = found
        .into_iter()
        .map(|s| {
            let related = root.is_some_and(|r| {
                s.panes
                    .iter()
                    .any(|p| p.cwd == r || p.cwd.starts_with(&format!("{r}/")))
            });
            DiscoveredSessionItem {
                session: s,
                related,
            }
        })
        .collect();
    Ok(Json(items))
}

/// Derive a valid agentum session name from an arbitrary tmux session
/// name (tmux allows characters `validate_name` rejects). Truncated to
/// leave room for a dedup suffix.
fn external_session_name(tmux_name: &str) -> String {
    let mut s: String = tmux_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.truncate(56);
    if s.is_empty() {
        s = "tmux".to_string();
    }
    s
}

/// `POST /api/hosts/{id}/tmux-sessions/{name}/attach` — bind a discovered
/// tmux session to a new agentum session record so it streams like any
/// managed session. The record is marked [`EXTERNAL_TMUX_FLAG`], which
/// makes the whole lifecycle non-destructive: stop/kill/delete only ever
/// detach; the underlying tmux session is never killed. Idempotent — an
/// existing record bound to the same (host, tmux session) is re-armed and
/// returned instead of duplicated.
///
/// A tmux session that is already the target of an agentum-MANAGED session
/// row is never given a second (external) binding — the managed session is
/// returned instead. A pane has exactly ONE `pipe-pane` slot, so a duplicate
/// binding can't stream anyway (the `#{pane_pipe}` guard in `pipe_pane`
/// leaves the managed session's live pipe untouched), and detaching the
/// duplicate used to `unpipe_pane` the shared pane — silently freezing the
/// real session's stream (no output, keystrokes never echo) with no
/// self-heal on local hosts.
async fn attach_tmux_session(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    let host_id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(host_id.to_string()))?;

    let all_sessions = state.store.list_sessions(None).await?;

    // Redirect: this tmux session already belongs to a managed agentum
    // session — "attaching" it means opening that session, not duplicating
    // it. Match the row's resolved target (stored, or derived from the name
    // the way spawn does) so a stopped row whose target was cleared still
    // claims its own `agentum-*` pane back instead of gaining a duplicate.
    if let Some(owned) = all_sessions.iter().find(|s| {
        s.host_id.unwrap_or(LOCAL_HOST_ID) == host_id
            && !s.flags.iter().any(|f| f == EXTERNAL_TMUX_FLAG)
            && s.tmux_target
                .as_deref()
                .unwrap_or(&agentum_tmux::target_for(&s.name))
                == name
    }) {
        return Ok((StatusCode::OK, Json(owned.clone())));
    }

    // Reuse an existing binding for this exact (host, tmux session).
    let existing = all_sessions.into_iter().find(|s| {
        s.host_id.unwrap_or(LOCAL_HOST_ID) == host_id
            && s.tmux_target.as_deref() == Some(name.as_str())
            && s.flags.iter().any(|f| f == EXTERNAL_TMUX_FLAG)
    });

    // Validate liveness + grab pane metadata in the same round trip.
    // Use list_all so agentum-managed sessions can also be attached for viewing.
    let discovered = crate::host_runtime::list_all_tmux_sessions(&host)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("tmux session not found on host: {name}")))?;

    let session = match existing {
        Some(s) => s,
        None => {
            let new = NewSession {
                name: external_session_name(&name),
                workdir: discovered
                    .panes
                    .first()
                    .map(|p| p.cwd.clone())
                    .unwrap_or_else(|| "~".to_string()),
                tool: "terminal".to_string(),
                model: None,
                flags: vec![EXTERNAL_TMUX_FLAG.to_string()],
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            };
            // The derived name can collide with an existing session; retry
            // once with a random suffix rather than failing the attach.
            match state
                .store
                .create_session_on_host(new.clone(), Some(host_id))
                .await
            {
                Ok(s) => s,
                Err(_) => {
                    let mut retry = new;
                    retry.name = format!(
                        "{}-{}",
                        retry.name,
                        &Uuid::new_v4().simple().to_string()[..6]
                    );
                    state
                        .store
                        .create_session_on_host(retry, Some(host_id))
                        .await?
                }
            }
        }
    };

    // Arm the pane log pipe and mark the record running with the external
    // target. `pipe_pane` skips panes whose pipe is already live (the
    // `#{pane_pipe}` guard), so re-attach is harmless.
    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;
    crate::host_runtime::pipe_pane(&host, &name, &log)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    state
        .store
        .update_status_and_target(session.id, Status::Running, Some(&name))
        .await?;
    let session = state
        .store
        .get_session_by_id(session.id)
        .await?
        .ok_or_else(|| ApiError::NotFound(session.id.to_string()))?;
    Ok((StatusCode::CREATED, Json(session)))
}

/// `DELETE /api/hosts/{id}/tmux-sessions/{name}` — kill a tmux session on the
/// Kill a named tmux session on the host. Callers are responsible for only
/// targeting inactive sessions — this endpoint does not protect `agentum-*`
/// sessions from deletion.
async fn kill_tmux_session_route(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let host_id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(host_id.to_string()))?;
    crate::host_runtime::kill_tmux_session(&host, &name)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
