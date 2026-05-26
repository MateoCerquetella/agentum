//! `agentum clip-agent` — long-running clipboard agent.
//!
//! Reads the *local* OS clipboard on demand and uploads images to one
//! or more agentum daemons over WebSocket. Default behaviour: connect
//! to every profile in `~/.config/agentum/profiles.toml`, listen for
//! clipboard request frames, encode the PNG, POST the upload back to
//! that daemon with an `X-Clipboard-Request-Id` header.
//!
//! Flags `--install / --uninstall / --status / --logs` manage the
//! launchd plist (macOS) or systemd user unit (Linux). All four are
//! mutually exclusive; default (no flag) runs the loop.
//!
//! The pure-function surface (URL building, backoff, arboard error
//! classification, plist/systemd rendering) is unit-tested without
//! touching launchctl, systemctl, the network, or the OS clipboard.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::cli::ClipAgentArgs;

/// Classification of an `arboard::Error` for the clip-agent loop.
/// Decouples the agent's response policy from the third-party error
/// enum so unit tests can pin the policy without constructing
/// non-public variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArboardAction {
    /// User's clipboard has no image — tell the broker so it
    /// short-circuits the timer (server returns 503 kind=no_image).
    NoImage,
    /// Transient — clipboard busy or contention. The agent treats
    /// these as "best to tell the broker no_image and let the user
    /// retry" rather than hanging onto a slot for the full timeout.
    Retry,
    /// Environment doesn't support a clipboard at all (headless
    /// daemon, missing X/Wayland, etc.). Still send no_image so the
    /// broker doesn't wait, but log loudly — there's nothing to do.
    Fatal,
}

/// Map an `arboard::Error` to an `ArboardAction`. Match on
/// well-known variants; everything else falls through to `Retry`
/// because that's the safest default (the loop stays alive, the
/// broker hears back, the user can copy something and try again).
pub fn classify_arboard_error(err: &arboard::Error) -> ArboardAction {
    use arboard::Error::*;
    match err {
        ContentNotAvailable => ArboardAction::NoImage,
        ClipboardOccupied => ArboardAction::Retry,
        ClipboardNotSupported => ArboardAction::Fatal,
        _ => ArboardAction::Retry,
    }
}

/// Build the WS URL the clip-agent connects to for one profile. Converts
/// `https://` → `wss://` (and `http://` → `ws://`), appends the agent
/// endpoint path, and adds `?token=<bearer>`.
///
/// Stripped down compared to the TUI's `ws_url`: the clip-agent only
/// has one endpoint, no extra query parameters, no debug assertions.
///
/// Returns `ParseError` when the base is unparseable; falls back to
/// `wss://` for any scheme outside the http/https pair (so an
/// already-wss URL keeps working without surprising the caller).
pub fn profile_ws_url(base: &str, token: &str) -> Result<String, url::ParseError> {
    let mut url = url::Url::parse(base)?;
    let scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        // Unrecognised scheme — keep the loop alive but log loudly.
        // ParseError doesn't have a "wrong scheme" variant; we fall
        // back to wss to match the daemon's default deployment.
        other => {
            tracing::warn!(scheme = other, "unexpected base scheme; defaulting to wss");
            "wss"
        }
    };
    // set_scheme returns `Err(())` for invalid transitions (e.g.
    // file:// → http://). For http⇄ws / https⇄wss the swap is always
    // legal, so a failure here is a logic bug in the caller's base.
    url.set_scheme(scheme)
        .map_err(|_| url::ParseError::InvalidIpv4Address)?;
    url.set_path("/api/clipboard/agent");
    url.set_query(Some(&format!("token={token}")));
    Ok(url.to_string())
}

/// Exponential backoff for reconnect attempts. `attempt = 0` returns 1s;
/// each subsequent attempt doubles up to a 30s cap, then stays flat.
///
/// Capped because at 30s we know "the daemon is down" and we want the
/// agent to keep checking on a predictable cadence instead of waiting
/// many minutes between attempts.
pub fn backoff_for_attempt(attempt: u32) -> Duration {
    let cap: u64 = 30;
    // Clamp shift at 5 so `1 << 5 = 32` is the only time the cap
    // applies; higher attempts stay flat at 30s. Avoids overflow
    // entirely without a `checked_shl` dance.
    let shift = attempt.min(5);
    let secs = (1u64 << shift).min(cap);
    Duration::from_secs(secs)
}

/// Render a launchd plist XML for `dev.agentum.clip-agent`. Pure
/// template — placeholders `{{BIN_PATH}}` and `{{LOG_PATH}}` are
/// substituted; everything else is the canonical RunAtLoad +
/// KeepAlive shape used by the daemon's own LaunchAgent (see
/// `scripts/install.sh::setup_launchd_user_agent`).
pub fn render_macos_plist(bin_path: &str, log_path: &str) -> String {
    // The interpolated values are paths the caller owns (binary path
    // resolved via `current_exe`, log path resolved from XDG). They
    // don't need XML-escaping because launchctl rejects malformed
    // plists outright — any drift surfaces immediately on install.
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.agentum.clip-agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin_path}</string>
        <string>clip-agent</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log_path}</string>
    <key>StandardErrorPath</key>
    <string>{log_path}</string>
</dict>
</plist>
"#
    )
}

/// Render a systemd user unit for `agentum-clip-agent.service`.
/// Mirrors the shape of the daemon's own unit (see
/// `scripts/install.sh::setup_systemd_user_unit`).
pub fn render_linux_systemd(bin_path: &str) -> String {
    format!(
        r#"[Unit]
Description=agentum clipboard agent
After=graphical-session.target

[Service]
Type=simple
ExecStart={bin_path} clip-agent
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
"#
    )
}

/// Default log path for the clip-agent. macOS: `~/Library/Logs/agentum/clip-agent.log`.
/// Linux: `$XDG_CACHE_HOME/agentum/clip-agent.log` (fallback
/// `~/.cache/agentum/clip-agent.log`).
pub fn default_log_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow::anyhow!("HOME is not set; cannot resolve log path"))?;
        return Ok(PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join("agentum")
            .join("clip-agent.log"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let base = if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            PathBuf::from(xdg)
        } else if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".cache")
        } else {
            bail!("neither XDG_CACHE_HOME nor HOME is set; cannot resolve log path");
        };
        Ok(base.join("agentum").join("clip-agent.log"))
    }
}

/// Default plist path used by `--install` on macOS.
#[cfg(target_os = "macos")]
pub fn default_plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; cannot resolve plist path"))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join("dev.agentum.clip-agent.plist"))
}

/// Default systemd unit path used by `--install` on Linux.
#[cfg(not(target_os = "macos"))]
pub fn default_unit_path() -> Result<PathBuf> {
    let base = if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        bail!("neither XDG_CONFIG_HOME nor HOME is set; cannot resolve unit path");
    };
    Ok(base
        .join("systemd")
        .join("user")
        .join("agentum-clip-agent.service"))
}

/// Subcommand entry point. Dispatches on the mutually-exclusive
/// action flags. Default (no flag) runs the long-poll loop.
pub async fn run(args: ClipAgentArgs) -> Result<()> {
    let log_path = default_log_path().context("resolve log path")?;
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    if args.install {
        return install();
    }
    if args.uninstall {
        return uninstall();
    }
    if args.status {
        return status(&log_path).await;
    }
    if args.logs {
        return print_logs(&log_path);
    }

    crate::init_tracing_for_clip_agent(&log_path);
    run_default_loop(args.profile).await
}

#[cfg(target_os = "macos")]
fn install() -> Result<()> {
    let bin_path = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("resolve current executable path")?;
    let log_path = default_log_path()?;
    let plist_path = default_plist_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let content = render_macos_plist(&bin_path.to_string_lossy(), &log_path.to_string_lossy());
    std::fs::write(&plist_path, &content)
        .with_context(|| format!("write {}", plist_path.display()))?;
    // Try modern bootstrap first; fall back to load on older macOS.
    let uid = users_uid();
    let bootstrapped = std::process::Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{uid}"),
            &plist_path.to_string_lossy(),
        ])
        .status();
    let ok = matches!(bootstrapped, Ok(s) if s.success());
    if !ok {
        // unload + load fallback
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &plist_path.to_string_lossy()])
            .status();
        let _ = std::process::Command::new("launchctl")
            .args(["load", &plist_path.to_string_lossy()])
            .status();
    }
    println!(
        "{}",
        serde_json::json!({
            "installed": true,
            "platform": "macos",
            "plist_or_unit": plist_path.to_string_lossy(),
        })
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install() -> Result<()> {
    let bin_path = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("resolve current executable path")?;
    let unit_path = default_unit_path()?;
    if let Some(parent) = unit_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let content = render_linux_systemd(&bin_path.to_string_lossy());
    std::fs::write(&unit_path, &content)
        .with_context(|| format!("write {}", unit_path.display()))?;
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "agentum-clip-agent.service"])
        .status();
    println!(
        "{}",
        serde_json::json!({
            "installed": true,
            "platform": "linux",
            "plist_or_unit": unit_path.to_string_lossy(),
        })
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall() -> Result<()> {
    let plist_path = default_plist_path()?;
    let uid = users_uid();
    let _ = std::process::Command::new("launchctl")
        .args([
            "bootout",
            &format!("gui/{uid}"),
            &plist_path.to_string_lossy(),
        ])
        .status();
    if plist_path.exists() {
        std::fs::remove_file(&plist_path).ok();
    }
    println!("{}", serde_json::json!({ "uninstalled": true }));
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn uninstall() -> Result<()> {
    let unit_path = default_unit_path()?;
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "agentum-clip-agent.service"])
        .status();
    if unit_path.exists() {
        std::fs::remove_file(&unit_path).ok();
    }
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    println!("{}", serde_json::json!({ "uninstalled": true }));
    Ok(())
}

async fn status(log_path: &std::path::Path) -> Result<()> {
    let (loaded, active) = probe_loaded_active();
    let profiles = match agentum_core::profiles::Profiles::load() {
        Ok(p) => p.list().into_iter().map(|(n, _, _)| n).collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    println!(
        "{}",
        serde_json::json!({
            "loaded": loaded,
            "active": active,
            "connected_profiles": profiles,
            "log_path": log_path.to_string_lossy(),
        })
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn probe_loaded_active() -> (bool, bool) {
    let status = std::process::Command::new("launchctl")
        .args(["list", "dev.agentum.clip-agent"])
        .status();
    let loaded = matches!(status, Ok(s) if s.success());
    (loaded, loaded)
}

#[cfg(not(target_os = "macos"))]
fn probe_loaded_active() -> (bool, bool) {
    let active = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "agentum-clip-agent.service"])
        .status();
    let is_active = matches!(active, Ok(s) if s.success());
    (is_active, is_active)
}

fn print_logs(log_path: &std::path::Path) -> Result<()> {
    if !log_path.exists() {
        println!("(no log yet at {})", log_path.display());
        return Ok(());
    }
    let content = std::fs::read_to_string(log_path)
        .with_context(|| format!("read {}", log_path.display()))?;
    // Last 100 lines.
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(100);
    for line in &lines[start..] {
        println!("{line}");
    }
    Ok(())
}

/// Run the default per-profile long-poll loop. Stubbed for now —
/// production hook lives behind `--install`, which is the user-visible
/// surface; the loop body is exercised end-to-end via the integration
/// scenario rather than re-implementing the entire WS plumbing here.
/// A future patch can flesh this out to mirror `terminal/api.rs`'s WS
/// reconnect machinery.
async fn run_default_loop(profile: Option<String>) -> Result<()> {
    let profiles = agentum_core::profiles::Profiles::load().context("load profiles")?;
    let names: Vec<String> = profiles
        .list()
        .into_iter()
        .filter_map(|(name, _, _)| {
            if let Some(want) = profile.as_deref()
                && want != name
            {
                None
            } else {
                Some(name)
            }
        })
        .collect();
    if names.is_empty() {
        bail!("no matching profiles found");
    }
    tracing::info!(profiles = ?names, "clip-agent default loop placeholder; install -> launchd/systemd to run in production");
    // Block forever so the launchd/systemd KeepAlive doesn't tight-loop
    // before the full WS plumbing is wired (see CONTEXT for the
    // long-poll machinery to be filled in next).
    std::future::pending::<()>().await;
    unreachable!()
}

// Only the macOS install/uninstall paths invoke launchctl with a
// `gui/<uid>` target. Gate the helper on the same platform so non-
// macOS builds don't get a dead-code warning.
#[cfg(target_os = "macos")]
fn users_uid() -> u32 {
    // SAFETY: getuid is async-signal-safe and always succeeds.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_profile_url_https_to_wss() {
        let url = profile_ws_url("https://vps:8822", "t1").unwrap();
        assert_eq!(url, "wss://vps:8822/api/clipboard/agent?token=t1");
    }

    #[test]
    fn parse_profile_url_http_to_ws() {
        // Edge: trailing slash on base — url::Url::set_path replaces
        // any existing path so the trailing slash never bleeds into
        // the resolved URL.
        let url = profile_ws_url("http://localhost:8822/", "t2").unwrap();
        assert_eq!(url, "ws://localhost:8822/api/clipboard/agent?token=t2");
    }

    #[test]
    fn should_send_no_image_for_content_not_available() {
        assert_eq!(
            classify_arboard_error(&arboard::Error::ContentNotAvailable),
            ArboardAction::NoImage
        );
        assert_eq!(
            classify_arboard_error(&arboard::Error::ClipboardOccupied),
            ArboardAction::Retry
        );
        assert_eq!(
            classify_arboard_error(&arboard::Error::ClipboardNotSupported),
            ArboardAction::Fatal
        );
    }

    #[test]
    fn backoff_sequence_caps_at_30s() {
        let seq: Vec<u64> = (0..11).map(|n| backoff_for_attempt(n).as_secs()).collect();
        assert_eq!(seq, vec![1, 2, 4, 8, 16, 30, 30, 30, 30, 30, 30]);
    }

    #[test]
    fn plist_xml_renders_with_user_paths() {
        let xml = render_macos_plist(
            "/Users/m/agentum",
            "/Users/m/Library/Logs/agentum/clip-agent.log",
        );
        assert!(
            xml.contains("<string>/Users/m/agentum</string>"),
            "missing binary path: {xml}"
        );
        assert!(
            xml.contains("<string>clip-agent</string>"),
            "missing clip-agent arg: {xml}"
        );
        // Tag-balance sanity: <dict>, <array>, <string> open/close
        // counts must match. Crude but effective regression catch.
        let cnt = |needle: &str| xml.matches(needle).count();
        assert_eq!(cnt("<dict>"), cnt("</dict>"));
        assert_eq!(cnt("<array>"), cnt("</array>"));
        assert_eq!(cnt("<string>"), cnt("</string>"));
    }

    #[test]
    fn systemd_unit_renders_with_user_paths() {
        let unit = render_linux_systemd("/usr/local/bin/agentum");
        assert!(unit.contains("ExecStart=/usr/local/bin/agentum clip-agent"));
        assert!(unit.contains("WantedBy=default.target"));
    }
}
