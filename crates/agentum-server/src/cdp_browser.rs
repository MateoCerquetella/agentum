//! Local CDP-Chromium browser: launch + lifecycle (009c-1, headless since 009c-3).
//!
//! agentum launches a **headless** Chromium-class browser with a known
//! `--remote-debugging-port`, so a Playwright MCP bound over `--cdp-endpoint`
//! (see [`crate::playwright_mcp::ensure_playwright_mcp_bound`]) drives the
//! **same** browser agentum renders. Originally (009c-1) this browser was headed
//! — a separate OS window the user watched — but user feedback rejected the
//! out-of-app window: 009c-3 makes it **headless** and renders it live *inside*
//! agentum's pane over a CDP screencast ([`crate::cdp_screencast`]). One window
//! the user sees, the same instance the agent drives — no OS window at all. 009c-2
//! reuses the wiring on an SSH host (sourcing the CDP endpoint from a tunnel).
//!
//! Shape deliberately mirrors [`crate::playwright_mcp`]: ONE shared instance per
//! machine, kept in its own long-lived tmux session (`agentum-cdp-browser`), and
//! the ensure step is **idempotent** on the listening CDP port + that session
//! name — so the N-th session reuses the same browser rather than spawning N.
//! (Per-scope multiplexing is a 009c-2 / future concern; the local minimal slice
//! is a singleton, exactly like the Playwright MCP server.)
//!
//! The engine is the **Playwright-managed Chromium** (`npx playwright install
//! chromium` populates it) — zero new dependency beyond what `@playwright/mcp`
//! already needs. We resolve its executable from the ms-playwright cache rather
//! than guessing a system Chrome. Fails **loud** (descriptive error) when the
//! browser isn't installed or never opens its CDP port — never a silent hang.

use crate::port_wait::{port_listening, wait_until_listening};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;

/// tmux session name for the shared local CDP browser. One per machine: the
/// ensure step is idempotent on this name plus the listening port.
const CDP_TMUX_TARGET: &str = "agentum-cdp-browser";

/// Default loopback port the browser exposes CDP on. A dedicated range distinct
/// from the Playwright MCP (`:8931`) and 009a's host-CDP range (`9200+`).
/// Overridable via `AGENTUM_CDP_BROWSER_PORT`.
const DEFAULT_CDP_PORT: u16 = 9300;

/// Resolve the CDP port (env override → default).
fn cdp_port() -> u16 {
    std::env::var("AGENTUM_CDP_BROWSER_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_CDP_PORT)
}

/// Device-scale the headless browser captures the screencast at. **1 (fast) by
/// default.** `Page.startScreencast` fixes its surface scale at browser LAUNCH, so
/// this is a `--force-device-scale-factor` flag. 2× quadruples the pixels in EVERY
/// JPEG frame streamed over the WebSocket — sharp on Retina but heavy and laggy,
/// which is the "browser is super slow / unusable" users hit. 1× is ~4× less data
/// per frame; it upscales on a Retina pane (slightly soft) but is the speed-over-
/// sharpness trade we default to. Tunable via `AGENTUM_CDP_DEVICE_SCALE` (clamped
/// to [1, 4]) — set `2` for a sharp (slower) capture on a Retina display.
fn cdp_device_scale() -> f64 {
    std::env::var("AGENTUM_CDP_DEVICE_SCALE")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0)
        .clamp(1.0, 4.0)
}

/// How long to wait for a freshly-launched Chromium to bind its CDP debug port.
/// Default 45s (was a hard-coded 20s). A cold profile plus GPU/Metal init — and
/// several per-worktree Chrome instances booting at once on a loaded machine —
/// routinely needs more than 20s to bind the port, which surfaced as a spurious
/// "did not expose CDP within 20s" failure while the browser was still coming up
/// (and would have worked on a retry). Overridable via
/// `AGENTUM_CDP_READY_TIMEOUT_SECS` for especially slow/contended machines.
fn cdp_ready_timeout() -> Duration {
    std::env::var("AGENTUM_CDP_READY_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(45))
}

/// Grace window for a leftover (not-yet-listening) tmux session before we treat it
/// as dead and kill+relaunch. Bumped from 2s so a slow-booting Chromium left by a
/// prior open isn't repeatedly killed and restarted (thrash) under load.
fn cdp_leftover_grace() -> Duration {
    Duration::from_secs(10)
}

/// The CDP base URL a Playwright MCP attaches to via `--cdp-endpoint`. Pinned to
/// IPv4 `127.0.0.1` for the same reason as the MCP host (macOS resolves
/// `localhost` to `::1`, which would miss our IPv4 launch + probe).
pub fn cdp_endpoint_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The configured CDP port for the shared local browser (env override → default).
pub fn port() -> u16 {
    cdp_port()
}

/// CDP port of the in-app browser the user is currently watching — the most recent
/// screencast pane attach (set by `routes::cdp_screencast`). A contextless MCP
/// `agentum_browser` op (no `worktreeId`/`cdpPort`, e.g. a top-level agent not
/// spawned into a worktree) drives THIS port, so "the agent drives the browser you
/// see" holds even without worktree context. `0` = no pane has attached yet.
/// Process-global because there is one desktop app (one foreground browser); this
/// keeps it out of `AppState` and every test constructor.
fn foreground_port_cell() -> &'static std::sync::atomic::AtomicU16 {
    static P: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    &P
}

/// Record the browser the user is now watching (called on each screencast attach).
/// Last attach wins — the foreground pane.
pub fn set_foreground_cdp_port(port: u16) {
    foreground_port_cell().store(port, std::sync::atomic::Ordering::Relaxed);
}

/// The foreground browser's CDP port, or `None` if no screencast pane has attached.
pub fn foreground_cdp_port() -> Option<u16> {
    match foreground_port_cell().load(std::sync::atomic::Ordering::Relaxed) {
        0 => None,
        p => Some(p),
    }
}

/// Is the shared local CDP browser currently up (serving on its port)? Cheap
/// probe; does NOT launch it. For a status surface that must not start a browser.
pub async fn is_running() -> bool {
    port_listening(cdp_port()).await
}

/// Ensure the shared local CDP-Chromium browser is running and exposing CDP,
/// launching it headed in its dedicated tmux session if not. Idempotent: a port
/// already serving (or a still-booting session we started earlier) is reused.
/// Returns the CDP endpoint URL to bind a Playwright MCP at.
///
/// Fails loud if the Playwright Chromium isn't installed or never opens its CDP
/// port — launching the bound MCP against a dead endpoint would only surface
/// later as a confusing tool-call error inside the agent session.
pub async fn ensure_local_cdp_browser() -> Result<String> {
    let port = cdp_port();
    let endpoint = cdp_endpoint_for(port);

    // Fast path (no lock): CDP already serving the port → reuse it. One browser
    // per machine, shared across sessions and surviving agent restarts.
    if port_listening(port).await {
        return Ok(endpoint);
    }

    // Serialize the launch: concurrent `provision()` calls (e.g. the harness
    // starting several sessions at once) would otherwise both pass the checks
    // below and race on `new_session`, and the loser would get a "duplicate
    // session" error and wrongly degrade to headless even though the browser is
    // healthy. Double-checked: re-probe under the lock in case a peer just
    // launched it while we waited.
    let _guard = launch_lock().lock().await;
    if port_listening(port).await {
        return Ok(endpoint);
    }

    // Need to start it — resolve the browser binary first so a missing install
    // fails loud *now* with an actionable message instead of spawning a tmux
    // pane that dies opaquely.
    let exe = chromium_executable()?;

    // A leftover session not (yet) listening is either still booting or dead.
    // Give a slow boot a brief grace window; otherwise reset the singleton.
    if agentum_tmux::has_session(CDP_TMUX_TARGET)
        .await
        .unwrap_or(false)
    {
        if wait_until_listening(port, cdp_leftover_grace()).await {
            return Ok(endpoint);
        }
        let _ = agentum_tmux::kill_session(CDP_TMUX_TARGET).await;
    }

    let user_data_dir = user_data_dir()?;
    let argv = build_chrome_argv(&exe, port, &user_data_dir);
    // Chromium needs no project context — run from $HOME so tmux has a valid cwd.
    let workdir = home_dir();
    agentum_tmux::new_session(CDP_TMUX_TARGET, &workdir, &argv, &[])
        .await
        .context("start the shared local CDP-Chromium tmux session")?;

    // Headless Chromium boots then binds the debugging port; allow boot time
    // (a cold profile + GPU init on a loaded machine can take tens of seconds).
    let ready = cdp_ready_timeout();
    if wait_until_listening(port, ready).await {
        Ok(endpoint)
    } else {
        anyhow::bail!(
            "Chromium launched but did not expose CDP on 127.0.0.1:{port} within {}s \
             (tmux session `{CDP_TMUX_TARGET}`). Check the pane; the browser may have \
             failed to start. Set AGENTUM_CDP_READY_TIMEOUT_SECS to allow more time.",
            ready.as_secs()
        )
    }
}

/// Tear the shared local CDP browser down: kill its tmux session and remove its
/// isolated profile. Idempotent. (Wired into session/browser teardown later;
/// exposed now so the lifecycle has a single owner.)
pub async fn stop_local_cdp_browser() -> Result<()> {
    agentum_tmux::kill_session(CDP_TMUX_TARGET)
        .await
        .context("kill the local CDP-Chromium tmux session")?;
    // The bound Playwright MCP holds an open CDP WebSocket to *this* browser
    // process; if we leave it running, the next launch (new browser, same port)
    // would be served by an MCP still pointing at the dead process. Reset it so
    // the next `ensure_playwright_mcp_bound` reconnects cleanly. Best-effort.
    let _ = crate::playwright_mcp::stop_bound_mcp().await;
    if let Ok(dir) = user_data_dir() {
        // Best-effort: a stale profile shouldn't block teardown.
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(())
}

/// Process-wide lock serializing browser launches (see `ensure_local_cdp_browser`).
fn launch_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// --- per-project browsers (isolation + persistence, spec 014) -----------------
//
// Each PROJECT (registry `Repo.id`) gets its OWN Chromium (own port + tmux
// session + profile) so opening a browser in project B never shows project A's
// tabs or cookies, while every workspace/worktree of ONE project shares a single
// PERSISTENT profile — logins survive tab close, worktree teardown, and app
// relaunch. Both the user's screencast pane AND the project's agents resolve the
// SAME browser via [`resolve_browser_scope`], so they still watch/drive one
// instance. Pseudo-worktrees with no repo (`github-pr:repo:42`, unresolvable
// paths) keep the old per-key ephemeral behavior (`BrowserScope::Adhoc`); an
// empty context falls back to the shared project-less browser.

/// Which browser a raw caller-supplied context resolves to (spec 014 D1/D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserScope {
    /// Truly project-less (empty context) → the shared root browser.
    Shared,
    /// A registered project, keyed by the registry `Repo.id` UUID.
    Project { repo_id: String },
    /// A pseudo-worktree with no repo root (e.g. `github-pr:repo:42`) or an
    /// unresolvable bare path — keeps per-key isolated, ephemeral behavior.
    Adhoc { key: String },
}

impl BrowserScope {
    /// Filesystem/tmux-safe token; also the registry + attach-count key. `None`
    /// for `Shared`. The `project-` prefix is applied AFTER sanitization so the
    /// 48-char tail bound in `sanitize_worktree_token` can never truncate it
    /// away — a UUID passes sanitization unchanged, so the profile dir is
    /// literally `project-<uuid>`. The prefix is what lets the boot sweep tell
    /// persistent project dirs from legacy/adhoc ones.
    fn profile_token(&self) -> Option<String> {
        match self {
            BrowserScope::Shared => None,
            BrowserScope::Project { repo_id } => {
                Some(format!("project-{}", sanitize_worktree_token(repo_id)))
            }
            BrowserScope::Adhoc { key } => Some(sanitize_worktree_token(key)),
        }
    }
}

/// Resolve a raw browser context (pane `<repoId>::<path>` id, bare agent path,
/// bare repo id, pseudo-key, or empty) to its scope — the pure, table-only core
/// of the chain. `None` means "an absolute path the tables don't know", which
/// the async wrapper follows up with a git probe. Tables: `worktrees` =
/// `(repo_id, full registry id)` rows; `repos` = `(id, path)`.
fn resolve_scope_from_tables(
    raw: &str,
    worktrees: &[(String, String)],
    repos: &[(String, String)],
) -> Option<BrowserScope> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(BrowserScope::Shared);
    }
    // Pane id `<repoId>::<path>` (folder projects append `::workspace:<uuid>`):
    // the prefix IS the identity (D2) — trusted without a registry read, so a
    // stale-but-real repoId still reaches its profile.
    if let Some((repo_id, _)) = raw.split_once("::") {
        return Some(BrowserScope::Project {
            repo_id: repo_id.to_string(),
        });
    }
    // A bare UUID: project-scoped surfaces with a repo but no worktree path
    // (plain workspace / project hub) send the repo id directly. A filesystem
    // path can never collide with a UUID. Unregistered UUIDs stay isolated.
    if uuid::Uuid::parse_str(raw).is_ok() {
        if repos.iter().any(|(id, _)| id == raw) {
            return Some(BrowserScope::Project {
                repo_id: raw.to_string(),
            });
        }
        return Some(BrowserScope::Adhoc {
            key: raw.to_string(),
        });
    }
    // A bare absolute path (the agent/MCP side): a registered worktree first,
    // then a session running in a repo's main checkout.
    if raw.starts_with('/') {
        if let Some((repo_id, _)) = worktrees
            .iter()
            .find(|(_, id)| canonical_worktree_key(id) == raw)
        {
            return Some(BrowserScope::Project {
                repo_id: repo_id.clone(),
            });
        }
        if let Some((id, _)) = repos.iter().find(|(_, path)| path == raw) {
            return Some(BrowserScope::Project {
                repo_id: id.clone(),
            });
        }
        return None; // unknown path → git probe, then Adhoc
    }
    // Anything else (github-pr pseudo-keys, …) keeps its own isolated browser.
    Some(BrowserScope::Adhoc {
        key: raw.to_string(),
    })
}

/// Git fallback for an absolute path the tables don't know: a worktree created
/// outside agentum still maps via its main repo root to a registered project.
async fn resolve_path_via_git(path: &str, repos: &[(String, String)]) -> Option<BrowserScope> {
    let out = tokio::process::Command::new("git")
        .args(["-C", path, "rev-parse", "--git-common-dir"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if common.is_empty() {
        return None;
    }
    let git_dir = if Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        PathBuf::from(path).join(common)
    };
    let root = git_dir.parent()?.to_path_buf();
    // Compare canonicalized on both sides — git reports realpaths while the
    // registry stores what the user typed (macOS `/var` vs `/private/var`).
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let hit = repos.iter().find(|(_, p)| {
        Path::new(p) == root || std::fs::canonicalize(p).map(|c| c == root).unwrap_or(false)
    })?;
    Some(BrowserScope::Project {
        repo_id: hit.0.clone(),
    })
}

/// Full scope resolution against injected tables (pure chain + git fallback).
/// An unresolvable context stays ISOLATED (`Adhoc`), never the shared profile —
/// dumping strangers into one cookie jar is the exact leak spec 014 kills.
async fn resolve_scope_with(
    raw: &str,
    worktrees: &[(String, String)],
    repos: &[(String, String)],
) -> BrowserScope {
    match resolve_scope_from_tables(raw, worktrees, repos) {
        Some(scope) => scope,
        None => match resolve_path_via_git(raw.trim(), repos).await {
            Some(scope) => scope,
            None => BrowserScope::Adhoc {
                key: raw.trim().to_string(),
            },
        },
    }
}

/// Resolve a raw browser context against the live worktree/repo registries.
pub(crate) async fn resolve_browser_scope(raw: &str) -> BrowserScope {
    let worktrees = crate::routes::worktrees::scope_worktree_pairs();
    let repos = crate::routes::repos::scope_repo_pairs();
    resolve_scope_with(raw, &worktrees, &repos).await
}

/// A launched scoped browser. The port is allocated once (via the OS) and
/// reused for that scope's lifetime; tmux + profile are derived from its token.
struct ScopedBrowser {
    port: u16,
    tmux: String,
    // Kept so a registry entry fully describes its Chromium (port + tmux +
    // profile). Teardown re-derives the profile path from the token; this
    // field keeps the entry self-contained for a future direct-path teardown.
    #[allow(dead_code)]
    profile: PathBuf,
}

/// `profile token → ScopedBrowser`. Keyed by the TOKEN (not the raw context) so
/// two contexts that sanitize to one profile dir can never race two Chromiums
/// onto it — Chromium allows exactly one process per `--user-data-dir`. A `std`
/// mutex (never held across `.await`): every access reads/writes a field and
/// drops the guard before any I/O.
fn browser_registry() -> &'static std::sync::Mutex<HashMap<String, ScopedBrowser>> {
    static REG: OnceLock<std::sync::Mutex<HashMap<String, ScopedBrowser>>> = OnceLock::new();
    REG.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// `token → live screencast attach count` (spec 014 AC 2). Ground truth for
/// "some pane is watching this project's browser": incremented per screencast
/// WS, decremented by the guard's Drop when the WS task ends (close/drop/panic).
fn attach_counts() -> &'static std::sync::Mutex<HashMap<String, usize>> {
    static C: OnceLock<std::sync::Mutex<HashMap<String, usize>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// RAII attach marker for one screencast connection. MUST be moved into the WS
/// task's future so the Drop decrement fires exactly when the stream ends —
/// dropped at handler scope it would zero the count while panes still stream,
/// resurrecting the "worktree A's close kills worktree B's browser" bug.
pub(crate) struct BrowserAttachGuard(Option<String>);

impl BrowserAttachGuard {
    /// A guard that counts nothing (shared / explicit-port / adhoc attaches).
    pub(crate) fn inert() -> Self {
        BrowserAttachGuard(None)
    }
}

impl Drop for BrowserAttachGuard {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            if let Ok(mut counts) = attach_counts().lock() {
                match counts.get_mut(&token) {
                    Some(c) if *c > 1 => *c -= 1,
                    Some(_) => {
                        counts.remove(&token);
                    }
                    None => {}
                }
            }
        }
    }
}

/// Register a live screencast attach for `raw`'s browser. Counts only Project
/// scopes: Adhoc keeps kill-always teardown and Shared's stop is explicit-only,
/// so neither needs a refcount — those get an inert guard.
pub(crate) async fn register_browser_attach(raw: &str) -> BrowserAttachGuard {
    let scope = resolve_browser_scope(raw).await;
    match &scope {
        BrowserScope::Project { .. } => {
            let token = scope.profile_token().expect("project scope has a token");
            if let Ok(mut counts) = attach_counts().lock() {
                *counts.entry(token.clone()).or_insert(0) += 1;
            }
            BrowserAttachGuard(Some(token))
        }
        _ => BrowserAttachGuard::inert(),
    }
}

/// Live screencast attaches for a token (0 when untracked).
fn project_attach_count(token: &str) -> usize {
    attach_counts()
        .lock()
        .ok()
        .map(|c| c.get(token).copied().unwrap_or(0))
        .unwrap_or(0)
}

/// Reduce a full worktree id to its bare filesystem path. The desktop's UI ids
/// are `<repoId>::<path>` (folder projects append a `::workspace:<uuid>`
/// instance suffix); agents send the bare `worktree_path`. Scope resolution
/// (`resolve_scope_from_tables`) uses this to match an agent's bare path
/// against registry rows, so BOTH sides land on the same project browser —
/// otherwise the agent would drive a different Chromium than the user watches.
/// Mirrors the desktop's `splitWorktreeIdForFilesystem`: drop the
/// `<repoId>::` prefix, then any `::workspace:<uuid>` suffix.
///
/// Assumes a worktree path contains no `::` (true for agentum-managed worktrees,
/// which live under a sanitized root) — the same assumption the desktop makes by
/// splitting the worktree id on its first `::`.
fn canonical_worktree_key(raw: &str) -> &str {
    // `<repoId>::<path>` → `<path>` (split on the FIRST `::`, as the desktop does).
    let path = raw.split_once("::").map_or(raw, |(_, rest)| rest);
    // Folder-project instance suffix `<path>::workspace:<uuid>` → `<path>`.
    path.split_once("::workspace:")
        .map_or(path, |(head, _)| head)
}

/// Make a worktree id safe for a tmux session name and a directory component.
fn sanitize_worktree_token(worktree_id: &str) -> String {
    let token: String = worktree_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Bound the length so the tmux name stays sane; keep the tail (often the
    // distinctive part of a worktree path/id).
    let trimmed = token.trim_matches('-');
    let tail = if trimmed.len() > 48 {
        &trimmed[trimmed.len() - 48..]
    } else {
        trimmed
    };
    if tail.is_empty() {
        "wt".to_string()
    } else {
        tail.to_string()
    }
}

/// Profile dir for a scoped browser: `…/cdp-browser/<token>` (sibling of
/// `shared`). `project-*` tokens persist across teardown/relaunch (spec 014);
/// everything else is ephemeral.
fn profile_dir_for_token(token: &str) -> Result<PathBuf> {
    let dir = agentum_store::paths::state_dir()
        .context("resolve agentum state dir")?
        .join("cdp-browser")
        .join(token);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create scoped CDP profile dir {}", dir.display()))?;
    Ok(dir)
}

/// The registered port for a scope token, if its browser is still serving.
async fn registered_listening_port(token: &str) -> Option<u16> {
    let port = browser_registry().lock().ok()?.get(token).map(|b| b.port)?;
    port_listening(port).await.then_some(port)
}

/// Ensure the CDP browser for this context and return `(endpoint, port)`.
/// Contexts resolving to a project share ONE persistent browser per project
/// (spec 014 D1) — the pane's `<repoId>::<path>` id, an agent's bare worktree
/// path, and a bare repo id all land on the same instance; pseudo/unknown keys
/// get their own ephemeral browser; an empty context falls back to the shared
/// default so contextless callers don't regress. Idempotent per scope (reuses
/// the registered, still-serving port).
pub async fn ensure_local_cdp_browser_for(worktree_id: &str) -> Result<(String, u16)> {
    let raw = worktree_id.trim();
    // Keyed isolation is ON by default; `AGENTUM_BROWSER_PER_WORKTREE=0` opts
    // out (every context shares one browser — the pre-v0.27 behavior; the
    // historical name predates project keying). Checked before resolution so
    // the opt-out never reads the registries.
    let enabled = std::env::var("AGENTUM_BROWSER_PER_WORKTREE")
        .map(|v| v.trim() != "0")
        .unwrap_or(true);
    let scope = if enabled && !raw.is_empty() {
        resolve_browser_scope(raw).await
    } else {
        BrowserScope::Shared
    };
    let Some(token) = scope.profile_token() else {
        let endpoint = ensure_local_cdp_browser().await?;
        return Ok((endpoint, cdp_port()));
    };

    if let Some(port) = registered_listening_port(&token).await {
        return Ok((cdp_endpoint_for(port), port));
    }

    let _guard = launch_lock().lock().await;
    if let Some(port) = registered_listening_port(&token).await {
        return Ok((cdp_endpoint_for(port), port));
    }

    let exe = chromium_executable()?;
    let tmux = format!("{CDP_TMUX_TARGET}-{token}");
    let profile = profile_dir_for_token(&token)?;
    // Reuse this scope's previously-allocated port (re-launch on the same port
    // after a crash) or take a fresh one from the OS.
    let port = browser_registry()
        .lock()
        .ok()
        .and_then(|reg| reg.get(&token).map(|b| b.port))
        .map_or_else(free_local_port, Ok)?;

    // A leftover-but-not-listening session is either booting or dead.
    if agentum_tmux::has_session(&tmux).await.unwrap_or(false) {
        if wait_until_listening(port, cdp_leftover_grace()).await {
            register_scoped_browser(&token, port, &tmux, &profile);
            return Ok((cdp_endpoint_for(port), port));
        }
        let _ = agentum_tmux::kill_session(&tmux).await;
    }

    let argv = build_chrome_argv(&exe, port, &profile);
    agentum_tmux::new_session(&tmux, &home_dir(), &argv, &[])
        .await
        .with_context(|| format!("start CDP-Chromium for browser context `{raw}`"))?;

    let ready = cdp_ready_timeout();
    if wait_until_listening(port, ready).await {
        register_scoped_browser(&token, port, &tmux, &profile);
        Ok((cdp_endpoint_for(port), port))
    } else {
        anyhow::bail!(
            "Chromium for `{raw}` did not expose CDP on 127.0.0.1:{port} within {}s \
             (tmux `{tmux}`). Set AGENTUM_CDP_READY_TIMEOUT_SECS to allow more time.",
            ready.as_secs()
        )
    }
}

fn register_scoped_browser(token: &str, port: u16, tmux: &str, profile: &Path) {
    if let Ok(mut reg) = browser_registry().lock() {
        reg.insert(
            token.to_string(),
            ScopedBrowser {
                port,
                tmux: tmux.to_string(),
                profile: profile.to_path_buf(),
            },
        );
    }
}

/// Kill every process whose command line references `needle` — how we reap the
/// Chromium launched for a CDP browser. Its `--user-data-dir` sits under our
/// `cdp-browser` profile dir (an agentum-only absolute path), so this never
/// matches the user's own Chrome. We match on the PROCESS, not the tmux session,
/// because a killed session can leave the browser orphaned (the source of the
/// leftover-Chrome pile-up). Best-effort; `pkill` is absent on Windows (where the
/// tmux-hosted browser doesn't run anyway), so a spawn failure is ignored.
async fn pkill_by_signature(needle: &str) {
    let needle = needle.trim();
    if needle.is_empty() {
        return;
    }
    // `-f` matches the full argv. The signature is an absolute path, so it can't
    // be mistaken for a pkill option (and `Command::arg` passes it verbatim, so a
    // space in it — e.g. macOS's "Application Support" — is fine).
    let _ = tokio::process::Command::new("pkill")
        .arg("-f")
        .arg(needle)
        .output()
        .await;
}

/// Reap every Chromium agentum launched for CDP — the shared browser and every
/// per-worktree one — by its `--user-data-dir` under our `cdp-browser` profile
/// dir. These live in detached tmux sessions and can outlive both the app and
/// their session, so without an explicit reap they accumulate ("dozens of
/// leftover Chrome processes"). Also clears the in-memory registry so a stale
/// entry can't hand back a dead port. Best-effort; safe to call on startup (a
/// fresh launch has no live agentum browser, so only orphans die) and on quit.
pub async fn reap_orphaned_cdp_browsers() {
    if let Ok(state) = agentum_store::paths::state_dir() {
        pkill_by_signature(&state.join("cdp-browser").to_string_lossy()).await;
    }
    if let Ok(mut reg) = browser_registry().lock() {
        reg.clear();
    }
}

/// Boot sweep (spec 014 D4): delete every TOP-LEVEL entry under `cdp-browser/`
/// that is neither the `shared` profile nor a persistent `project-*` dir —
/// legacy per-worktree profiles (deleted-on-close under the old contract, so
/// nothing durable ever lived there), pre-relocation shared-profile internals
/// (`Default/`, `Local State`, …), and leftover adhoc dirs. Idempotent — runs
/// every boot; a second pass finds nothing legacy. MUST run strictly AFTER
/// [`reap_orphaned_cdp_browsers`] so no profile is ripped out from under a live
/// process. Never recurses into (or deletes) `project-*` or `shared`, and never
/// touches anything outside `cdp-browser/`.
pub async fn sweep_legacy_profile_dirs() {
    let Ok(state) = agentum_store::paths::state_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(state.join("cdp-browser")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "shared" || name.starts_with("project-") {
            continue;
        }
        let path = entry.path();
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// Release one context's claim on its browser. Scope-aware (spec 014 AC 2): a
/// PROJECT browser is stopped (tmux + process killed) only when no screencast
/// pane anywhere is still attached to it, and its profile dir is NEVER deleted
/// here — the explicit clear action is the only deleter (D3), so logins persist
/// across tab close / worktree removal / relaunch. An ADHOC (pseudo-key or
/// unresolvable) browser keeps the old contract: kill + delete. Shared is a
/// no-op (it has its own explicit stop route). Idempotent; safe for an unknown
/// context. Called on worktree removal, prune, and when the user closes the
/// last browser tab in a workspace.
pub async fn stop_local_cdp_browser_for(worktree_id: &str) -> Result<()> {
    let scope = resolve_browser_scope(worktree_id.trim()).await;
    let Some(token) = scope.profile_token() else {
        return Ok(()); // Shared: never stopped from a per-workspace signal
    };
    let is_project = matches!(scope, BrowserScope::Project { .. });
    // Another workspace of this project still has a live pane → releasing this
    // one must not kill the browser it is mid-screencasting (AC 2).
    if is_project && project_attach_count(&token) > 0 {
        return Ok(());
    }
    let entry = browser_registry()
        .lock()
        .ok()
        .and_then(|mut reg| reg.remove(&token));
    if let Some(b) = &entry {
        let _ = agentum_tmux::kill_session(&b.tmux).await;
    }
    // Kill the Chromium by its profile dir even without a live registry entry (it
    // may have been launched in a previous run) — the browser can outlive its
    // tmux session, so killing the session alone leaks it.
    if let Ok(state) = agentum_store::paths::state_dir() {
        let profile = state.join("cdp-browser").join(&token);
        pkill_by_signature(&profile.to_string_lossy()).await;
        // Project profiles PERSIST (the point of spec 014); adhoc dirs keep
        // their ephemeral delete-on-stop contract.
        if !is_project {
            let _ = std::fs::remove_dir_all(&profile);
        }
    }
    Ok(())
}

/// Explicit project-scoped clear (spec 014 AC 5) — the ONLY deleter of a
/// project profile (D3). Force-stops the project's browser — attach counts
/// deliberately IGNORED: a clear is explicit user intent, and a live pane's WS
/// dies and reconnects onto a fresh, empty profile — then deletes ONLY that
/// project's dir. Errors propagate to the caller; silent success is the stub
/// failure mode this action replaces.
pub async fn clear_project_browser_data(repo_id: &str) -> Result<()> {
    let repo_id = repo_id.trim();
    if repo_id.is_empty() {
        anyhow::bail!("a repo id is required to clear a project's browser data");
    }
    let scope = BrowserScope::Project {
        repo_id: repo_id.to_string(),
    };
    let token = scope.profile_token().expect("project scope has a token");
    let entry = browser_registry()
        .lock()
        .ok()
        .and_then(|mut reg| reg.remove(&token));
    if let Some(b) = &entry {
        let _ = agentum_tmux::kill_session(&b.tmux).await;
    }
    let state = agentum_store::paths::state_dir().context("resolve agentum state dir")?;
    let profile = state.join("cdp-browser").join(&token);
    pkill_by_signature(&profile.to_string_lossy()).await;
    if profile.exists() {
        std::fs::remove_dir_all(&profile)
            .with_context(|| format!("delete project browser profile {}", profile.display()))?;
    }
    Ok(())
}

// --- remote (SSH host) browser — F11 / criterion #5 --------------------------
//
// Headless Chromium runs ON the SSH host; the Mac reaches its loopback CDP port
// through an `ssh -L` forward (`ssh_control_local_forward_cmd`) and drives it via
// the existing `cdpPort` seam — so the agent contract is byte-identical to local
// (spec §7). Composes the proven SSH primitives; the SSH round-trip needs a real
// host with a Chromium binary, so it's construction-unit-tested here and the
// drive-half is covered by the explicit-port path the local live test exercises.

/// CDP port headless Chromium binds on the REMOTE host (loopback only — the
/// `ssh -L` tunnel is the only way in). Distinct from the local browser's :9300.
const REMOTE_CDP_PORT: u16 = 9222;

/// Remote shell that launches headless Chromium in a detached tmux on the host
/// (idempotent on the session name), resolving whichever Chromium binary exists.
/// Mirrors [`build_chrome_argv`]'s flags. Pure → unit-tested without an SSH host.
/// `$B`/`$HOME` expand in the host's shell before tmux sees the command.
fn remote_chrome_launch_script(host_port: u16) -> String {
    let flags = format!(
        "--headless=new --hide-scrollbars --window-size=1280,800 \
         --force-device-scale-factor=2 \
         --remote-debugging-address=127.0.0.1 --remote-debugging-port={host_port} \
         --user-data-dir=$HOME/.agentum/cdp-browser --no-first-run \
         --no-default-browser-check about:blank"
    );
    format!(
        "mkdir -p \"$HOME/.agentum/cdp-browser\"; \
         B=$(command -v chromium || command -v chromium-browser || \
         command -v google-chrome || command -v google-chrome-stable || echo chromium); \
         tmux has-session -t {CDP_TMUX_TARGET} 2>/dev/null || \
         tmux new-session -d -s {CDP_TMUX_TARGET} \"$B {flags}\""
    )
}

/// A free loopback TCP port for the Mac end of the `ssh -L` tunnel.
fn free_local_port() -> Result<u16> {
    let l = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .context("bind a free local port for the SSH tunnel")?;
    Ok(l.local_addr()?.port())
}

/// Ensure headless Chromium is running on the SSH `host` and reachable locally via
/// an `ssh -L` forward; returns the LOCAL port to drive through the `cdpPort` seam.
pub async fn ensure_remote_cdp_browser(host: &agentum_core::Host) -> Result<u16> {
    use agentum_tmux::ssh::{
        ssh_control_local_cancel_cmd, ssh_control_local_forward_cmd, ssh_output,
    };
    let ssh_timeout = Duration::from_secs(20);

    // Launch Chromium on the host (idempotent). `ssh_output` rides the interactive
    // ControlMaster, warming it — which the `-L` forward below then attaches to.
    let out = ssh_output(
        host,
        &remote_chrome_launch_script(REMOTE_CDP_PORT),
        ssh_timeout,
    )
    .await
    .context("launch headless Chromium on the SSH host")?;
    if !out.status.success() {
        anyhow::bail!(
            "remote Chromium launch failed on host `{}`: {}",
            host.name,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    // Forward a fresh local loopback port → the host's loopback CDP port.
    let mac_port = free_local_port()?;
    if let Some(mut cancel) = ssh_control_local_cancel_cmd(host, mac_port, REMOTE_CDP_PORT) {
        let _ = cancel.output().await; // best-effort: clear a stale forward
    }
    let mut fwd =
        ssh_control_local_forward_cmd(host, mac_port, REMOTE_CDP_PORT).ok_or_else(|| {
            anyhow::anyhow!(
                "host `{}` is not an SSH host or has no warm ControlMaster",
                host.name
            )
        })?;
    let fwd_out = fwd.output().await.context("establish the ssh -L forward")?;
    if !fwd_out.status.success() {
        anyhow::bail!(
            "ssh -L forward failed for host `{}`: {}",
            host.name,
            String::from_utf8_lossy(&fwd_out.stderr).trim()
        );
    }

    // The tunneled local port should now reach the host's CDP within boot time.
    if wait_until_listening(mac_port, Duration::from_secs(20)).await {
        Ok(mac_port)
    } else {
        anyhow::bail!(
            "remote Chromium on `{}` not reachable via the ssh -L tunnel on \
             127.0.0.1:{mac_port} within 20s (is Chromium installed on the host?)",
            host.name
        )
    }
}

/// Build the Chromium argv for the headless (screencast) browser. Split out so the
/// flag shape is unit-testable without launching a browser.
///
/// `--remote-debugging-address=127.0.0.1` keeps CDP loopback-only (remote reaches it
/// solely via the authenticated SSH tunnel); `--no-first-run` / `--no-default-browser-check`
/// suppress nags; an isolated `--user-data-dir` keeps this browser off the user's real
/// profile. `--headless=new` is full modern headless (has `Page.startScreencast`, unlike
/// `chromium_headless_shell`); `--force-device-scale-factor` (see [`cdp_device_scale`])
/// MUST be a launch flag because `Page.startScreencast` fixes its surface scale at launch
/// and ignores the per-frame `setDeviceMetricsOverride.deviceScaleFactor` the pane sends.
fn build_chrome_argv(
    exe: &std::path::Path,
    port: u16,
    user_data_dir: &std::path::Path,
) -> Vec<String> {
    let mut argv = vec![exe.to_string_lossy().into_owned()];
    argv.push("--headless=new".to_string());
    argv.push("--window-size=1280,800".to_string());
    argv.push(format!(
        "--force-device-scale-factor={}",
        cdp_device_scale()
    ));
    argv.push("--hide-scrollbars".to_string());
    argv.push("--remote-debugging-address=127.0.0.1".to_string());
    argv.push(format!("--remote-debugging-port={port}"));
    argv.push(format!(
        "--user-data-dir={}",
        user_data_dir.to_string_lossy()
    ));
    argv.push("--no-first-run".to_string());
    argv.push("--no-default-browser-check".to_string());
    argv.push("about:blank".to_string());
    argv
}

/// Candidate locations for a system-installed full Chrome/Chromium, by OS. Pure
/// (no env, no filesystem) so it's unit-testable; `system_chrome_executable`
/// filters these to the ones that actually exist.
fn chrome_candidate_paths() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        candidates.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
        candidates.push("/Applications/Chromium.app/Contents/MacOS/Chromium".into());
    }
    #[cfg(target_os = "windows")]
    {
        candidates.push(r"C:\Program Files\Google\Chrome\Application\chrome.exe".into());
        candidates.push(r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe".into());
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local).join(r"Google\Chrome\Application\chrome.exe"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        for c in [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
        ] {
            candidates.push(c.into());
        }
    }
    candidates
}

/// Locate a system-installed **full** Chrome/Chromium (it supports
/// `Page.startScreencast`, unlike `chrome-headless-shell`). Preferred over the
/// Playwright cache so the common case needs no `npx playwright install`
/// download — most machines already have Chrome. Escape hatches:
/// `AGENTUM_CDP_CHROME_PATH` pins an exact binary; `AGENTUM_CDP_USE_PLAYWRIGHT=1`
/// skips system Chrome entirely (force the pinned Playwright build for version
/// determinism).
fn system_chrome_executable() -> Option<PathBuf> {
    if std::env::var_os("AGENTUM_CDP_USE_PLAYWRIGHT").is_some() {
        return None;
    }
    if let Some(p) = std::env::var_os("AGENTUM_CDP_CHROME_PATH") {
        let path = PathBuf::from(p);
        return path.is_file().then_some(path);
    }
    chrome_candidate_paths().into_iter().find(|p| p.is_file())
}

/// Resolve the Chrome/Chromium executable to drive over CDP. Prefers a
/// system-installed full Chrome (no download), then falls back to the
/// Playwright-managed Chromium from the ms-playwright cache (highest-revision
/// `chromium-<rev>`). Fails loud with an install hint when neither is found.
pub(crate) fn chromium_executable() -> Result<PathBuf> {
    if let Some(exe) = system_chrome_executable() {
        return Ok(exe);
    }
    let root = playwright_browsers_root();
    let entries = std::fs::read_dir(&root).with_context(|| {
        format!(
            "No browser for the agent browser: system Chrome wasn't found and the Playwright \
             browser cache is missing ({}). Install Google Chrome, or run \
             `npx playwright install chromium`.",
            root.display()
        )
    })?;

    // `chromium-<rev>` only — `chromium_headless_shell-<rev>` (note the `_`)
    // never matches `strip_prefix("chromium-")`, so we never pick the reduced
    // headless shell. We run the FULL Chromium in `--headless=new` mode instead,
    // because the shell lacks `Page.startScreencast` (no frames to render).
    let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rev) = parse_chromium_rev(&name) else {
            continue;
        };
        if let Some(exe) = find_chrome_exe_in(&entry.path()) {
            candidates.push((rev, exe));
        }
    }
    candidates.sort_by_key(|(rev, _)| *rev);
    candidates.pop().map(|(_, exe)| exe).ok_or_else(|| {
        anyhow::anyhow!(
            "No browser for the agent browser: system Chrome wasn't found and no `chromium-*` \
             build with an executable exists under {}. Install Google Chrome, or run \
             `npx playwright install chromium`.",
            root.display()
        )
    })
}

/// Parse the revision from a `chromium-<rev>` cache dir name, or `None` for
/// anything else (including the reduced `chromium_headless_shell-<rev>`, which
/// can't screencast).
fn parse_chromium_rev(dir_name: &str) -> Option<u64> {
    dir_name.strip_prefix("chromium-")?.parse::<u64>().ok()
}

/// Locate the full Chrome/Chromium executable inside a `chromium-<rev>` cache
/// dir. Discovered by **searching the layout** rather than hardcoding a path,
/// because Playwright varies it by version AND arch: the platform subdir is
/// `chrome-mac` / `chrome-mac-x64` / `chrome-mac-arm64` (or `chrome-linux*` /
/// `chrome-win*`), and the macOS app/executable was renamed from `Chromium` to
/// `Google Chrome for Testing` in recent builds. (A hardcoded path silently
/// failed on this machine — build 1228 ships `chrome-mac-x64/Google Chrome for
/// Testing.app/Contents/MacOS/Google Chrome for Testing`.)
fn find_chrome_exe_in(rev_dir: &std::path::Path) -> Option<PathBuf> {
    let subdirs = std::fs::read_dir(rev_dir).ok()?;
    for sub in subdirs.flatten() {
        let sub_name = sub.file_name().to_string_lossy().into_owned();
        let path = sub.path();
        if cfg!(target_os = "macos") && sub_name.starts_with("chrome-mac") {
            // Inside the platform dir, find the single `*.app`; the main
            // executable is `Contents/MacOS/<app name without .app>`.
            if let Some(exe) = mac_app_executable(&path) {
                return Some(exe);
            }
        } else if cfg!(target_os = "windows") && sub_name.starts_with("chrome-win") {
            let exe = path.join("chrome.exe");
            if exe.is_file() {
                return Some(exe);
            }
        } else if sub_name.starts_with("chrome-linux") {
            let exe = path.join("chrome");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// Given a macOS `chrome-mac*` dir, return the executable inside its `*.app`
/// bundle (`Contents/MacOS/<app stem>`). Handles both `Chromium.app/.../Chromium`
/// and `Google Chrome for Testing.app/.../Google Chrome for Testing`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_app_executable(platform_dir: &std::path::Path) -> Option<PathBuf> {
    for app in std::fs::read_dir(platform_dir).ok()?.flatten() {
        let app_name = app.file_name().to_string_lossy().into_owned();
        if let Some(stem) = app_name.strip_suffix(".app") {
            let exe = app.path().join("Contents/MacOS").join(stem);
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// The ms-playwright browser cache root (honors `PLAYWRIGHT_BROWSERS_PATH`).
fn playwright_browsers_root() -> PathBuf {
    if let Some(p) = std::env::var_os("PLAYWRIGHT_BROWSERS_PATH") {
        return PathBuf::from(p);
    }
    let home = home_dir();
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Caches/ms-playwright")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".cache/ms-playwright")
    }
}

/// Isolated profile dir for the SHARED (project-less) agent browser, under
/// agentum's state dir. Nested at `cdp-browser/shared` — a SIBLING of the
/// persistent `project-<repoId>` dirs — so `stop_local_cdp_browser`'s existing
/// `remove_dir_all` is structurally incapable of touching a project profile,
/// and the boot sweep can tell legacy top-level entries from live ones
/// (spec 014, Decision G).
fn user_data_dir() -> Result<PathBuf> {
    let dir = agentum_store::paths::state_dir()
        .context("resolve agentum state dir")?
        .join("cdp-browser")
        .join("shared");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create CDP browser profile dir {}", dir.display()))?;
    Ok(dir)
}

// --- small network/path helpers, mirroring `playwright_mcp`'s shape ---------

/// `$HOME`, falling back to `/` so a spawn never fails on an unset HOME.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    // Why: `isolate_home()` hands each test a `TEST_ENV_LOCK` guard that must
    // span the whole test body — including awaits — to serialize AGENTUM_HOME
    // mutation across the crate's tests. Each #[tokio::test] runs on its own
    // single-thread runtime, so blocking peers on the std mutex is safe.
    #![allow(clippy::await_holding_lock)]
    use super::*;
    use std::path::Path;

    #[test]
    fn endpoint_is_ipv4_loopback() {
        assert_eq!(cdp_endpoint_for(9300), "http://127.0.0.1:9300");
        assert_eq!(cdp_endpoint_for(9999), "http://127.0.0.1:9999");
    }

    #[test]
    fn canonical_worktree_key_unifies_pane_id_and_agent_path() {
        // The whole per-worktree contract: the user's pane (full UI id) and the
        // worktree's agent (bare path) MUST collapse to the SAME registry key, or
        // the agent drives a different browser than the user is watching.
        let path = "/Users/x/.agentum/worktrees/feat";
        assert_eq!(
            canonical_worktree_key(&format!("repo-abc::{path}")),
            canonical_worktree_key(path),
        );
        assert_eq!(canonical_worktree_key("repo::/a/b"), "/a/b");
        assert_eq!(canonical_worktree_key("/a/b"), "/a/b");
        // Folder-project instance suffix is stripped so both sides still match.
        assert_eq!(
            canonical_worktree_key("repo::/folder::workspace:0123abcd-0000-0000-0000-000000000000"),
            "/folder",
        );
        // No worktree context → empty → caller falls back to the shared browser.
        assert_eq!(canonical_worktree_key(""), "");
        // A github-pr pseudo-worktree (single colons) keeps its own isolated key.
        assert_eq!(
            canonical_worktree_key("github-pr:repo:42"),
            "github-pr:repo:42"
        );
    }

    #[test]
    fn chrome_argv_is_headless_with_debugging_and_isolated_profile() {
        let argv = build_chrome_argv(Path::new("/x/Chromium"), 9300, Path::new("/tmp/prof"));
        // Headless screencast path — agentum renders the frames in its own pane.
        // Must be the full `--headless=new` (screencast-capable), not the bare
        // legacy flag, and never windowed.
        assert!(argv.iter().any(|a| a == "--headless=new"));
        // CDP exposed on the exact port we probe, loopback-only.
        assert!(argv.iter().any(|a| a == "--remote-debugging-port=9300"));
        assert!(
            argv.iter()
                .any(|a| a == "--remote-debugging-address=127.0.0.1")
        );
        // A fixed viewport so screencast frames have a sane default size.
        assert!(argv.iter().any(|a| a == "--window-size=1280,800"));
        // Force the capture device-scale so the screencast is device-pixel sharp.
        assert!(
            argv.iter()
                .any(|a| a.starts_with("--force-device-scale-factor=")),
            "headless must force a capture device-scale: {argv:?}"
        );
        // Isolated profile + first-run nags suppressed.
        assert!(argv.iter().any(|a| a == "--user-data-dir=/tmp/prof"));
        assert!(argv.iter().any(|a| a == "--no-first-run"));
        // argv[0] is the resolved executable.
        assert_eq!(argv[0], "/x/Chromium");
    }

    #[test]
    fn remote_launch_script_is_idempotent_headless_and_binary_agnostic() {
        let s = remote_chrome_launch_script(9222);
        // Idempotent on the session name (don't double-launch), detached tmux.
        assert!(s.contains(&format!("tmux has-session -t {CDP_TMUX_TARGET}")));
        assert!(s.contains(&format!("tmux new-session -d -s {CDP_TMUX_TARGET}")));
        // Same headless/loopback flags as local, on the requested host port.
        assert!(s.contains("--headless=new"));
        assert!(s.contains("--remote-debugging-address=127.0.0.1"));
        assert!(s.contains("--remote-debugging-port=9222"));
        // Resolves whichever Chromium the host has (no hard-coded binary).
        assert!(s.contains("command -v chromium"));
        assert!(s.contains("google-chrome"));
        // No nested single-quotes (which don't nest in POSIX sh) — uses "$B …".
        assert!(!s.contains("'"));
    }

    #[test]
    fn rev_parsing_matches_headed_chromium_only() {
        assert_eq!(parse_chromium_rev("chromium-1124"), Some(1124));
        assert_eq!(parse_chromium_rev("chromium-1187"), Some(1187));
        // The windowless headless shell must NEVER be picked.
        assert_eq!(parse_chromium_rev("chromium_headless_shell-1124"), None);
        // Other browsers / junk.
        assert_eq!(parse_chromium_rev("firefox-1234"), None);
        assert_eq!(parse_chromium_rev("chromium-"), None);
        assert_eq!(parse_chromium_rev("ffmpeg-1011"), None);
    }

    #[test]
    fn system_chrome_candidates_are_listed_per_os() {
        let candidates = chrome_candidate_paths();
        // Every OS we ship on must offer at least one well-known Chrome path so
        // the no-download path (system Chrome) is reachable.
        assert!(!candidates.is_empty());
        #[cfg(target_os = "macos")]
        assert!(candidates.iter().any(
            |p| p == Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        ));
        #[cfg(target_os = "linux")]
        assert!(
            candidates
                .iter()
                .any(|p| p == Path::new("/usr/bin/google-chrome"))
        );
        #[cfg(target_os = "windows")]
        assert!(
            candidates
                .iter()
                .any(|p| p == Path::new(r"C:\Program Files\Google\Chrome\Application\chrome.exe"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_renamed_arch_suffixed_chrome_for_testing() {
        // Regression for the bug live-verify caught: Playwright build 1228 ships
        // `chrome-mac-x64/Google Chrome for Testing.app/.../Google Chrome for
        // Testing` — the arch suffix + app rename a hardcoded path missed.
        let tmp = std::env::temp_dir().join(format!("agentum-cdp-disc-{}", std::process::id()));
        let macos = tmp
            .join("chrome-mac-x64")
            .join("Google Chrome for Testing.app")
            .join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("Google Chrome for Testing");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();

        let found = find_chrome_exe_in(&tmp).expect("should discover the renamed exe");
        assert_eq!(found, exe);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_legacy_chromium_app_too() {
        // Older builds: `chrome-mac/Chromium.app/.../Chromium`.
        let tmp = std::env::temp_dir().join(format!("agentum-cdp-legacy-{}", std::process::id()));
        let macos = tmp
            .join("chrome-mac")
            .join("Chromium.app")
            .join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("Chromium");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();

        let found = find_chrome_exe_in(&tmp).expect("should discover the legacy Chromium exe");
        assert_eq!(found, exe);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- spec 014: browser scope resolution + per-project persistence --------

    fn tbl(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    /// Serialize + isolate env-mutating tests (AGENTUM_HOME is process-global).
    /// Mirrors `routes/profiles.rs::isolate_xdg`, incl. the escape assertion —
    /// every asserted profile path must sit under the temp home so a test's
    /// pkill signature can never match a real process.
    fn isolate_home() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        // SAFETY: `set_var` is unsound under concurrent env access;
        // TEST_ENV_LOCK serializes every env-mutating test in the crate.
        unsafe {
            std::env::set_var("AGENTUM_HOME", dir.path());
        }
        let state = agentum_store::paths::state_dir().expect("state_dir resolves");
        assert!(
            state.starts_with(dir.path()),
            "AGENTUM_HOME isolation broken: {state:?} escaped {:?}",
            dir.path()
        );
        (dir, guard)
    }

    #[test]
    fn scope_pane_id_and_agent_path_resolve_to_same_project() {
        // The per-project contract: the user's pane (full UI id) and the
        // worktree's agent (bare path) MUST collapse to the SAME project scope,
        // or the agent drives a different browser than the user is watching.
        let path = "/Users/x/.agentum/worktrees/feat";
        let full = format!("repo-abc::{path}");
        let wts = tbl(&[("repo-abc", full.as_str())]);
        let a = resolve_scope_from_tables(&full, &wts, &[]).unwrap();
        let b = resolve_scope_from_tables(path, &wts, &[]).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a,
            BrowserScope::Project {
                repo_id: "repo-abc".into()
            }
        );
        assert_eq!(a.profile_token(), b.profile_token());
    }

    #[test]
    fn scope_bare_repo_id_workspace_suffix_and_repo_main_path() {
        let uuid = "0123abcd-0000-0000-0000-000000000000";
        let repos = tbl(&[(uuid, "/repo/main")]);
        // Bare registered repo id (plain-workspace / project-hub surfaces).
        assert_eq!(
            resolve_scope_from_tables(uuid, &[], &repos).unwrap(),
            BrowserScope::Project {
                repo_id: uuid.into()
            }
        );
        // Folder-project instance id — the `<repoId>::` prefix wins.
        assert_eq!(
            resolve_scope_from_tables(&format!("{uuid}::/folder::workspace:{uuid}"), &[], &repos)
                .unwrap(),
            BrowserScope::Project {
                repo_id: uuid.into()
            }
        );
        // A session running in the repo's main checkout (bare path, repos hit).
        assert_eq!(
            resolve_scope_from_tables("/repo/main", &[], &repos).unwrap(),
            BrowserScope::Project {
                repo_id: uuid.into()
            }
        );
    }

    #[test]
    fn scope_pseudo_keys_and_unknown_uuid_are_adhoc_empty_is_shared() {
        assert_eq!(
            resolve_scope_from_tables("", &[], &[]).unwrap(),
            BrowserScope::Shared
        );
        assert_eq!(
            resolve_scope_from_tables("github-pr:repo:42", &[], &[]).unwrap(),
            BrowserScope::Adhoc {
                key: "github-pr:repo:42".into()
            }
        );
        // A bare UUID nobody registered stays isolated — never Project/Shared.
        assert_eq!(
            resolve_scope_from_tables("9999abcd-0000-0000-0000-000000000000", &[], &[]).unwrap(),
            BrowserScope::Adhoc {
                key: "9999abcd-0000-0000-0000-000000000000".into()
            }
        );
        // An unknown absolute path defers to the git probe (None) — NOT Shared.
        assert_eq!(resolve_scope_from_tables("/no/such/path", &[], &[]), None);
    }

    #[test]
    fn project_profile_token_is_prefixed_fs_safe_and_uncollidable() {
        // A UUID passes sanitization unchanged → the dir is literally
        // `project-<uuid>` (spec 014 AC 1).
        let uuid = "0123abcd-0000-0000-0000-000000000000";
        let p = BrowserScope::Project {
            repo_id: uuid.into(),
        };
        assert_eq!(p.profile_token().unwrap(), format!("project-{uuid}"));
        // The prefix is applied AFTER sanitization: a pathological repo id long
        // enough to hit the 48-char tail bound still keeps the prefix (it is
        // what the boot sweep keys persistence on).
        let p = BrowserScope::Project {
            repo_id: "x".repeat(200),
        };
        assert!(p.profile_token().unwrap().starts_with("project-"));
        // Adhoc path tokens never land in the project namespace; Shared has none.
        let a = BrowserScope::Adhoc {
            key: "/some/legacy/path".into(),
        };
        assert!(!a.profile_token().unwrap().starts_with("project-"));
        assert!(BrowserScope::Shared.profile_token().is_none());
    }

    #[tokio::test]
    async fn scope_miss_is_adhoc_never_shared() {
        // A path that is in no table and not a registered repo's git tree stays
        // ISOLATED — dumping unknown contexts into the shared cookie jar is the
        // exact leak spec 014 kills.
        let tmp = std::env::temp_dir().join(format!("agentum-scope-miss-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let raw = tmp.to_string_lossy().into_owned();
        let scope = resolve_scope_with(&raw, &[], &[]).await;
        assert_eq!(scope, BrowserScope::Adhoc { key: raw });
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn scope_git_common_dir_fallback_maps_foreign_worktree_to_project() {
        // A git worktree agentum didn't create (absent from worktrees.json)
        // still resolves to its registered main repo via `--git-common-dir`.
        let base = std::env::temp_dir().join(format!("agentum-scope-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let main = base.join("main");
        let wt = base.join("wt");
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str], cwd: &std::path::Path| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .expect("git runs")
        };
        assert!(git(&["init", "-q"], &main).status.success());
        assert!(
            git(
                &[
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "user.name=t",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "x",
                    "-q"
                ],
                &main
            )
            .status
            .success()
        );
        assert!(
            git(&["worktree", "add", "-q", wt.to_str().unwrap()], &main)
                .status
                .success()
        );

        // Register the MAIN repo path (canonicalized — git reports realpaths).
        let main_c = std::fs::canonicalize(&main).unwrap();
        let repos = tbl(&[("rid-1", main_c.to_str().unwrap())]);
        let scope = resolve_scope_with(wt.to_str().unwrap(), &[], &repos).await;
        assert_eq!(
            scope,
            BrowserScope::Project {
                repo_id: "rid-1".into()
            }
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn stop_project_scope_never_deletes_profile_dir() {
        let (home, _guard) = isolate_home();
        let raw = "repo-keep::/tmp/agentum-test-wt-keep";
        let token = "project-repo-keep";
        let profile = agentum_store::paths::state_dir()
            .unwrap()
            .join("cdp-browser")
            .join(token);
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("Cookies"), b"c").unwrap();
        assert!(profile.starts_with(home.path()));
        register_scoped_browser(token, 1, "agentum-cdp-browser-test-keep", &profile);

        stop_local_cdp_browser_for(raw).await.unwrap();

        assert!(
            !browser_registry().lock().unwrap().contains_key(token),
            "release must drop the registry entry"
        );
        assert!(
            profile.join("Cookies").exists(),
            "a project profile must PERSIST across stop (spec 014 AC 2)"
        );
    }

    #[tokio::test]
    async fn stop_adhoc_scope_deletes_profile_dir() {
        let (home, _guard) = isolate_home();
        let raw = "github-pr:repo:42";
        let token = sanitize_worktree_token(raw);
        let profile = agentum_store::paths::state_dir()
            .unwrap()
            .join("cdp-browser")
            .join(&token);
        std::fs::create_dir_all(&profile).unwrap();
        assert!(profile.starts_with(home.path()));

        stop_local_cdp_browser_for(raw).await.unwrap();
        assert!(
            !profile.exists(),
            "adhoc profiles keep the ephemeral delete-on-stop contract"
        );
    }

    #[tokio::test]
    async fn release_is_noop_while_project_attached() {
        let (home, _guard) = isolate_home();
        let raw = "repo-att::/tmp/agentum-test-wt-att";
        let token = "project-repo-att";
        let profile = agentum_store::paths::state_dir()
            .unwrap()
            .join("cdp-browser")
            .join(token);
        std::fs::create_dir_all(&profile).unwrap();
        assert!(profile.starts_with(home.path()));
        register_scoped_browser(token, 1, "agentum-cdp-browser-test-att", &profile);

        let attach = register_browser_attach(raw).await;
        stop_local_cdp_browser_for(raw).await.unwrap();
        assert!(
            browser_registry().lock().unwrap().contains_key(token),
            "a live attach anywhere in the project makes release a no-op (AC 2)"
        );
        assert!(profile.exists());

        drop(attach);
        stop_local_cdp_browser_for(raw).await.unwrap();
        assert!(
            !browser_registry().lock().unwrap().contains_key(token),
            "the last release stops the browser"
        );
        assert!(profile.exists(), "…but the profile still persists");
    }

    #[tokio::test]
    async fn sweep_deletes_only_legacy_entries() {
        let (_home, _guard) = isolate_home();
        let root = agentum_store::paths::state_dir()
            .unwrap()
            .join("cdp-browser");
        std::fs::create_dir_all(root.join("old-worktree-token")).unwrap();
        std::fs::create_dir_all(root.join("Default")).unwrap();
        std::fs::write(root.join("Local State"), b"{}").unwrap();
        std::fs::create_dir_all(root.join("project-a")).unwrap();
        std::fs::write(root.join("project-a").join("Cookies"), b"c").unwrap();
        std::fs::create_dir_all(root.join("shared")).unwrap();

        sweep_legacy_profile_dirs().await;
        assert!(!root.join("old-worktree-token").exists());
        assert!(!root.join("Default").exists());
        assert!(!root.join("Local State").exists());
        assert!(root.join("project-a").join("Cookies").exists());
        assert!(root.join("shared").exists());

        // Idempotent: a second sweep changes nothing.
        sweep_legacy_profile_dirs().await;
        assert!(root.join("project-a").exists() && root.join("shared").exists());
    }

    #[tokio::test]
    async fn clear_project_browser_data_deletes_only_that_project() {
        let (home, _guard) = isolate_home();
        let root = agentum_store::paths::state_dir()
            .unwrap()
            .join("cdp-browser");
        let a = root.join("project-repo-p");
        let b = root.join("project-repo-q");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("Cookies"), b"a").unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("Cookies"), b"b").unwrap();
        assert!(a.starts_with(home.path()));
        register_scoped_browser("project-repo-p", 1, "agentum-cdp-browser-test-clr", &a);
        // Even a LIVE attach doesn't block an explicit clear (user intent).
        let _attach = register_browser_attach("repo-p::/tmp/agentum-test-wt-clr").await;

        clear_project_browser_data("repo-p").await.unwrap();

        assert!(!a.exists(), "the cleared project's profile must be deleted");
        assert!(
            b.join("Cookies").exists(),
            "every OTHER project's profile must be untouched (AC 5)"
        );
        assert!(
            !browser_registry()
                .lock()
                .unwrap()
                .contains_key("project-repo-p")
        );
    }

    #[tokio::test]
    async fn clear_project_browser_data_requires_a_repo_id() {
        // An empty id would sanitize into a junk token — refuse loudly instead.
        assert!(clear_project_browser_data("  ").await.is_err());
    }

    #[test]
    fn shared_user_data_dir_is_nested_shared_subdir() {
        let (home, _guard) = isolate_home();
        let dir = user_data_dir().unwrap();
        assert!(dir.starts_with(home.path()));
        assert!(
            dir.ends_with("cdp-browser/shared"),
            "the shared profile must be a SIBLING of project-* dirs, so its \
             delete-on-stop can never touch them (spec 014 G): {dir:?}"
        );
    }
}
