//! Host-aware tmux operations + the shared SSH connection builder.
//!
//! The watchdog and the server both need to sample tmux panes on a session's
//! host, which may be `Local` (run tmux directly) or `Ssh` (run tmux over an
//! `ssh` connection). The connection builder ([`ssh_command`]) and the small
//! set of read/poll ops the watchdog uses live here, in the shared lower
//! crate, so the watchdog can be host-aware without depending on
//! `agentum-server` (which depends on the watchdog — the dependency only runs
//! one way).
//!
//! `agentum-server`'s `host_runtime` imports [`ssh_command`] from here rather
//! than re-deriving the argv, so there is a single source of truth for the
//! `ssh`/`sshpass` flags (BatchMode / ConnectTimeout / StrictHostKeyChecking /
//! auth handling).

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use agentum_core::{Host, HostKind, SshAuth};
use tokio::process::Command;
use tokio::time::timeout;

use crate::{Result, TmuxError};

/// Matches `host_runtime`'s probe budget so a hung remote can't wedge the
/// watchdog's tick loop (`tokio::time::timeout` bounds every SSH round trip).
const SSH_TIMEOUT: Duration = Duration::from_secs(12);

/// Directory holding the OpenSSH ControlMaster sockets. Kept short (unix socket
/// paths cap at ~104 chars and `%C` is already a fixed-length hash of
/// host+port+user) and created on demand. We ignore an `AlreadyExists` error so
/// concurrent ops racing to create it don't fail; any other error is swallowed
/// — ssh just falls back to a fresh connection per op rather than panicking.
fn control_path_template() -> String {
    let dir: PathBuf = std::env::temp_dir().join("agentum-ssh");
    // Best-effort: if this fails, ControlPath points at a missing dir and ssh
    // simply doesn't multiplex (it logs and connects directly). Never panic.
    let _ = std::fs::create_dir_all(&dir);
    dir.join("agentum-cm-%C").to_string_lossy().into_owned()
}

/// Build the `ssh`/`sshpass` argv for running `script` on `host`. Returns a
/// plain tokio [`Command`]; the caller drives `.output()` / `.status()`.
///
/// This is the single source of truth for our SSH connection flags. Moved
/// verbatim from `agentum-server::host_runtime` so the watchdog (which can't
/// depend on the server) and the server share one builder.
pub fn ssh_command(host: &Host, script: &str) -> Command {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        auth,
    } = &host.kind
    else {
        return Command::new("false");
    };

    // Password auth shells through `sshpass`, which answers ssh's password
    // prompt. That requires BatchMode=no (BatchMode=yes suppresses the
    // prompt entirely, so sshpass would have nothing to answer). Key/agent
    // auth keeps BatchMode=yes so it never blocks waiting on a prompt.
    let password = match auth {
        SshAuth::Password { password } if !password.trim().is_empty() => Some(password.as_str()),
        _ => None,
    };

    let mut cmd = match password {
        Some(pw) => {
            let mut c = Command::new("sshpass");
            c.arg("-p").arg(pw).arg("ssh");
            c
        }
        None => Command::new("ssh"),
    };

    cmd.arg("-o")
        .arg(if password.is_some() {
            "BatchMode=no"
        } else {
            "BatchMode=yes"
        })
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
        // ControlMaster connection pooling: the first op authenticates and opens
        // a master socket; subsequent ops within ControlPersist reuse it, skipping
        // the TCP+auth handshake entirely. This is the big remote-latency win —
        // each tmux/git/fs round trip would otherwise pay a full SSH handshake.
        // Applies to both key/agent and password (sshpass) auth.
        .arg("-o")
        .arg("ControlMaster=auto")
        .arg("-o")
        .arg(format!("ControlPath={}", control_path_template()))
        .arg("-o")
        .arg("ControlPersist=30s")
        .arg("-p")
        .arg(port.to_string());

    match auth {
        SshAuth::Key { path } if !path.trim().is_empty() => {
            cmd.arg("-i").arg(path);
        }
        SshAuth::Password { .. } => {
            // Force password auth so ssh doesn't silently try a key first
            // (which would bypass sshpass and fail confusingly).
            cmd.arg("-o")
                .arg("PreferredAuthentications=password")
                .arg("-o")
                .arg("PubkeyAuthentication=no");
        }
        _ => {}
    }

    cmd.arg(format!("{user}@{hostname}")).arg(script);
    cmd
}

/// shell-quote `s`, mapping a quoting failure to [`TmuxError::Quote`].
fn q(s: &str) -> Result<Cow<'_, str>> {
    shlex::try_quote(s).map_err(|_| TmuxError::Quote)
}

/// Run `script` over SSH and return its stdout, erroring on a non-zero exit
/// or timeout. Mirrors `host_runtime::ssh_stdout`.
async fn ssh_stdout(host: &Host, script: &str) -> Result<String> {
    let output = timeout(SSH_TIMEOUT, ssh_command(host, script).output())
        .await
        .map_err(|_| TmuxError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "ssh timed out")))??;
    if !output.status.success() {
        return Err(TmuxError::NonZero {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Run `script` over SSH for its exit status only, erroring on transport
/// failure or timeout (a non-zero remote exit is reported via the bool/`()`
/// the caller maps from `success`).
async fn ssh_status(host: &Host, script: &str) -> Result<std::process::ExitStatus> {
    timeout(SSH_TIMEOUT, ssh_command(host, script).status())
        .await
        .map_err(|_| {
            TmuxError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "ssh timed out",
            ))
        })?
        .map_err(TmuxError::Io)
}

/// Run `script` over SSH and error on a non-zero exit (or transport
/// failure / timeout). Mirrors `host_runtime::ssh_checked`.
async fn ssh_checked(host: &Host, script: &str) -> Result<()> {
    let output = timeout(SSH_TIMEOUT, ssh_command(host, script).output())
        .await
        .map_err(|_| {
            TmuxError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "ssh timed out",
            ))
        })??;
    if output.status.success() {
        Ok(())
    } else {
        Err(TmuxError::NonZero {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

// ───────────────────────── host-aware tmux ops ─────────────────────────
// Each branches on `host.kind`: Local calls the existing `crate::<fn>`
// (identical behaviour to before the refactor); Ssh runs the same tmux
// command wrapped in `sh -c` (the remote login shell may be fish/zsh, which
// reject the POSIX `for`/`case`/quoting the SSH branches build).

/// `tmux has-session` on `host`. Non-zero remote exit means "no such session".
pub async fn has_session(host: &Host, target: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => crate::has_session(target).await,
        HostKind::Ssh { .. } => {
            let script = format!("tmux has-session -t {}", q(&format!("={target}"))?);
            Ok(ssh_status(host, &script).await?.success())
        }
    }
}

/// Capture the last `lines` of `target`'s pane (incl. scrollback) as plain
/// text on `host`.
pub async fn capture_pane(host: &Host, target: &str, lines: usize) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::capture_pane(target, lines).await,
        HostKind::Ssh { .. } => {
            ssh_stdout(
                host,
                &format!("tmux capture-pane -p -S -{lines} -t {}", q(target)?),
            )
            .await
        }
    }
}

/// Capture only the currently-visible viewport of `target` (no scrollback)
/// as plain text on `host`.
pub async fn capture_pane_visible(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::capture_pane_visible(target).await,
        HostKind::Ssh { .. } => {
            ssh_stdout(
                host,
                &format!("tmux capture-pane -p -S 0 -t {}", q(target)?),
            )
            .await
        }
    }
}

/// Send `keys` (a tmux key spec or text) to `target` on `host`, optionally
/// appending Enter.
pub async fn send_keys(host: &Host, target: &str, keys: &str, append_enter: bool) -> Result<()> {
    match &host.kind {
        HostKind::Local => crate::send_keys(target, keys, append_enter).await,
        HostKind::Ssh { .. } => {
            let mut script = format!("tmux send-keys -t {} {}", q(target)?, q(keys)?);
            if append_enter {
                script.push_str(" Enter");
            }
            ssh_checked(host, &script).await
        }
    }
}

/// Basename of the foreground process inside `target`'s pane
/// (`#{pane_current_command}`) on `host`, trimmed.
pub async fn pane_current_command(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::pane_current_command(target).await,
        HostKind::Ssh { .. } => {
            let out = ssh_stdout(
                host,
                &format!(
                    "tmux display-message -p -t {} '#{{pane_current_command}}'",
                    q(target)?
                ),
            )
            .await?;
            Ok(out.trim().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_host(auth: SshAuth) -> Host {
        Host {
            id: agentum_core::LOCAL_HOST_ID,
            name: "t".into(),
            kind: HostKind::Ssh {
                user: "me".into(),
                hostname: "box.local".into(),
                port: 2222,
                auth,
            },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    // `ssh_command` returns a tokio Command; `.as_std()` exposes the inner
    // std Command for introspecting program + args.
    fn arg_strings(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn ssh_command_key_uses_plain_ssh_with_batchmode() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
        let args = arg_strings(&cmd);
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.iter().any(|a| a == "me@box.local"));
        // Key/agent must never reach for sshpass options.
        assert!(!args.contains(&"PreferredAuthentications=password".to_string()));
    }

    /// ControlMaster pooling must be present on every connection so repeated
    /// remote ops reuse one authenticated socket. Shared assertion for both auth
    /// paths (key/agent and password).
    fn assert_control_master(args: &[String]) {
        assert!(
            args.contains(&"ControlMaster=auto".to_string()),
            "missing ControlMaster=auto: {args:?}"
        );
        assert!(
            args.contains(&"ControlPersist=30s".to_string()),
            "missing ControlPersist=30s: {args:?}"
        );
        let control_path = args
            .iter()
            .find(|a| a.starts_with("ControlPath="))
            .unwrap_or_else(|| panic!("missing ControlPath=: {args:?}"));
        // `%C` is a fixed-length host+port+user hash; keep the socket name short
        // so the unix socket path stays under the ~104-char cap.
        assert!(
            control_path.ends_with("agentum-cm-%C"),
            "unexpected ControlPath: {control_path}"
        );
        // The socket dir must exist (we create it on demand) — strip the
        // `ControlPath=` prefix and the `/agentum-cm-%C` leaf.
        let path = control_path.trim_start_matches("ControlPath=");
        let dir = std::path::Path::new(path).parent().expect("control dir");
        assert!(dir.is_dir(), "control dir not created: {}", dir.display());
    }

    #[test]
    fn ssh_command_key_enables_control_master_pooling() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert_control_master(&arg_strings(&cmd));
    }

    #[test]
    fn ssh_command_password_shells_through_sshpass() {
        let cmd = ssh_command(
            &ssh_host(SshAuth::Password {
                password: "s3cret".into(),
            }),
            "echo hi",
        );
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "sshpass");
        let args = arg_strings(&cmd);
        // `sshpass -p <pw> ssh …`
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], "s3cret");
        assert_eq!(args[2], "ssh");
        // Password auth must NOT use BatchMode=yes (it suppresses the
        // prompt sshpass needs to answer) and must force password auth.
        assert!(args.contains(&"BatchMode=no".to_string()));
        assert!(!args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"PreferredAuthentications=password".to_string()));
        assert!(args.iter().any(|a| a == "me@box.local"));
        // Pooling applies to the sshpass path too.
        assert_control_master(&args);
    }

    #[test]
    fn control_path_template_creates_dir_idempotently() {
        // Calling twice must not panic even though the dir already exists.
        let first = control_path_template();
        let second = control_path_template();
        assert_eq!(first, second);
        let dir = std::path::Path::new(&first).parent().expect("dir");
        assert!(dir.is_dir());
    }
}
