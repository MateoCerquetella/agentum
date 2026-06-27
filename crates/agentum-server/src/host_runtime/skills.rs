//! Host skill detection and provisioning.
use std::path::{Path, PathBuf};

use super::*;

/// The local `~/.claude/skills` directory (the daemon user's global Claude
/// skills) — the source of truth for what we can provision to a host.
fn local_skills_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude/skills"))
}

/// Agentum skills installed locally that we can provision to a host: each
/// directory under `~/.claude/skills` that contains a `SKILL.md`. Returns the
/// directory names (skill ids), sorted.
pub(crate) fn local_provisionable_skill_ids() -> Vec<String> {
    let Some(root) = local_skills_root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("SKILL.md").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids
}

/// For each locally-known skill id, whether the host already has it under
/// `~/.claude/skills`. One SSH round trip (an `ls` of the skills dir). Errors
/// (no dir, unreachable) read as "none present" rather than failing readiness.
pub(crate) async fn detect_host_skills(host: &Host, ids: &[String]) -> Vec<SkillCheck> {
    if ids.is_empty() {
        return Vec::new();
    }
    let present: HashSet<String> = match &host.kind {
        // The local host is the source — every known skill is present.
        HostKind::Local => ids.iter().cloned().collect(),
        HostKind::Ssh { .. } => {
            // `$HOME` expands inside the inner `sh -c`; `|| true` keeps exit 0
            // (and stdout empty) when the skills dir doesn't exist yet.
            let inner = "ls -1 \"$HOME/.claude/skills\" 2>/dev/null || true";
            let Ok(quoted) = q(inner) else {
                return Vec::new();
            };
            let script = format!("sh -c {quoted}");
            match ssh_stdout(host, &script).await {
                Ok(out) => out
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect(),
                Err(_) => HashSet::new(),
            }
        }
    };
    ids.iter()
        .map(|id| SkillCheck {
            id: id.clone(),
            label: id.clone(),
            installed: present.contains(id),
        })
        .collect()
}

/// Copy agentum skills (by id) from the local `~/.claude/skills` to the host's
/// `~/.claude/skills`, then re-probe and return fresh readiness. File-copy only
/// (base64 over SSH) — never runs an arbitrary command on the remote. The caller
/// (route) gates this behind `confirm: true`; unknown / path-suspicious ids are
/// rejected here so a client can't write outside the skills tree.
pub async fn provision_skills(host: &Host, ids: &[String]) -> Result<HostReadiness> {
    let root = local_skills_root()
        .ok_or_else(|| HostRuntimeError::Bootstrap("no local ~/.claude/skills directory".into()))?;
    for id in ids {
        if id.is_empty() || id.contains('/') || id.contains("..") {
            return Err(HostRuntimeError::Bootstrap(format!(
                "invalid skill id `{id}`"
            )));
        }
        if !root.join(id).join("SKILL.md").is_file() {
            return Err(HostRuntimeError::Bootstrap(format!(
                "unknown local skill `{id}` (no ~/.claude/skills/{id}/SKILL.md)"
            )));
        }
    }
    match &host.kind {
        // Local host is the source; the skills are already there.
        HostKind::Local => {}
        HostKind::Ssh { .. } => {
            let home = remote_home(host).await?;
            for id in ids {
                tracing::info!(host = %host.name, skill = %id, "provisioning skill to host");
                copy_skill_dir_ssh(host, &root.join(id), &home, id).await?;
            }
        }
    }
    Ok(readiness(host).await)
}

/// Resolve the host's absolute `$HOME` (one SSH round trip) so subsequent file
/// writes use absolute paths — `$HOME` can't survive the double shell-quoting
/// the write commands need.
async fn remote_home(host: &Host) -> Result<String> {
    let script = format!("sh -c {}", q("printf %s \"$HOME\"")?);
    let home = ssh_stdout(host, &script).await?.trim().to_string();
    if home.is_empty() {
        return Err(HostRuntimeError::Bootstrap(
            "could not resolve remote $HOME".into(),
        ));
    }
    Ok(home)
}

/// Copy every regular file directly under `src_dir` to
/// `<home>/.claude/skills/<id>/` on the host via base64 (`base64 -d`), creating
/// the directory first. Non-recursive (skills are flat: SKILL.md + a few files).
async fn copy_skill_dir_ssh(host: &Host, src_dir: &Path, home: &str, id: &str) -> Result<()> {
    let remote_dir = format!("{home}/.claude/skills/{id}");
    let mkdir = format!("sh -c {}", q(&format!("mkdir -p {}", q(&remote_dir)?))?);
    ssh_checked(host, &mkdir).await?;
    let mut entries = tokio::fs::read_dir(src_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.path().is_file() {
            continue;
        }
        let Ok(fname) = entry.file_name().into_string() else {
            continue;
        };
        if fname.contains('/') || fname.contains("..") {
            continue;
        }
        let bytes = tokio::fs::read(entry.path()).await?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let remote_file = format!("{remote_dir}/{fname}");
        // `printf %s '<b64>' | base64 -d > '<file>'` — content is base64 (no
        // shell-special chars), paths are absolute + quoted. No arbitrary exec.
        let inner = format!("printf %s {} | base64 -d > {}", q(&b64)?, q(&remote_file)?);
        let script = format!("sh -c {}", q(&inner)?);
        ssh_checked(host, &script).await?;
    }
    Ok(())
}

/// Install required system packages (`tmux`/`git`) on a host via its
/// package manager, then re-probe and return the fresh readiness. Phase
/// 2. The caller (route) restricts `items` to [`crate::host_install_hints::BOOTSTRAPABLE`]
/// — we never install agent CLIs or arbitrary packages.
///
/// The install runs under `sudo`. Because the daemon's SSH always uses
/// `BatchMode=yes` with no TTY, this only succeeds where passwordless
/// `sudo` (or a root login) is configured; otherwise the install fails
/// and the remote `stderr` is surfaced verbatim. See
/// `docs/plans/SSH_HOST_READINESS_PRD.md` §7.3 + §12.
pub async fn bootstrap(host: &Host, items: &[String]) -> Result<HostReadiness> {
    // Detect the package manager (and current state) first.
    let pre = readiness(host).await;
    let item_refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let cmd = crate::host_install_hints::bootstrap_command(&pre.system.pkg_manager, &item_refs)
        .ok_or_else(|| {
            HostRuntimeError::Bootstrap(format!(
                "no known package manager on this host (detected `{}`); install {} manually",
                pre.system.pkg_manager,
                item_refs.join(" ")
            ))
        })?;
    // Log the action (no secrets — the command is just `sudo <pm> install …`).
    tracing::info!(
        host = %host.name,
        pkg_manager = %pre.system.pkg_manager,
        items = %item_refs.join(","),
        "bootstrapping host system packages"
    );
    match &host.kind {
        HostKind::Local => run_checked_local(&cmd).await?,
        HostKind::Ssh { .. } => {
            // Wrap in `sh -c` for the same POSIX-shell reason as the probe.
            let script = format!("sh -c {}", q(&cmd)?);
            run_checked_ssh(host, &script, BOOTSTRAP_TIMEOUT).await?;
        }
    }
    // Re-probe so the response reflects the post-install state.
    Ok(readiness(host).await)
}

/// Install one or more agent CLIs on a host by running each tool's
/// official installer (see [`crate::host_install_hints::agent_install_command`])
/// over SSH, then re-probing. Phase 3 — opt-in, confirmed by the caller.
///
/// Unlike [`bootstrap`], these are not `sudo` package-manager installs —
/// they're the vendors' own `npm -g` / `curl | bash` / `pip` scripts run
/// as the SSH user. They may still need a working node/python on the
/// remote; a missing prerequisite surfaces as the installer's stderr.
/// Tools without a verified installer are skipped (logged at `warn`).
pub async fn install_agents(host: &Host, tools: &[String]) -> Result<HostReadiness> {
    let mut unknown: Vec<&str> = Vec::new();
    let mut cmds: Vec<&'static str> = Vec::new();
    for t in tools {
        match crate::host_install_hints::agent_install_command(t) {
            Some(cmd) => cmds.push(cmd),
            None => unknown.push(t.as_str()),
        }
    }
    if !unknown.is_empty() {
        tracing::warn!(
            host = %host.name,
            skipped = %unknown.join(","),
            "skipping agent CLIs without a verified installer"
        );
    }
    if cmds.is_empty() {
        return Err(HostRuntimeError::Bootstrap(format!(
            "no installable agent CLIs in [{}]",
            tools.join(", ")
        )));
    }
    // Chain installers with `&&` so the first failure stops the rest and
    // its stderr is what surfaces.
    let combined = cmds.join(" && ");
    tracing::info!(host = %host.name, tools = %tools.join(","), "installing agent CLIs on host");
    match &host.kind {
        HostKind::Local => run_checked_local(&combined).await?,
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&combined)?);
            run_checked_ssh(host, &script, BOOTSTRAP_TIMEOUT).await?;
        }
    }
    Ok(readiness(host).await)
}
