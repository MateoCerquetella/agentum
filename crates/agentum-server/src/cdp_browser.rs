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

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
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
        if wait_until_listening(port, Duration::from_secs(2)).await {
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
    // (an MCP-cold machine populating the profile can take a few seconds).
    if wait_until_listening(port, Duration::from_secs(20)).await {
        Ok(endpoint)
    } else {
        anyhow::bail!(
            "Chromium launched but did not expose CDP on 127.0.0.1:{port} within 20s \
             (tmux session `{CDP_TMUX_TARGET}`). Check the pane; the browser may have \
             failed to start."
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

/// Build the headless-Chromium argv. Split out so the flag shape is unit-testable
/// without launching a browser.
///
/// `--headless=new` runs Chrome's modern headless (full rendering + CDP
/// `Page.startScreencast`, unlike the reduced `chromium_headless_shell`) — agentum
/// renders the frames inside its own pane (009c-3), so there is no OS window.
/// `--window-size` fixes the headless viewport so screencast frames have a sane
/// default size (the screencast `maxWidth/maxHeight` caps it further).
/// `--remote-debugging-address=127.0.0.1` keeps CDP loopback-only (never a public
/// interface; 009c-2 reaches it solely via the authenticated SSH tunnel).
/// `--no-first-run` / `--no-default-browser-check` suppress the first-run nags
/// that would otherwise block automation. An isolated `--user-data-dir` keeps
/// this browser off the user's real profile. `about:blank` is a benign initial
/// page (the agent navigates its own tab afterwards).
fn build_chrome_argv(
    exe: &std::path::Path,
    port: u16,
    user_data_dir: &std::path::Path,
) -> Vec<String> {
    vec![
        exe.to_string_lossy().into_owned(),
        "--headless=new".to_string(),
        "--hide-scrollbars".to_string(),
        "--window-size=1280,800".to_string(),
        "--remote-debugging-address=127.0.0.1".to_string(),
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={}", user_data_dir.to_string_lossy()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "about:blank".to_string(),
    ]
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

/// Isolated profile dir for the agent browser, under agentum's state dir.
fn user_data_dir() -> Result<PathBuf> {
    let dir = agentum_store::paths::state_dir()
        .context("resolve agentum state dir")?
        .join("cdp-browser");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create CDP browser profile dir {}", dir.display()))?;
    Ok(dir)
}

// --- small network/path helpers, mirroring `playwright_mcp`'s shape ---------

/// A plain TCP connect is enough to know "something is serving here"; the bound
/// MCP's CDP client performs the protocol handshake (with its own
/// `--cdp-timeout`). Short timeout so a dead port fails fast on the hot path.
async fn port_listening(port: u16) -> bool {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    matches!(
        tokio::time::timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Poll the port until it accepts connections or the deadline passes.
async fn wait_until_listening(port: u16, max: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + max;
    loop {
        if port_listening(port).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// `$HOME`, falling back to `/` so a spawn never fails on an unset HOME.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn endpoint_is_ipv4_loopback() {
        assert_eq!(cdp_endpoint_for(9300), "http://127.0.0.1:9300");
        assert_eq!(cdp_endpoint_for(9999), "http://127.0.0.1:9999");
    }

    #[test]
    fn chrome_argv_is_headless_with_debugging_and_isolated_profile() {
        let argv = build_chrome_argv(Path::new("/x/Chromium"), 9300, Path::new("/tmp/prof"));
        // Headless since 009c-3 — agentum renders the frames in its own pane.
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
        // Isolated profile + first-run nags suppressed.
        assert!(argv.iter().any(|a| a == "--user-data-dir=/tmp/prof"));
        assert!(argv.iter().any(|a| a == "--no-first-run"));
        // argv[0] is the resolved executable.
        assert_eq!(argv[0], "/x/Chromium");
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
}
