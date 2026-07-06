//! axum HTTP(S) server for agentum.
//!
//! HTTPS via self-signed rustls cert + bearer-token middleware on `/api/*`
//! (excluding `/api/health` + `/api/cert`). A plain-HTTP cert-server runs
//! on a side port for trust-on-first-use bootstrap.

// The `agentum_browser` tool schema in `routes::mcp::tool_specs` is one large
// `json!` literal; its recursive macro expansion exceeds the default limit of 128.
#![recursion_limit = "512"]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agentum_core::Event;
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

pub mod auth;
pub mod bridge;
pub mod cdp_browser;
pub mod cdp_driver;
pub(crate) mod cdp_http;
pub mod cdp_screencast;
pub mod endpoint;
mod error;
pub mod git;
pub mod harness;
mod headers;
pub mod host_install_hints;
pub mod host_runtime;
pub mod linear;
mod logging;
pub mod mcp_provision;
mod pane_log_reaper;
mod pane_repair;
pub mod planner;
pub mod playwright_mcp;
mod port_wait;
pub mod ratelimit;
mod routes;
mod rules;
pub mod task_sink;
pub mod tls;
mod transcript_store;
pub mod usage;
pub(crate) mod wiki;
pub(crate) mod wiki_rag;

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

/// The composite key for [`AppState::wiki_keys`]: `(repo_id, path, host_id)` —
/// `host_id` is `LOCAL_HOST_ID` (the nil UUID) for local repos. A change to any
/// component (repo re-added, moved, re-homed to another host) builds a
/// *different* key → cache miss → re-resolve; that lookup-time
/// self-invalidation is the whole staleness story (spec 009 D-A3), no
/// mutation hooks.
pub type WikiKeyCacheKey = (String, String, uuid::Uuid);

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
    /// repo→wiki-key cache (spec 009 D-A3): a hit skips the per-call
    /// `git remote get-url` subprocess in `routes::wiki::resolve_target` — the
    /// macOS TCC-prompt trigger (and, over SSH, a network round trip). Keyed by
    /// `(repo_id, path, host_id)` (`LOCAL_HOST_ID` = the nil UUID for local
    /// repos) so a moved or re-homed repo builds a *different* key and
    /// self-invalidates on lookup — no repo-mutation hook needed. Positive-only:
    /// only a successful, non-empty remote resolution (a `git__…` key) is ever
    /// cached; the `path__<hash>` fallback never is (over SSH a transport
    /// failure is indistinguishable from "no origin", and caching it would pin
    /// the repo to the wrong wiki until restart). `std::sync::Mutex` like
    /// `stream_positions`: never held across an `.await`.
    pub wiki_keys: Arc<std::sync::Mutex<std::collections::HashMap<WikiKeyCacheKey, String>>>,
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
    /// Secret bearer token guarding the agentum MCP server (`/mcp`). Minted once
    /// at boot. Every agentum-launched agent gets it baked into its MCP config
    /// (`Authorization: Bearer …`), and the `/mcp` handler rejects any request
    /// without it — *even on the no-auth embedded server*. This is what makes the
    /// MCP safe to expose to a remote host over the reverse SSH tunnel: the port
    /// is loopback-bound on the host AND the tool surface needs this token, so
    /// another user/process on the host can't drive agentum.
    pub mcp_token: Arc<String>,
    /// The base URL this server is reachable at on loopback (`http://127.0.0.1:<port>`),
    /// set only for the embedded desktop server (which binds an ephemeral port). The
    /// session-start handler injects it into each pane's `AGENTUM_API_URL` so a CLI run
    /// inside the pane can find THIS server instead of guessing the standalone-daemon
    /// port. `None` for the standalone `agentum serve` daemon (clients use 8822 / their
    /// profile). It also anchors the PostToolUse hook URL, replacing a hardcoded 8822.
    pub api_base_url: Option<String>,
    /// Hook into the host desktop process for ops only it can do (browser
    /// webview automation, macOS computer-use). `None` for the standalone
    /// daemon — `/api/browser/*` and `/api/computer/*` then return 501.
    pub desktop_bridge: Option<std::sync::Arc<dyn crate::bridge::DesktopBridge>>,
    /// The Harness Engine: drives agents one feature at a time behind a
    /// verification gate. Shared (`Arc`) so the `/api/harness/*` routes and the
    /// background [`harness::drive`] task operate on the same in-memory runs +
    /// event bus. Cheap to construct; always present.
    pub harness: Arc<harness::HarnessEngine>,
    /// Live `/api/events` WebSocket client count. The host-metrics ticker
    /// gates its sysinfo sampling on THIS, not `bus.receiver_count()`: the
    /// goal reconciler and comment bridge hold permanent bus subscriptions,
    /// so the receiver count never reaches zero and the "no dashboards →
    /// don't sample" guard was dead code — the daemon paid an all-cores CPU
    /// refresh every 2 s forever. Only the events route touches this.
    pub events_ws_clients: Arc<std::sync::atomic::AtomicUsize>,
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
            wiki_keys: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            hostname: detect_short_hostname(),
            no_auth: false,
            clipboard_pending: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            clipboard_request_bus,
            hook_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            // Minted once per server instance; agents present it on every /mcp call.
            mcp_token: Arc::new(auth::new_token()),
            // Only the embedded loopback server knows its own URL (it binds an
            // ephemeral port); the standalone daemon leaves this None.
            api_base_url: None,
            // Set only by the desktop via serve_embedded_loopback_with_bridge.
            desktop_bridge: None,
            harness: Arc::new(harness::HarnessEngine::new()),
            events_ws_clients: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        // 016a: server-side board←GitHub pull + durable tracker bindings. Adds
        // `/api/board/bindings*` only; #58's `POST /api/board/sync` stays in
        // `board::router()` above, untouched.
        .merge(routes::board_sync::router())
        .merge(routes::notes::router())
        .merge(routes::wiki::router())
        .merge(routes::preferences::router())
        .merge(routes::preflight::router())
        .merge(routes::profiles::router())
        .merge(routes::channels::router())
        .merge(routes::chat::router())
        .merge(routes::orchestration::router())
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
        .merge(routes::github::router())
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

    if state.store.count_users().await.unwrap_or(0) == 0 {
        tracing::warn!(
            "no users registered yet — open the desktop app to register the first one (or run `agentum auth add <name>` on the host)"
        );
    }

    spawn_background_workers(&state, &bus);

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

        tracing::info!(addr = %opts.addr, "agentum-server listening (https)");
        let result = axum_server::bind_rustls(opts.addr, tls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await;
        cert_handle.abort();
        result.map_err(Into::into)
    } else {
        tracing::warn!(addr = %opts.addr, "agentum-server listening (plain http; --no-tls)");
        let listener = TcpListener::bind(opts.addr).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .map_err(Into::into)
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

    // Boot revival, then the watchdog — strictly in that order, on one task.
    // An OS reboot kills the local tmux server while the store still says
    // `running`; the sweep respawns those panes (Claude resumes its
    // conversation via the transcript-aware adapter). It must finish before
    // the watchdog's first reconcile, which samples every running session's
    // pane and would mark the not-yet-revived ones crashed (issue #267).
    {
        let state = state.clone();
        let bus = bus.clone();
        tokio::spawn(async move {
            routes::sessions::boot_revive_dead_sessions(&state).await;
            agentum_watchdog::Watchdog::new(bus, state.store.clone())
                .run()
                .await;
        });
    }

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
    // every connected client over the events WS; idles while no client is on.
    routes::host::spawn_ticker(bus.clone(), state.events_ws_clients.clone());

    // One-shot cache hygiene: drop pane logs whose session no longer exists
    // (pipe-pane appends every session's raw output forever; deleted sessions
    // used to leave their logs behind for the life of the install), then
    // prune aged event history (the connect-time snapshot queries scan this
    // table; unbounded growth made them slower every month). 30 days keeps
    // far more than the watchdog feed's ~50-row window ever shows, and each
    // session's newest agent.* row survives regardless of age.
    {
        let store = state.store.clone();
        tokio::spawn(async move {
            // Disjoint resources (filesystem cache vs sqlite) — run them
            // concurrently so the DB prune isn't gated on the log sweep.
            let prune = async {
                match store.prune_events(30).await {
                    Ok(pruned) if pruned > 0 => {
                        tracing::info!(pruned, "pruned aged event history")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = ?e, "event history prune failed"),
                }
            };
            tokio::join!(pane_log_reaper::reap_orphan_pane_logs(store.clone()), prune);
        });
    }

    // One-shot pane-pipe repair: remove duplicate external bindings that
    // hijacked a managed session's pane and re-arm local pipes disarmed by
    // the pre-#244 bug, so poisoned installs heal on first boot of the fix.
    {
        let store = state.store.clone();
        tokio::spawn(async move {
            pane_repair::repair_pane_bindings(store).await;
        });
    }

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
                    tokio::spawn(async move {
                        let _ = crate::host_runtime::warm_ssh_master(&host).await;
                    });
                }
            }
        });
    }

    // R3: one-shot boot drift rescan. Re-syncs any running session whose
    // provisioned endpoint drifted from the live one (the ephemeral-rebind case
    // R1+R2 can't cover). Spawned here — after `mcp_token`/`api_base_url` are
    // final — so it reads the authoritative live endpoint. Best-effort; a
    // standalone daemon (no `api_base_url`) returns immediately.
    {
        let state = state.clone();
        tokio::spawn(async move {
            routes::sessions::boot_drift_rescan(state).await;
        });
    }
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
    // Prefer the persisted/stable port so a restart doesn't invalidate live
    // sessions' baked-in MCP config (R2); reuse the persisted /mcp token (R1).
    let listener = endpoint::bind_stable_loopback().await?;
    let addr = listener.local_addr()?;
    let (mut state, bus) = embedded_app_state(store, addr);
    state.mcp_token = Arc::new(endpoint::load_or_create_mcp_token());
    state.desktop_bridge = Some(bridge);
    spawn_background_workers(&state, &bus);
    let app = router(state);
    tracing::info!(%addr, "agentum-server listening (embedded loopback, desktop bridge)");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            tracing::error!("embedded agentum-server exited: {e}");
        }
    });
    Ok(addr)
}

/// As [`serve_embedded_loopback`], but also returns the `AppState` the router was
/// built from. The desktop only needs the address; this exists so the boot path
/// has a single source of truth for the embedded state.
pub async fn serve_embedded_loopback_state(store: Store) -> anyhow::Result<(SocketAddr, AppState)> {
    // rustls provider selection is process-global; harmless if already set.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Bind first so we know the port BEFORE building the state — it must carry
    // its own URL. Prefer the persisted/stable port so a restart doesn't
    // invalidate live sessions' baked-in MCP config (R2); reuse the token (R1).
    let listener = endpoint::bind_stable_loopback().await?;
    let addr = listener.local_addr()?;

    let (mut state, bus) = embedded_app_state(store, addr);
    state.mcp_token = Arc::new(endpoint::load_or_create_mcp_token());

    spawn_background_workers(&state, &bus);

    let app = router(state.clone());
    tracing::info!(%addr, "agentum-server listening (embedded loopback, no-auth)");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        {
            tracing::error!("embedded agentum-server exited: {e}");
        }
    });
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
    }
}
