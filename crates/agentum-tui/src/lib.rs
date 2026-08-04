//! Support library for the `agentum` terminal application.

pub mod clipboard;
pub mod commands;

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

/// Append-only file tracing subscriber.
/// Skips init entirely on any I/O error — silent is preferable to
/// crashing the alt-screen or daemonised process at boot.
fn init_tracing_to_file(log_path: &std::path::Path) {
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
