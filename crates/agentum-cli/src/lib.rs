//! Library shim that exposes the CLI plumbing so multiple binaries
//! (`agentum`, `lazyagentum`) can share it.

pub mod cli;
pub mod clipboard;
pub mod commands;

use std::path::Path;

pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("AGENTUM_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,hyper=warn,h2=warn,tower_http=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

/// Tracing init for the TUI process. Writes to a log file under
/// `$XDG_CACHE_HOME/agentum/tui.log` (append-only) instead of stderr,
/// because stderr shares the TTY with the alt-screen ratatui owns:
/// any `tracing::info!` from a dependency lands directly on the
/// rendered cells and scrambles the diff renderer's view of the
/// screen. On some emulators (especially inside tmux) a single
/// misplaced escape leaves the alt-screen completely black until the
/// next full repaint. Skips subscriber init entirely if the cache
/// dir can't be created — better silent than corrupting the screen.
pub fn init_tracing_for_tui() {
    let Ok(dir) = agentum_store::paths::cache_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    init_tracing_to_file(&dir.join("tui.log"));
}

/// Tracing init for the `agentum clip-agent` subcommand. Mirrors
/// `init_tracing_for_tui` but takes a caller-supplied path so the
/// macOS/Linux log locations can diverge (`~/Library/Logs/agentum`
/// vs `$XDG_CACHE_HOME/agentum`) without forcing every subcommand
/// to negotiate that here.
pub fn init_tracing_for_clip_agent(log_path: &Path) {
    init_tracing_to_file(log_path);
}

/// Append-only file tracing subscriber. Private helper shared by the
/// TUI and clip-agent init paths to keep their behaviour identical.
/// Skips init entirely on any I/O error — silent is preferable to
/// crashing the alt-screen or daemonised process at boot.
fn init_tracing_to_file(log_path: &Path) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("AGENTUM_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,sqlx=warn,hyper=warn,h2=warn"));
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .compact()
        .try_init();
}
