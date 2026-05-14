//! `agentum uninstall` — remove the binary + everything it wrote on disk.
//!
//! Intentionally noisy: lists every path that will be touched, demands
//! confirmation by default, and skips paths that don't exist instead
//! of failing. On Linux it also disables a systemd user unit if one
//! was installed by the wizard. Does *not* touch profiles by default
//! (those are user data) unless `--all` is passed.

use anyhow::{Context, Result};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct Options {
    /// Skip the y/N confirmation prompt.
    pub yes: bool,
    /// Also remove profile + credential files (otherwise kept so a
    /// reinstall picks up where you left off).
    pub all: bool,
    /// Print what would be removed without removing anything.
    pub dry_run: bool,
}

pub async fn run(opts: Options) -> Result<()> {
    let targets = gather_targets(opts.all)?;
    let present: Vec<&Target> = targets.iter().filter(|t| t.path.exists()).collect();

    if present.is_empty() {
        println!("nothing to uninstall — no agentum files found on this machine.");
        return Ok(());
    }

    println!("agentum uninstall — the following will be removed:");
    println!();
    for t in &present {
        println!(
            "  {} {}",
            if t.is_dir() { "[dir] " } else { "[file]" },
            t.path.display()
        );
        if let Some(note) = t.note {
            println!("         └─ {note}");
        }
    }
    println!();

    if opts.dry_run {
        println!("dry-run: nothing removed. Re-run without --dry-run to apply.");
        return Ok(());
    }

    if !opts.yes && !confirm("Remove all of the above? [y/N]: ")? {
        println!("aborted.");
        return Ok(());
    }

    // Stop a running daemon if we can find one. Best-effort — if the
    // unit lives somewhere we don't recognise, we still wipe the
    // files; the user's next reboot will clean up the orphan.
    stop_running_daemon();

    let mut removed = 0usize;
    let mut failed: Vec<(PathBuf, std::io::Error)> = Vec::new();
    for t in &present {
        let result = if t.is_dir() {
            std::fs::remove_dir_all(&t.path)
        } else {
            std::fs::remove_file(&t.path)
        };
        match result {
            Ok(()) => {
                removed += 1;
                println!("  removed {}", t.path.display());
            }
            Err(e) => failed.push((t.path.clone(), e)),
        }
    }

    println!();
    println!("done: removed {removed} item(s).");
    if !failed.is_empty() {
        println!();
        eprintln!("the following could not be removed:");
        for (p, e) in &failed {
            eprintln!("  {} — {e}", p.display());
        }
        anyhow::bail!("uninstall finished with {} error(s)", failed.len());
    }

    // Trailing PATH hint: the binary often lives in $HOME/.local/bin
    // which the user added to PATH at install time. We don't touch
    // shell rc files (too easy to corrupt) but we do mention it.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            println!();
            println!(
                "note: {} may still be on your PATH if you added it manually.",
                parent.display()
            );
            println!("      no shell rc files were modified.");
        }
    }

    Ok(())
}

struct Target {
    path: PathBuf,
    kind: Kind,
    note: Option<&'static str>,
}

enum Kind {
    File,
    Dir,
}

impl Target {
    fn file(path: PathBuf, note: Option<&'static str>) -> Self {
        Self {
            path,
            kind: Kind::File,
            note,
        }
    }
    fn dir(path: PathBuf, note: Option<&'static str>) -> Self {
        Self {
            path,
            kind: Kind::Dir,
            note,
        }
    }
    fn is_dir(&self) -> bool {
        matches!(self.kind, Kind::Dir)
    }
}

fn gather_targets(include_user_data: bool) -> Result<Vec<Target>> {
    let mut t = Vec::new();

    // Binary on disk. current_exe() points at the running binary —
    // that's *us*. We intentionally schedule it for removal too;
    // POSIX unlink-while-open works, and on macOS the kernel keeps
    // the file mapped until we exit.
    if let Ok(exe) = std::env::current_exe() {
        t.push(Target::file(exe, Some("the agentum binary itself")));
    }

    // Data dir holds the SQLite DB, auth token, TLS cert/key.
    if let Ok(p) = agentum_store::paths::data_dir() {
        t.push(Target::dir(p, Some("database, auth token, TLS material")));
    }

    // Cache dir — pane logs and other regenerable scratch.
    if let Ok(p) = agentum_store::paths::cache_dir() {
        t.push(Target::dir(p, Some("pane logs and cached data")));
    }

    // State dir — daemon log file lives here on Linux. On macOS the
    // directories crate falls back to data_dir, so this may collide
    // and that's fine: remove_dir_all is idempotent on a missing path.
    if let Ok(p) = agentum_store::paths::state_dir() {
        t.push(Target::dir(p, Some("daemon logs")));
    }

    // Config dir contains the canonical profiles.toml. We treat it
    // as user data — keep it unless --all was passed so that a
    // reinstall picks up the user's remote profiles.
    if include_user_data {
        if let Ok(p) = agentum_store::paths::config_dir() {
            t.push(Target::dir(p, Some("profiles + credentials (user data)")));
        }
    }

    // systemd user unit, if the installer wrote one.
    if cfg!(target_os = "linux") {
        let unit = dirs_systemd_unit();
        if let Some(p) = unit {
            t.push(Target::file(p, Some("systemd user unit (autostart)")));
        }
    }

    Ok(t)
}

#[cfg(target_os = "linux")]
fn dirs_systemd_unit() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs_home_join(".config"))?;
    Some(base.join("systemd/user/agentum.service"))
}

#[cfg(not(target_os = "linux"))]
fn dirs_systemd_unit() -> Option<PathBuf> {
    None
}

#[allow(dead_code)]
fn dirs_home_join(rel: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(rel))
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush().context("flush stdout")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("read confirmation")?;
    let ans = line.trim().to_ascii_lowercase();
    Ok(matches!(ans.as_str(), "y" | "yes"))
}

// Best-effort daemon shutdown: stop+disable the systemd unit on
// Linux, then kill any lingering `agentum serve` process. Failures
// are silent — at worst the user has an orphaned process that goes
// away on next reboot, and the file removal still succeeds.
fn stop_running_daemon() {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", "agentum.service"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "agentum.service"])
            .status();
    }

    // Cross-platform: pkill any `agentum serve` we own.
    let _ = std::process::Command::new("pkill")
        .args(["-f", "agentum serve"])
        .status();
}

#[allow(dead_code)]
fn looks_like_agentum_path(p: &Path) -> bool {
    // Belt-and-braces guard for future refactors that might pass a
    // user-supplied path through gather_targets. Every path we ship
    // contains the literal "agentum" in its last few components.
    p.components()
        .rev()
        .take(3)
        .any(|c| c.as_os_str().to_string_lossy().contains("agentum"))
}
