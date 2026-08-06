//! `/api/hosts` — machines controlled directly by this daemon.

use std::sync::OnceLock;

use agentum_core::{
    EXTERNAL_TMUX_FLAG, Host, HostKind, HostReadiness, NewHost, NewSession, REDACTED_SSH_PASSWORD,
    Session, SshAuth, Status,
};
use agentum_store::paths;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;
use crate::host_runtime::{DiscoveredTmuxSession, HostProbe};

/// Host mutations may invalidate authenticated ControlMasters. Serialize them
/// so two edits cannot interleave "load old → close old master → persist new"
/// and leave a connection authenticated with credentials no longer in the DB.
fn host_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Public host representation. Runtime [`Host`] values need credentials in
/// memory, while this type deliberately cannot represent a password value.
#[derive(Debug, serde::Serialize)]
struct HostResponse {
    id: Uuid,
    name: String,
    #[serde(flatten)]
    kind: HostResponseKind,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    last_seen_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum HostResponseKind {
    Local,
    Ssh {
        user: String,
        hostname: String,
        port: u16,
        auth: HostResponseAuth,
    },
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "auth", rename_all = "lowercase")]
enum HostResponseAuth {
    Agent,
    Key { path: String },
    Password,
}

impl From<Host> for HostResponse {
    fn from(host: Host) -> Self {
        let Host {
            id,
            name,
            kind,
            created_at,
            updated_at,
            last_seen_at,
        } = host;
        let kind = match kind {
            HostKind::Local => HostResponseKind::Local,
            HostKind::Ssh {
                user,
                hostname,
                port,
                auth,
            } => HostResponseKind::Ssh {
                user,
                hostname,
                port,
                auth: match auth {
                    SshAuth::Agent => HostResponseAuth::Agent,
                    SshAuth::Key { path } => HostResponseAuth::Key { path },
                    SshAuth::Password { .. } => HostResponseAuth::Password,
                },
            },
        };
        Self {
            id,
            name,
            kind,
            created_at,
            updated_at,
            last_seen_at,
        }
    }
}

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

async fn list(State(state): State<AppState>) -> Result<Json<Vec<HostResponse>>, ApiError> {
    Ok(Json(
        state
            .store
            .list_hosts()
            .await?
            .into_iter()
            .map(HostResponse::from)
            .collect(),
    ))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<HostResponse>, ApiError> {
    let id = parse_uuid(&id)?;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    Ok(Json(host.into()))
}

/// Validate + normalise an incoming host payload (shared by create and
/// update). Trims the name/user/hostname, rejects empty required fields and
/// out-of-range ports, resolves explicit key paths on the daemon machine,
/// collapses a blank key path to ssh-agent, and refuses `HostKind::Local` (the
/// local host is a singleton, never created/edited). Passwords are kept
/// verbatim (they may carry meaningful surrounding whitespace); only a value
/// containing no non-whitespace characters is rejected.
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
            validate_ssh_destination_part("user", &user)?;
            validate_ssh_destination_part("hostname", &hostname)?;
            if port == 0 {
                return Err(ApiError::BadRequest(
                    "ssh port must be between 1 and 65535".into(),
                ));
            }
            let auth = match auth {
                SshAuth::Key { path } if path.trim().is_empty() => SshAuth::Agent,
                SshAuth::Key { path } => SshAuth::Key {
                    path: resolve_key_path(&path)?,
                },
                SshAuth::Agent => SshAuth::Agent,
                SshAuth::Password { password }
                    if password.trim().is_empty() || password == REDACTED_SSH_PASSWORD =>
                {
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

/// `ssh` receives `user@hostname` as one argv element. Reject option-looking
/// and control-character-bearing components up front so a malformed saved host
/// cannot be reinterpreted by OpenSSH or inject terminal/log control bytes into
/// diagnostics. Whitespace is invalid in both SSH usernames and hostnames and
/// otherwise only produces a late, confusing connection failure.
fn validate_ssh_destination_part(label: &str, value: &str) -> Result<(), ApiError> {
    if value.starts_with('-') {
        return Err(ApiError::BadRequest(format!(
            "ssh {label} cannot begin with '-'"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(format!(
            "ssh {label} cannot contain control characters"
        )));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(ApiError::BadRequest(format!(
            "ssh {label} cannot contain whitespace"
        )));
    }
    Ok(())
}

/// Resolve an explicit private-key path once when the host is saved. `ssh -i`
/// is spawned directly (there is no shell to expand `~`), and retaining a
/// relative path would make authentication depend on the daemon's future cwd.
/// Canonicalising also gives users an immediate, actionable error for a typo
/// instead of a generic authentication failure during session start.
fn resolve_key_path(raw: &str) -> Result<String, ApiError> {
    let expanded = super::util::expand_workdir(raw)?;
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|e| ApiError::Internal(format!("cannot resolve ssh key path: {e}")))?
            .join(expanded)
    };
    let canonical = std::fs::canonicalize(&absolute).map_err(|e| {
        ApiError::BadRequest(format!(
            "ssh key does not exist or is inaccessible: {} ({e})",
            absolute.display()
        ))
    })?;
    if !canonical.is_file() {
        return Err(ApiError::BadRequest(format!(
            "ssh key is not a file: {}",
            canonical.display()
        )));
    }
    Ok(canonical.to_string_lossy().into_owned())
}

/// Close cached connections before persisting a destination or credential
/// change (or deleting the host). Spawn/timeout failures abort the mutation
/// visibly rather than leaving an authenticated connection alive after its
/// exact configuration disappeared from the database.
async fn invalidate_control_masters(host: &Host) -> Result<(), ApiError> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Ok(());
    }
    crate::host_runtime::invalidate_ssh_control_masters(host)
        .await
        .map_err(|error| ApiError::from_host_runtime(host, error))
}

/// Validate a PUT while preserving an omitted/redacted password as a marker.
/// The stored secret is copied only into a temporary validation value; the
/// returned payload still carries the marker so the store can retain the
/// password atomically at commit time rather than writing a stale route read.
fn prepare_host_update(old: &Host, new: NewHost) -> Result<(NewHost, bool), ApiError> {
    let retain_password = matches!(
        &new.kind,
        HostKind::Ssh {
            auth: SshAuth::Password { password },
            ..
        } if password.is_empty() || password == REDACTED_SSH_PASSWORD
    );
    if !retain_password {
        return validate_host(new).map(|new| (new, false));
    }

    let HostKind::Ssh {
        auth: SshAuth::Password { password: stored },
        ..
    } = &old.kind
    else {
        return Err(ApiError::BadRequest(
            "ssh password is required when switching to password authentication".into(),
        ));
    };

    let mut for_validation = new;
    let HostKind::Ssh {
        auth: SshAuth::Password { password },
        ..
    } = &mut for_validation.kind
    else {
        unreachable!("retain_password is true only for password auth")
    };
    password.clone_from(stored);
    let mut validated = validate_host(for_validation)?;
    let HostKind::Ssh {
        auth: SshAuth::Password { password },
        ..
    } = &mut validated.kind
    else {
        unreachable!("password auth validation preserves the auth mode")
    };
    *password = REDACTED_SSH_PASSWORD.to_string();
    Ok((validated, true))
}

fn ssh_destination_changed(old: &HostKind, new: &HostKind) -> bool {
    match (old, new) {
        (
            HostKind::Ssh {
                user: old_user,
                hostname: old_hostname,
                port: old_port,
                ..
            },
            HostKind::Ssh {
                user: new_user,
                hostname: new_hostname,
                port: new_port,
                ..
            },
        ) => old_user != new_user || old_hostname != new_hostname || old_port != new_port,
        _ => false,
    }
}

async fn create(
    State(state): State<AppState>,
    Json(new): Json<NewHost>,
) -> Result<(StatusCode, Json<HostResponse>), ApiError> {
    let new = validate_host(new)?;
    let host = state.store.create_host(new).await?;
    Ok((StatusCode::CREATED, Json(host.into())))
}

/// `PUT /api/hosts/{id}` — edit an existing SSH host's connection settings.
/// Same validation as create; the store rewrites the row in place (keeping
/// the id so sessions stay attached) and returns the refreshed host.
async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(new): Json<NewHost>,
) -> Result<Json<HostResponse>, ApiError> {
    let id = parse_uuid(&id)?;
    // Host first, then the route-local database mutation gate. Session
    // lifecycle routes use the same host gate before their per-session lock,
    // so no launch can establish a stale master between invalidation and the
    // persisted edit.
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
    let _guard = host_mutation_lock().lock().await;
    let old_host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    if matches!(&old_host.kind, HostKind::Local) {
        return Err(ApiError::NotFound("the local host is not editable".into()));
    }
    let (new, retain_password) = prepare_host_update(&old_host, new)?;
    let destination_changed = ssh_destination_changed(&old_host.kind, &new.kind);
    if destination_changed {
        let bound_count = state
            .store
            .list_sessions(None)
            .await?
            .into_iter()
            .filter(|session| session.host_id == Some(id))
            .count();
        if bound_count > 0 {
            return Err(ApiError::Conflict(format!(
                "cannot change this SSH destination while {bound_count} session(s) are bound; delete them first, then retry the host edit"
            )));
        }
    }
    // A long-lived pane tail/input process may have been spawned but not yet
    // attached to revision A's ControlMaster. Cancel and explicitly reap every
    // registered stream before closing A, otherwise that delayed child could
    // create a fresh `cm/cms-A` after revision B commits.
    super::sessions::cancel_remote_streams_for_host(id).await?;
    crate::host_browser::retire_host_bridges_for_mutation(&old_host)
        .await
        .map_err(|error| ApiError::from_host_runtime(&old_host, error))?;

    // Preserve desired reverse-forward intent until the Store mutation
    // commits. Destination moves discard it only after success; if persistence
    // fails, A remains authoritative and can be warmed again safely.
    invalidate_control_masters(&old_host).await?;
    let update_result = if retain_password {
        state.store.update_host_retaining_password(id, new).await
    } else {
        state.store.update_host(id, new).await
    };
    let host = match update_result {
        Ok(host) => host,
        Err(error) => {
            if let Err(warm_error) = crate::host_runtime::warm_ssh_master(&old_host).await {
                tracing::warn!(host_id = %id, %warm_error, "host update failed and revision A could not be rewarmed");
            }
            return Err(error.into());
        }
    };
    if destination_changed {
        crate::host_runtime::discard_ssh_control_state(id).await;
    }
    // The read/close/commit transaction is complete. Do not make an
    // unreachable host's best-effort warm block edits to unrelated hosts.
    drop(_guard);
    if !destination_changed && let Err(error) = crate::host_runtime::warm_ssh_master(&host).await {
        // The edit is already committed, so returning an error would invite an
        // unsafe blind retry. Preserve the successful response and make the
        // failed immediate tunnel rearm visible; the periodic warmer and next
        // explicit operation retry the same current revision.
        tracing::warn!(host_id = %host.id, %error, "host updated but its SSH master could not be warmed immediately");
    }
    Ok(Json(host.into()))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_uuid(&id)?;
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
    let _guard = host_mutation_lock().lock().await;
    let old_host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    if matches!(&old_host.kind, HostKind::Local) {
        return Err(ApiError::NotFound("the local host is not deletable".into()));
    }

    // Never make a host disappear underneath a managed or externally-bound
    // session. Store::delete_host repeats this check atomically at commit time;
    // this early check avoids closing a healthy master for a deletion that is
    // already known to conflict.
    let bound_count = state
        .store
        .list_sessions(None)
        .await?
        .into_iter()
        .filter(|session| session.host_id == Some(id))
        .count();
    if bound_count > 0 {
        return Err(ApiError::Conflict(format!(
            "host has {bound_count} bound session(s); delete or move them before deleting the host"
        )));
    }

    // Drain any not-yet-attached SSH stream before closing the old revision.
    // Preserve reverse-forward intent until deletion commits so a Store
    // failure can safely restore the still-authoritative host row.
    super::sessions::cancel_remote_streams_for_host(id).await?;
    crate::host_browser::retire_host_bridges_for_mutation(&old_host)
        .await
        .map_err(|error| ApiError::from_host_runtime(&old_host, error))?;
    invalidate_control_masters(&old_host).await?;
    match state.store.delete_host(id).await {
        Ok(true) => {
            crate::host_runtime::discard_ssh_control_state(id).await;
            Ok(StatusCode::NO_CONTENT)
        }
        Ok(false) => {
            if let Err(warm_error) = crate::host_runtime::warm_ssh_master(&old_host).await {
                tracing::warn!(host_id = %id, %warm_error, "host delete lost its Store race and revision A could not be rewarmed");
            }
            Err(ApiError::NotFound(id.to_string()))
        }
        Err(error) => {
            if let Err(warm_error) = crate::host_runtime::warm_ssh_master(&old_host).await {
                tracing::warn!(host_id = %id, %warm_error, "host delete failed and revision A could not be rewarmed");
            }
            Err(error.into())
        }
    }
}

async fn test(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<HostProbe>, ApiError> {
    let id = parse_uuid(&id)?;
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
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
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
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
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    // A failed remote install (sudo password required, no network, etc.)
    // surfaces as a host-boundary 502 with the remote diagnostic.
    let report = crate::host_runtime::bootstrap(&host, &req.items)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?;
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
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let report = crate::host_runtime::install_agents(&host, &req.tools)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?;
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
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
    let host = state
        .store
        .get_host(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(id.to_string()))?;
    let report = crate::host_runtime::provision_skills(&host, &req.skills)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?;
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
    let _host_guard = super::sessions::acquire_host_lifecycle(id).await;
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
    .map_err(|e| ApiError::from_host_runtime(&host, e))?;
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
async fn attach_tmux_session(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    let host_id = parse_uuid(&id)?;
    // The host gate prevents a raw tmux delete or host edit from racing the
    // external binding claim. It must precede the per-session lock below.
    let _host_guard = super::sessions::acquire_host_lifecycle(host_id).await;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(host_id.to_string()))?;

    // Validate liveness + grab pane metadata in the same round trip.
    // Use list_all so agentum-managed sessions can also be attached for viewing.
    let discovered = crate::host_runtime::list_all_tmux_sessions(&host)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("tmux session not found on host: {name}")))?;

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
    let (session, created) = state
        .store
        .create_or_get_external_session(new, host_id, &name)
        .await?;

    // Join the same lifecycle critical section used by start/stop/kill/delete.
    // The binding claim happens first because a newly-created row has no UUID
    // before that point; reload under the lock so a mutation that won the race
    // is observed rather than piping/updating a stale or deleted record.
    let _guard = super::sessions::acquire_session_lifecycle(session.id).await;
    let session = state
        .store
        .get_session_by_id(session.id)
        .await?
        .ok_or_else(|| ApiError::NotFound(session.id.to_string()))?;

    // Arm the pane log pipe and mark the record running with the external
    // target. `pipe-pane -o` is idempotent, so re-attach is harmless.
    // `create_or_get_external_session` may return an existing managed row for
    // this target; verify/migrate its remote ownership marker before any tmux
    // I/O instead of treating it as user-owned merely because this is the
    // attach route.
    let control_target =
        super::sessions::guard_managed_ssh_tmux_io(&state, &host, &session, &name).await?;
    let log =
        paths::pane_log(&session.id.to_string()).map_err(|e| ApiError::Internal(e.to_string()))?;
    crate::host_runtime::pipe_pane(&host, &control_target, &log)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?;
    state
        .store
        .update_status_and_target(session.id, Status::Running, Some(&name))
        .await?;
    let session = state
        .store
        .get_session_by_id(session.id)
        .await?
        .ok_or_else(|| ApiError::NotFound(session.id.to_string()))?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(session),
    ))
}

/// `DELETE /api/hosts/{id}/tmux-sessions/{name}` — kill an unbound, external
/// tmux session on the host. Agentum-managed targets and any target currently
/// bound to a session record must go through `/api/sessions/{id}` so ownership,
/// status, pipes, hooks, and the per-session lifecycle lock stay consistent.
async fn kill_tmux_session_route(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let host_id = parse_uuid(&id)?;
    let _host_guard = super::sessions::acquire_host_lifecycle(host_id).await;
    let host = state
        .store
        .get_host(host_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(host_id.to_string()))?;

    // Managed targets can exist briefly without a persisted tmux_target (for
    // example while spawn recovery is settling), so the namespace itself is
    // reserved even when no row currently matches it.
    if is_agentum_managed_target(&name) {
        return Err(ApiError::Conflict(format!(
            "tmux session `{name}` is Agentum-managed; use the Agentum session lifecycle"
        )));
    }

    // The host lock freezes binding creation/target changes while we inspect.
    // Take the matching session lock as well before the final re-read so this
    // route composes safely with all canonical lifecycle operations.
    if let Some(bound) = state
        .store
        .list_sessions(None)
        .await?
        .into_iter()
        .find(|session| {
            session.host_id == Some(host_id) && session.tmux_target.as_deref() == Some(&name)
        })
    {
        let _session_guard = super::sessions::acquire_session_lifecycle(bound.id).await;
        if state
            .store
            .get_session_by_id(bound.id)
            .await?
            .is_some_and(|session| {
                session.host_id == Some(host_id) && session.tmux_target.as_deref() == Some(&name)
            })
        {
            return Err(ApiError::Conflict(format!(
                "tmux session `{name}` is bound to Agentum session `{}`; use /api/sessions/{}/kill",
                bound.name, bound.id
            )));
        }
    }

    // Defense in depth against tmux's prefix target matching: do not pass a
    // non-existent short prefix (for example `agent`) to a destructive helper
    // when only `agentum-<uuid>` exists. The lower destructive helper must also
    // resolve the exact name to an immutable tmux session id to close the
    // remaining discovery→kill TOCTOU window.
    let discovered = crate::host_runtime::list_all_tmux_sessions(&host)
        .await
        .map_err(|error| ApiError::from_host_runtime(&host, error))?;
    if !has_exact_tmux_target(&discovered, &name) {
        return Err(ApiError::NotFound(format!(
            "tmux session not found on host: {name}"
        )));
    }

    crate::host_runtime::kill_tmux_session(&host, &name)
        .await
        .map_err(|e| ApiError::from_host_runtime(&host, e))?;
    Ok(StatusCode::NO_CONTENT)
}

fn is_agentum_managed_target(name: &str) -> bool {
    name.starts_with("agentum-")
}

fn has_exact_tmux_target(discovered: &[DiscoveredTmuxSession], name: &str) -> bool {
    discovered.iter().any(|session| session.name == name)
}

fn parse_uuid(s: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::BadRequest(format!("invalid uuid: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_store::Store;
    use tokio::sync::broadcast;

    fn ssh_host(user: &str, hostname: &str, auth: SshAuth) -> NewHost {
        NewHost {
            name: "remote".into(),
            kind: HostKind::Ssh {
                user: user.into(),
                hostname: hostname.into(),
                port: 22,
                auth,
            },
        }
    }

    fn stored_ssh_host(auth: SshAuth) -> Host {
        Host {
            id: Uuid::from_u128(7),
            name: "remote".into(),
            kind: HostKind::Ssh {
                user: "user".into(),
                hostname: "example.com".into(),
                port: 22,
                auth,
            },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    async fn state_with_bound_session(
        tmux_target: Option<&str>,
    ) -> (tempfile::TempDir, AppState, Host, Session) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("hosts-route.sqlite"))
            .await
            .unwrap();
        let (bus, _rx) = broadcast::channel(16);
        let state = AppState::new(store, bus);
        let host = state
            .store
            .create_host(ssh_host("user", "127.0.0.1", SshAuth::Agent))
            .await
            .unwrap();
        let session = state
            .store
            .create_session_on_host(
                NewSession {
                    name: format!("bound-{}", Uuid::new_v4().simple()),
                    workdir: "/tmp".into(),
                    tool: "terminal".into(),
                    model: None,
                    flags: if tmux_target.is_some() {
                        vec![EXTERNAL_TMUX_FLAG.to_string()]
                    } else {
                        Vec::new()
                    },
                    card_id: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_base_ref: None,
                },
                Some(host.id),
            )
            .await
            .unwrap();
        let session = if let Some(target) = tmux_target {
            state
                .store
                .update_status_and_target(session.id, Status::Running, Some(target))
                .await
                .unwrap();
            state
                .store
                .get_session_by_id(session.id)
                .await
                .unwrap()
                .unwrap()
        } else {
            session
        };
        (dir, state, host, session)
    }

    #[test]
    fn password_host_response_cannot_serialize_secret_and_remains_client_compatible() {
        let secret = "never-send-this-value";
        let response = HostResponse::from(stored_ssh_host(SshAuth::Password {
            password: secret.into(),
        }));
        let json = serde_json::to_value(response).unwrap();

        assert_eq!(json["kind"], "ssh");
        assert_eq!(json["auth"]["auth"], "password");
        assert!(json["auth"].get("password").is_none());
        assert!(!json.to_string().contains(secret));

        // Existing TUI/API clients still deserialize the response as Host.
        // The in-memory marker lets an unchanged edit round-trip safely.
        let decoded: Host = serde_json::from_value(json).unwrap();
        assert!(matches!(
            decoded.kind,
            HostKind::Ssh {
                auth: SshAuth::Password { password },
                ..
            } if password == REDACTED_SSH_PASSWORD
        ));
    }

    #[test]
    fn new_host_request_still_serializes_an_explicit_password() {
        let new = ssh_host(
            "user",
            "example.com",
            SshAuth::Password {
                password: "new-secret".into(),
            },
        );
        let json = serde_json::to_string(&new).unwrap();
        assert!(json.contains("new-secret"));
    }

    #[test]
    fn omitted_or_redacted_put_password_is_kept_as_an_atomic_store_marker() {
        let stored = "  meaningful saved secret  ";
        let old = stored_ssh_host(SshAuth::Password {
            password: stored.into(),
        });
        for requested in ["", REDACTED_SSH_PASSWORD] {
            let (prepared, retain) = prepare_host_update(
                &old,
                ssh_host(
                    "user",
                    "example.com",
                    SshAuth::Password {
                        password: requested.into(),
                    },
                ),
            )
            .unwrap();
            assert!(retain);
            assert!(matches!(
                prepared.kind,
                HostKind::Ssh {
                    auth: SshAuth::Password { password },
                    ..
                } if password == REDACTED_SSH_PASSWORD
            ));
        }
    }

    #[test]
    fn put_password_replacement_and_auth_switch_do_not_retain_old_secret() {
        let old = stored_ssh_host(SshAuth::Password {
            password: "old-secret".into(),
        });
        let (replacement, retain) = prepare_host_update(
            &old,
            ssh_host(
                "user",
                "example.com",
                SshAuth::Password {
                    password: "new-secret".into(),
                },
            ),
        )
        .unwrap();
        assert!(!retain);
        assert!(matches!(
            replacement.kind,
            HostKind::Ssh {
                auth: SshAuth::Password { password },
                ..
            } if password == "new-secret"
        ));

        let (switched, retain) =
            prepare_host_update(&old, ssh_host("user", "example.com", SshAuth::Agent)).unwrap();
        assert!(!retain);
        assert!(matches!(
            switched.kind,
            HostKind::Ssh {
                auth: SshAuth::Agent,
                ..
            }
        ));
    }

    #[test]
    fn switching_into_password_auth_requires_a_real_secret() {
        let old = stored_ssh_host(SshAuth::Agent);
        for requested in ["", REDACTED_SSH_PASSWORD, "   "] {
            let result = prepare_host_update(
                &old,
                ssh_host(
                    "user",
                    "example.com",
                    SshAuth::Password {
                        password: requested.into(),
                    },
                ),
            );
            assert!(matches!(result, Err(ApiError::BadRequest(_))));
        }
    }

    #[test]
    fn rejects_option_like_or_control_bearing_destination_parts() {
        for new in [
            ssh_host("-oProxyCommand=bad", "example.com", SshAuth::Agent),
            ssh_host("user\nname", "example.com", SshAuth::Agent),
            ssh_host("user", "-Fbad", SshAuth::Agent),
            ssh_host("user", "example.com\rjunk", SshAuth::Agent),
        ] {
            assert!(matches!(validate_host(new), Err(ApiError::BadRequest(_))));
        }
    }

    #[test]
    fn rejects_whitespace_only_password_but_preserves_meaningful_whitespace() {
        let blank = ssh_host(
            "user",
            "example.com",
            SshAuth::Password {
                password: "   \t".into(),
            },
        );
        assert!(matches!(validate_host(blank), Err(ApiError::BadRequest(_))));

        let password = "  meaningful secret  ".to_string();
        let valid = validate_host(ssh_host(
            "user",
            "example.com",
            SshAuth::Password {
                password: password.clone(),
            },
        ))
        .unwrap();
        let HostKind::Ssh {
            auth: SshAuth::Password { password: stored },
            ..
        } = valid.kind
        else {
            panic!("expected password auth");
        };
        assert_eq!(stored, password);
    }

    #[test]
    fn explicit_key_path_is_validated_and_canonicalized() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id test");
        std::fs::write(&key, "fake test key").unwrap();

        let valid = validate_host(ssh_host(
            "user",
            "example.com",
            SshAuth::Key {
                path: format!("  {}  ", key.display()),
            },
        ))
        .unwrap();
        let HostKind::Ssh {
            auth: SshAuth::Key { path },
            ..
        } = valid.kind
        else {
            panic!("expected key auth");
        };
        assert_eq!(std::path::PathBuf::from(path), key.canonicalize().unwrap());
    }

    #[test]
    fn missing_explicit_key_is_rejected_before_ssh() {
        let missing = tempfile::tempdir().unwrap().path().join("missing-key");
        let result = validate_host(ssh_host(
            "user",
            "example.com",
            SshAuth::Key {
                path: missing.to_string_lossy().into_owned(),
            },
        ));
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn destination_classifier_excludes_auth_only_changes() {
        let old = stored_ssh_host(SshAuth::Password {
            password: "old-secret".into(),
        });

        // A redacted password round-trip retains the actual stored credential
        // and therefore is not a connection identity change.
        let retained = ssh_host(
            "user",
            "example.com",
            SshAuth::Password {
                password: REDACTED_SSH_PASSWORD.into(),
            },
        );
        let (retained, marker) = prepare_host_update(&old, retained).unwrap();
        assert!(marker);
        assert!(!ssh_destination_changed(&old.kind, &retained.kind));

        let (password_changed, _) = prepare_host_update(
            &old,
            ssh_host(
                "user",
                "example.com",
                SshAuth::Password {
                    password: "new-secret".into(),
                },
            ),
        )
        .unwrap();
        assert!(!ssh_destination_changed(&old.kind, &password_changed.kind));

        for destination_changed in [
            ssh_host("another-user", "example.com", SshAuth::Agent),
            ssh_host("user", "new.example.com", SshAuth::Agent),
        ] {
            let (destination_changed, _) = prepare_host_update(&old, destination_changed).unwrap();
            assert!(ssh_destination_changed(
                &old.kind,
                &destination_changed.kind
            ));
        }
    }

    #[test]
    fn agentum_tmux_namespace_is_never_raw_deleted() {
        assert!(is_agentum_managed_target("agentum-1234"));
        assert!(is_agentum_managed_target("agentum-legacy-name"));
        assert!(!is_agentum_managed_target("agentumx-user-session"));
        assert!(!is_agentum_managed_target("user-session"));
    }

    #[test]
    fn raw_delete_discovery_does_not_accept_a_managed_prefix() {
        let discovered = vec![DiscoveredTmuxSession {
            name: "agentum-1234".into(),
            attached: false,
            created_at: None,
            panes: Vec::new(),
        }];
        assert!(!has_exact_tmux_target(&discovered, "agent"));
        assert!(has_exact_tmux_target(&discovered, "agentum-1234"));
    }

    #[tokio::test]
    async fn host_delete_rejects_bound_sessions_without_touching_the_host() {
        let (_dir, state, host, session) = state_with_bound_session(None).await;

        let result = remove(State(state.clone()), Path(host.id.to_string())).await;
        assert!(matches!(
            result,
            Err(ApiError::Conflict(message)) if message.contains("bound session")
        ));
        assert!(state.store.get_host(host.id).await.unwrap().is_some());
        assert!(
            state
                .store
                .get_session_by_id(session.id)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn destination_edit_rejects_bound_sessions_without_retargeting_them() {
        let (_dir, state, host, session) = state_with_bound_session(None).await;
        let changed = ssh_host("user", "another.example.com", SshAuth::Agent);

        let result = update(
            State(state.clone()),
            Path(host.id.to_string()),
            Json(changed),
        )
        .await;
        assert!(matches!(
            result,
            Err(ApiError::Conflict(message))
                if message.contains("SSH destination") && message.contains("delete them first")
        ));

        let stored = state.store.get_host(host.id).await.unwrap().unwrap();
        assert_eq!(stored.kind, host.kind);
        let stored_session = state
            .store
            .get_session_by_id(session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_session.host_id, Some(host.id));
    }

    #[tokio::test]
    async fn credential_edit_is_allowed_with_bound_sessions_after_revision_rotation() {
        let (_dir, state, host, _session) = state_with_bound_session(None).await;
        let changed = ssh_host(
            "user",
            "127.0.0.1",
            SshAuth::Password {
                password: "replacement-secret".into(),
            },
        );
        let expected_kind = changed.kind.clone();

        let _response = update(
            State(state.clone()),
            Path(host.id.to_string()),
            Json(changed),
        )
        .await
        .unwrap();
        assert_eq!(
            state.store.get_host(host.id).await.unwrap().unwrap().kind,
            expected_kind
        );
    }

    #[tokio::test]
    async fn display_name_edit_remains_safe_with_bound_sessions() {
        let (_dir, state, host, _session) = state_with_bound_session(None).await;
        let renamed = NewHost {
            name: "renamed remote".into(),
            kind: host.kind.clone(),
        };

        let response = update(
            State(state.clone()),
            Path(host.id.to_string()),
            Json(renamed),
        )
        .await
        .unwrap();
        assert_eq!(response.0.name, "renamed remote");
        assert_eq!(
            state.store.get_host(host.id).await.unwrap().unwrap().kind,
            host.kind
        );
    }

    #[tokio::test]
    async fn raw_tmux_delete_rejects_a_bound_external_target() {
        let target = "user-owned-shell";
        let (_dir, state, host, session) = state_with_bound_session(Some(target)).await;

        let result = kill_tmux_session_route(
            State(state),
            Path((host.id.to_string(), target.to_string())),
        )
        .await;
        assert!(matches!(
            result,
            Err(ApiError::Conflict(message))
                if message.contains(&session.name) && message.contains("/kill")
        ));
    }
}
