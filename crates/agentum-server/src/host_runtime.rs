//! Local/SSH host execution helpers.
//!
//! The SSH backend intentionally drives only stock `ssh` + `tmux` on the
//! remote machine. The remote host never needs an `agentum` binary.

use std::path::Path;
use std::time::{Duration, Instant};

use agentum_core::{Host, HostKind, SshAuth};
use tokio::process::Command;
use tokio::time::{sleep, timeout};

const SSH_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, thiserror::Error)]
pub enum HostRuntimeError {
    #[error("unsupported operation on host kind")]
    Unsupported,
    #[error("ssh/tmux exited with status {status:?} (stderr: {stderr})")]
    NonZero { status: Option<i32>, stderr: String },
    #[error("output was not valid utf-8")]
    NotUtf8(#[from] std::string::FromUtf8Error),
    #[error("could not shell-quote remote command")]
    Quote,
    #[error("ssh command timed out")]
    Timeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tmux(#[from] agentum_tmux::TmuxError),
}

pub type Result<T> = std::result::Result<T, HostRuntimeError>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct HostProbe {
    pub ok: bool,
    pub message: String,
    pub uname: Option<String>,
    pub tmux: bool,
    pub git: bool,
}

pub async fn probe(host: &Host) -> HostProbe {
    match &host.kind {
        HostKind::Local => {
            let tmux = which::which("tmux").is_ok();
            let git = which::which("git").is_ok();
            HostProbe {
                ok: tmux && git,
                message: if tmux && git {
                    "local host ready".into()
                } else {
                    "local host is missing tmux or git".into()
                },
                uname: std::process::Command::new("uname")
                    .arg("-sr")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                tmux,
                git,
            }
        }
        HostKind::Ssh { .. } => match ssh_stdout(host, "command -v tmux >/dev/null; tmux_ok=$?; command -v git >/dev/null; git_ok=$?; uname -sr; exit $((tmux_ok || git_ok))").await {
            Ok(out) => HostProbe {
                ok: true,
                message: "ssh host ready".into(),
                uname: Some(out.trim().to_string()).filter(|s| !s.is_empty()),
                tmux: true,
                git: true,
            },
            Err(e) => HostProbe {
                ok: false,
                message: e.to_string(),
                uname: None,
                tmux: false,
                git: false,
            },
        },
    }
}

pub async fn has_session(host: &Host, target: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::has_session(target).await?),
        HostKind::Ssh { .. } => {
            let script = format!("tmux has-session -t {}", q(&format!("={target}"))?);
            let status = timeout(SSH_TIMEOUT, ssh_command(host, &script).status())
                .await
                .map_err(|_| HostRuntimeError::Timeout)??;
            Ok(status.success())
        }
    }
}

pub async fn new_session(
    host: &Host,
    target: &str,
    workdir: &Path,
    cmd: &[String],
    env: &[(String, String)],
) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::new_session(target, workdir, cmd, env).await?),
        HostKind::Ssh { .. } => {
            let cmd_str = shlex::try_join(cmd.iter().map(String::as_str))
                .map_err(|_| HostRuntimeError::Quote)?;
            let mut parts = vec![
                "tmux".to_string(),
                "new-session".to_string(),
                "-d".to_string(),
                "-s".to_string(),
                q(target)?.into_owned(),
                "-x".to_string(),
                agentum_tmux::DEFAULT_PANE_COLS.to_string(),
                "-y".to_string(),
                agentum_tmux::DEFAULT_PANE_ROWS.to_string(),
                "-c".to_string(),
                q(&workdir.to_string_lossy())?.into_owned(),
            ];
            for (k, v) in env {
                parts.push("-e".into());
                parts.push(q(&format!("{k}={v}"))?.into_owned());
            }
            parts.push(q(&cmd_str)?.into_owned());
            ssh_checked(host, &parts.join(" ")).await
        }
    }
}

pub async fn kill_session(host: &Host, target: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::kill_session(target).await?),
        HostKind::Ssh { .. } => {
            if !has_session(host, target).await? {
                return Ok(());
            }
            ssh_checked(host, &format!("tmux kill-session -t {}", q(target)?)).await
        }
    }
}

pub async fn graceful_stop(host: &Host, target: &str, timeout: Duration) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::graceful_stop(target, timeout).await?),
        HostKind::Ssh { .. } => {
            let _ = send_keys(host, target, "C-c", false).await;
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if !has_session(host, target).await? {
                    return Ok(());
                }
                sleep(Duration::from_millis(150)).await;
            }
            kill_session(host, target).await
        }
    }
}

pub async fn capture_pane_ansi(host: &Host, target: &str) -> Result<Vec<u8>> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::capture_pane_ansi(target).await?),
        HostKind::Ssh { .. } => {
            let out =
                ssh_stdout(host, &format!("tmux capture-pane -p -e -t {}", q(target)?)).await?;
            let mut buf = Vec::with_capacity(out.len() + 64);
            for line in out.split('\n') {
                buf.extend_from_slice(line.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Ok(buf)
        }
    }
}

pub async fn capture_pane_visible(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::capture_pane_visible(target).await?),
        HostKind::Ssh { .. } => {
            ssh_stdout(
                host,
                &format!("tmux capture-pane -p -S 0 -t {}", q(target)?),
            )
            .await
        }
    }
}

pub async fn send_keys(host: &Host, target: &str, keys: &str, append_enter: bool) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::send_keys(target, keys, append_enter).await?),
        HostKind::Ssh { .. } => {
            let mut script = format!("tmux send-keys -t {} {}", q(target)?, q(keys)?);
            if append_enter {
                script.push_str(" Enter");
            }
            ssh_checked(host, &script).await
        }
    }
}

pub async fn send_bytes(host: &Host, target: &str, bytes: &[u8]) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::send_bytes(target, bytes).await?),
        HostKind::Ssh { .. } => {
            for chunk in bytes.chunks(4096) {
                let mut script = format!("tmux send-keys -H -t {}", q(target)?);
                for b in chunk {
                    script.push(' ');
                    script.push_str(&format!("{b:02x}"));
                }
                ssh_checked(host, &script).await?;
            }
            Ok(())
        }
    }
}

pub async fn resize_window(host: &Host, target: &str, cols: u16, rows: u16) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::resize_window(target, cols, rows).await?),
        HostKind::Ssh { .. } => {
            let cols = cols.max(20);
            let rows = rows.max(5);
            let target = q(target)?;
            let script = format!(
                "tmux set-option -q -t {target} window-size manual; tmux resize-window -t {target} -x {cols} -y {rows}"
            );
            ssh_checked(host, &script).await
        }
    }
}

pub async fn pipe_pane(host: &Host, target: &str, out_path: &Path) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::pipe_pane(target, out_path).await?),
        HostKind::Ssh { .. } => {
            // Remote sessions stream via capture-pane polling in the local
            // daemon, so they don't need a remote pipe-pane sink.
            let _ = (target, out_path);
            Ok(())
        }
    }
}

pub async fn ssh_stdout(host: &Host, script: &str) -> Result<String> {
    let output = timeout(SSH_TIMEOUT, ssh_command(host, script).output())
        .await
        .map_err(|_| HostRuntimeError::Timeout)??;
    if !output.status.success() {
        return Err(HostRuntimeError::NonZero {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8(output.stdout)?)
}

async fn ssh_checked(host: &Host, script: &str) -> Result<()> {
    let output = timeout(SSH_TIMEOUT, ssh_command(host, script).output())
        .await
        .map_err(|_| HostRuntimeError::Timeout)??;
    if output.status.success() {
        Ok(())
    } else {
        Err(HostRuntimeError::NonZero {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn ssh_command(host: &Host, script: &str) -> Command {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        auth,
    } = &host.kind
    else {
        return Command::new("false");
    };

    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=8")
        .arg("-o")
        .arg("ConnectionAttempts=1")
        .arg("-o")
        .arg("ServerAliveInterval=5")
        .arg("-o")
        .arg("ServerAliveCountMax=1")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-p")
        .arg(port.to_string());
    if let SshAuth::Key { path } = auth {
        if !path.trim().is_empty() {
            cmd.arg("-i").arg(path);
        }
    }
    cmd.arg(format!("{user}@{hostname}")).arg(script);
    cmd
}

fn q(s: &str) -> Result<std::borrow::Cow<'_, str>> {
    shlex::try_quote(s).map_err(|_| HostRuntimeError::Quote)
}
