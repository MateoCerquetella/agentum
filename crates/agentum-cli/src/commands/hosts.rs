//! `agentum hosts` — manage SSH-agentless hosts plus the legacy
//! SSH-style known_hosts file for remote Agentum server certificates.

use agentum_core::{HostKind, NewHost, SshAuth};
use anyhow::Result;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const SSH_TEST_TIMEOUT: Duration = Duration::from_secs(12);

use crate::cli::HostsCmd;
use crate::commands::terminal::trust;

pub async fn run(action: HostsCmd) -> Result<()> {
    match action {
        HostsCmd::List => list().await,
        HostsCmd::Add {
            name,
            user,
            hostname,
            port,
            key,
        } => add(name, user, hostname, port, key).await,
        HostsCmd::Test { name } => test(name).await,
        HostsCmd::Rm { name } => remove(name).await,
        HostsCmd::Forget { host } => forget(&host).await,
    }
}

async fn list() -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let hosts = store.list_hosts().await?;
    if hosts.is_empty() {
        println!("no hosts defined");
        return Ok(());
    }
    println!("{:<18}  {:<7}  TARGET", "NAME", "KIND");
    for h in hosts {
        match h.kind {
            HostKind::Local => println!("{:<18}  {:<7}  this machine", h.name, "local"),
            HostKind::Ssh {
                user,
                hostname,
                port,
                auth,
            } => {
                let auth = match auth {
                    SshAuth::Agent => "agent".to_string(),
                    SshAuth::Key { path } => format!("key={path}"),
                };
                println!(
                    "{:<18}  {:<7}  {}@{}:{} ({})",
                    h.name, "ssh", user, hostname, port, auth
                );
            }
        }
    }
    Ok(())
}

async fn add(
    name: String,
    user: String,
    hostname: String,
    port: u16,
    key: Option<String>,
) -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let auth = key
        .filter(|p| !p.trim().is_empty())
        .map(|path| SshAuth::Key { path })
        .unwrap_or(SshAuth::Agent);
    let host = store
        .create_host(NewHost {
            name: name.clone(),
            kind: HostKind::Ssh {
                user,
                hostname,
                port,
                auth,
            },
        })
        .await?;
    println!("saved host `{}` ({})", host.name, host.id);
    println!("test with: agentum hosts test {}", name);
    Ok(())
}

async fn test(name: String) -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let host = store
        .list_hosts()
        .await?
        .into_iter()
        .find(|h| h.name == name)
        .ok_or_else(|| anyhow::anyhow!("no host named `{name}`"))?;
    match host.kind {
        HostKind::Local => {
            println!(
                "local host: tmux={} git={}",
                which::which("tmux").is_ok(),
                which::which("git").is_ok()
            );
        }
        HostKind::Ssh {
            user,
            hostname,
            port,
            auth,
        } => {
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
                cmd.arg("-i").arg(path);
            }
            let output = timeout(
                SSH_TEST_TIMEOUT,
                cmd.arg(format!("{user}@{hostname}"))
                    .arg("command -v tmux >/dev/null && command -v git >/dev/null && uname -sr")
                    .output(),
            )
            .await
            .map_err(|_| anyhow::anyhow!("ssh test timed out"))??;
            if output.status.success() {
                println!("ok: {}", String::from_utf8_lossy(&output.stdout).trim());
                store.update_host_seen(host.id).await?;
            } else {
                anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
            }
        }
    }
    Ok(())
}

async fn remove(name: String) -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let host = store
        .list_hosts()
        .await?
        .into_iter()
        .find(|h| h.name == name)
        .ok_or_else(|| anyhow::anyhow!("no host named `{name}`"))?;
    if store.delete_host(host.id).await? {
        println!("removed host `{name}`");
    }
    Ok(())
}

#[allow(dead_code)]
async fn list_known_hosts() -> Result<()> {
    let known = trust::KnownHosts::load()?;
    let entries: Vec<(String, String)> = known.entries().collect();
    if entries.is_empty() {
        println!(
            "(no pinned hosts — `agentum terminal --api https://…` will prompt on first contact)"
        );
        return Ok(());
    }
    for (host, fp) in entries {
        println!("{host:<32}  {fp}");
    }
    Ok(())
}

async fn forget(host: &str) -> Result<()> {
    let mut known = trust::KnownHosts::load()?;
    let dropped_pin = known.remove(host)?;

    let mut creds = trust::Credentials::load()?;
    let dropped_token = creds.remove(host)?;

    match (dropped_pin, dropped_token) {
        (true, true) => println!("forgot pin and cached login for {host}"),
        (true, false) => println!("forgot pin for {host} (no cached login was set)"),
        (false, true) => println!("dropped cached login for {host} (no pin was set)"),
        (false, false) => println!("nothing to forget for {host}"),
    }
    Ok(())
}
