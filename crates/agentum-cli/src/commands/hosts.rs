//! `agentum hosts` — manage SSH-agentless hosts plus the legacy
//! SSH-style known_hosts file for remote Agentum server certificates.

use agentum_core::{Host, HostKind, HostReadiness, NewHost, SshAuth};
use anyhow::Result;

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
        HostsCmd::Readiness { name } => readiness(name).await,
        HostsCmd::Bootstrap { name, yes } => bootstrap(name, yes).await,
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
    println!();
    // Run readiness in-process: the CLI runs on the same machine as the
    // local daemon (the box that SSHes to the host), so the user sees the
    // full dependency report immediately on add — no running `agentum
    // serve` required.
    let report = agentum_server::host_runtime::readiness(&host).await;
    print_readiness(&host, &report);
    if report.ok {
        let _ = store.update_host_seen(host.id).await;
    } else {
        // US-1: non-zero exit when a required dependency is missing.
        std::process::exit(1);
    }
    Ok(())
}

/// `agentum hosts test <name>` — concise, script-friendly one-line
/// summary. For the full per-dependency table use `hosts readiness`.
async fn test(name: String) -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let host = find_host(&store, &name).await?;
    let report = agentum_server::host_runtime::readiness(&host).await;
    if report.ok {
        let agents = report.agents.iter().filter(|a| a.installed).count();
        println!(
            "{}: ready · {agents} agent CLI{} available",
            host.name,
            if agents == 1 { "" } else { "s" }
        );
        let _ = store.update_host_seen(host.id).await;
    } else {
        println!("{}: not ready — {}", host.name, report.message);
        std::process::exit(1);
    }
    Ok(())
}

/// `agentum hosts readiness <name>` — full dependency report.
async fn readiness(name: String) -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let host = find_host(&store, &name).await?;
    let report = agentum_server::host_runtime::readiness(&host).await;
    print_readiness(&host, &report);
    if report.ok {
        let _ = store.update_host_seen(host.id).await;
    } else {
        std::process::exit(1);
    }
    Ok(())
}

/// `agentum hosts bootstrap <name> [--yes]` — install the missing
/// required deps (tmux/git) via the host's package manager. Runs the
/// install in-process (CLI is co-located with the daemon). Prompts unless
/// `--yes`. PRD US-4.
async fn bootstrap(name: String, yes: bool) -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let host = find_host(&store, &name).await?;

    // Probe first so we only install what's actually missing — and so a
    // fully-ready host short-circuits without a sudo round trip.
    let pre = agentum_server::host_runtime::readiness(&host).await;
    let missing: Vec<String> = pre
        .required
        .iter()
        .filter(|d| !d.installed && d.bootstrapable)
        .map(|d| d.id.clone())
        .collect();
    if missing.is_empty() {
        println!("{}: required deps already installed", host.name);
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!(
            "Install {} on `{}` (sudo)? [y/N] ",
            missing.join(" + "),
            host.name
        );
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    println!("installing {} on `{}`…", missing.join(" + "), host.name);
    match agentum_server::host_runtime::bootstrap(&host, &missing).await {
        Ok(report) => {
            println!();
            print_readiness(&host, &report);
            if report.ok {
                let _ = store.update_host_seen(host.id).await;
            } else {
                std::process::exit(1);
            }
        }
        Err(e) => anyhow::bail!("bootstrap failed: {e}"),
    }
    Ok(())
}

async fn find_host(store: &agentum_store::Store, name: &str) -> Result<Host> {
    store
        .list_hosts()
        .await?
        .into_iter()
        .find(|h| h.name == name)
        .ok_or_else(|| anyhow::anyhow!("no host named `{name}`"))
}

/// Render a readiness report as a Hermes-style table (PRD §7.5).
fn print_readiness(host: &Host, report: &HostReadiness) {
    println!("Host: {} ({})", host.name, host_target_label(host));
    let uname = report.system.uname.as_deref().unwrap_or("unknown");
    println!(
        "System: {uname} · pkg_manager={}",
        report.system.pkg_manager
    );
    println!();

    println!("REQUIRED");
    for dep in &report.required {
        print_dep_row(&dep.label, dep.installed, dep.install_hint.as_deref());
    }
    println!();

    println!("AGENTS (optional)");
    for agent in &report.agents {
        print_dep_row(&agent.id, agent.installed, agent.install_hint.as_deref());
    }
    println!();

    if report.ok {
        println!("Ready: yes");
    } else if report.system.uname.is_none() {
        // Unreachable host — `message` carries the SSH error; the table
        // above is all "missing" because nothing could be verified.
        println!("Ready: no — {}", report.message);
    } else {
        let missing = report.required.iter().filter(|d| !d.installed).count();
        println!("Ready: no ({missing} required missing)");
    }
}

fn print_dep_row(label: &str, installed: bool, hint: Option<&str>) {
    let mark = if installed { "[x]" } else { "[ ]" };
    match hint {
        Some(h) if !installed => println!("  {mark} {label:<10} — {h}"),
        _ => println!("  {mark} {label}"),
    }
}

fn host_target_label(host: &Host) -> String {
    match &host.kind {
        HostKind::Local => "this machine".to_string(),
        HostKind::Ssh {
            user,
            hostname,
            port,
            ..
        } => format!("ssh {user}@{hostname}:{port}"),
    }
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
