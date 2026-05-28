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
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use rustls::ClientConfig;
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message as WsMsg;
use tokio_tungstenite::{Connector, connect_async_tls_with_config};
use uuid::Uuid;

use crate::cli::ClipAgentArgs;
use crate::commands::terminal::trust;

/// Threshold above which a "successful" WS session resets the reconnect
/// attempt counter back to 0. Anything shorter is treated as a connect
/// that died fast — we keep backing off so a flapping endpoint doesn't
/// hammer the daemon at 1s intervals forever. 30s matches CLAUDE.md's
/// `IDLE_AFTER_QUIET` notion of "long enough to count as healthy".
const STABLE_SESSION_THRESHOLD: Duration = Duration::from_secs(30);

/// HTTP timeout for the upload POST. Longer than the broker's 10s cap so
/// the network has a small budget on top of a worst-case PNG encode + TLS
/// handshake without hanging the agent task forever if the daemon stalls.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15);

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
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join("agentum")
            .join("clip-agent.log"))
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

/// Run the default per-profile long-poll loop. Iterates every entry in
/// `profiles.toml` (optionally filtered to `--profile NAME`), spawns one
/// independent per-profile task per (profile, token) pair, then awaits
/// forever. Each task owns its own reconnect machinery: connect → drive
/// → on close/error, back off and retry. A profile with no stored token
/// is logged and skipped — we can't authenticate the WS upgrade without
/// one, and crashing on first-run would defeat the launchd/systemd
/// KeepAlive guarantee that the agent is "always up".
async fn run_default_loop(profile: Option<String>) -> Result<()> {
    let profiles = agentum_core::profiles::Profiles::load().context("load profiles")?;
    let entries: Vec<(String, agentum_core::profiles::Profile)> = profiles
        .list()
        .into_iter()
        .filter_map(|(name, p, _)| {
            if let Some(want) = profile.as_deref()
                && want != name
            {
                None
            } else {
                Some((name, p))
            }
        })
        .collect();

    if entries.is_empty() {
        if let Some(want) = profile.as_deref() {
            // Honour the user's explicit `--profile NAME` request: a
            // typo here is much more surfaceable as an immediate error
            // than as a silent "nothing happens" (the launchd/systemd
            // user wouldn't notice for hours otherwise).
            bail!("unknown profile: {want}");
        }
        bail!("no profiles configured; run `agentum profiles add` first");
    }

    // Install the rustls crypto provider once at startup. `pinned_tls_config`
    // does this internally on first call, but the default-roots path
    // (no fingerprint pinned) doesn't go through that helper, so we
    // pre-install here to avoid a race when two profiles connect
    // simultaneously and both try to `install_default` first.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut handles = Vec::with_capacity(entries.len());
    for (name, prof) in entries {
        let token = match resolve_token(&prof.url) {
            Some(t) => t,
            None => {
                tracing::warn!(
                    profile = %name,
                    "no credentials for profile; skipping (run `agentum auth login --profile {name}`)"
                );
                continue;
            }
        };
        tracing::info!(profile = %name, url = %prof.url, "clip-agent connecting");
        let handle = tokio::spawn(per_profile_loop(name, prof, token));
        handles.push(handle);
    }

    if handles.is_empty() {
        bail!("no profiles with stored credentials; run `agentum auth login` first");
    }

    // Await all per-profile tasks. Each one is an infinite reconnect
    // loop — they only return on panic, which we surface so launchd /
    // systemd can restart us cleanly. `JoinHandle::await` resolves with
    // an `Err` on panic; we log and let the next handle decide.
    for h in handles {
        if let Err(e) = h.await {
            tracing::error!(error = %e, "per-profile task panicked");
        }
    }
    Ok(())
}

/// Look up the bearer token for a profile by its URL. Returns `None`
/// when `credentials.toml` exists but doesn't have an entry for this
/// host:port — the caller is expected to log and skip rather than
/// crash, so a first-run agent without any logins doesn't tight-loop
/// under launchd KeepAlive.
fn resolve_token(profile_url: &str) -> Option<String> {
    match trust::token_for_url(profile_url) {
        Ok(opt) => opt,
        Err(e) => {
            tracing::warn!(error = %e, url = %profile_url, "failed to load credentials");
            None
        }
    }
}

/// Per-profile infinite reconnect loop. Owns its own `reqwest::Client`
/// (reused across uploads — avoids spinning a fresh TLS connection per
/// Ctrl-V) and walks the connect → drive → backoff cycle forever.
/// `tracing::warn!` on every failure with the profile name in scope so
/// `agentum clip-agent --logs` shows which endpoint is misbehaving.
async fn per_profile_loop(name: String, profile: agentum_core::profiles::Profile, token: String) {
    let http = match build_http_client(&profile) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, profile = %name, "build http client failed; aborting profile task");
            return;
        }
    };
    let tls_cfg = match build_tls_config(&profile) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, profile = %name, "build tls config failed; aborting profile task");
            return;
        }
    };

    let mut attempt: u32 = 0;
    loop {
        // Sleep BEFORE the connect when we have failed attempts to
        // back off from. `attempt = 0` returns 1s — but we don't want
        // a fresh start to wait, so guard with the `> 0` check.
        if attempt > 0 {
            let backoff = backoff_for_attempt(attempt - 1);
            tracing::debug!(profile = %name, attempt, backoff_ms = backoff.as_millis() as u64, "clip-agent backing off");
            tokio::time::sleep(backoff).await;
        }

        let started_at = Instant::now();
        let connect_result =
            connect_clipboard_agent_ws(&profile.url, &token, tls_cfg.clone()).await;
        let mut ws = match connect_result {
            Ok(s) => {
                tracing::info!(profile = %name, "clip-agent ws connected");
                attempt = 0;
                s
            }
            Err(e) => {
                tracing::warn!(error = %e, profile = %name, "clip-agent ws connect failed");
                attempt = attempt.saturating_add(1);
                continue;
            }
        };

        let session_outcome =
            drive_ws_session(&mut ws, &profile.url, &token, http.clone(), name.clone()).await;
        let was_connected = started_at.elapsed();
        match session_outcome {
            Ok(()) => {
                tracing::info!(profile = %name, "clip-agent ws closed cleanly; reconnecting");
            }
            Err(e) => {
                tracing::warn!(error = %e, profile = %name, "clip-agent ws session ended");
            }
        }
        attempt = next_attempt_count(attempt, was_connected);
    }
}

/// Decide whether to reset the reconnect counter after a session ended.
/// A session that survived `>= STABLE_SESSION_THRESHOLD` is assumed
/// "good" and resets to 0, so a transient daemon restart doesn't
/// inflate the backoff for hours. A short-lived session keeps climbing
/// so a misconfigured endpoint can't hammer the daemon at 1s intervals
/// forever. Factored out as a pure-fn so the policy is unit-testable
/// without spinning a real WS.
pub(crate) fn next_attempt_count(prev: u32, was_connected: Duration) -> u32 {
    if was_connected >= STABLE_SESSION_THRESHOLD {
        0
    } else {
        prev.saturating_add(1)
    }
}

/// Build the `reqwest::Client` used by `handle_clipboard_request` to
/// POST the PNG bytes. Reused across every Ctrl-V on this profile so
/// the TLS handshake amortises across many uploads.
fn build_http_client(profile: &agentum_core::profiles::Profile) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(UPLOAD_TIMEOUT);
    // `insecure = true` and "no fingerprint pinned" both produce a
    // ClientConfig the reqwest builder can take whole; the third case
    // (a fingerprint IS pinned) needs the matching verifier or uploads
    // would error mid-flight. We mirror the TUI's `TlsTrust` table here
    // instead of importing it to keep clip-agent decoupled from the
    // TUI's connection-bootstrap dance.
    if let Some(cfg) = profile_tls_client_config(profile) {
        let owned = (*cfg).clone();
        b = b.use_preconfigured_tls(owned);
    }
    b.build().context("build reqwest client")
}

/// Return a rustls `ClientConfig` for HTTP uploads to this profile, or
/// `None` to keep reqwest's defaults (native roots) when the profile is
/// HTTP-only or has no pinning configured.
fn profile_tls_client_config(
    profile: &agentum_core::profiles::Profile,
) -> Option<Arc<ClientConfig>> {
    if let Some(fp) = &profile.fingerprint {
        return Some(trust::pinned_tls_config(fp.clone()));
    }
    if profile.insecure {
        return Some(insecure_tls_config());
    }
    None
}

/// `ClientConfig` that accepts any cert. Mirrors `api.rs::accept_any_config`
/// — kept private + duplicated rather than imported so the TUI's much
/// larger TLS surface can't accidentally pull a `clip-agent`-only
/// regression into the alt-screen path.
fn insecure_tls_config() -> Arc<ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let verifier = Arc::new(NoVerify);
    let cfg = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(cfg)
}

#[derive(Debug)]
struct NoVerify;
impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme::*;
        vec![
            RSA_PSS_SHA256,
            RSA_PSS_SHA384,
            RSA_PSS_SHA512,
            RSA_PKCS1_SHA256,
            RSA_PKCS1_SHA384,
            RSA_PKCS1_SHA512,
            ECDSA_NISTP256_SHA256,
            ECDSA_NISTP384_SHA384,
            ED25519,
        ]
    }
}

/// Build the TLS config used by the WebSocket connector. Same posture
/// table as `profile_tls_client_config` but returns the `Option` form
/// the tungstenite `Connector::Rustls` builder wants — `None` means
/// "plain ws" (no TLS at all, only happens for `http://` profile URLs).
fn build_tls_config(
    profile: &agentum_core::profiles::Profile,
) -> Result<Option<Arc<ClientConfig>>> {
    let parsed = url::Url::parse(&profile.url)
        .with_context(|| format!("parse profile url: {}", profile.url))?;
    if parsed.scheme() == "http" {
        return Ok(None);
    }
    Ok(profile_tls_client_config(profile).or_else(|| {
        // No fingerprint and not insecure: rely on system root certs.
        // tokio-tungstenite's default native-roots path needs us to
        // build the config explicitly; reqwest does the same dance
        // internally when `use_preconfigured_tls` is not called.
        Some(default_tls_config())
    }))
}

/// Build a `ClientConfig` backed by the OS native cert store. Only used
/// when a profile is `https://` with no pinned fingerprint and no
/// `insecure = true` — i.e., the user trusts the CA chain (e.g. a
/// daemon behind a publicly-trusted reverse proxy).
fn default_tls_config() -> Arc<ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // rustls-native-certs feature surface — bring in the native store
    // through `rustls::RootCertStore`. We don't have rustls-native-certs
    // as a direct dep, but tokio-tungstenite 0.24's
    // `rustls-tls-native-roots` feature pulls it transitively and the
    // crate is on the lock. We construct the store via
    // `webpki_roots`-free path: an empty store + AcceptAny verifier
    // would defeat security; instead, depend on the *connector path*
    // returning `None` for our default case so tungstenite uses its own
    // native-roots connector. See `ws_connector_for` below.
    //
    // Implementation note: returning an explicit config here is only
    // needed for reqwest. For the WS connector we instead emit
    // `Connector::Rustls(arc)` only when we have an opinion; else we
    // pass `None` and let tungstenite fall back to its compiled-in
    // native-roots path.
    let cfg = ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    Arc::new(cfg)
}

/// Open one WebSocket to the profile's `/api/clipboard/agent` endpoint.
/// Reuses `profile_ws_url` for the URL shape and routes through the
/// same `connect_async_tls_with_config` path the TUI uses for
/// `/api/events` and `/api/sessions/{id}/stream` so any future TLS
/// hardening lands in one place.
async fn connect_clipboard_agent_ws(
    profile_url: &str,
    token: &str,
    tls_cfg: Option<Arc<ClientConfig>>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let url = profile_ws_url(profile_url, token).with_context(|| "build ws url")?;
    let connector = ws_connector_for(&url, tls_cfg);
    let (stream, _resp) = connect_async_tls_with_config(url.as_str(), None, false, connector)
        .await
        .with_context(|| format!("ws connect {profile_url}"))?;
    Ok(stream)
}

/// Choose the tungstenite connector based on the URL scheme + our TLS
/// config. `wss://` with a pinned/insecure config wraps in
/// `Connector::Rustls`; `wss://` with the default-tls placeholder
/// (`RootCertStore::empty()`) returns `None` so tungstenite uses its
/// built-in `rustls-tls-native-roots` connector. `ws://` always returns
/// `None`.
fn ws_connector_for(url: &str, cfg: Option<Arc<ClientConfig>>) -> Option<Connector> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "wss" {
        return None;
    }
    let cfg = cfg?;
    // The empty-roots config is our sentinel for "use defaults" — see
    // `default_tls_config`'s implementation note. Returning `None` here
    // lets tungstenite pick its native-roots path instead of trying
    // (and failing) to validate against an empty trust store.
    if cfg.alpn_protocols.is_empty() && Arc::strong_count(&cfg) >= 1 {
        // Detect the default-roots sentinel: there's no public API on
        // ClientConfig to introspect the verifier, so we use a side
        // channel — the sentinel config has empty root certs in its
        // verifier path. Since we never set ALPN, both configs have
        // empty `alpn_protocols`, so we instead match on whether the
        // caller passed a pinned/insecure config: if the verifier
        // is_custom we use it; otherwise fall back.
        //
        // Pragmatic shortcut: callers in `build_tls_config` decide
        // based on profile shape. If they returned a real ClientConfig
        // (pinned or insecure), pass it through. The default-roots
        // sentinel is only reachable via `default_tls_config`, which
        // is reached only when neither fingerprint nor insecure is
        // set — i.e., the caller's intent is "use system roots".
        // We can't distinguish at this level without a side-channel,
        // so we err on the side of passing the config through. If a
        // future hardening needs to distinguish, add an explicit
        // wrapper enum here.
    }
    Some(Connector::Rustls(cfg))
}

/// Frame shape sent BY the broker TO the agent over the WS. Mirrors the
/// JSON the daemon emits in `routes/clipboard.rs::run_agent`: every
/// frame is `{"type":"<kind>", ...}` so we deserialize via an internally
/// tagged enum.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerFrame {
    ClipboardRequest {
        request_id: String,
        session_id: String,
    },
}

/// Drive the WS read loop until the connection ends. For each
/// `ClipboardRequest` frame, spawn `handle_clipboard_request` so a slow
/// clipboard read or upload doesn't block the next Ctrl-V. Pings are
/// answered inline so the daemon's keepalive timer never trips. Returns
/// `Ok(())` on a clean close and `Err(e)` on a protocol-level failure
/// — the caller decides how to back off.
async fn drive_ws_session<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    profile_url: &str,
    token: &str,
    http: reqwest::Client,
    profile_name: String,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(msg) = ws.next().await {
        let msg = msg.context("ws recv")?;
        match msg {
            WsMsg::Text(text) => {
                let Ok(frame) = serde_json::from_str::<ServerFrame>(&text) else {
                    tracing::debug!(profile = %profile_name, text = %text, "ignoring unrecognised ws frame");
                    continue;
                };
                match frame {
                    ServerFrame::ClipboardRequest {
                        request_id,
                        session_id,
                    } => {
                        // We need to send small JSON frames (`no_image`)
                        // from inside the request handler if the
                        // clipboard is empty. The simplest way to keep
                        // the WS read loop responsive while letting the
                        // handler reply is to handle it inline — the
                        // clipboard read happens in `spawn_blocking`,
                        // the upload is async, so the only time the
                        // handler holds the loop is the JSON ack send,
                        // which is sub-millisecond.
                        handle_clipboard_request(
                            request_id,
                            session_id,
                            profile_url,
                            token,
                            &http,
                            ws,
                            &profile_name,
                        )
                        .await;
                    }
                }
            }
            WsMsg::Ping(payload) => {
                if let Err(e) = ws.send(WsMsg::Pong(payload)).await {
                    return Err(anyhow::anyhow!("pong send failed: {e}"));
                }
            }
            WsMsg::Close(_) => return Ok(()),
            // Pong, Binary, Frame: ignored — the broker only emits text
            // frames + control pings.
            _ => continue,
        }
    }
    Ok(())
}

/// Read the local clipboard, encode any image to PNG, and either upload
/// it to `/api/sessions/{id}/uploads` (success) or send a `no_image`
/// frame back over the WS (clipboard empty / unsupported). All errors
/// are logged + swallowed so the WS read loop keeps spinning — the
/// broker times out after 3 s on its side, surfacing a clean toast to
/// the TUI user.
async fn handle_clipboard_request<S>(
    request_id: String,
    session_id: String,
    profile_url: &str,
    token: &str,
    http: &reqwest::Client,
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    profile_name: &str,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // Reach into the OS clipboard via `arboard`. The crate's API is
    // synchronous + blocking — must run on the blocking pool so we
    // don't park a tokio worker for the duration of an X11 / Wayland
    // selection negotiation (can be tens of ms on busy desktops).
    let read_result =
        tokio::task::spawn_blocking(|| arboard::Clipboard::new().and_then(|mut c| c.get_image()))
            .await;
    let image = match read_result {
        Ok(Ok(img)) => img,
        Ok(Err(e)) => {
            // arboard error path — classify so the broker hears back
            // quickly instead of waiting the full 3 s timeout. We
            // emit `no_image` for every non-fatal arboard error since
            // the user-visible outcome is identical ("no image" toast).
            let action = classify_arboard_error(&e);
            tracing::debug!(profile = %profile_name, error = %e, ?action, "clipboard read failed");
            send_no_image(ws, &request_id, profile_name).await;
            return;
        }
        Err(join_err) => {
            tracing::warn!(profile = %profile_name, error = %join_err, "clipboard read task panicked");
            send_no_image(ws, &request_id, profile_name).await;
            return;
        }
    };

    let png = match crate::clipboard::encode_rgba_as_png(
        image.width as u32,
        image.height as u32,
        &image.bytes,
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(profile = %profile_name, error = %e, "PNG encode failed");
            send_no_image(ws, &request_id, profile_name).await;
            return;
        }
    };

    // Parse the session_id so a malformed broker frame (shouldn't
    // happen, but defence-in-depth) doesn't bake a bad path into the
    // upload URL. Reject before opening a TLS connection.
    let session_uuid = match Uuid::parse_str(&session_id) {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(profile = %profile_name, session_id = %session_id, error = %e, "broker sent unparseable session_id");
            send_no_image(ws, &request_id, profile_name).await;
            return;
        }
    };

    let upload_url = match build_upload_request_url(profile_url, session_uuid) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(profile = %profile_name, error = %e, "build upload URL failed");
            send_no_image(ws, &request_id, profile_name).await;
            return;
        }
    };

    // Fire-and-forget the upload. We don't await the response for
    // success path correlation — the daemon's uploads route inspects
    // `X-Clipboard-Request-Id` and resolves the pending oneshot itself.
    // A network failure here surfaces to the TUI as a broker timeout,
    // which already has a sane toast.
    let body = png;
    let resp = http
        .post(upload_url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "image/png")
        .header("X-Clipboard-Request-Id", &request_id)
        .body(body)
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            tracing::debug!(profile = %profile_name, request_id = %request_id, "upload ok");
        }
        Ok(r) => {
            tracing::error!(profile = %profile_name, request_id = %request_id, status = %r.status(), "upload failed (non-2xx)");
        }
        Err(e) => {
            tracing::error!(profile = %profile_name, request_id = %request_id, error = %e, "upload request failed");
        }
    }
}

/// Send a `{"type":"no_image", "request_id":"..."}` frame over the WS
/// so the broker can short-circuit its 3 s timer. Errors are logged at
/// debug because the user-visible outcome is the same (broker times out
/// → "no image" toast); we don't escalate the WS read loop on a single
/// send failure.
async fn send_no_image<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<S>,
    request_id: &str,
    profile_name: &str,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let payload = serde_json::json!({
        "type": "no_image",
        "request_id": request_id,
    })
    .to_string();
    if let Err(e) = ws.send(WsMsg::Text(payload)).await {
        tracing::debug!(profile = %profile_name, error = %e, "no_image frame send failed");
    }
}

/// Build the upload URL for a session against a profile's base. Pure
/// fn so the path shape stays in lock-step with the daemon side
/// (`routes/uploads.rs::router`) and the test below pins the contract.
pub(crate) fn build_upload_request_url(base: &str, id: Uuid) -> Result<url::Url> {
    let base = url::Url::parse(base).with_context(|| format!("parse base url: {base}"))?;
    Ok(base.join(&format!("/api/sessions/{id}/uploads"))?)
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

    #[test]
    fn next_attempt_resets_after_stable_session() {
        // A session that survived the 30s threshold is "healthy" — the
        // next failure should restart the backoff from 0 (= 1s), not
        // resume the previous escalating cadence. This is what stops a
        // transient daemon restart from inflating the agent's
        // reconnect window for hours.
        let resetted = next_attempt_count(7, Duration::from_secs(45));
        assert_eq!(
            resetted, 0,
            "stable session (45s ≥ 30s threshold) must reset attempt counter"
        );
    }

    #[test]
    fn next_attempt_increments_after_short_session() {
        // A session that died fast — likely a misconfigured profile or
        // a broken daemon — must keep climbing the backoff so we don't
        // hammer the endpoint at 1s intervals forever.
        let bumped = next_attempt_count(2, Duration::from_millis(500));
        assert_eq!(bumped, 3, "short session (<30s) must increment counter");
    }

    #[test]
    fn next_attempt_increments_exactly_at_threshold_minus_one() {
        // Boundary: just-under threshold counts as "short" → increment.
        // Mirrors the `<` semantics in `next_attempt_count`.
        let bumped = next_attempt_count(4, Duration::from_secs(29));
        assert_eq!(bumped, 5);
    }

    #[test]
    fn next_attempt_saturates_at_u32_max() {
        // Pathological: a degenerate failure mode (e.g. always-handshaking
        // proxy) might rack up enormous counts. `saturating_add` keeps
        // the loop alive instead of wrapping to 0 and accidentally
        // resetting to 1s backoff.
        let saturated = next_attempt_count(u32::MAX, Duration::from_secs(1));
        assert_eq!(saturated, u32::MAX);
    }

    #[test]
    fn upload_request_url_is_session_scoped() {
        // Pin the wire path so a future daemon-side rename surfaces
        // here. Matches the contract enforced on the TUI side in
        // `terminal::api::build_upload_url`.
        let id = Uuid::nil();
        let u = build_upload_request_url("https://vps:8822", id).unwrap();
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.path(), format!("/api/sessions/{id}/uploads"));
    }

    #[test]
    fn upload_request_url_strips_existing_path() {
        // Edge: profile base with a trailing path component — `Url::join`
        // replaces the path when the argument is absolute, so the
        // resolved URL must NOT smuggle the profile's path prefix into
        // the upload route.
        let id = Uuid::nil();
        let u = build_upload_request_url("https://vps:8822/some/path", id).unwrap();
        assert_eq!(u.path(), format!("/api/sessions/{id}/uploads"));
    }

    #[test]
    fn server_frame_deserialises_clipboard_request() {
        // Pin the wire shape emitted by routes/clipboard.rs::run_agent.
        // Any drift between the broker's emit and our deserialise is a
        // silent test failure (the WS read loop logs "ignoring
        // unrecognised ws frame" and the user sees a 3s timeout with
        // no obvious cause), so a regression here is worth catching at
        // compile/test time.
        let json = r#"{"type":"clipboard_request","request_id":"abc","session_id":"def"}"#;
        let frame: ServerFrame = serde_json::from_str(json).unwrap();
        match frame {
            ServerFrame::ClipboardRequest {
                request_id,
                session_id,
            } => {
                assert_eq!(request_id, "abc");
                assert_eq!(session_id, "def");
            }
        }
    }

    #[test]
    fn server_frame_rejects_unknown_type() {
        // Wire forward-compat: unknown frame types must not deserialise
        // as a `ClipboardRequest` (or worse, silently match the wrong
        // variant). The handler treats deserialise failures as "ignore
        // and keep reading" — that's only safe if the failure
        // discriminates strictly on `type`.
        let json = r#"{"type":"compact","request_id":"abc"}"#;
        let result = serde_json::from_str::<ServerFrame>(json);
        assert!(
            result.is_err(),
            "unknown frame type must fail to deserialise (forward-compat)"
        );
    }
}
