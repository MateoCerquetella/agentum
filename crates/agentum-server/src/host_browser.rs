//! Host-resident headless browser (spec 009a, Phase 1).
//!
//! Launches a headless Chromium **on the host** — in its own persistent tmux
//! session — with a CDP debugger bound to the host's loopback. The caller then
//! forward-tunnels that CDP port to the Mac (`host_runtime::ensure_forward_tunnel`)
//! and, in later phases, drives a screencast over it.
//!
//! Because the browser lives in a host tmux session it survives the Mac
//! sleeping / agentum closing; reconnect is a deterministic per-worktree lookup
//! (`agentum-hostbrowser-<wt>`), not a relaunch. Teardown is `kill_session` —
//! headless Chromium ignores `C-c`, so a graceful stop never reaps it.

use std::path::Path;
use std::time::{Duration, Instant};

use agentum_core::Host;
use tokio::time::sleep;

use crate::host_runtime::{self, HostRuntimeError, Result};

/// Browser binary launched on the host. Preflight / selection across
/// `chromium-browser` / `google-chrome` is a later phase; Phase 1 targets the
/// common `chromium` (present on the Arch/Omarchy test host at `/usr/bin/chromium`).
const CHROMIUM_BIN: &str = "chromium";

/// How long to wait for headless Chromium to bind its CDP port and write the
/// `DevToolsActivePort` file before giving up (a cold start + MCP-free boot is
/// a second or two; allow generous slack on a distant host).
const CDP_READY_TIMEOUT: Duration = Duration::from_secs(20);
const CDP_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A host-resident browser launched (or re-attached) for one worktree.
#[derive(Debug, Clone)]
pub struct HostBrowser {
    /// Sanitized worktree slug (the `<wt>` in the deterministic names).
    pub workdir_slug: String,
    /// tmux session name on the host (`agentum-hostbrowser-<wt>`).
    pub tmux_target: String,
    /// Chromium `--user-data-dir` on the host (`/tmp/agentum-hostbrowser-<wt>`).
    pub user_data_dir: String,
    /// The CDP port Chromium bound on the host's loopback (from DevToolsActivePort).
    pub cdp_port: u16,
    /// True when launch re-attached to an already-running session (reconnect).
    pub attached: bool,
}

/// Sanitized worktree slug from a workdir basename — `[A-Za-z0-9-]`, mirroring
/// the harness `sanitize`. Deterministic per worktree so reconnect is a lookup;
/// an empty/`/`-only path degrades to `default` rather than producing an empty
/// tmux/dir name.
fn workdir_slug(workdir: &Path) -> String {
    let base = workdir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("default");
    let slug: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if slug.is_empty() {
        "default".to_string()
    } else {
        slug
    }
}

/// tmux session name for a worktree's host browser. `agentum-hostbrowser-<wt>`.
fn host_browser_target(slug: &str) -> String {
    agentum_tmux::target_for(&format!("hostbrowser-{slug}"))
}

/// Chromium `--user-data-dir` for a worktree. Deterministic so reconnect finds
/// the same DevToolsActivePort; `/tmp` keeps it isolated + auto-cleared on reboot.
fn host_browser_user_data_dir(slug: &str) -> String {
    format!("/tmp/agentum-hostbrowser-{slug}")
}

/// Path to Chromium's `DevToolsActivePort` file under a user-data dir.
fn devtools_active_port_path(user_data_dir: &str) -> String {
    format!("{user_data_dir}/DevToolsActivePort")
}

/// Parse the bound CDP port from a `DevToolsActivePort` file: its first line is
/// the port, its second the browser WS path. We only need the port.
fn parse_devtools_active_port(contents: &str) -> Option<u16> {
    contents.lines().next()?.trim().parse::<u16>().ok()
}

/// Chromium argv for a headless, **loopback-only** CDP browser. Port `0` →
/// Chromium picks a free port and records it in `<user_data_dir>/DevToolsActivePort`.
fn chromium_argv(bin: &str, user_data_dir: &str) -> Vec<String> {
    vec![
        bin.to_string(),
        "--headless=new".to_string(),
        // Security invariant: CDP reachable only via the SSH tunnel, never a
        // public interface.
        "--remote-debugging-address=127.0.0.1".to_string(),
        // 0 → Chromium binds a free port and writes it to DevToolsActivePort.
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={user_data_dir}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        // Most remote hosts are headless servers with no GPU.
        "--disable-gpu".to_string(),
        "about:blank".to_string(),
    ]
}

/// The full tmux pane command: clear any stale `DevToolsActivePort` (from a
/// prior crashed run, so the await loop can't read a dead port), then `exec`
/// Chromium as the pane's process. Wrapped in `sh -c` so the `rm`/`exec`
/// sequence runs regardless of the host's login shell.
fn launch_command(bin: &str, user_data_dir: &str, port_file: &str) -> Result<Vec<String>> {
    let argv = chromium_argv(bin, user_data_dir);
    let joined =
        shlex::try_join(argv.iter().map(String::as_str)).map_err(|_| HostRuntimeError::Quote)?;
    let pf = shlex::try_quote(port_file).map_err(|_| HostRuntimeError::Quote)?;
    let inner = format!("rm -f {pf}; exec {joined}");
    Ok(vec!["sh".to_string(), "-c".to_string(), inner])
}

/// Poll the host's `DevToolsActivePort` until Chromium has written a parseable
/// port, or time out (the bind is async to process start).
async fn await_cdp_port(host: &Host, port_file: &str) -> Result<u16> {
    let deadline = Instant::now() + CDP_READY_TIMEOUT;
    loop {
        if let Some(bytes) = host_runtime::read_file_bytes(host, port_file).await? {
            if let Some(port) = parse_devtools_active_port(&String::from_utf8_lossy(&bytes)) {
                return Ok(port);
            }
        }
        if Instant::now() >= deadline {
            return Err(HostRuntimeError::Bootstrap(format!(
                "headless Chromium did not write {port_file} within {}s",
                CDP_READY_TIMEOUT.as_secs()
            )));
        }
        sleep(CDP_POLL_INTERVAL).await;
    }
}

/// Launch (or **re-attach** to) the host browser for `workdir`. Returns once the
/// CDP port is known. The caller forward-tunnels `cdp_port` to reach it.
///
/// Re-attach is a lookup: an existing `agentum-hostbrowser-<wt>` session means
/// the browser is still running (it outlived a Mac sleep / agentum close), so we
/// read its current CDP port rather than relaunching.
pub async fn launch_host_browser(host: &Host, workdir: &Path) -> Result<HostBrowser> {
    let slug = workdir_slug(workdir);
    let target = host_browser_target(&slug);
    let user_data_dir = host_browser_user_data_dir(&slug);
    let port_file = devtools_active_port_path(&user_data_dir);

    let attached = host_runtime::has_session(host, &target).await?;
    if !attached {
        let cmd = launch_command(CHROMIUM_BIN, &user_data_dir, &port_file)?;
        host_runtime::new_session(host, &target, workdir, &cmd, &[]).await?;
    }

    let cdp_port = await_cdp_port(host, &port_file).await?;

    // Best-effort per-worktree marker so a later reconnect can find the port by
    // lookup. DevToolsActivePort is the live source of truth; this is a copy.
    let _ = host_runtime::write_home_relative_file(
        host,
        &format!(".agentum/hostbrowser/{slug}.port"),
        &format!("{cdp_port}\n"),
    )
    .await;

    Ok(HostBrowser {
        workdir_slug: slug,
        tmux_target: target,
        user_data_dir,
        cdp_port,
        attached,
    })
}

/// Kill the worktree's host browser (tmux session). `kill_session`, **not** a
/// graceful `C-c` — headless Chromium ignores SIGINT and would linger.
pub async fn teardown_host_browser(host: &Host, workdir: &Path) -> Result<()> {
    let target = host_browser_target(&workdir_slug(workdir));
    host_runtime::kill_session(host, &target).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn workdir_slug_sanitizes_basename_and_defaults_empty() {
        assert_eq!(workdir_slug(Path::new("/home/malloc/My Repo")), "My-Repo");
        assert_eq!(workdir_slug(Path::new("/home/malloc/repo.git")), "repo-git");
        // No basename (root) → a stable fallback, never an empty name.
        assert_eq!(workdir_slug(Path::new("/")), "default");
    }

    #[test]
    fn names_are_deterministic_per_worktree() {
        let slug = "myrepo";
        assert_eq!(host_browser_target(slug), "agentum-hostbrowser-myrepo");
        assert_eq!(
            host_browser_user_data_dir(slug),
            "/tmp/agentum-hostbrowser-myrepo"
        );
        assert_eq!(
            devtools_active_port_path("/tmp/agentum-hostbrowser-myrepo"),
            "/tmp/agentum-hostbrowser-myrepo/DevToolsActivePort"
        );
    }

    #[test]
    fn parse_devtools_active_port_reads_first_line() {
        // The real file: port on line 1, browser WS path on line 2.
        assert_eq!(
            parse_devtools_active_port("45821\n/devtools/browser/abc-123\n"),
            Some(45821)
        );
        assert_eq!(parse_devtools_active_port("  45821  \n"), Some(45821));
        assert_eq!(parse_devtools_active_port(""), None);
        assert_eq!(parse_devtools_active_port("not-a-port\n"), None);
    }

    #[test]
    fn chromium_argv_is_headless_and_loopback_only() {
        let argv = chromium_argv("chromium", "/tmp/agentum-hostbrowser-myrepo");
        assert_eq!(argv[0], "chromium", "binary must lead the argv");
        assert!(argv.iter().any(|a| a == "--headless=new"), "{argv:?}");
        // Security invariant: the CDP debugger must bind loopback only, reached
        // solely via the SSH tunnel — never a public interface.
        assert!(
            argv.iter()
                .any(|a| a == "--remote-debugging-address=127.0.0.1"),
            "CDP not pinned to loopback: {argv:?}"
        );
        // Port 0 → Chromium auto-picks a free port (recorded in DevToolsActivePort).
        assert!(
            argv.iter().any(|a| a == "--remote-debugging-port=0"),
            "CDP port not auto-assigned: {argv:?}"
        );
        assert!(
            argv.iter()
                .any(|a| a == "--user-data-dir=/tmp/agentum-hostbrowser-myrepo"),
            "user-data-dir missing: {argv:?}"
        );
    }

    #[test]
    fn launch_command_clears_stale_port_then_execs_chromium() {
        let cmd = launch_command(
            "chromium",
            "/tmp/agentum-hostbrowser-myrepo",
            "/tmp/agentum-hostbrowser-myrepo/DevToolsActivePort",
        )
        .unwrap();
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[1], "-c");
        let inner = &cmd[2];
        // Stale DevToolsActivePort cleared so the await loop reads a fresh bind.
        assert!(inner.contains("rm -f"), "stale port not cleared: {inner}");
        assert!(inner.contains("DevToolsActivePort"), "{inner}");
        // Chromium exec'd as the pane process so a tmux kill reaps it cleanly.
        assert!(inner.contains("exec "), "chromium not exec'd: {inner}");
        assert!(
            inner.contains("--remote-debugging-port=0"),
            "chromium argv missing: {inner}"
        );
    }
}
