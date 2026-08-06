//! axum HTTP(S) server for agentum.
//!
//! HTTPS via self-signed rustls cert + bearer-token middleware on `/api/*`
//! (excluding `/api/health` + `/api/cert`). A plain-HTTP cert-server runs
//! on a side port for trust-on-first-use bootstrap.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentum_core::{Event, Host, HostKind, Status};
use agentum_store::Store;
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware as axum_mw;
use axum::response::IntoResponse;
use axum::routing::get;
use axum_server::tls_rustls::RustlsConfig;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

const EMBEDDED_RUNTIME_LOCK_DIR: &str = "embedded-runtime.lock";
const EMBEDDED_RUNTIME_OWNER_FILE: &str = "owner.json";
const EMBEDDED_RUNTIME_INITIALIZE_GRACE: Duration = Duration::from_secs(30);

pub mod auth;
pub mod bridge;
pub mod cdp_browser;
pub mod cdp_screencast;
mod error;
pub mod git;
pub mod harness;
mod headers;
pub mod host_browser;
pub mod host_install_hints;
pub mod host_runtime;
pub mod linear;
mod logging;
pub mod mcp_provision;
pub mod planner;
pub mod playwright_mcp;
pub mod ratelimit;
mod routes;
mod rules;
pub mod task_sink;
pub mod tls;
mod transcript_store;
pub mod usage;

pub use transcript_store::TranscriptStore;

pub use error::ApiError;

/// Process-wide lock for tests that mutate global env (`AGENTUM_HOME`, …).
/// Env vars are shared across every test thread, so a per-module mutex can't
/// prevent cross-module races — all such tests must take THIS one lock.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resume marker for a session's WS stream. Tracks the byte offset the
/// client last received plus the pane size that was active at that
/// moment, so the next `?resume=true` can replay only the missed delta
/// — but only if the size still matches. See [`AppState::stream_positions`].
#[derive(Clone, Copy, Debug)]
pub struct StreamCheckpoint {
    pub pos: u64,
    pub cols: u16,
    pub rows: u16,
}

/// Capacity of the broadcast bus that fans `Event`s out to every
/// connected SSE / WS client. Slow consumers that lag behind by more
/// than this many events get a `Lagged(n)` error and miss those
/// events — for the activity dots that means a sticky grey while the
/// agent is actually working again. Bumped from 256 → 1024 so a
/// transient client hiccup (focus-stolen TUI, network roundtrip
/// stall, etc.) won't drop a state-change event the user is staring
/// at.
const EVENT_BUS_CAPACITY: usize = 1024;

/// Login + register attempts per remote IP per window.
const AUTH_RATE_LIMIT_ATTEMPTS: usize = 8;
const AUTH_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Sweep expired auth_session rows on this cadence.
const AUTH_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub bus: broadcast::Sender<Event>,
    pub started_at: Instant,
    pub version: &'static str,
    pub auth_limiter: Arc<ratelimit::RateLimiter>,
    /// SHA-256 fingerprint of the active TLS cert, formatted `AB:CD:…`.
    /// Empty when running with `--no-tls`. The client's connect wizard needs
    /// this anonymously (before login) so the user can verify it matches
    /// what `agentum serve` printed on the host TTY.
    pub cert_fingerprint: Arc<String>,
    /// Per-session in-memory cache of plan/todos/tasks, populated by
    /// tailing each agent's Claude Code transcript. See
    /// [`transcript_store`] for how it stays in sync.
    pub transcripts: TranscriptStore,
    /// Per-session resume checkpoint. When a client reconnects with
    /// `{"resume":true}`, the WS handler uses this to replay only the
    /// bytes the client missed during the gap (typically a session-
    /// switch round trip) instead of sending a full `capture-pane`
    /// snapshot that would clobber the client's preserved parser state
    /// with whatever the agent's UI happens to look like *now*. The
    /// checkpoint also remembers the pane size at last save so the
    /// handler can invalidate the resume when the client's viewport
    /// changed during the disconnect — replaying bytes emitted at a
    /// different grid size produces visible layout corruption (cursor
    /// moves target stale cells) that's hard to recover from short of
    /// a full snapshot.
    pub stream_positions:
        Arc<std::sync::Mutex<std::collections::HashMap<uuid::Uuid, StreamCheckpoint>>>,
    /// Short hostname of the box this daemon runs on. Cached once at
    /// boot so the `/api/health` reads are zero-cost. Clients use it to
    /// label the "this server" row with a meaningful identity (e.g.
    /// `omarchy`, `mateo-mac`) instead of a generic placeholder. Cut
    /// at the first `.` so `omarchy.local` reads as `omarchy`.
    pub hostname: String,
    /// When `true`, the auth middleware is bypassed and all API routes are
    /// accessible without a bearer token. Set via `agentum serve --no-auth`.
    pub no_auth: bool,
    /// Pending clipboard requests, keyed by request_id. Inserted by
    /// `POST /api/clipboard/request`, removed by either the timeout
    /// path, the uploads route (on a matching
    /// `X-Clipboard-Request-Id` header), or a `no_image` WS frame
    /// from the agent. `std::sync::Mutex` for symmetry with
    /// `stream_positions` — the critical section is a HashMap touch,
    /// no `.await` while holding the lock.
    pub clipboard_pending: Arc<
        std::sync::Mutex<
            std::collections::HashMap<
                uuid::Uuid,
                tokio::sync::oneshot::Sender<routes::clipboard::ClipboardOutcome>,
            >,
        >,
    >,
    /// Broadcast bus that fans clipboard request frames out to every
    /// connected `agentum clip-agent`. Capacity 64 is well above the
    /// expected concurrent request count (one user × one Ctrl-V
    /// every few seconds, at most) and gives the per-agent WS
    /// handler comfortable headroom against transient task
    /// scheduling jitter.
    pub clipboard_request_bus: broadcast::Sender<routes::clipboard::ClipboardRequestFrame>,
    /// Ephemeral per-session hook tokens. Inserted on `start`, removed on
    /// stop/kill/crash. The hook endpoint validates POSTs against this map
    /// instead of the standard bearer-token auth so agent CLIs (which don't
    /// know the user's bearer token) can self-report lifecycle events via
    /// a simple curl one-liner injected as env vars at launch time.
    /// `std::sync::Mutex` because the critical section is a single HashMap
    /// lookup/insert/remove — no `.await` while holding the lock.
    pub hook_tokens: Arc<std::sync::Mutex<std::collections::HashMap<uuid::Uuid, String>>>,
    /// Owner-private master used to derive a different MCP bearer for each
    /// session. The master itself is never given to an agent. Persisting it
    /// keeps a preserved pane's derived credential valid across daemon restarts.
    pub mcp_token: Arc<String>,
    /// The base URL this server is reachable at on loopback (`http://127.0.0.1:<port>`),
    /// set only for the embedded desktop server (which binds an ephemeral port). The
    /// session-start handler injects it into each pane's `AGENTUM_API_URL` so a CLI run
    /// inside the pane can find THIS server instead of guessing the standalone-daemon
    /// port. `None` for the standalone `agentum serve` daemon (clients use 8822 / their
    /// profile). It also anchors the PostToolUse hook URL, replacing a hardcoded 8822.
    pub api_base_url: Option<String>,
    /// Dedicated loopback listener exposing only `POST /mcp`. Remote reverse
    /// tunnels terminate here, never at [`Self::api_base_url`]: the embedded
    /// REST listener intentionally skips user auth on local loopback, so
    /// forwarding it wholesale would let another process on the SSH host reach
    /// session/host/clipboard/browser APIs. The MCP-only route independently
    /// requires a live session-scoped bearer on every request.
    pub mcp_base_url: Option<String>,
    /// Hook into the host desktop process for ops only it can do (browser
    /// webview automation, macOS computer-use). `None` for the standalone
    /// daemon — `/api/browser/*` and `/api/computer/*` then return 501.
    pub desktop_bridge: Option<std::sync::Arc<dyn crate::bridge::DesktopBridge>>,
    /// The Harness Engine: drives agents one feature at a time behind a
    /// verification gate. Shared (`Arc`) so the `/api/harness/*` routes and the
    /// background [`harness::drive`] task operate on the same in-memory runs +
    /// event bus. Cheap to construct; always present.
    pub harness: Arc<harness::HarnessEngine>,
}

const MCP_TOKEN_SETTING: &str = "runtime.mcp_bearer_token.v1";

fn valid_persisted_mcp_token(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Load the stable MCP derivation master from the owner-only SQLite store,
/// creating it atomically on first upgrade/install. The master is never an
/// accepted wire credential; [`mcp_provision::session_mcp_token`] derives the
/// scoped bearer an individual live session receives. Store::open enforces
/// private DB/sidecar paths, and `setting_get_or_insert` makes concurrent boots
/// converge on one value.
async fn persistent_mcp_token(store: &Store) -> anyhow::Result<String> {
    let candidate = auth::new_token();
    let (stored, _) = store
        .setting_get_or_insert(MCP_TOKEN_SETTING, &candidate)
        .await?;
    if valid_persisted_mcp_token(&stored) {
        return Ok(stored);
    }

    // A manually-corrupted/legacy value is not usable as a derivation master.
    // Rotate it explicitly; tmux's non-secret generation marker makes the next
    // reconciliation restart only panes carrying an old derived credential.
    let replacement = auth::new_token();
    store.setting_set(MCP_TOKEN_SETTING, &replacement).await?;
    Ok(replacement)
}

/// Identity written into the embedded-runtime lease. PID alone is insufficient:
/// a crashed process can leave the directory behind and the OS may later reuse
/// its PID. `started_at` makes that stale-owner check PID-reuse safe.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct EmbeddedRuntimeOwner {
    pid: u32,
    started_at: u64,
    instance_id: uuid::Uuid,
}

/// Process-lifetime lease for the embedded server.
///
/// SSH ControlMasters and their reverse-forward tables are process-external
/// shared state. Two embedded Agentum processes must therefore never reconcile
/// them concurrently: the later process could silently repoint a preserved
/// agent's MCP tunnel at its own ephemeral port. The atomic directory is the
/// dependency-free lock primitive; owner PID + start time permit crash-safe
/// stale recovery.
struct EmbeddedRuntimeLease {
    path: PathBuf,
    owner: EmbeddedRuntimeOwner,
}

impl EmbeddedRuntimeLease {
    fn acquire() -> anyhow::Result<Self> {
        let lock_path = agentum_store::paths::state_dir()
            .map_err(|error| anyhow::anyhow!(error))?
            .join(EMBEDDED_RUNTIME_LOCK_DIR);
        Self::acquire_at(lock_path)
    }

    fn acquire_at(path: PathBuf) -> anyhow::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "embedded runtime lock has no parent directory: {}",
                path.display()
            )
        })?;
        std::fs::create_dir_all(parent)?;
        restrict_private_directory(parent)?;

        let pid = std::process::id();
        let started_at = process_start_time(pid).ok_or_else(|| {
            anyhow::anyhow!("could not determine this Agentum process start time")
        })?;
        let owner = EmbeddedRuntimeOwner {
            pid,
            started_at,
            instance_id: uuid::Uuid::new_v4(),
        };

        // One pass normally acquires the directory. Extra passes cover a race
        // where another process concurrently renames a stale lease.
        for _ in 0..4 {
            match create_private_directory(&path) {
                Ok(()) => {
                    if let Err(error) = write_runtime_owner(&path, &owner) {
                        let _ = std::fs::remove_file(path.join(EMBEDDED_RUNTIME_OWNER_FILE));
                        let _ = std::fs::remove_dir(&path);
                        return Err(error);
                    }
                    return Ok(Self { path, owner });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }

            validate_private_lock_directory(&path)?;
            if let Some(active) = active_runtime_owner(&path)? {
                anyhow::bail!(
                    "another embedded Agentum server is already running (pid {}, started at {}); close it before starting a second desktop/TUI server",
                    active.pid,
                    active.started_at
                );
            }

            // Claim the stale directory by rename before deleting it. A rival
            // process can win the rename, but neither contender can delete a
            // newly-created successor at the canonical lock path.
            let stale_path = parent.join(format!(
                ".embedded-runtime.stale-{}-{}",
                pid, owner.instance_id
            ));
            match std::fs::rename(&path, &stale_path) {
                Ok(()) => std::fs::remove_dir_all(&stale_path)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
        }

        anyhow::bail!(
            "embedded Agentum runtime lock remained contended at {}",
            path.display()
        )
    }
}

impl Drop for EmbeddedRuntimeLease {
    fn drop(&mut self) {
        let owner_path = self.path.join(EMBEDDED_RUNTIME_OWNER_FILE);
        let still_ours = std::fs::read(&owner_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<EmbeddedRuntimeOwner>(&bytes).ok())
            .is_some_and(|owner| owner == self.owner);
        if still_ours {
            let _ = std::fs::remove_file(owner_path);
            // Deliberately non-recursive: unexpected contents make cleanup fail
            // closed instead of deleting anything another actor placed here.
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}

fn process_start_time(pid: u32) -> Option<u64> {
    let system = sysinfo::System::new_all();
    system
        .process(sysinfo::Pid::from_u32(pid))
        .map(sysinfo::Process::start_time)
}

fn active_runtime_owner(path: &Path) -> anyhow::Result<Option<EmbeddedRuntimeOwner>> {
    let owner_path = path.join(EMBEDDED_RUNTIME_OWNER_FILE);
    let owner_bytes = match std::fs::read(&owner_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            reject_recent_unowned_lock(path)?;
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let owner = match serde_json::from_slice::<EmbeddedRuntimeOwner>(&owner_bytes) {
        Ok(owner) => owner,
        Err(_) => {
            reject_recent_unowned_lock(path)?;
            return Ok(None);
        }
    };
    let active = process_start_time(owner.pid) == Some(owner.started_at);
    Ok(active.then_some(owner))
}

fn reject_recent_unowned_lock(path: &Path) -> anyhow::Result<()> {
    let modified = std::fs::metadata(path)?.modified()?;
    if modified.elapsed().unwrap_or(Duration::ZERO) < EMBEDDED_RUNTIME_INITIALIZE_GRACE {
        anyhow::bail!(
            "embedded Agentum runtime lock at {} is still being initialized; retry shortly",
            path.display()
        );
    }
    Ok(())
}

fn write_runtime_owner(path: &Path, owner: &EmbeddedRuntimeOwner) -> anyhow::Result<()> {
    use std::io::Write;

    let owner_path = path.join(EMBEDDED_RUNTIME_OWNER_FILE);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&owner_path)?;
    let bytes = serde_json::to_vec(owner)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn restrict_private_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn validate_private_lock_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!(
            "refusing unsafe embedded runtime lock path (not a real directory): {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            anyhow::bail!(
                "refusing embedded runtime lock with unsafe permissions at {} (expected 0700)",
                path.display()
            );
        }
    }
    Ok(())
}

fn saved_ssh_hosts(hosts: Vec<Host>) -> Vec<Host> {
    hosts
        .into_iter()
        .filter(|host| matches!(host.kind, HostKind::Ssh { .. }))
        .collect()
}

impl AppState {
    pub fn new(store: Store, bus: broadcast::Sender<Event>) -> Self {
        Self::with_fingerprint(store, bus, String::new())
    }

    pub fn with_fingerprint(
        store: Store,
        bus: broadcast::Sender<Event>,
        cert_fingerprint: String,
    ) -> Self {
        let transcripts = TranscriptStore::new(bus.clone());
        // Capacity 64: see ClipboardRequestFrame docs. Slow-agent
        // lag is logged in the WS handler and the channel survives.
        let (clipboard_request_bus, _) = broadcast::channel(64);
        Self {
            store: Arc::new(store),
            bus,
            started_at: Instant::now(),
            version: env!("CARGO_PKG_VERSION"),
            auth_limiter: Arc::new(ratelimit::RateLimiter::new(
                AUTH_RATE_LIMIT_ATTEMPTS,
                AUTH_RATE_LIMIT_WINDOW,
            )),
            cert_fingerprint: Arc::new(cert_fingerprint),
            transcripts,
            stream_positions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: detect_short_hostname(),
            no_auth: false,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Standalone/default construction starts ephemeral. Embedded boot
            // replaces this with the atomically persisted Store credential
            // before the router or any agent launch can observe the state.
            mcp_token: Arc::new(auth::new_token()),
            // Only the embedded loopback server knows its own URL (it binds an
            // ephemeral port); the standalone daemon leaves this None.
            api_base_url: None,
            // Every real boot binds this before constructing either router.
            // Plain AppState::new remains useful to unit tests that do not
            // launch agents or listeners.
            mcp_base_url: None,
            // Set only by the desktop via serve_embedded_loopback_with_bridge.
            desktop_bridge: None,
            harness: Arc::new(harness::HarnessEngine::new()),
        }
    }
}

/// Reconcile persisted sessions at embedded-server boot. `Idle` rows resume
/// through the same host-aware operation as `POST /start`; managed SSH agents
/// persisted as `Running` re-arm their stable remote MCP listener to this boot's
/// ephemeral Mac port. Ownership + credential-generation markers make normal
/// boots non-destructive and first-upgrade credential rotation explicit.
/// `Stopped`, local, terminal and external rows stay untouched. Failures are
/// logged independently so the rest of the sweep still runs.
pub async fn resume_idle_sessions(state: &AppState) {
    let sessions = match state.store.list_sessions(Some(Status::Idle)).await {
        Ok(sessions) => sessions,
        Err(error) => {
            tracing::warn!(%error, "could not load idle sessions for startup resume");
            Vec::new()
        }
    };

    // Snapshot originally-Running rows before any Idle resume can change
    // statuses. Otherwise an Idle SSH MCP session launched by the first sweep
    // would immediately appear in a second `list_sessions(Running)` query and
    // be killed/reprovisioned a second time.
    let running = match state.store.list_sessions(Some(Status::Running)).await {
        Ok(sessions) => sessions,
        Err(error) => {
            tracing::warn!(%error, "could not load running sessions for MCP reprovisioning");
            Vec::new()
        }
    };

    // Inventory hosts independently of session rows. A saved SSH host can have
    // only Stopped sessions (or no rows after those sessions were deleted) and
    // still own a pooled ControlMaster carrying a reverse forward to the prior
    // embedded server's now-dead ephemeral port.
    let ssh_hosts: Vec<Host> = match state.store.list_hosts().await {
        Ok(hosts) => saved_ssh_hosts(hosts),
        Err(error) => {
            tracing::warn!(%error, "could not load SSH hosts for startup tunnel reset");
            Vec::new()
        }
    };

    let boot_mcp_host_ids: std::collections::HashSet<_> = sessions
        .iter()
        .chain(running.iter())
        .filter(|session| routes::sessions::managed_session_consumes_agentum_mcp(session))
        .map(|session| session.host_id.unwrap_or(agentum_core::LOCAL_HOST_ID))
        .collect();

    // The agent-facing reverse port and bearer token are stable, while the
    // embedded Mac port changes each boot. Close old masters for EVERY saved
    // SSH host, which removes stale forwards even when no resumable session
    // references that host. Hold the route-level host lifecycle lock across
    // reset + optional rearm: a user start that wins the lock first is observed
    // by the fresh store read below and rearmed; one that runs afterward builds
    // its own tunnel on the already-reset master.
    //
    // `reset_ssh_master_for_mcp_rearm` also serializes the transport's global
    // master/tunnel state, so spawning per-host tasks would only queue behind
    // the same critical section and provide no safe parallelism.
    for saved_host in &ssh_hosts {
        let _host_guard = routes::sessions::acquire_host_lifecycle(saved_host.id).await;
        // The saved-host inventory predates lock acquisition. A credential
        // update may have won the lock first, so reload the exact revision we
        // will reset/warm rather than resurrecting the stale snapshot.
        let host = match state.store.get_host(saved_host.id).await {
            Ok(Some(host)) if matches!(host.kind, HostKind::Ssh { .. }) => host,
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(host_id = %saved_host.id, %error, "could not refresh SSH host during boot reset");
                continue;
            }
        };
        if let Err(error) = host_runtime::reset_ssh_master_for_mcp_rearm(&host).await {
            tracing::warn!(host_id = %host.id, %error, "could not reset SSH master at boot");
        }

        // Refresh while this host's lifecycle is locked so a concurrent API
        // launch cannot fall between the boot snapshot and master reset.
        let has_live_mcp_consumer = match state.store.list_sessions(None).await {
            Ok(current) => current.iter().any(|session| {
                session.host_id == Some(host.id)
                    && matches!(session.status, Status::Idle | Status::Running)
                    && routes::sessions::managed_session_consumes_agentum_mcp(session)
            }),
            Err(error) => {
                tracing::warn!(host_id = %host.id, %error, "could not refresh sessions during SSH rearm");
                boot_mcp_host_ids.contains(&host.id)
            }
        };
        if has_live_mcp_consumer {
            let Some(mac_port) = mcp_provision::local_mcp_port(state) else {
                tracing::warn!(host_id = %host.id, "embedded MCP port unavailable during SSH rearm");
                continue;
            };
            if let Err(error) = host_runtime::ensure_reverse_tunnel(&host, mac_port).await {
                tracing::warn!(host_id = %host.id, %error, "could not rearm stable remote MCP listener");
            }
        }
    }

    for session in sessions {
        if let Err(error) = routes::sessions::resume_idle_session_by_id(state, session.id).await {
            tracing::warn!(
                session_id = %session.id,
                session_name = %session.name,
                host_id = ?session.host_id,
                %error,
                "could not resume idle session"
            );
        }
    }

    // Reconcile persisted Running SSH MCP rows too. Their owner + credential
    // generation markers decide whether this is a normal no-kill reattach or a
    // one-time upgrade restart. Local panes, terminals and external sessions
    // remain untouched.
    for session in running {
        if let Err(error) =
            routes::sessions::reprovision_running_remote_mcp_session_by_id(state, session.id).await
        {
            tracing::warn!(
                session_id = %session.id,
                session_name = %session.name,
                host_id = ?session.host_id,
                %error,
                "could not reprovision preserved remote MCP session"
            );
        }
    }
}

/// Resolve the system hostname once at boot, trimming the FQDN suffix
/// so `omarchy.local` reads as `omarchy`. Falls back to `"local"` when
/// the OS lookup fails — better than empty, which would re-trigger
/// the generic placeholder on the client.
fn detect_short_hostname() -> String {
    let raw = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match raw {
        Some(name) => name.split('.').next().unwrap_or(&name).to_ascii_lowercase(),
        None => "local".to_string(),
    }
}

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub addr: SocketAddr,
    pub cert_addr: SocketAddr,
    pub tls: bool,
    /// When `true`, skip all bearer-token checks. Set via `agentum serve --no-auth`.
    pub no_auth: bool,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(routes::health::router())
        .merge(routes::host::router())
        .merge(routes::host_browser::router())
        .merge(routes::hosts::router())
        .merge(routes::mcp::router())
        .merge(routes::cert::router())
        .merge(routes::doctor::router())
        .merge(routes::auth::router())
        .merge(routes::sessions::router())
        .merge(routes::uploads::router())
        .merge(routes::agents::router())
        .merge(routes::agent_tasks::router())
        .merge(routes::board::router())
        .merge(routes::board_goals::router())
        .merge(routes::board_links::router())
        .merge(routes::board_rules::router())
        .merge(routes::notes::router())
        .merge(routes::preferences::router())
        .merge(routes::preflight::router())
        .merge(routes::profiles::router())
        .merge(routes::channels::router())
        .merge(routes::orchestration::router())
        .merge(routes::browser::router())
        .merge(routes::cdp_browser::router())
        .merge(routes::cdp_screencast::router())
        .merge(routes::computer::router())
        .merge(routes::clipboard::router())
        .merge(routes::events::router())
        .merge(routes::watchdog::router())
        .merge(routes::fs::router())
        .merge(routes::git::router())
        .merge(routes::repos::router())
        .merge(routes::worktrees::router())
        .merge(routes::forge::router())
        .merge(routes::usage::router())
        .merge(routes::harness::router())
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        // API-only daemon: no embedded web UI. Unmatched non-`/api` paths 404.
        // Clients are the TUI (`agentum terminal`) and the desktop app.
        .with_state(state)
        // Security response headers wrap every response. See `headers.rs` for
        // the CSP rationale.
        .layer(axum_mw::from_fn(headers::security_headers))
        // CORS: permissive on origin so a client served by one daemon
        // can talk to another daemon (the named-profiles feature). The
        // API is still bearer-protected — an open `Allow-Origin` only
        // grants the same access an unauthenticated cross-origin call
        // would already get (none, except `/api/health` + `/api/cert`).
        // We intentionally do NOT set `Allow-Credentials: true` because
        // we don't use cookies; the bearer is sent via `Authorization`
        // header (or `?token=` for WS), so wildcard-origin is safe.
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .layer(logging::redacting_trace_layer())
}

/// Router bound behind the SSH reverse tunnel. Keep this deliberately tiny:
/// the full embedded API is loopback/no-auth for the local TUI and desktop,
/// while a reverse tunnel makes its destination reachable from another
/// machine. `/mcp` performs its own live, session-scoped bearer check before it
/// parses or dispatches a JSON-RPC message.
fn mcp_only_router(state: AppState) -> Router {
    routes::mcp::router()
        .with_state(state)
        .layer(axum_mw::from_fn(headers::security_headers))
        .layer(logging::redacting_trace_layer())
}

/// Bind the private MCP-only listener before session reconciliation starts and
/// publish its exact loopback URL into shared state. Callers must keep the
/// returned listener alive for the same lifetime as their primary API server.
async fn bind_mcp_only_listener(state: &mut AppState) -> anyhow::Result<TcpListener> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    state.mcp_base_url = Some(format!("http://{addr}"));
    Ok(listener)
}

fn spawn_mcp_only_listener(listener: TcpListener, state: AppState) -> tokio::task::JoinHandle<()> {
    let addr = listener.local_addr().ok();
    let app = mcp_only_router(state);
    tracing::info!(?addr, "agentum MCP-only listener ready");
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::error!(%error, "agentum MCP-only listener exited");
        }
    })
}

/// Bind and serve forever. Drives both the main API server (TLS or plain),
/// the small plain-HTTP cert-server, the watchdog reconcile loop, and a
/// periodic auth-session sweeper.
pub async fn serve(opts: ServeOptions, store: Store) -> anyhow::Result<()> {
    // rustls 0.23 requires picking a CryptoProvider when both ring and
    // aws-lc-rs are pulled in transitively. We don't care which one wins;
    // ring is the smaller of the two.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Load cert artifacts up-front so the fingerprint can flow into
    // AppState (needed by `/api/cert/fingerprint` for the wizard).
    let tls_artifacts = if opts.tls {
        Some(tls::ensure_artifacts()?)
    } else {
        None
    };
    let cert_fingerprint = tls_artifacts
        .as_ref()
        .and_then(|a| tls::cert_fingerprint(&a.cert_pem).ok())
        .unwrap_or_default();

    let (bus, _) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
    let mut state = AppState::with_fingerprint(store, bus.clone(), cert_fingerprint);
    state.no_auth = opts.no_auth;
    // Standalone and embedded boots must derive the same scoped credential for
    // preserved panes. An ephemeral master here strands every already-running
    // agent after a daemon restart even though its tmux pane survived.
    state.mcp_token = Arc::new(persistent_mcp_token(&state.store).await?);
    let mcp_listener = bind_mcp_only_listener(&mut state).await?;

    if state.store.count_users().await.unwrap_or(0) == 0 {
        tracing::warn!(
            "no users registered yet — open the desktop app to register the first one (or run `agentum auth add <name>` on the host)"
        );
    }

    let app = router(state.clone());

    if let Some(artifacts) = tls_artifacts {
        // Print SHA-256 fingerprint so the operator can verify out-of-band
        // when a second device first trusts the cert.
        if !state.cert_fingerprint.is_empty() {
            tracing::info!(
                "TLS cert fingerprint (verify on second device): SHA-256 {}",
                state.cert_fingerprint
            );
        }
        let tls_config =
            RustlsConfig::from_pem_file(&artifacts.cert_path, &artifacts.key_path).await?;

        let cert_router = cert_server_router(artifacts.cert_pem.clone());
        let cert_listener = TcpListener::bind(opts.cert_addr).await?;
        tracing::info!(addr = %opts.cert_addr, "cert-server listening (plain http)");
        let cert_handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(cert_listener, cert_router).await {
                tracing::error!("cert server exited: {e}");
            }
        });

        let mcp_handle = spawn_mcp_only_listener(mcp_listener, state.clone());
        spawn_startup_reconciliation_then_workers(state.clone(), bus.clone());
        tracing::info!(addr = %opts.addr, "agentum-server listening (https)");
        let result = axum_server::bind_rustls(opts.addr, tls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await;
        cert_handle.abort();
        mcp_handle.abort();
        result.map_err(Into::into)
    } else {
        tracing::warn!(addr = %opts.addr, "agentum-server listening (plain http; --no-tls)");
        let listener = TcpListener::bind(opts.addr).await?;
        let mcp_handle = spawn_mcp_only_listener(mcp_listener, state.clone());
        spawn_startup_reconciliation_then_workers(state.clone(), bus.clone());
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
        mcp_handle.abort();
        result.map_err(Into::into)
    }
}

/// Spawn the always-on background workers shared by every server boot path:
/// the auth-session sweeper, the watchdog, the goal-status reconciler, the
/// session→comment bridge, and the host-metrics ticker. Factored out so the
/// in-process embedded boot (desktop) and the standalone `serve()` (TUI/daemon)
/// stay in lockstep.
fn spawn_background_workers(state: &AppState, bus: &broadcast::Sender<Event>) {
    // Sweep stale tokens once at boot, then on a slow timer.
    let sweep_state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(AUTH_SWEEP_INTERVAL);
        loop {
            tick.tick().await;
            match sweep_state.store.sweep_expired_auth_sessions().await {
                Ok(0) => {}
                Ok(n) => tracing::info!(rows = n, "swept expired auth sessions"),
                Err(e) => tracing::warn!(error = %e, "auth session sweep failed"),
            }
        }
    });

    let watchdog = agentum_watchdog::Watchdog::new(bus.clone(), state.store.clone());
    tokio::spawn(watchdog.run());

    // Goal-status auto-progression reconciler: enforces `goal.status = max(child
    // statuses)` and fires the planner auto-stop on first child arrival.
    {
        let store = state.store.clone();
        let bus = bus.clone();
        tokio::spawn(async move {
            agentum_watchdog::run_goal_reconciler(store, bus).await;
        });
    }

    // Watchdog → comment bridge: converts agent.*/session.crashed events into
    // [system] comments on the bound card's thread.
    {
        let store = state.store.clone();
        let bus = bus.clone();
        tokio::spawn(async move {
            agentum_watchdog::run_session_comment_bridge(store, bus).await;
        });
    }

    // Host-metrics ticker: publishes CPU+RAM onto the bus so one sampler feeds
    // every connected client over the events WS.
    let _host_metrics = routes::host::spawn_ticker(bus.clone());

    // SSH ControlMaster warmer: a no-op exec per known SSH host opens the
    // pooled master at boot (interval's first tick is immediate) and refreshes
    // it just inside the ControlPersist window (600s), so interactive remote
    // ops — sidebar git, session spawn, file browse — never pay the 1-3s
    // TCP+auth handshake. Unreachable hosts cost one bounded (ConnectTimeout=8)
    // background attempt per tick.
    {
        const SSH_MASTER_WARM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(480);
        let store = state.store.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SSH_MASTER_WARM_INTERVAL);
            loop {
                tick.tick().await;
                let hosts = match store.list_hosts().await {
                    Ok(hosts) => hosts,
                    Err(_) => continue,
                };
                for host in hosts
                    .into_iter()
                    .filter(|h| matches!(h.kind, agentum_core::HostKind::Ssh { .. }))
                {
                    // Per-host spawn: one slow/dead host must not delay warming
                    // the others.
                    let store = store.clone();
                    tokio::spawn(async move {
                        // Serialize with host credential edits. Otherwise a
                        // warmer holding a stale Host snapshot can reopen the
                        // just-closed old-credential master after PATCH /host.
                        let _host_guard =
                            crate::routes::sessions::acquire_host_lifecycle(host.id).await;
                        // Re-read after acquiring the lock: `host` came from
                        // the outer sweep and may predate a credential update
                        // that won the lifecycle lock first.
                        let current = match store.get_host(host.id).await {
                            Ok(Some(current))
                                if matches!(current.kind, agentum_core::HostKind::Ssh { .. }) =>
                            {
                                current
                            }
                            _ => return,
                        };
                        let _ = crate::host_runtime::warm_ssh_master(&current).await;
                    });
                }
            }
        });
    }
}

/// Run the potentially slow SSH/session boot reconciliation after the MCP-only
/// listener is live. Standalone and embedded boots use the same ordering so a
/// preserved ControlMaster cannot retain a reverse forward to the preceding
/// process's loopback port. Worker startup intentionally remains after
/// reconciliation: the watchdog must not observe the controlled reset and race
/// it into a false crash transition.
fn spawn_startup_reconciliation_then_workers(state: AppState, bus: broadcast::Sender<Event>) {
    tokio::spawn(async move {
        resume_idle_sessions(&state).await;
        spawn_background_workers(&state, &bus);
    });
}

/// Boot the API server in-process on an ephemeral loopback port with auth
/// disabled (loopback bind → only this machine can reach it). Spawns the same
/// background workers as [`serve`] and serves on the current Tokio runtime,
/// returning the bound `127.0.0.1:<port>` address. The desktop shell embeds the
/// server this way so the webview drives the exact same core as the TUI.
pub async fn serve_embedded_loopback(store: Store) -> anyhow::Result<SocketAddr> {
    let (addr, _state) = serve_embedded_loopback_state(store).await?;
    Ok(addr)
}

/// Build the embedded-server `AppState` for a given bound address: no-auth, with
/// `api_base_url` set so the session-start handler can inject `AGENTUM_API_URL`
/// into panes and anchor the hook URL. Pure (no spawning / no serving) so the
/// construction can be unit-tested without standing up the full server. Returns
/// the bus alongside so the caller can wire background workers.
fn embedded_app_state(store: Store, addr: SocketAddr) -> (AppState, broadcast::Sender<Event>) {
    let (bus, _) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
    let mut state = AppState::with_fingerprint(store, bus.clone(), String::new());
    state.no_auth = true;
    state.api_base_url = Some(format!("http://{addr}"));
    (state, bus)
}

/// As [`serve_embedded_loopback`], but installs a [`bridge::DesktopBridge`] so
/// `/api/browser/*` and `/api/computer/*` can drive the host desktop process.
/// The desktop calls this with a bridge holding its Tauri `AppHandle`.
pub async fn serve_embedded_loopback_with_bridge(
    store: Store,
    bridge: Arc<dyn bridge::DesktopBridge>,
) -> anyhow::Result<SocketAddr> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let runtime_lease = EmbeddedRuntimeLease::acquire()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let (mut state, bus) = embedded_app_state(store, addr);
    state.mcp_token = Arc::new(persistent_mcp_token(&state.store).await?);
    state.desktop_bridge = Some(bridge);
    let mcp_listener = bind_mcp_only_listener(&mut state).await?;
    let mcp_handle = spawn_mcp_only_listener(mcp_listener, state.clone());
    let app = router(state.clone());
    tracing::info!(%addr, "agentum-server listening (embedded loopback, desktop bridge)");
    tokio::spawn(async move {
        // Keep the cross-process lease for exactly the listener's lifetime.
        let _runtime_lease = runtime_lease;
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            tracing::error!("embedded agentum-server exited: {e}");
        }
        mcp_handle.abort();
    });
    spawn_startup_reconciliation_then_workers(state, bus);
    Ok(addr)
}

/// As [`serve_embedded_loopback`], but also returns the `AppState` the router was
/// built from. The desktop only needs the address; this exists so the boot path
/// has a single source of truth for the embedded state.
pub async fn serve_embedded_loopback_state(store: Store) -> anyhow::Result<(SocketAddr, AppState)> {
    // rustls provider selection is process-global; harmless if already set.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let runtime_lease = EmbeddedRuntimeLease::acquire()?;

    // Bind first so we know the ephemeral port BEFORE building the state — it
    // must carry its own URL.
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;

    let (mut state, bus) = embedded_app_state(store, addr);
    state.mcp_token = Arc::new(persistent_mcp_token(&state.store).await?);
    let mcp_listener = bind_mcp_only_listener(&mut state).await?;
    let mcp_handle = spawn_mcp_only_listener(mcp_listener, state.clone());

    let app = router(state.clone());
    tracing::info!(%addr, "agentum-server listening (embedded loopback, no-auth)");
    tokio::spawn(async move {
        let _runtime_lease = runtime_lease;
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            tracing::error!("embedded agentum-server exited: {e}");
        }
        mcp_handle.abort();
    });
    spawn_startup_reconciliation_then_workers(state.clone(), bus);
    Ok((addr, state))
}

fn cert_server_router(cert_pem: String) -> Router {
    let pem = Arc::new(cert_pem);
    let pem_for_route = pem.clone();
    Router::new()
        .route(
            "/api/cert",
            get(move || {
                let pem = pem_for_route.clone();
                async move {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/x-pem-file")],
                        pem.as_str().to_string(),
                    )
                }
            }),
        )
        .fallback(get(cert_redirect_hint))
        .layer(logging::redacting_trace_layer())
}

async fn cert_redirect_hint(_: Request<Body>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        "agentum cert server. GET /api/cert for the PEM. The full app is on the TLS port.\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentum_core::{NewHost, NewSession, SshAuth};
    use tower::ServiceExt as _;

    #[test]
    fn embedded_runtime_lease_is_exclusive_and_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EMBEDDED_RUNTIME_LOCK_DIR);

        let first = EmbeddedRuntimeLease::acquire_at(path.clone()).unwrap();
        let error = EmbeddedRuntimeLease::acquire_at(path.clone())
            .err()
            .expect("a second embedded runtime must be rejected");
        assert!(error.to_string().contains("already running"));

        drop(first);
        assert!(!path.exists());
        let replacement = EmbeddedRuntimeLease::acquire_at(path.clone()).unwrap();
        drop(replacement);
        assert!(!path.exists());
    }

    #[test]
    fn embedded_runtime_lease_reclaims_pid_reuse_safe_stale_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EMBEDDED_RUNTIME_LOCK_DIR);
        create_private_directory(&path).unwrap();
        let actual_start = process_start_time(std::process::id()).unwrap();
        let stale = EmbeddedRuntimeOwner {
            pid: std::process::id(),
            started_at: actual_start.saturating_add(1),
            instance_id: uuid::Uuid::new_v4(),
        };
        write_runtime_owner(&path, &stale).unwrap();

        let lease = EmbeddedRuntimeLease::acquire_at(path.clone()).unwrap();
        assert_ne!(lease.owner.instance_id, stale.instance_id);
        drop(lease);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn embedded_runtime_lease_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EMBEDDED_RUNTIME_LOCK_DIR);
        let lease = EmbeddedRuntimeLease::acquire_at(path.clone()).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(path.join(EMBEDDED_RUNTIME_OWNER_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(lease);
    }

    #[cfg(unix)]
    #[test]
    fn embedded_runtime_lease_rejects_a_symlink_lock_path() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("attacker-controlled");
        std::fs::create_dir(&target).unwrap();
        let path = dir.path().join(EMBEDDED_RUNTIME_LOCK_DIR);
        symlink(&target, &path).unwrap();

        let error = EmbeddedRuntimeLease::acquire_at(path)
            .err()
            .expect("a symlink must never be treated as the runtime lock directory");
        assert!(
            error
                .to_string()
                .contains("unsafe embedded runtime lock path")
        );
    }

    #[tokio::test]
    async fn embedded_app_state_carries_its_url_and_is_no_auth() {
        // Use the pure builder, NOT serve_embedded_loopback_state — the latter
        // spawns the server + background workers, which would keep the test
        // process alive and hang the suite.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();
        let addr: SocketAddr = "127.0.0.1:5544".parse().unwrap();
        let (state, _bus) = embedded_app_state(store, addr);
        // The embedded state must carry its own URL (so panes get AGENTUM_API_URL)
        // and be no-auth (loopback bind).
        assert_eq!(state.api_base_url.as_deref(), Some("http://127.0.0.1:5544"));
        assert!(state.no_auth);
    }

    #[tokio::test]
    async fn standalone_app_state_has_no_api_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();
        let (bus, _) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
        let state = AppState::new(store, bus);
        assert_eq!(state.api_base_url, None);
        assert_eq!(state.mcp_base_url, None);
    }

    #[tokio::test]
    async fn dedicated_mcp_listener_exposes_no_rest_api_and_requires_its_bearer() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();
        let (bus, _) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
        let mut state = AppState::new(store, bus);
        let listener = bind_mcp_only_listener(&mut state).await.unwrap();
        let mcp_addr = listener.local_addr().unwrap();
        assert_eq!(
            state.mcp_base_url.as_deref(),
            Some(format!("http://{mcp_addr}").as_str())
        );
        drop(listener);

        let app = mcp_only_router(state);
        let rest = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rest.status(), axum::http::StatusCode::NOT_FOUND);

        let unauthorized = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn persistent_mcp_token_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();

        let first = persistent_mcp_token(&store).await.unwrap();
        let second = persistent_mcp_token(&store).await.unwrap();

        assert!(valid_persisted_mcp_token(&first));
        assert_eq!(second, first);
        assert_eq!(
            store
                .setting_get(MCP_TOKEN_SETTING)
                .await
                .unwrap()
                .as_deref(),
            Some(first.as_str())
        );
    }

    #[tokio::test]
    async fn persistent_mcp_token_rotates_an_invalid_stored_value() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();
        store
            .setting_set(MCP_TOKEN_SETTING, "corrupt token")
            .await
            .unwrap();

        let token = persistent_mcp_token(&store).await.unwrap();

        assert!(valid_persisted_mcp_token(&token));
        assert_ne!(token, "corrupt token");
        assert_eq!(
            store
                .setting_get(MCP_TOKEN_SETTING)
                .await
                .unwrap()
                .as_deref(),
            Some(token.as_str())
        );
    }

    #[tokio::test]
    async fn saved_ssh_host_inventory_does_not_depend_on_session_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();
        let ssh_host = store
            .create_host(NewHost {
                name: "sessionless-ssh".into(),
                kind: HostKind::Ssh {
                    user: "agentum".into(),
                    hostname: "192.0.2.1".into(),
                    port: 22,
                    auth: SshAuth::Agent,
                },
            })
            .await
            .unwrap();
        let deleted = store
            .create_session_on_host(
                NewSession {
                    name: "deleted-remote".into(),
                    workdir: "/tmp".into(),
                    tool: "terminal".into(),
                    model: None,
                    flags: Vec::new(),
                    card_id: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_base_ref: None,
                },
                Some(ssh_host.id),
            )
            .await
            .unwrap();
        store.delete_session(deleted.id).await.unwrap();

        let hosts = saved_ssh_hosts(store.list_hosts().await.unwrap());
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].id, ssh_host.id);
    }

    #[tokio::test]
    async fn startup_resume_leaves_explicitly_stopped_sessions_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).await.unwrap();
        let session = store
            .create_session(NewSession {
                name: "stay-stopped".into(),
                workdir: dir.path().to_string_lossy().into_owned(),
                tool: "terminal".into(),
                model: None,
                flags: Vec::new(),
                card_id: None,
                worktree_path: None,
                worktree_branch: None,
                worktree_base_ref: None,
            })
            .await
            .unwrap();
        store
            .update_status_and_target(session.id, Status::Stopped, None)
            .await
            .unwrap();
        let (bus, _) = broadcast::channel::<Event>(EVENT_BUS_CAPACITY);
        let state = AppState::new(store, bus);

        resume_idle_sessions(&state).await;

        let stored = state
            .store
            .get_session_by_id(session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, Status::Stopped);
        assert!(stored.tmux_target.is_none());
    }
}
