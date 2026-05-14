use std::net::SocketAddr;

use agentum_core::Status;
use agentum_server::ServeOptions;
use anyhow::Result;

pub async fn run(
    addr: SocketAddr,
    cert_addr: SocketAddr,
    tls: bool,
    no_resume: bool,
    detach: bool,
    no_auth: bool,
) -> Result<()> {
    if detach {
        daemonize()?;
    }

    let (store, db_path) = super::open_store().await?;
    tracing::info!(?db_path, %addr, %cert_addr, tls, no_auth, "store opened");

    if no_auth {
        tracing::warn!("--no-auth: authentication disabled — do NOT expose this to untrusted networks");
    }

    // Boot everything: resume stopped/idle sessions that have a known tool.
    if !no_resume {
        resume_sessions(&store).await;
    }

    agentum_server::serve(
        ServeOptions {
            addr,
            cert_addr,
            tls,
            no_auth,
        },
        store,
    )
    .await
}

/// Daemonize the current process: double-fork, detach from the
/// controlling terminal, and redirect stdio to a log file so the
/// daemon survives the parent shell exiting.
///
/// After this call returns successfully, the parent process has
/// already exited. The remaining code runs in a fully detached
/// child (grandchild of the original process).
fn daemonize() -> Result<()> {
    #[cfg(not(unix))]
    {
        anyhow::bail!("--detach is only supported on Unix (Linux/macOS)");
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        // Resolve the log path: $XDG_STATE_HOME/agentum/daemon.log
        // falling back to ~/.local/state/agentum/daemon.log
        let log_dir = std::env::var_os("XDG_STATE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".local/state"))
            })
            .map(|p| p.join("agentum"))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/agentum"));
        std::fs::create_dir_all(&log_dir)?;
        let log_path = log_dir.join("daemon.log");

        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        // Re-exec ourselves with the same arguments *minus* --detach.
        // This avoids tricky in-process fork+setsid from within a
        // Tokio runtime, which can deadlock or corrupt state.
        let exe = std::env::current_exe()?;
        let args: Vec<String> = std::env::args()
            .filter(|a| a != "--detach" && a != "-d")
            .collect();

        let child = Command::new(&exe)
            .args(&args[1..]) // skip argv[0] (the binary name)
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone()?)
            .stderr(log_file)
            .process_group(0) // new process group, detached from terminal
            .spawn()?;

        let pid = child.id();
        eprintln!("Daemon started (pid {pid}). Logs: {}", log_path.display());
        eprintln!("Dashboard: https://127.0.0.1:8822");
        // Parent exits immediately; the child continues as the daemon.
        std::process::exit(0);
    }
}

/// Bring up any sessions that aren't currently running.
async fn resume_sessions(store: &agentum_store::Store) {
    let sessions = match store.list_sessions(None).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("could not list sessions for auto-resume: {e}");
            return;
        }
    };

    let to_resume: Vec<_> = sessions
        .into_iter()
        .filter(|s| matches!(s.status, Status::Idle | Status::Stopped))
        .collect();

    if to_resume.is_empty() {
        return;
    }

    tracing::info!(count = to_resume.len(), "resuming sessions");

    for session in to_resume {
        let name = session.name.clone();
        match crate::commands::up::run(name.clone()).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(session = %name, "could not resume: {e}");
            }
        }
    }
}
