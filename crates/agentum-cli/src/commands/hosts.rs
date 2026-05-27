//! `agentum hosts` — manage the SSH-style known_hosts file used by the
//! TUI / dashboard CLI client to verify remote agentum servers.

use anyhow::Result;

use crate::cli::HostsCmd;
use crate::commands::terminal::trust;

pub async fn run(action: HostsCmd) -> Result<()> {
    match action {
        HostsCmd::List => list().await,
        HostsCmd::Forget { host } => forget(&host).await,
    }
}

async fn list() -> Result<()> {
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
