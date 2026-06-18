//! Login-shell PATH hydration for the GUI launch path.
//!
//! macOS (Finder/Dock) and Linux (.desktop/AppImage) launch GUI apps with a
//! minimal PATH — on macOS that's `/usr/bin:/bin:/usr/sbin:/sbin` (plus
//! `/etc/paths.d`), NOT the PATH the user's shell builds in `.zshrc` /
//! `config.fish` / `.profile` / a node version-manager (fnm, nvm, asdf). Agent
//! CLIs (`claude`, `cursor-agent`, `codex`, `gemini`, …) and `gh`/`glab` almost
//! always live in `~/.local/bin`, Homebrew, or a version-manager shim dir —
//! none of which are on that minimal PATH.
//!
//! The embedded agentum-server probes those CLIs with `which` (see
//! `agentum-server/src/routes/preflight.rs`). Launched from a terminal
//! (`tauri dev`, `agentum serve`) the process already has the full shell PATH,
//! so detection works. Launched as a packaged `.app` it does not, so detection
//! reports "No agents detected" even though every CLI is installed — they still
//! *launch* fine because tmux runs them through the user's login shell.
//!
//! Fix: read the login shell's PATH once at boot and merge it into the process
//! env before the server starts, so detection — and every child process (tmux,
//! PTYs, git/gh/glab) — sees the same tools the terminal does.

use std::process::Command;
use std::time::Duration;

/// Hydrate the process `PATH` from the user's login shell. No-op on Windows
/// (GUI processes there inherit the registry PATH correctly), or whenever the
/// login-shell read fails — in which case behavior is exactly as before (the
/// minimal GUI PATH), so this can never regress a working launch.
pub fn hydrate_path_from_login_shell() {
    let Some(login) = login_shell_path() else {
        return;
    };
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_paths(&login, &current);
    if merged != current {
        // Affects this process and every child it spawns: the embedded server's
        // `which` probes, tmux/PTY launches, and git/gh/glab calls. Safe to set
        // here — we run before the server boots and before any thread reads PATH
        // (edition 2021: `set_var` is not `unsafe`).
        std::env::set_var("PATH", merged);
    }
}

/// Read the PATH an interactive login shell produces. Returns `None` on any
/// failure (no `$SHELL`, spawn error, timeout, empty result) so the caller
/// no-ops rather than clobbering a usable PATH.
fn login_shell_path() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty())?;

    // Run the login shell, then have it exec `env` and read the `PATH=` line.
    // Why `env` instead of `echo $PATH`: PATH is always an exported env var, so
    // its child `env` prints it colon-separated regardless of shell — fish, for
    // one, stores PATH as a space-separated list and `echo $PATH` would yield
    // the wrong delimiter. `-l -i -c` (separate flags, not `-ilc`, for fish
    // compatibility) sources both login files (`.zprofile`/`.bash_profile`/
    // `config.fish`) and interactive files (`.zshrc`, where many users set PATH).
    //
    // Run on a thread with a timeout: a misbehaving rc file (a prompt, a `read`)
    // must not freeze app startup. On timeout we fall back to the current PATH.
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let result = Command::new(&shell)
            .args(["-l", "-i", "-c", "env"])
            .output()
            .ok();
        let _ = tx.send(result);
    });
    let output = rx.recv_timeout(Duration::from_secs(3)).ok()??;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("PATH="))?
        .trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Merge `login` PATH segments ahead of `current`, deduplicating and dropping
/// empty segments while preserving order. Login segments come first so the
/// process resolves binaries the same way the user's terminal does. Pure and
/// deterministic — unit-tested without spawning a shell.
fn merge_paths(login: &str, current: &str) -> String {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut out: Vec<&str> = Vec::new();
    for seg in login.split(sep).chain(current.split(sep)) {
        if seg.is_empty() {
            continue;
        }
        // O(n²) but PATH has only a few dozen segments — not worth a HashSet,
        // and a Vec keeps the result order stable for tests.
        if !out.contains(&seg) {
            out.push(seg);
        }
    }
    out.join(&sep.to_string())
}

#[cfg(test)]
mod tests {
    use super::merge_paths;

    #[test]
    fn login_segments_come_first() {
        assert_eq!(
            merge_paths("/home/u/.local/bin", "/usr/bin:/bin"),
            "/home/u/.local/bin:/usr/bin:/bin"
        );
    }

    #[test]
    fn dedups_overlap_preserving_login_precedence() {
        // /b appears in both; it stays where login put it, not duplicated.
        assert_eq!(merge_paths("/a:/b", "/b:/c"), "/a:/b:/c");
    }

    #[test]
    fn empty_login_keeps_current() {
        assert_eq!(merge_paths("", "/usr/bin:/bin"), "/usr/bin:/bin");
    }

    #[test]
    fn empty_current_keeps_login() {
        assert_eq!(merge_paths("/opt/homebrew/bin", ""), "/opt/homebrew/bin");
    }

    #[test]
    fn skips_empty_segments_from_doubled_separators() {
        assert_eq!(merge_paths("/a::/b", ""), "/a:/b");
    }
}
