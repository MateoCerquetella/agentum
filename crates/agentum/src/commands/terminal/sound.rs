//! System-sound playback for TUI notifications.
//!
//! Fire-and-forget: spawns a platform-native player (`paplay`/`pw-play`
//! on Linux, `afplay` on macOS) via `tokio::process::Command`. The
//! returned `Child` is moved into a detached `tokio::spawn` so the OS
//! reaps it on natural exit and we never block the run loop. If no
//! player is on `PATH`, falls back to writing the BEL byte (`\x07`) so
//! the user still gets a host-terminal alert.
//!
//! No new Cargo deps — keeps the cc-rs build path empty.

use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;

use super::app::NotifKind;

/// Cached "is any player available?" probe. Avoids the spawn-then-fail
/// cost on every notification when the box has no audio stack.
static PLAYER: OnceLock<Option<&'static str>> = OnceLock::new();

#[cfg(target_os = "linux")]
fn detect_player() -> Option<&'static str> {
    ["paplay", "pw-play"].into_iter().find(|c| which(c))
}

#[cfg(target_os = "macos")]
fn detect_player() -> Option<&'static str> {
    if which("afplay") {
        Some("afplay")
    } else {
        None
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_player() -> Option<&'static str> {
    None
}

fn which(bin: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    let sep = if cfg!(windows) { ';' } else { ':' };
    path.split(sep)
        .filter(|p| !p.is_empty())
        .any(|p| Path::new(p).join(bin).is_file())
}

#[cfg(target_os = "linux")]
fn asset_for(kind: NotifKind) -> &'static str {
    match kind {
        NotifKind::Error => "/usr/share/sounds/freedesktop/stereo/dialog-error.oga",
        NotifKind::Warn => "/usr/share/sounds/freedesktop/stereo/dialog-warning.oga",
        NotifKind::Info => "/usr/share/sounds/freedesktop/stereo/dialog-information.oga",
    }
}

#[cfg(target_os = "macos")]
fn asset_for(kind: NotifKind) -> &'static str {
    match kind {
        NotifKind::Error => "/System/Library/Sounds/Sosumi.aiff",
        NotifKind::Warn => "/System/Library/Sounds/Funk.aiff",
        NotifKind::Info => "/System/Library/Sounds/Glass.aiff",
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn asset_for(_: NotifKind) -> &'static str {
    ""
}

/// Play the system sound for `kind`. Non-blocking. Silently no-ops on
/// any error — sound is a nice-to-have, never load-bearing.
pub fn play(kind: NotifKind) {
    let player = *PLAYER.get_or_init(detect_player);
    let Some(bin) = player else {
        bell();
        return;
    };
    let asset = asset_for(kind);
    if asset.is_empty() || !Path::new(asset).exists() {
        bell();
        return;
    }

    match tokio::process::Command::new(bin)
        .arg(asset)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false)
        .spawn()
    {
        Ok(mut child) => {
            // Reap the zombie on natural exit. `kill_on_drop(false)` so
            // the player keeps running past the next tick.
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(_) => bell(),
    }
}

/// BEL fallback. Disabled in v0.6.36+ — same reason `write_osc52` is
/// disabled (see `app::write_osc52`): a raw byte to stdout *while
/// ratatui owns the screen* bypasses the diff renderer, splits a
/// neighbouring escape sequence at the byte boundary, and the host
/// terminal (especially inside tmux) can drop into a "swallowing
/// parameters until I see a final byte" state — which on some
/// emulators presents as the alt-screen going entirely black until
/// the next full repaint. Reinstate by routing through a between-
/// frames flush queue, not by writing to `std::io::stdout()` directly.
fn bell() {}
