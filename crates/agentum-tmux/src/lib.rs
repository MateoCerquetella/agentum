//! tmux subprocess adapter.
//!
//! Every method shells out to `tmux` via `tokio::process::Command` with one
//! `.arg()` per argument — no shell-string interpolation in our process
//! invocation. The single shell-command string we pass to
//! `tmux new-session` / `tmux pipe-pane` is safely quoted with [`shlex`].

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::time::sleep;

#[derive(Debug, thiserror::Error)]
pub enum TmuxError {
    #[error("tmux exited with status {status:?} (stderr: {stderr})")]
    NonZero { status: i32, stderr: String },
    #[error("tmux output was not valid utf-8")]
    NotUtf8(#[from] std::string::FromUtf8Error),
    #[error("could not parse tmux output: {0}")]
    Parse(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("could not shell-quote command components")]
    Quote,
}

pub type Result<T> = std::result::Result<T, TmuxError>;

/// Returns the tmux session name for an agentum session.
pub fn target_for(name: &str) -> String {
    format!("agentum-{name}")
}

/// `tmux has-session -t <target>` → bool. Non-zero exit means "no such session".
pub async fn has_session(target: &str) -> Result<bool> {
    let status = Command::new("tmux")
        .arg("has-session")
        .arg("-t")
        .arg(format!("={target}")) // exact-match to avoid prefix collisions
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;
    Ok(status.success())
}

/// Spawn a detached tmux session running `cmd` (as argv) in `workdir`.
///
/// `env` entries are forwarded as `-e KEY=VAL` to tmux so the spawned shell
/// inherits them. Workdir must exist on disk.
pub async fn new_session(
    target: &str,
    workdir: &Path,
    cmd: &[String],
    env: &[(String, String)],
) -> Result<()> {
    let cmd_str = shlex::try_join(cmd.iter().map(String::as_str)).map_err(|_| TmuxError::Quote)?;

    let mut c = Command::new("tmux");
    c.arg("new-session")
        .arg("-d")
        .arg("-s")
        .arg(target)
        .arg("-c")
        .arg(workdir);
    for (k, v) in env {
        c.arg("-e").arg(format!("{k}={v}"));
    }
    c.arg(cmd_str);

    run_checked(&mut c).await
}

/// `tmux kill-session -t <target>`. Idempotent — non-existent target returns Ok.
pub async fn kill_session(target: &str) -> Result<()> {
    if !has_session(target).await? {
        return Ok(());
    }
    let mut c = Command::new("tmux");
    c.arg("kill-session").arg("-t").arg(target);
    run_checked(&mut c).await
}

/// Capture last `lines` of pane content as plain text (no ANSI escapes —
/// suitable for regex matching by the watchdog).
pub async fn capture_pane(target: &str, lines: usize) -> Result<String> {
    let start = format!("-{lines}");
    let out = Command::new("tmux")
        .args(["capture-pane", "-p", "-S", &start, "-t"])
        .arg(target)
        .output()
        .await?;
    check(&out)?;
    Ok(String::from_utf8(out.stdout)?)
}

/// Send raw key spec (e.g. "C-c", "Enter") or text to a pane.
/// `append_enter` adds a trailing Enter, useful for chat-style input bars.
pub async fn send_keys(target: &str, keys: &str, append_enter: bool) -> Result<()> {
    let mut c = Command::new("tmux");
    c.arg("send-keys").arg("-t").arg(target).arg(keys);
    if append_enter {
        c.arg("Enter");
    }
    run_checked(&mut c).await
}

/// Pipe the pane's output to `out_path` (append). Uses `-o`: noop if a pipe
/// is already active for this pane.
///
/// tmux interprets the shell-command via `/bin/sh -c`, so `>>` is the shell's
/// append-redirect operator. Only the path is shell-quoted.
pub async fn pipe_pane(target: &str, out_path: &Path) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let path_str = out_path
        .to_str()
        .ok_or_else(|| TmuxError::Parse("non-utf8 log path".into()))?;
    let quoted_path = shlex::try_quote(path_str).map_err(|_| TmuxError::Quote)?;
    let cmd_str = format!("cat >> {quoted_path}");

    let mut c = Command::new("tmux");
    c.arg("pipe-pane")
        .arg("-o")
        .arg("-t")
        .arg(target)
        .arg(cmd_str);
    run_checked(&mut c).await
}

/// PID of the foreground process inside the pane.
pub async fn pane_pid(target: &str) -> Result<u32> {
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(target)
        .arg("#{pane_pid}")
        .output()
        .await?;
    check(&out)?;
    let s = String::from_utf8(out.stdout)?;
    s.trim()
        .parse()
        .map_err(|e: std::num::ParseIntError| TmuxError::Parse(e.to_string()))
}

/// Send SIGTERM to the pane's process; if it's still alive after `timeout`,
/// SIGKILL it. Then `kill-session` cleans up tmux state. Idempotent.
pub async fn graceful_stop(target: &str, timeout: Duration) -> Result<()> {
    if !has_session(target).await? {
        return Ok(());
    }
    // pane gone between checks → just skip the signal phase.
    let pid = pane_pid(target).await.ok();

    if let Some(pid) = pid {
        let _ = signal(pid, "TERM").await;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !is_alive(pid).await {
                break;
            }
            sleep(Duration::from_millis(200)).await;
        }
        if is_alive(pid).await {
            tracing::warn!(pid, "process did not exit after SIGTERM; sending SIGKILL");
            let _ = signal(pid, "KILL").await;
        }
    }

    kill_session(target).await
}

async fn signal(pid: u32, sig: &str) -> Result<()> {
    let mut c = Command::new("kill");
    c.arg(format!("-{sig}")).arg(pid.to_string());
    run_checked(&mut c).await
}

async fn is_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn run_checked(c: &mut Command) -> Result<()> {
    let out = c.output().await?;
    check(&out)
}

fn check(out: &std::process::Output) -> Result<()> {
    if out.status.success() {
        Ok(())
    } else {
        Err(TmuxError::NonZero {
            status: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_format() {
        assert_eq!(target_for("alpha"), "agentum-alpha");
    }

    #[tokio::test]
    async fn lifecycle_smoke() {
        // Skip if tmux isn't available in CI.
        if Command::new("tmux").arg("-V").status().await.is_err() {
            return;
        }
        let target = "agentum-test-smoke";
        let _ = kill_session(target).await;

        let workdir = std::env::temp_dir();
        new_session(target, &workdir, &["sleep".into(), "3600".into()], &[])
            .await
            .unwrap();

        assert!(has_session(target).await.unwrap());
        let pid = pane_pid(target).await.unwrap();
        assert!(pid > 0);

        kill_session(target).await.unwrap();
        assert!(!has_session(target).await.unwrap());
    }
}
