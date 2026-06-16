//! `agentum hosts` — manage SSH-agentless hosts plus the legacy
//! SSH-style known_hosts file for remote Agentum server certificates.

use agentum_core::{
    EXTERNAL_TMUX_FLAG, Host, HostKind, HostReadiness, LOCAL_HOST_ID, NewHost, Session, SshAuth,
    Status,
};
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
            yes,
        } => add(name, user, hostname, port, key, yes).await,
        HostsCmd::Setup { name, yes } => setup(name, yes).await,
        HostsCmd::Test { name } => test(name).await,
        HostsCmd::Readiness { name } => readiness(name).await,
        HostsCmd::Rm { name } => remove(name).await,
        HostsCmd::Forget { host } => forget(&host).await,
        HostsCmd::PruneTmux { name, yes } => prune_tmux(name, yes).await,
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
                    // Never print the password — just note the auth kind.
                    SshAuth::Password { .. } => "password".to_string(),
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
    yes: bool,
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
    // One install flow: check → install required deps → ask which agents
    // → install → done. Runs in-process (the CLI is co-located with the
    // local daemon that SSHes to the host), so no `agentum serve` needed.
    provision_flow(&store, &host, yes).await
}

/// `agentum hosts setup <name> [--yes]` — re-run the install flow on an
/// existing host.
async fn setup(name: String, yes: bool) -> Result<()> {
    let (store, _) = agentum_store::open_default().await?;
    let host = find_host(&store, &name).await?;
    provision_flow(&store, &host, yes).await
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

/// The one install flow shared by `add` and `setup`: probe → install the
/// required deps (tmux/git) → ask which agents to install → install them →
/// report the final readiness. `yes` installs everything missing without
/// prompting. Runs in-process against the local daemon's host primitives.
async fn provision_flow(store: &agentum_store::Store, host: &Host, yes: bool) -> Result<()> {
    let report = agentum_server::host_runtime::readiness(host).await;
    print_readiness(host, &report);

    // Unreachable host: nothing to install. Surface and stop.
    if report.system.uname.is_none() {
        anyhow::bail!("host unreachable — {}", report.message);
    }

    // 1) Required deps (tmux/git) — mandatory for the host to run sessions.
    let missing_deps: Vec<String> = report
        .required
        .iter()
        .filter(|d| !d.installed && d.bootstrapable)
        .map(|d| d.id.clone())
        .collect();
    if !missing_deps.is_empty() {
        let go = yes
            || confirm(&format!(
                "Install required {} on `{}` (sudo)?",
                missing_deps.join(" + "),
                host.name
            ))?;
        if go {
            println!("installing {} …", missing_deps.join(" + "));
            if let Err(e) = agentum_server::host_runtime::bootstrap(host, &missing_deps).await {
                eprintln!("  required-deps install failed: {e}");
            }
        } else {
            println!(
                "skipped — `{}` can't run sessions until tmux + git are installed",
                host.name
            );
        }
    }

    // 2) Agent CLIs — ask which to install (optional).
    let missing_agents: Vec<String> = report
        .agents
        .iter()
        .filter(|a| {
            !a.installed
                && agentum_server::host_install_hints::agent_install_command(&a.id).is_some()
        })
        .map(|a| a.id.clone())
        .collect();
    if !missing_agents.is_empty() {
        let targets = if yes {
            missing_agents.clone()
        } else {
            prompt_agent_pick(&missing_agents)?
        };
        if targets.is_empty() {
            println!("no agents selected");
        } else {
            println!("installing {} …", targets.join(", "));
            if let Err(e) = agentum_server::host_runtime::install_agents(host, &targets).await {
                eprintln!("  agent install failed: {e}");
            }
        }
    }

    // 3) Final state.
    let after = agentum_server::host_runtime::readiness(host).await;
    let _ = store.update_host_seen(host.id).await;
    println!();
    if after.ok {
        let agents = after.agents.iter().filter(|a| a.installed).count();
        println!(
            "Done. `{}` ready · {agents} agent CLI(s) available.",
            host.name
        );
    } else {
        println!("Done, but `{}` not ready — {}", host.name, after.message);
    }
    Ok(())
}

/// `y/N` prompt; returns true only on `y`/`yes`.
fn confirm(prompt: &str) -> Result<bool> {
    use std::io::Write;
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Prompt the user to pick which missing agents to install. Accepts
/// `all`, `none`/empty, or a space/comma-separated subset; names not in
/// `missing` are ignored.
fn prompt_agent_pick(missing: &[String]) -> Result<Vec<String>> {
    use std::io::Write;
    println!("Missing agents: {}", missing.join(", "));
    print!("Install which? [all / none / list]: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let input = line.trim().to_ascii_lowercase();
    let picked = match input.as_str() {
        "all" | "a" | "*" => missing.to_vec(),
        "" | "none" | "n" => Vec::new(),
        _ => input
            .split([',', ' '])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter(|s| missing.iter().any(|m| m == s))
            .map(|s| s.to_string())
            .collect(),
    };
    Ok(picked)
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
    // Surface passwordless-sudo up front: it decides whether `bootstrap`
    // (sudo install of tmux/git) can succeed over BatchMode SSH.
    let sudo = match report.system.sudo_nopasswd {
        Some(true) => "sudo: passwordless",
        Some(false) => "sudo: password required (bootstrap will fail)",
        None => "sudo: unknown",
    };
    println!(
        "System: {uname} · pkg_manager={} · {sudo}",
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

    print_fix_block(host, report);
}

/// Hermes-`doctor`-style "here's exactly what to run" block: the deps
/// install command (if tmux/git missing) plus each missing agent's
/// installer, followed by the agentum shortcuts. Only shown when there's
/// something to fix and the host was reachable.
fn print_fix_block(host: &Host, report: &HostReadiness) {
    if report.system.uname.is_none() {
        return; // unreachable — nothing to suggest
    }
    let missing_req: Vec<&str> = report
        .required
        .iter()
        .filter(|d| !d.installed)
        .map(|d| d.id.as_str())
        .collect();
    let missing_agents: Vec<&str> = report
        .agents
        .iter()
        .filter(|a| !a.installed && a.install_hint.is_some())
        .map(|a| a.id.as_str())
        .collect();
    if missing_req.is_empty() && missing_agents.is_empty() {
        return;
    }

    println!();
    println!("To fix, run on {}:", host.name);
    if !missing_req.is_empty() {
        match agentum_server::host_install_hints::bootstrap_command(
            &report.system.pkg_manager,
            &missing_req,
        ) {
            Some(cmd) => println!("  {cmd}"),
            None => println!(
                "  # install {} with your package manager",
                missing_req.join(" ")
            ),
        }
    }
    for tool in &missing_agents {
        if let Some(cmd) = agentum_server::host_install_hints::agent_install_command(tool) {
            println!("  {cmd}");
        }
    }
    println!();
    println!("  or let agentum do it: agentum hosts setup {}", host.name);
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

/// The tmux target backing a session: its explicit target, else the canonical
/// `agentum-<name>` — the same fallback `prune` and session spawn use.
fn session_target(session: &Session) -> String {
    session
        .tmux_target
        .clone()
        .unwrap_or_else(|| agentum_tmux::target_for(&session.name))
}

/// `agentum hosts prune-tmux <name> [--yes]` — kill orphaned `agentum-*` tmux
/// sessions a crashed/abandoned session left running on a host. Runs in-process
/// (store + host_runtime, like the other host commands). Dry-run unless `yes`.
///
/// Safety rests on [`zombie_tmux_targets`]: a session is killed only when it is
/// managed, unattached, not backed by a live (running/idle) record, and not an
/// externally-attached binding. We never kill a tmux a user started themselves.
///
/// [`zombie_tmux_targets`]: agentum_server::host_runtime::zombie_tmux_targets
async fn prune_tmux(name: String, yes: bool) -> Result<()> {
    use std::collections::HashSet;

    let (store, _) = agentum_store::open_default().await?;
    let host = find_host(&store, &name).await?;

    // Reconcile the host's managed tmux against what the store believes:
    //   live      = running/idle records → their tmux must survive
    //   protected = EXTERNAL_TMUX_FLAG bindings → user-owned tmux, never killed
    let sessions = store.list_sessions(None).await?;
    let mut live: HashSet<String> = HashSet::new();
    let mut protected: HashSet<String> = HashSet::new();
    for session in sessions
        .iter()
        .filter(|s| s.host_id.unwrap_or(LOCAL_HOST_ID) == host.id)
    {
        if matches!(session.status, Status::Running | Status::Idle) {
            live.insert(session_target(session));
        }
        if session.flags.iter().any(|f| f == EXTERNAL_TMUX_FLAG) {
            protected.insert(session_target(session));
        }
    }

    let on_host = agentum_server::host_runtime::list_managed_tmux_sessions(&host)
        .await
        .map_err(|e| anyhow::anyhow!("listing tmux on `{}` failed: {e}", host.name))?;
    let zombies = agentum_server::host_runtime::zombie_tmux_targets(&on_host, &live, &protected);

    if zombies.is_empty() {
        println!("no zombie tmux sessions on `{}`", host.name);
        return Ok(());
    }

    if !yes {
        println!(
            "Would kill {} zombie tmux session(s) on `{}` — dry run, pass --yes to kill:",
            zombies.len(),
            host.name
        );
        for target in &zombies {
            println!("  {target}");
        }
        return Ok(());
    }

    let mut killed = 0u32;
    for target in &zombies {
        match agentum_server::host_runtime::kill_session(&host, target).await {
            Ok(()) => {
                println!("killed      {target}");
                killed += 1;
            }
            // A session that vanished between listing and kill is fine; surface
            // real failures but keep sweeping the rest.
            Err(e) => eprintln!("  failed to kill {target}: {e}"),
        }
    }
    println!(
        "\nkilled {killed} zombie tmux session(s) on `{}`",
        host.name
    );
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
