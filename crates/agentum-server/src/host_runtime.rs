//! Local/SSH host execution helpers.
//!
//! The SSH backend intentionally drives only stock `ssh` + `tmux` on the
//! remote machine. The remote host never needs an `agentum` binary.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentum_core::{
    AgentDepCheck, DepCheck, Host, HostKind, HostReadiness, HostSystemInfo, SkillCheck,
};
use agentum_executor::{binary_for, probed_tools};
use base64::Engine as _;
// The SSH connection builder lives in the shared lower crate so the watchdog
// (which can't depend on agentum-server) shares one source of truth for the
// ssh flags + the SSH_ASKPASS password helper. `ssh_output` is the resilient
// runner: it replays an op on a fresh, unmultiplexed connection when a pooled
// ControlMaster socket goes stale, so a flaky master never hard-fails a remote
// op that never actually ran.
use agentum_tmux::ssh::{
    SshMux, ssh_command_opts, ssh_control_cancel_cmd, ssh_control_forward_cmd,
    ssh_control_local_cancel_cmd, ssh_control_local_forward_cmd, ssh_output,
};
use tokio::process::Command;
use tokio::time::{sleep, timeout};

const SSH_TIMEOUT: Duration = Duration::from_secs(12);

/// Package installs (apt/pacman/dnf/brew) download + unpack, so the
/// readiness `SSH_TIMEOUT` (12s) is far too short. Bootstrap is an
/// explicit, user-confirmed action — give it room to finish.
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(180);

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
    #[error("{0}")]
    Bootstrap(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tmux(#[from] agentum_tmux::TmuxError),
}

pub type Result<T> = std::result::Result<T, HostRuntimeError>;

/// Map [`ssh_output`]'s `io::Error` to a [`HostRuntimeError`], preserving the
/// distinct `Timeout` variant (the runner reports a timeout as `ErrorKind::
/// TimedOut`) that callers and the UI surface separately from other I/O.
fn map_ssh_io(e: std::io::Error) -> HostRuntimeError {
    if e.kind() == std::io::ErrorKind::TimedOut {
        HostRuntimeError::Timeout
    } else {
        HostRuntimeError::Io(e)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HostProbe {
    pub ok: bool,
    pub message: String,
    pub uname: Option<String>,
    pub tmux: bool,
    pub git: bool,
}

/// Coarse boolean probe, kept for the dashboard's `POST /test` contract
/// (`{ok, tmux, git, uname}`). Now a thin down-map over [`readiness`] so
/// the booleans are *accurate* — the previous SSH branch hard-coded
/// `tmux: true, git: true` on any connection success, which reported a
/// box without tmux as ready and let spawns fail later with
/// `command not found`. New surfaces (TUI/CLI) use [`readiness`] directly.
pub async fn probe(host: &Host) -> HostProbe {
    let r = readiness(host).await;
    let installed = |id: &str| r.required.iter().any(|d| d.id == id && d.installed);
    HostProbe {
        ok: r.ok,
        message: r.message,
        uname: r.system.uname,
        tmux: installed("tmux"),
        git: installed("git"),
    }
}

/// Required system dependencies. Every entry must be installed for a host
/// to be `ok` (and therefore spawnable). Order is the display order.
const REQUIRED_DEPS: &[&str] = &["tmux", "git"];

/// Full structured readiness check: one SSH round trip (or local `which`
/// calls) reporting required deps + every probed agent CLI + the package
/// manager, with install hints filled in. See
/// `docs/plans/SSH_HOST_READINESS_PRD.md` §7.1.
pub async fn readiness(host: &Host) -> HostReadiness {
    // The agentum skills we could provision to this host (the local user's
    // installed skills). We report which of these the host already has.
    let known_skills = local_provisionable_skill_ids();
    let mut report = match &host.kind {
        HostKind::Local => {
            let mut r = assemble_readiness(probe_local());
            r.skills = detect_host_skills(host, &known_skills).await;
            r
        }
        HostKind::Ssh { .. } => match probe_ssh(host).await {
            Ok(probe) => {
                let mut r = assemble_readiness(probe);
                // Host reachable → one more round trip to see which skills it has.
                r.skills = detect_host_skills(host, &known_skills).await;
                r
            }
            // Connection / auth / timeout failure: surface the error
            // verbatim and report everything as missing so the UI shows
            // the full (unverifiable) dependency list rather than a bare
            // error with no guidance. Skills stay empty (host unreachable).
            Err(e) => unreachable_readiness(e.to_string()),
        },
    };
    crate::host_install_hints::fill_hints(&mut report);
    report
}

/// The local `~/.claude/skills` directory (the daemon user's global Claude
/// skills) — the source of truth for what we can provision to a host.
fn local_skills_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude/skills"))
}

/// Agentum skills installed locally that we can provision to a host: each
/// directory under `~/.claude/skills` that contains a `SKILL.md`. Returns the
/// directory names (skill ids), sorted.
fn local_provisionable_skill_ids() -> Vec<String> {
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
async fn detect_host_skills(host: &Host, ids: &[String]) -> Vec<SkillCheck> {
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

/// Run `script` over SSH with a caller-chosen timeout, surfacing remote
/// `stderr` on a non-zero exit. (`ssh_checked` hard-codes `SSH_TIMEOUT`;
/// bootstrap needs a longer budget.)
async fn run_checked_ssh(host: &Host, script: &str, dur: Duration) -> Result<()> {
    let output = ssh_output(host, script, dur).await.map_err(map_ssh_io)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(HostRuntimeError::NonZero {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

/// Run a shell command on the local host (for bootstrapping a `Local`
/// host), surfacing `stderr` on a non-zero exit.
async fn run_checked_local(cmd: &str) -> Result<()> {
    let output = timeout(
        BOOTSTRAP_TIMEOUT,
        Command::new("sh").arg("-c").arg(cmd).output(),
    )
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

/// Raw output of the preflight, before it's shaped into a
/// [`HostReadiness`]. `bins` maps a probed binary name to its resolved
/// path; an empty string means "not found".
#[derive(Debug)]
struct ProbeOutput {
    uname: Option<String>,
    pkg_manager: String,
    /// `Some(true)` if `sudo -n true` succeeded on the remote (passwordless
    /// sudo / root); `Some(false)` if it failed; `None` if undetermined.
    sudo_nopasswd: Option<bool>,
    bins: HashMap<String, String>,
}

/// The binaries to probe, deduped, required deps first then agent CLIs.
/// `cursor` and `agent` may both map to distinct binaries; dedup keeps a
/// single `command -v` per binary regardless of how many tool ids share
/// it.
fn probe_binaries() -> Vec<String> {
    let mut bins: Vec<String> = Vec::new();
    let mut push = |b: String| {
        if !bins.contains(&b) {
            bins.push(b);
        }
    };
    for d in REQUIRED_DEPS {
        push(d.to_string());
    }
    for tool in probed_tools() {
        push(binary_for(tool).to_string());
    }
    bins
}

/// Build the structured report from raw probe data. Sets `ok` from the
/// required deps only; agent gaps never block here (the New Session form
/// blocks only when the *picked* tool is unavailable). Install hints are
/// left empty — [`crate::host_install_hints::fill_hints`] fills them.
fn assemble_readiness(probe: ProbeOutput) -> HostReadiness {
    let required: Vec<DepCheck> = REQUIRED_DEPS
        .iter()
        .map(|id| {
            let installed = probe.bins.get(*id).map(|p| !p.is_empty()).unwrap_or(false);
            DepCheck {
                id: (*id).to_string(),
                label: (*id).to_string(),
                installed,
                install_hint: None,
                bootstrapable: false,
            }
        })
        .collect();

    let agents: Vec<AgentDepCheck> = probed_tools()
        .map(|tool| {
            let binary = binary_for(tool);
            let path = probe.bins.get(binary).filter(|p| !p.is_empty()).cloned();
            AgentDepCheck {
                id: tool.to_string(),
                binary: binary.to_string(),
                installed: path.is_some(),
                path,
                install_hint: None,
                bootstrapable: false,
            }
        })
        .collect();

    let missing: Vec<&str> = required
        .iter()
        .filter(|d| !d.installed)
        .map(|d| d.id.as_str())
        .collect();
    let ok = missing.is_empty();
    let message = if ok {
        "host ready".to_string()
    } else {
        format!(
            "{} required {} missing: {}",
            missing.len(),
            if missing.len() == 1 {
                "dependency"
            } else {
                "dependencies"
            },
            missing.join(", ")
        )
    };

    HostReadiness {
        ok,
        message,
        system: HostSystemInfo {
            uname: probe.uname,
            pkg_manager: probe.pkg_manager,
            sudo_nopasswd: probe.sudo_nopasswd,
        },
        required,
        agents,
        // Filled by `readiness()` after assembly (needs an async host probe);
        // empty here and for the unreachable path.
        skills: Vec::new(),
    }
}

/// Readiness for a host we couldn't reach. Everything reports missing;
/// `message` carries the SSH error.
fn unreachable_readiness(message: String) -> HostReadiness {
    let mut report = assemble_readiness(ProbeOutput {
        uname: None,
        pkg_manager: "unknown".to_string(),
        sudo_nopasswd: None,
        bins: HashMap::new(),
    });
    report.message = message;
    report
}

/// Local-host probe: synchronous `which` lookups, no SSH.
fn probe_local() -> ProbeOutput {
    let uname = std::process::Command::new("uname")
        .arg("-sr")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut bins = HashMap::new();
    for b in probe_binaries() {
        let path = which::which(&b)
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        bins.insert(b, path);
    }
    // Local passwordless-sudo check mirrors the remote `sudo -n true`.
    let sudo_nopasswd = Some(
        std::process::Command::new("sudo")
            .arg("-n")
            .arg("true")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
    );
    ProbeOutput {
        uname,
        pkg_manager: detect_local_pkg_manager(),
        sudo_nopasswd,
        bins,
    }
}

fn detect_local_pkg_manager() -> String {
    for (bin, name) in [
        ("apt-get", "apt"),
        ("dnf", "dnf"),
        ("pacman", "pacman"),
        ("brew", "brew"),
    ] {
        if which::which(bin).is_ok() {
            return name.to_string();
        }
    }
    "unknown".to_string()
}

/// SSH probe: one round trip running a POSIX `sh -c` script that prints
/// `uname`, the package manager, and a `command -v` line per binary. We
/// wrap in `sh -c` because the remote login shell may be fish/zsh, which
/// don't share bash's `for`/`case` syntax — `sh -c` forces POSIX.
async fn probe_ssh(host: &Host) -> Result<ProbeOutput> {
    let inner = build_probe_script(&probe_binaries())?;
    let script = format!("sh -c {}", q(&inner)?);
    let stdout = ssh_stdout(host, &script).await?;
    Ok(parse_probe_output(&stdout))
}

fn build_probe_script(bins: &[String]) -> Result<String> {
    let mut s = String::new();
    s.push_str(r#"printf 'uname\t%s\n' "$(uname -sr 2>/dev/null)"; "#);
    s.push_str(
        r#"pm=unknown; for c in apt-get dnf pacman brew; do if command -v "$c" >/dev/null 2>&1; then case "$c" in apt-get) pm=apt;; *) pm="$c";; esac; break; fi; done; "#,
    );
    s.push_str(r#"printf 'pkg\t%s\n' "$pm"; "#);
    // Passwordless-sudo check: `sudo -n true` never prompts. Reports yes
    // only when sudo runs without a password (or the user is root).
    s.push_str(
        r#"if sudo -n true >/dev/null 2>&1; then printf 'sudo\tyes\n'; else printf 'sudo\tno\n'; fi; "#,
    );
    for b in bins {
        let qb = q(b)?;
        s.push_str(&format!(
            r#"printf 'bin\t%s\t%s\n' {qb} "$(command -v {qb} 2>/dev/null)"; "#
        ));
    }
    Ok(s)
}

/// Parse the tab-delimited preflight output. Tolerant of trailing `\r`,
/// blank lines, and unknown keys (forward-compat).
fn parse_probe_output(stdout: &str) -> ProbeOutput {
    let mut out = ProbeOutput {
        uname: None,
        pkg_manager: "unknown".to_string(),
        sudo_nopasswd: None,
        bins: HashMap::new(),
    };
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        let mut it = line.splitn(2, '\t');
        let key = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("");
        match key {
            "uname" => {
                let v = rest.trim();
                if !v.is_empty() {
                    out.uname = Some(v.to_string());
                }
            }
            "sudo" => out.sudo_nopasswd = Some(rest.trim() == "yes"),
            "pkg" => {
                let v = rest.trim();
                out.pkg_manager = if v.is_empty() {
                    "unknown".to_string()
                } else {
                    v.to_string()
                };
            }
            "bin" => {
                let mut bit = rest.splitn(2, '\t');
                let name = bit.next().unwrap_or("").trim();
                // The path may legitimately contain spaces; only trim the
                // surrounding whitespace, never split it.
                let path = bit.next().unwrap_or("").trim();
                if !name.is_empty() {
                    out.bins.insert(name.to_string(), path.to_string());
                }
            }
            _ => {}
        }
    }
    out
}

pub async fn has_session(host: &Host, target: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::has_session(target).await?),
        HostKind::Ssh { .. } => {
            let script = format!("tmux has-session -t {}", q(&format!("={target}"))?);
            let output = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(output.status.success())
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
            // Cursor sample + grid in one remote shell so the anchor can't
            // drift from the content it anchors (same contract as the local
            // `capture_pane_ansi`). The `|| echo X` keeps the line structure
            // if display-message fails — "X" parses to no anchor, degrading
            // to the legacy unanchored snapshot instead of misparsing the
            // grid's first row as coordinates. `sh -c`-wrapped like every
            // other remote script: the login shell may be fish/zsh.
            let inner = format!(
                "tmux display-message -p -t {t} {fmt} 2>/dev/null || echo X; tmux capture-pane -p -e -t {t}",
                t = q(target)?,
                fmt = q(agentum_tmux::CURSOR_SAMPLE_FORMAT)?,
            );
            let out = ssh_stdout(host, &format!("sh -c {}", q(&inner)?)).await?;
            let (cursor_line, grid) = out.split_once('\n').unwrap_or((out.as_str(), ""));
            Ok(agentum_tmux::assemble_anchored_snapshot(
                grid.as_bytes(),
                agentum_tmux::parse_cursor_sample(cursor_line),
            ))
        }
    }
}

/// SSH only: one round trip returning the remote pane-log's current byte size
/// AND a cursor-anchored capture-pane snapshot. The caller paints the snapshot,
/// then starts the streaming tail from the byte offset (`tail -c +N -f`).
///
/// Ordering is load-bearing: the offset is sampled *after* `capture-pane`, so
/// the tail resumes just past the bytes the snapshot already reflects. The tail
/// therefore never replays a byte the snapshot painted. That matters because
/// agent TUIs that render on the *normal* screen (cursor-agent) redraw with
/// RELATIVE motion (`ESC[1A` cursor-up + `ESC[2K` erase). Replaying even a
/// partial redraw frame on top of the snapshot desyncs the cursor, and since
/// every following frame is relative, the desync compounds into stacked spinner
/// lines ("Composing… Composing…"). Alt-screen apps (Claude/Codex) reposition
/// absolutely and were immune, which is why only cursor-agent corrupted.
///
/// The flip side is a sub-millisecond GAP (bytes emitted *during* the
/// capture-pane exec are in neither snapshot nor tail). For a redraw app the
/// next frame (~100 ms) repaints it; for a streaming pane it's a few dropped
/// bytes, far cheaper than the permanent stacking a duplicate caused.
///
/// The size and cursor halves are fallback-guarded so a not-yet-rendered pane
/// still yields the offset (with an empty snapshot) instead of failing the
/// connect.
pub async fn capture_pane_with_log_offset(
    host: &Host,
    target: &str,
    out_path: &Path,
) -> Result<(u64, Vec<u8>)> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    let out = ssh_stdout(host, &snapshot_with_offset_script(target, out_path)?).await?;
    let mut parts = out.splitn(3, '\n');
    let size_line = parts.next().unwrap_or("");
    let cursor_line = parts.next().unwrap_or("");
    let grid = parts.next().unwrap_or("");
    // BSD `wc` left-pads with spaces; an unparsable size degrades to 0, which
    // only risks a duplicate replay, never a gap.
    // An unparsable size degrades to 0, which only risks a duplicate replay of
    // the whole log, never a gap. (BSD `wc` left-pads with spaces — trimmed.)
    let size = size_line.trim().parse::<u64>().unwrap_or(0);
    let snap = agentum_tmux::assemble_anchored_snapshot(
        grid.as_bytes(),
        agentum_tmux::parse_cursor_sample(cursor_line),
    );
    Ok((size, snap))
}

/// Build the remote shell script behind [`capture_pane_with_log_offset`].
/// Output is three sections: the pane-log byte size, the cursor sample, then
/// the raw ANSI grid (rest of stdout).
///
/// This ALSO (idempotently) arms `pipe-pane` first — folding what used to be a
/// separate connect-time round-trip into this one. On a distant host each SSH
/// exec is ~450 ms even over the warm master, so doing arm-then-capture as two
/// sequential calls cost ~900 ms of blank screen before the first paint; one
/// combined exec halves that. A pipe-pane failure here is swallowed (the
/// snapshot still paints; only live updates would be missing) rather than
/// failing the connect.
///
/// The cursor is sampled then the grid captured (both ≈ the same instant), and
/// the log size is read LAST — after `capture-pane` — so the tail resumes just
/// past what the snapshot covers and never replays a painted byte (see
/// [`capture_pane_with_log_offset`] for why that ordering prevents the
/// relative-redraw stacking). The grid is buffered to a temp file so the size
/// can still be emitted on line 1 despite being computed last; fallbacks
/// (`echo 0` / `echo X`) keep the three-section shape when the log or pane
/// isn't there yet.
fn snapshot_with_offset_script(target: &str, out_path: &Path) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    let pipe = q(&format!("cat >> {log}"))?.into_owned();
    let inner = format!(
        "mkdir -p \"{REMOTE_PANE_DIR}\" 2>/dev/null; tmux pipe-pane -o -t {t} {pipe} 2>/dev/null; \
         c=$(tmux display-message -p -t {t} {fmt} 2>/dev/null || echo X); \
         f=$(mktemp 2>/dev/null || echo /tmp/agentum-snap.$$); \
         tmux capture-pane -p -e -t {t} > \"$f\" 2>/dev/null || true; \
         o=$({{ wc -c < {log}; }} 2>/dev/null || echo 0); \
         printf \"%s\\n%s\\n\" \"$o\" \"$c\"; cat \"$f\" 2>/dev/null; rm -f \"$f\"",
        t = q(target)?,
        fmt = q(agentum_tmux::CURSOR_SAMPLE_FORMAT)?,
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

/// Open (or refresh) BOTH pooled SSH masters for `host` with a no-op remote
/// command. The boot-time/periodic warmer calls this so interactive remote ops
/// AND the first stream tail find a live master instead of paying the 1-3s
/// TCP+auth handshake. Warming the streaming master matters most: it means the
/// first session's `tail -f` multiplexes onto a hot connection instead of
/// opening a cold one (~2s) and stalling the first live updates. No-op for local
/// hosts. The streaming warm is best-effort — its failure never fails the call.
pub async fn warm_ssh_master(host: &Host) -> Result<()> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Ok(());
    }
    // Establish the streaming master (`cms-`) alongside the interactive one so
    // both are hot before the user interacts. Run concurrently; the streaming
    // leg is best-effort.
    let stream_warm = ssh_command_opts(host, "true", SshMux::Streaming).output();
    let (interactive, _) = tokio::join!(ssh_output(host, "true", SSH_TIMEOUT), stream_warm);
    interactive.map_err(map_ssh_io)?;
    Ok(())
}

/// First port of the loopback range scanned for the reverse tunnel on a host.
pub const REMOTE_MCP_PORT_BASE: u16 = 8990;
/// How many consecutive ports to try before giving up (host services or stale
/// forwards may already hold some).
const REMOTE_MCP_PORT_TRIES: u16 = 24;

/// Ensure a **reverse** SSH tunnel so this host can reach the Mac's embedded
/// agentum MCP server: on the host, `127.0.0.1:<port>` → (over SSH) → Mac's
/// `127.0.0.1:<mac_port>`. Returns the **host port** that was armed (the caller
/// writes it into the agent's MCP URL).
///
/// Scans a small loopback-port range — a fixed port collides with whatever the
/// host already runs there (verified: a real service held the first choice on a
/// live host) or with a stale forward from a prior app instance that the current
/// master can't cancel. We cancel-then-arm each candidate and take the first that
/// binds, so the tunnel always points at THIS server's live port. Rides the warm
/// interactive ControlMaster via `-O forward` (no extra connection). Loopback-
/// bound both ends; the per-server bearer token guards on-host access.
pub async fn ensure_reverse_tunnel(host: &Host, mac_port: u16) -> Result<u16> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    // `-O forward` attaches to an existing master, so the master must be up first.
    warm_ssh_master(host).await?;

    let mut last_err = String::new();
    for host_port in
        REMOTE_MCP_PORT_BASE..REMOTE_MCP_PORT_BASE.saturating_add(REMOTE_MCP_PORT_TRIES)
    {
        // Cancel any forward already bound to this port (e.g. a stale one from a
        // prior app instance pointing at a now-dead Mac port), then arm fresh so
        // the tunnel always targets the current Mac port. No-op when none exists.
        if let Some(mut cancel) = ssh_control_cancel_cmd(host, host_port) {
            let _ = cancel.output().await;
        }
        let Some(mut cmd) = ssh_control_forward_cmd(host, host_port, mac_port) else {
            return Err(HostRuntimeError::Bootstrap(
                "no ControlPath available for the reverse MCP tunnel".into(),
            ));
        };
        let out = cmd.output().await.map_err(map_ssh_io)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        let s = stderr.to_ascii_lowercase();
        if out.status.success() || s.contains("already") || s.contains("exists") {
            return Ok(host_port);
        }
        // Port busy (host service or unreachable stale forward) → try the next.
        last_err = stderr.trim().to_string();
    }
    Err(HostRuntimeError::Bootstrap(format!(
        "no free reverse-tunnel port on host in {REMOTE_MCP_PORT_BASE}..; last: {last_err}"
    )))
}

/// First Mac-loopback port scanned for the **forward** (CDP screencast) tunnel.
/// A SEPARATE range from [`REMOTE_MCP_PORT_BASE`] (8990) so the reverse MCP
/// tunnel and this forward CDP tunnel can coexist on the one Interactive
/// ControlMaster without one cancel/arm clobbering the other.
pub const REMOTE_CDP_PORT_BASE: u16 = 9200;
/// How many consecutive Mac ports to try before giving up (another local app
/// may already hold some, or a stale forward may linger).
const REMOTE_CDP_PORT_TRIES: u16 = 24;

/// The Mac-loopback port range scanned by [`ensure_forward_tunnel`]. Pure so the
/// range (and its disjointness from the MCP range) is unit-testable.
fn forward_tunnel_ports() -> std::ops::Range<u16> {
    REMOTE_CDP_PORT_BASE..REMOTE_CDP_PORT_BASE.saturating_add(REMOTE_CDP_PORT_TRIES)
}

/// Did an `ssh -O forward -L` attempt bind the Mac port? A clean exit means
/// bound; ssh reporting the forward as already established is an idempotent
/// success (cancel-then-arm can race a still-present forward). Any other
/// non-zero exit means the port is unusable → scan the next. Mirrors the
/// reverse-tunnel predicate in [`ensure_reverse_tunnel`].
fn forward_arm_bound(status_success: bool, stderr: &str) -> bool {
    if status_success {
        return true;
    }
    let s = stderr.to_ascii_lowercase();
    s.contains("already") || s.contains("exists")
}

/// Ensure a **forward** SSH tunnel so the Mac can reach the host's headless
/// Chromium CDP debugger: on the Mac, `127.0.0.1:<mac_port>` → (over SSH) →
/// host's `127.0.0.1:<host_port>`. Returns the **Mac port** that was armed (the
/// caller connects its CDP client + screencast bridge there).
///
/// The mirror of [`ensure_reverse_tunnel`]: CDP lives on the host, so the Mac
/// reaches it with a local (-L) forward rather than the reverse (-R) the MCP
/// server needs. Scans a small Mac-loopback range — a fixed port collides with
/// whatever the Mac already runs there or a stale forward a prior app instance
/// left. We cancel-then-arm each candidate and take the first that binds, so the
/// tunnel always points at THIS host's CDP port. Rides the warm interactive
/// ControlMaster via `-O forward` (no extra connection). Loopback-bound both
/// ends; the SSH channel is the only path to the host's CDP.
pub async fn ensure_forward_tunnel(host: &Host, host_port: u16) -> Result<u16> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    // `-O forward` attaches to an existing master, so the master must be up first.
    warm_ssh_master(host).await?;

    let mut last_err = String::new();
    for mac_port in forward_tunnel_ports() {
        // Cancel any forward already bound to this Mac→host pair (e.g. a stale
        // one left after a Mac sleep, when re-attaching to the same browser),
        // then arm fresh. OpenSSH needs the full spec to cancel a -L, so this
        // only clears a forward to the SAME host port — exactly the re-attach
        // case; a foreign holder of the Mac port instead fails the arm below and
        // we scan on. No-op when none; best-effort, so failures are ignored.
        if let Some(mut cancel) = ssh_control_local_cancel_cmd(host, mac_port, host_port) {
            let _ = cancel.output().await;
        }
        let Some(mut cmd) = ssh_control_local_forward_cmd(host, mac_port, host_port) else {
            return Err(HostRuntimeError::Bootstrap(
                "no ControlPath available for the CDP forward tunnel".into(),
            ));
        };
        let out = cmd.output().await.map_err(map_ssh_io)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if forward_arm_bound(out.status.success(), &stderr) {
            return Ok(mac_port);
        }
        // Port busy (local service or unbindable stale forward) → try the next.
        last_err = stderr.trim().to_string();
    }
    Err(HostRuntimeError::Bootstrap(format!(
        "no free CDP forward-tunnel port on Mac in {REMOTE_CDP_PORT_BASE}..; last: {last_err}"
    )))
}

/// Write `content` to `abs_path` on `host` (local fs, or on the SSH host) with
/// owner-only (0600) permissions. Used to place a remote agent's `--mcp-config`
/// file — which carries the MCP **bearer token** — where the agent can read it.
///
/// Security: the file must be unreadable to other users on the host (the token
/// is a credential). We write with `umask 077` to a `mktemp` file and `mv` it
/// into place atomically — so the final path can't be a pre-planted symlink we'd
/// follow, and the file is never briefly world-readable. The filename is a random
/// session UUID, so it can't be pre-created by an attacker. Content is
/// base64-piped so JSON quoting can't break the write.
pub async fn write_remote_file(host: &Host, abs_path: &str, content: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => {
            if let Some(parent) = std::path::Path::new(abs_path).parent() {
                std::fs::create_dir_all(parent).map_err(map_ssh_io)?;
            }
            std::fs::write(abs_path, content).map_err(map_ssh_io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(abs_path, std::fs::Permissions::from_mode(0o600))
                    .map_err(map_ssh_io)?;
            }
            Ok(())
        }
        HostKind::Ssh { .. } => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(content);
            let parent = std::path::Path::new(abs_path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/tmp".to_string());
            // The host's LOGIN shell may be fish/zsh (not POSIX sh), so a bash-y
            // script run directly fails. Build a POSIX-sh script and feed it to
            // `sh` via a base64 pipe: the only chars in the outer command are
            // base64 (shell-safe everywhere), so fish/zsh/bash all run it the
            // same. `umask 077` + `chmod 600` keep the token file owner-only; the
            // random-UUID filename means no attacker can pre-plant a symlink.
            let inner = format!(
                "umask 077; mkdir -p {dir}; printf %s {b64} | base64 -d > {path}; chmod 600 {path}",
                dir = q(&parent)?,
                b64 = q(&b64)?,
                path = q(abs_path)?,
            );
            let inner_b64 = base64::engine::general_purpose::STANDARD.encode(&inner);
            let remote = format!("printf %s {} | base64 -d | sh", q(&inner_b64)?);
            ssh_checked(host, &remote).await
        }
    }
}

/// Read `abs_path` from `host` (local fs or SSH), or `None` when it doesn't
/// exist. Used to merge agentum into an existing agent config file (Cursor,
/// Gemini, OpenCode) without clobbering the user's other servers. Only stdout is
/// read, so the host's login-shell noise (fnm, etc.) on stderr is ignored.
pub async fn read_remote_file(host: &Host, abs_path: &str) -> Result<Option<String>> {
    match &host.kind {
        HostKind::Local => Ok(std::fs::read_to_string(abs_path).ok()),
        HostKind::Ssh { .. } => {
            let mut cmd =
                ssh_command_opts(host, &format!("cat {}", q(abs_path)?), SshMux::Interactive);
            let out = cmd.output().await.map_err(map_ssh_io)?;
            if out.status.success() && !out.stdout.is_empty() {
                Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
            } else {
                // Missing file (cat exits non-zero) → None, not an error.
                Ok(None)
            }
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

/// Read the pane's current title (`#{pane_title}`). tmux captures the agent's
/// OSC title here but never forwards it over a `capture-pane` stream (set-titles
/// off), so the desktop's title-derived agent status has no input. The session
/// stream re-injects this as a synthetic OSC title. Trimmed of trailing newline.
pub async fn pane_title(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::pane_title(target).await?),
        HostKind::Ssh { .. } => {
            let out = ssh_stdout(
                host,
                &format!(
                    "tmux display-message -p -t {} '#{{pane_title}}'",
                    q(target)?
                ),
            )
            .await?;
            Ok(out.trim_matches(|c| c == '\n' || c == '\r').to_string())
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
            // Push-based remote streaming, mirroring the local pipe-pane→tail
            // path: tmux appends the pane's raw output to a per-session log on
            // the *remote* host, which `spawn_remote_pane_tail` follows over one
            // persistent SSH channel. This replaces the old capture-pane polling
            // (700 ms full-screen snapshots), which was the source of the remote
            // terminal lag and flicker. `-o` makes re-arming idempotent.
            ssh_checked(host, &remote_pipe_script(target, out_path)?).await
        }
    }
}

/// Fixed remote directory for per-session pane logs, under the SSH user's home.
/// Used as a `$HOME`-relative shell expression so it resolves on the remote
/// without us having to round-trip for the home path first.
const REMOTE_PANE_DIR: &str = "$HOME/.agentum/panes";

/// Remote pane-log location as a double-quoted shell expression
/// (`"$HOME/.agentum/panes/<uuid>.log"`). The basename is the session's local
/// pane-log filename so the streaming tail addresses the identical file the
/// session-start `pipe_pane` created. `$HOME` expands on the remote; the quotes
/// keep a home dir with spaces a single token. The basename is a UUID
/// (`paths::pane_log`), so it carries no shell-metacharacter risk.
fn remote_pane_log_expr(out_path: &Path) -> Result<String> {
    let name = out_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(HostRuntimeError::Quote)?;
    Ok(format!("\"{REMOTE_PANE_DIR}/{name}\""))
}

/// Build the `sh -c …` script that arms tmux pipe-pane on the remote, writing
/// raw pane output to the per-session log. Factored out so the (untestable
/// without a live host) quoting is at least covered by a string-shape unit test.
fn remote_pipe_script(target: &str, out_path: &Path) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    // tmux runs this command via `/bin/sh -c` on every flush; single-quoting it
    // keeps `$HOME` unexpanded through the outer shells so it resolves there.
    let pipe = format!("cat >> {log}");
    let inner = format!(
        "mkdir -p \"{REMOTE_PANE_DIR}\" && tmux pipe-pane -o -t {} {}",
        q(target)?,
        q(&pipe)?
    );
    // Wrap in `sh -c` so a fish/zsh remote login shell still runs POSIX syntax.
    Ok(format!("sh -c {}", q(&inner)?))
}

/// Build the `sh -c …` script that follows the remote pane log.
///
/// With `from_offset = Some(n)` the tail replays from byte `n` (0-based; `tail
/// -c +N` is 1-based) — the caller sampled `n` together with the capture-pane
/// snapshot ([`capture_pane_with_log_offset`]), so every byte the pane emits
/// while this tail's SSH connection is still handshaking is delivered once it
/// attaches instead of being lost. `None` falls back to `tail -n 0 -f` (start
/// at EOF), for callers with no snapshot to anchor against.
///
/// `touch` avoids a race where the log doesn't exist yet; `exec` lets a kill
/// of the ssh child reap the remote tail cleanly.
fn remote_tail_script(out_path: &Path, from_offset: Option<u64>) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    let mode = match from_offset {
        Some(n) => format!("-c +{}", n.saturating_add(1)),
        None => "-n 0".to_string(),
    };
    let inner = format!("touch {log} 2>/dev/null; exec tail {mode} -f {log}");
    Ok(format!("sh -c {}", q(&inner)?))
}

/// Spawn a long-lived `tail -f` of the remote pane log over a single persistent
/// SSH channel. The caller reads `child.stdout` for raw pane bytes and kills the
/// child on disconnect (also guarded by `kill_on_drop`). SSH hosts only — local
/// sessions tail the on-disk log directly via [`stream_session`].
pub fn spawn_remote_pane_tail(
    host: &Host,
    out_path: &Path,
    from_offset: Option<u64>,
) -> Result<tokio::process::Child> {
    let script = remote_tail_script(out_path, from_offset)?;
    // Tails ride a SEPARATE pooled master ([`SshMux::Streaming`], the `cms-`
    // socket) — NOT the interactive one. Two reasons:
    //   * Connection storm: a dedicated connection per tail meant opening the
    //     app fired one fresh TCP+auth handshake PER remote session at once,
    //     overrunning sshd's `MaxStartups` (10 concurrent) — surplus connects
    //     timed out ("ssh: connect … Operation timed out") and the client showed
    //     "[session stream closed]". One shared streaming master = one connection
    //     per host no matter how many sessions, so no storm.
    //   * Channel budget: keeping tails off the interactive master leaves its
    //     `MaxSessions` channels for keystrokes/title/capture execs. The
    //     streaming master's own budget (10 channels) caps concurrent tails per
    //     host; past that a tail's channel is refused and it exits — far rarer
    //     than the every-session storm the dedicated path caused.
    let mut cmd = ssh_command_opts(host, &script, SshMux::Streaming);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        // Piped (not null) so a transport failure that ends the tail — e.g. the
        // remote refusing the channel — is logged by the caller instead of
        // vanishing behind a bare "stream closed".
        .stderr(std::process::Stdio::piped())
        // If the WS task drops the child without an explicit kill, still reap the
        // local ssh (which SIGHUPs the remote tail) rather than leak a process.
        .kill_on_drop(true);
    Ok(cmd.spawn()?)
}

/// Build the `sh -c …` script behind [`spawn_remote_input_writer`]: a remote
/// read-loop that turns each newline-terminated line of space-separated hex on
/// stdin into one `tmux send-keys -H` for the pane. `$line` is intentionally
/// unquoted so the hex pairs split into separate `send-keys` args. Errors per
/// line (e.g. the pane briefly gone) are swallowed so one bad write never ends
/// the loop. `exec` lets a kill of the ssh child reap it cleanly.
fn remote_input_script(target: &str) -> Result<String> {
    let inner = format!(
        "exec sh -c 'while IFS= read -r l; do tmux send-keys -H -t {} $l 2>/dev/null; done'",
        q(target)?
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

/// Spawn a long-lived keystroke writer over one persistent SSH channel: the
/// caller writes hex-encoded keystroke lines to `child.stdin` and a remote
/// read-loop feeds them to the pane.
///
/// Why this exists: the old path ran one `ssh … tmux send-keys` *exec per
/// keystroke*. Each exec opens a fresh ControlMaster channel and round-trips a
/// command — ~450 ms against a distant host (measured to a 150 ms-RTT box) —
/// so typing into a remote agent was unusable. With a persistent channel a
/// keystroke is just a one-way write down an already-open stream: ~1 RTT
/// (~150 ms) to delivery, no per-key channel setup, no master-channel churn.
///
/// Rides the SHARED ControlMaster (`use_mux = true`), unlike the tail. The
/// master is kept hot by the boot-time/periodic warmer, so opening this channel
/// is ~1 RTT — whereas a dedicated connection pays a full TCP+auth handshake
/// (~2 s over a distant host), which landed entirely on the FIRST keystroke and
/// made opening a remote session feel frozen. Input is low-volume, so its
/// channel barely dents the master's `MaxSessions` budget (the high-volume tail
/// stays dedicated); if the master ever refuses the channel, the writer dies and
/// the caller falls back to per-exec `send_bytes`.
pub fn spawn_remote_input_writer(host: &Host, target: &str) -> Result<tokio::process::Child> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    let script = remote_input_script(target)?;
    let mut cmd = ssh_command_opts(host, &script, SshMux::Interactive);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    Ok(cmd.spawn()?)
}

/// Encode raw keystroke bytes as one space-separated lowercase-hex line
/// (newline-terminated) for the [`spawn_remote_input_writer`] remote loop.
pub fn encode_input_hex_line(bytes: &[u8]) -> Vec<u8> {
    let mut line = Vec::with_capacity(bytes.len() * 3 + 1);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            line.push(b' ');
        }
        line.extend_from_slice(format!("{b:02x}").as_bytes());
    }
    line.push(b'\n');
    line
}

/// Disarm `pipe-pane` on a pane (a bare `tmux pipe-pane` closes the pipe).
/// Used when detaching from an external tmux session: the underlying
/// session must stay alive, but its output should stop feeding our log.
pub async fn unpipe_pane(host: &Host, target: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::unpipe_pane(target).await?),
        HostKind::Ssh { .. } => {
            ssh_checked(host, &format!("tmux pipe-pane -t {}", q(target)?)).await
        }
    }
}

// ───────────────────── external tmux session discovery ─────────────────────

/// One pane of a discovered (non-agentum) tmux session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveredPane {
    /// Foreground process basename (`pane_current_command`).
    pub command: String,
    /// Pane working directory (`pane_current_path`).
    pub cwd: String,
}

/// A tmux session found on a host that agentum does not manage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveredTmuxSession {
    pub name: String,
    /// At least one tmux client is currently attached.
    pub attached: bool,
    /// Session creation time, unix seconds (`session_created`).
    pub created_at: Option<i64>,
    pub panes: Vec<DiscoveredPane>,
}

/// Discovery format: one line per pane, tab-delimited. `list-panes -a`
/// covers every session in a single round trip, so sessions and their
/// pane cwds (used for "related to this project" ranking) come from one
/// SSH exchange.
const TMUX_DISCOVER_FORMAT: &str = "#{session_name}\t#{session_attached}\t#{session_created}\t#{pane_current_command}\t#{pane_current_path}";

/// List the tmux sessions running on `host` that agentum does not manage
/// (anything not named `agentum-*`). "tmux missing" and "no server
/// running" both return an empty list — for discovery they mean the same
/// thing; only transport failures (SSH unreachable/timeout) are errors.
pub async fn list_tmux_sessions(host: &Host) -> Result<Vec<DiscoveredTmuxSession>> {
    Ok(parse_tmux_panes(&tmux_discover_raw(host).await?))
}

/// Like [`list_tmux_sessions`] but returns the agentum-MANAGED (`agentum-*`)
/// sessions instead of the external ones — the basis for the zombie sweep
/// (orphaned panes a crashed/abandoned session left running on a host).
pub async fn list_managed_tmux_sessions(host: &Host) -> Result<Vec<DiscoveredTmuxSession>> {
    Ok(parse_tmux_panes_managed(&tmux_discover_raw(host).await?))
}

/// Run the pane-discovery query on `host`, returning raw stdout. "tmux missing"
/// and "no server running" both collapse to an empty string (for discovery they
/// mean the same — nothing to report); only SSH transport failures error.
async fn tmux_discover_raw(host: &Host) -> Result<String> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::list_panes_all(TMUX_DISCOVER_FORMAT).await?),
        HostKind::Ssh { .. } => {
            let script = format!("tmux list-panes -a -F {}", q(TMUX_DISCOVER_FORMAT)?);
            let output = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            if !output.status.success() {
                // tmux exits 1 for "no server running", the shell exits 127
                // when tmux isn't installed; ssh itself exits 255 on transport
                // failure — only the latter should surface.
                return match output.status.code() {
                    Some(1) | Some(127) => Ok(String::new()),
                    code => Err(HostRuntimeError::NonZero {
                        status: code,
                        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                    }),
                };
            }
            Ok(String::from_utf8(output.stdout)?)
        }
    }
}

/// Parse [`TMUX_DISCOVER_FORMAT`] pane lines into the EXTERNAL (non-`agentum-*`)
/// sessions — the discovery view. Thin wrapper over [`parse_tmux_panes_filtered`].
fn parse_tmux_panes(stdout: &str) -> Vec<DiscoveredTmuxSession> {
    parse_tmux_panes_filtered(stdout, false)
}

/// Parse [`TMUX_DISCOVER_FORMAT`] pane lines into the agentum-MANAGED
/// (`agentum-*`) sessions — the zombie-sweep view.
fn parse_tmux_panes_managed(stdout: &str) -> Vec<DiscoveredTmuxSession> {
    parse_tmux_panes_filtered(stdout, true)
}

/// Parse [`TMUX_DISCOVER_FORMAT`] pane lines into sessions, preserving tmux's
/// order. `managed = false` keeps only EXTERNAL sessions (discovery); `managed =
/// true` keeps only agentum-MANAGED (`agentum-*`) ones (zombie sweep). Tolerant
/// of trailing `\r` and malformed lines.
fn parse_tmux_panes_filtered(stdout: &str, managed: bool) -> Vec<DiscoveredTmuxSession> {
    let mut sessions: Vec<DiscoveredTmuxSession> = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let mut it = line.splitn(5, '\t');
        let (Some(name), Some(attached), Some(created), Some(command), Some(cwd)) =
            (it.next(), it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        // Keep only the requested class: managed (`agentum-*`) vs external.
        if name.starts_with("agentum-") != managed {
            continue;
        }
        let pane = DiscoveredPane {
            command: command.to_string(),
            cwd: cwd.to_string(),
        };
        match sessions.iter_mut().find(|s| s.name == name) {
            Some(s) => s.panes.push(pane),
            None => sessions.push(DiscoveredTmuxSession {
                name: name.to_string(),
                // `session_attached` is the count of attached clients.
                attached: attached
                    .trim()
                    .parse::<u32>()
                    .map(|n| n > 0)
                    .unwrap_or(false),
                created_at: created.trim().parse().ok(),
                panes: vec![pane],
            }),
        }
    }
    sessions
}

/// Decide which agentum-managed (`agentum-*`) tmux sessions on a host are
/// zombies — orphaned panes a crashed/abandoned session left running, safe to
/// kill. A session is a zombie ONLY when ALL hold:
///   - it is managed (`agentum-*`) — external/user sessions never qualify;
///   - it is NOT attached (no tmux client is using it right now);
///   - it is NOT backed by a live (running/idle) store session;
///   - it is NOT `protected` (e.g. an [`EXTERNAL_TMUX_FLAG`] binding, whose
///     underlying tmux is user-owned and must never be killed).
///
/// Pure (no I/O) so the safety invariants can be exhaustively unit-tested. The
/// destructive caller passes the result to [`kill_session`] only on `--yes`.
///
/// [`EXTERNAL_TMUX_FLAG`]: agentum_core::EXTERNAL_TMUX_FLAG
pub fn zombie_tmux_targets(
    on_host: &[DiscoveredTmuxSession],
    live_targets: &HashSet<String>,
    protected_targets: &HashSet<String>,
) -> Vec<String> {
    on_host
        .iter()
        .filter(|s| {
            s.name.starts_with("agentum-")
                && !s.attached
                && !live_targets.contains(&s.name)
                && !protected_targets.contains(&s.name)
        })
        .map(|s| s.name.clone())
        .collect()
}

pub async fn ssh_stdout(host: &Host, script: &str) -> Result<String> {
    let output = ssh_output(host, script, SSH_TIMEOUT)
        .await
        .map_err(map_ssh_io)?;
    if !output.status.success() {
        return Err(HostRuntimeError::NonZero {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8(output.stdout)?)
}

// ───────────────────────── host-aware git / fs ─────────────────────────
// Generic command plumbing so the repo/worktree/git routes run their git
// (and the few fs touches around it) on the *repo's host*: directly when
// the host is `Local`, over `ssh` when it's `Ssh`. The SSH form always
// wraps in `sh -c` for the same reason every other remote path does — the
// login shell may be fish/zsh, which reject the bash/POSIX `&&`/`cd` we
// build here. See `fs::list_remote_dir`.

/// `git worktree add` (and a clone-from-scratch checkout) can take far
/// longer than the 12s probe budget, so host-aware git gets its own,
/// roomier timeout. Still bounded so a hung remote can't wedge a request.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Captured output of a command run on a host. Unlike [`ssh_stdout`], a
/// non-zero exit is NOT an error — callers inspect `success`/`stderr`
/// themselves, because git uses exit codes to signal *expected* states
/// (a branch that "already exists", a path absent at a revision, …).
#[derive(Debug)]
pub struct HostCommandOutput {
    pub success: bool,
    /// Process exit code, when known. For SSH this is the *remote* command's
    /// code (ssh forwards it). `None` if the process was signalled. Callers
    /// that branch on specific codes (e.g. `git check-ignore`: 0/1/≥2) need
    /// this; most only read `success`.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl HostCommandOutput {
    /// stdout as lossy UTF-8 (callers trim as needed).
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Run `git <args>` with `cwd` as the working directory on `host`.
/// Local → `git -C <cwd> <args>`; SSH → `sh -c 'cd <cwd> && git <args>'`
/// with every token shell-quoted. A non-zero git exit is reported via
/// `success`, not as an `Err` (only transport/timeout failures error).
pub async fn git_in_dir(host: &Host, cwd: &str, args: &[&str]) -> Result<HostCommandOutput> {
    match &host.kind {
        HostKind::Local => {
            let out = Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .output()
                .await?;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                stdout: out.stdout,
            })
        }
        HostKind::Ssh { .. } => {
            let mut inner = format!("cd {} && git", q(cwd)?);
            for a in args {
                inner.push(' ');
                inner.push_str(&q(a)?);
            }
            let script = format!("sh -c {}", q(&inner)?);
            let out = ssh_output(host, &script, GIT_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(HostCommandOutput {
                success: out.status.success(),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                stdout: out.stdout,
            })
        }
    }
}

/// True when `cwd` is inside a git work tree on `host`
/// (`git rev-parse --is-inside-work-tree`). Host-aware replacement for
/// `crate::git::is_git_repo`, which is local-only.
pub async fn is_git_repo(host: &Host, cwd: &str) -> bool {
    git_in_dir(host, cwd, &["rev-parse", "--is-inside-work-tree"])
        .await
        .map(|o| o.success)
        .unwrap_or(false)
}

/// `mkdir -p <path>` on `host`. The worktree routes need the
/// `.claude/worktrees` parent to exist before `git worktree add`.
pub async fn mkdir_p(host: &Host, path: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => {
            tokio::fs::create_dir_all(path).await?;
            Ok(())
        }
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("mkdir -p {}", q(path)?))?);
            ssh_checked(host, &script).await
        }
    }
}

/// Read a file's raw bytes from `host`, or `None` when it doesn't exist.
/// Used for the `worktree` revision of a git diff (the on-disk content,
/// which may differ from index/HEAD). SSH reads via `cat`; a missing file
/// exits non-zero → `None`, mirroring the local `NotFound` branch.
pub async fn read_file_bytes(host: &Host, abs_path: &str) -> Result<Option<Vec<u8>>> {
    match &host.kind {
        HostKind::Local => match tokio::fs::read(abs_path).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        },
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("cat {}", q(abs_path)?))?);
            let out = ssh_output(host, &script, GIT_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(out.status.success().then_some(out.stdout))
        }
    }
}

/// Whether `abs_path` exists on `host` (`test -e` over SSH). Used by the
/// diff route to decide whether an empty `git diff` means "untracked file"
/// (so it can synthesise a diff) versus "no change".
pub async fn path_exists(host: &Host, abs_path: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => Ok(tokio::fs::try_exists(abs_path).await.unwrap_or(false)),
        HostKind::Ssh { .. } => {
            let script = format!("sh -c {}", q(&format!("test -e {}", q(abs_path)?))?);
            let out = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            Ok(out.status.success())
        }
    }
}

/// Build the inner POSIX script that writes `content` to `$HOME/<rel_path>` on
/// the host, creating the parent dir and keeping the file owner-only. `$HOME` is
/// left for the remote `sh` to expand (the login shell may be fish/zsh, so this
/// inner script is base64-piped to `sh` by [`write_home_relative_file`], never
/// run directly). `content` rides as base64 so any payload writes verbatim.
/// `rel_path` must be a caller-controlled safe slug path (embedded in quotes).
fn marker_inner_script(rel_path: &str, content: &str) -> Result<String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(content);
    let mkdir = match rel_path.rsplit_once('/') {
        Some((parent, _)) => format!("mkdir -p \"$HOME/{parent}\"; "),
        None => String::new(),
    };
    Ok(format!(
        "umask 077; {mkdir}printf %s {b64} | base64 -d > \"$HOME/{rel_path}\"",
        b64 = q(&b64)?,
    ))
}

/// Write `content` to `$HOME/<rel_path>` on `host` (the local home, or the SSH
/// host's home), owner-only, creating parents. Unlike [`write_remote_file`] this
/// resolves the host's `$HOME` so callers can drop a marker without knowing the
/// absolute home path. Used for the host-browser per-worktree port marker.
pub async fn write_home_relative_file(host: &Host, rel_path: &str, content: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => {
            let home = std::env::var("HOME")
                .map_err(|_| HostRuntimeError::Bootstrap("no HOME for local marker".into()))?;
            // The local branch of `write_remote_file` mkdir-p's + chmods 600.
            write_remote_file(host, &format!("{home}/{rel_path}"), content).await
        }
        HostKind::Ssh { .. } => {
            let inner = marker_inner_script(rel_path, content)?;
            let inner_b64 = base64::engine::general_purpose::STANDARD.encode(&inner);
            // Only base64 chars in the outer command, so fish/zsh/bash run it the
            // same; the decoded inner runs under `sh`, where `$HOME` expands.
            let remote = format!("printf %s {} | base64 -d | sh", q(&inner_b64)?);
            ssh_checked(host, &remote).await
        }
    }
}

async fn ssh_checked(host: &Host, script: &str) -> Result<()> {
    let output = ssh_output(host, script, SSH_TIMEOUT)
        .await
        .map_err(map_ssh_io)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(HostRuntimeError::NonZero {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn q(s: &str) -> Result<std::borrow::Cow<'_, str>> {
    shlex::try_quote(s).map_err(|_| HostRuntimeError::Quote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_binaries_dedups_and_orders_required_first() {
        let bins = probe_binaries();
        assert_eq!(bins[0], "tmux");
        assert_eq!(bins[1], "git");
        // cursor maps to cursor-agent; both should appear once.
        assert!(bins.iter().any(|b| b == "cursor-agent"));
        assert!(bins.iter().any(|b| b == "claude"));
        // No duplicates.
        let mut sorted = bins.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), bins.len(), "probe_binaries has duplicates");
    }

    #[test]
    fn parse_probe_output_extracts_fields() {
        // Representative output of the remote preflight script: tmux
        // present, git missing (empty path), one agent installed, a path
        // with a space to prove we don't split it.
        let stdout = "uname\tLinux 6.12.1-arch1-1\n\
             pkg\tpacman\n\
             sudo\tyes\n\
             bin\ttmux\t/usr/bin/tmux\n\
             bin\tgit\t\n\
             bin\tclaude\t/home/me/my tools/claude\n";
        let probe = parse_probe_output(stdout);
        assert_eq!(probe.uname.as_deref(), Some("Linux 6.12.1-arch1-1"));
        assert_eq!(probe.pkg_manager, "pacman");
        assert_eq!(probe.sudo_nopasswd, Some(true));
        assert_eq!(probe.bins.get("tmux").unwrap(), "/usr/bin/tmux");
        assert_eq!(probe.bins.get("git").unwrap(), "");
        assert_eq!(
            probe.bins.get("claude").unwrap(),
            "/home/me/my tools/claude",
            "paths with spaces must survive parsing"
        );
    }

    #[test]
    fn parse_probe_output_handles_crlf_and_blank_lines() {
        let stdout = "uname\tDarwin 24.0\r\n\npkg\tbrew\r\nbin\ttmux\t/opt/homebrew/bin/tmux\r\n";
        let probe = parse_probe_output(stdout);
        assert_eq!(probe.uname.as_deref(), Some("Darwin 24.0"));
        assert_eq!(probe.pkg_manager, "brew");
        assert_eq!(probe.bins.get("tmux").unwrap(), "/opt/homebrew/bin/tmux");
    }

    #[test]
    fn parse_probe_output_defaults_pkg_to_unknown() {
        let probe = parse_probe_output("uname\tLinux\n");
        assert_eq!(probe.pkg_manager, "unknown");
    }

    // The `ssh_command` builder + its argv tests moved to
    // `agentum_tmux::ssh` (the shared lower crate the watchdog can depend
    // on). This module no longer owns the builder.

    #[test]
    fn assemble_readiness_ok_when_required_present() {
        let mut bins = HashMap::new();
        bins.insert("tmux".to_string(), "/usr/bin/tmux".to_string());
        bins.insert("git".to_string(), "/usr/bin/git".to_string());
        bins.insert("claude".to_string(), "/usr/local/bin/claude".to_string());
        let r = assemble_readiness(ProbeOutput {
            uname: Some("Linux 6.12".to_string()),
            pkg_manager: "pacman".to_string(),
            sudo_nopasswd: Some(true),
            bins,
        });
        assert!(r.ok);
        assert_eq!(r.message, "host ready");
        assert_eq!(r.system.sudo_nopasswd, Some(true));
        assert!(r.required.iter().all(|d| d.installed));
        let claude = r.agents.iter().find(|a| a.id == "claude").unwrap();
        assert!(claude.installed);
        assert_eq!(claude.path.as_deref(), Some("/usr/local/bin/claude"));
        let codex = r.agents.iter().find(|a| a.id == "codex").unwrap();
        assert!(!codex.installed);
    }

    #[test]
    fn assemble_readiness_blocks_when_tmux_missing() {
        let mut bins = HashMap::new();
        bins.insert("git".to_string(), "/usr/bin/git".to_string());
        let r = assemble_readiness(ProbeOutput {
            uname: None,
            pkg_manager: "apt".to_string(),
            sudo_nopasswd: Some(false),
            bins,
        });
        assert!(!r.ok);
        assert!(
            r.message.contains("tmux"),
            "message names the gap: {}",
            r.message
        );
        let tmux = r.required.iter().find(|d| d.id == "tmux").unwrap();
        assert!(!tmux.installed);
    }

    #[test]
    fn unreachable_readiness_carries_error_and_blocks() {
        let r = unreachable_readiness("ssh: connect to host failed".to_string());
        assert!(!r.ok);
        assert_eq!(r.message, "ssh: connect to host failed");
        assert!(r.required.iter().all(|d| !d.installed));
        assert!(r.agents.iter().all(|a| !a.installed));
    }

    #[test]
    fn build_probe_script_quotes_each_binary() {
        let script = build_probe_script(&["tmux".to_string(), "cursor-agent".to_string()]).unwrap();
        assert!(script.contains("uname -sr"));
        assert!(script.contains("command -v tmux"));
        assert!(script.contains("command -v cursor-agent"));
        // package-manager detection probes all four managers.
        for pm in ["apt-get", "dnf", "pacman", "brew"] {
            assert!(script.contains(pm), "script probes {pm}");
        }
    }

    // ── remote pane streaming (SSH push path) ─────────────────────────────
    // The SSH backend needs a live host to truly validate, but the shell
    // scripts these build are pure functions of (target, log path) — cover
    // their shape so a quoting regression can't silently break remote streams.

    #[test]
    fn remote_pane_log_expr_is_home_relative_and_keeps_basename() {
        let p = std::path::Path::new("/home/me/.cache/agentum/sessions/abc-123.log");
        let expr = remote_pane_log_expr(p).unwrap();
        // `$HOME` so it resolves on the remote; quoted so a home dir with
        // spaces stays one token; basename is the session's local log name.
        assert_eq!(expr, "\"$HOME/.agentum/panes/abc-123.log\"");
    }

    #[test]
    fn remote_pipe_script_arms_pipe_pane_to_session_log() {
        let p = std::path::Path::new("/x/sessions/sess-1.log");
        let script = remote_pipe_script("agentum-demo", p).unwrap();
        // Wrapped for fish/zsh logins, makes the dir, idempotently arms the
        // pipe, and routes raw pane output into the home-relative session log.
        assert!(script.starts_with("sh -c "), "not sh-wrapped: {script}");
        assert!(script.contains("mkdir -p"), "no mkdir: {script}");
        assert!(
            script.contains("tmux pipe-pane -o -t"),
            "no idempotent pipe-pane: {script}"
        );
        assert!(script.contains("agentum-demo"), "target missing: {script}");
        assert!(
            script.contains("sess-1.log"),
            "log basename missing: {script}"
        );
        assert!(script.contains("cat >>"), "not an append sink: {script}");
        assert!(script.contains("$HOME"), "log not home-relative: {script}");
    }

    #[test]
    fn remote_tail_script_follows_log_from_eof_without_offset() {
        let p = std::path::Path::new("/x/sessions/sess-2.log");
        let script = remote_tail_script(p, None).unwrap();
        assert!(script.starts_with("sh -c "), "not sh-wrapped: {script}");
        // No anchor → `-n 0` starts at EOF; `exec` lets a child kill reap the
        // remote tail cleanly.
        assert!(script.contains("tail -n 0 -f"), "wrong tail mode: {script}");
        assert!(script.contains("exec tail"), "tail not exec'd: {script}");
        assert!(
            script.contains("sess-2.log"),
            "log basename missing: {script}"
        );
    }

    #[test]
    fn remote_tail_script_replays_from_snapshot_offset() {
        let p = std::path::Path::new("/x/sessions/sess-3.log");
        // Offset 41 (0-based bytes already covered by the snapshot) → tail's
        // 1-based `-c +42`, so byte 41 onward replays once the tail attaches —
        // nothing emitted during the tail's SSH handshake is lost.
        let script = remote_tail_script(p, Some(41)).unwrap();
        assert!(
            script.contains("tail -c +42 -f"),
            "wrong tail mode: {script}"
        );
        assert!(script.contains("exec tail"), "tail not exec'd: {script}");
    }

    #[test]
    fn remote_input_script_feeds_hex_lines_to_send_keys() {
        let script = remote_input_script("agentum-demo").unwrap();
        assert!(script.starts_with("sh -c "), "not sh-wrapped: {script}");
        // A read-loop turning stdin hex lines into send-keys; $l unquoted so the
        // hex pairs split into separate args. exec lets a kill reap it.
        assert!(
            script.contains("while IFS= read -r l"),
            "no read loop: {script}"
        );
        assert!(
            script.contains("send-keys -H -t"),
            "no hex send-keys: {script}"
        );
        assert!(script.contains("agentum-demo"), "target missing: {script}");
        assert!(script.contains("exec sh -c"), "loop not exec'd: {script}");
    }

    #[test]
    fn encode_input_hex_line_is_space_separated_and_newline_terminated() {
        // "hi" → "68 69\n"; the remote `send-keys -H 68 69` reproduces the bytes.
        assert_eq!(encode_input_hex_line(b"hi"), b"68 69\n");
        // Control bytes (e.g. CR, ESC) encode the same way — raw, lossless.
        assert_eq!(encode_input_hex_line(b"\r"), b"0d\n");
        assert_eq!(encode_input_hex_line(&[0x1b, 0x5b, 0x41]), b"1b 5b 41\n");
        assert_eq!(encode_input_hex_line(b""), b"\n");
    }

    #[test]
    fn snapshot_with_offset_script_samples_size_cursor_then_grid() {
        let p = std::path::Path::new("/x/sessions/sess-4.log");
        let script = snapshot_with_offset_script("agentum-demo", p).unwrap();
        assert!(script.starts_with("sh -c "), "not sh-wrapped: {script}");
        // Load-bearing ordering: the log size (`wc -c`) is read AFTER
        // `capture-pane`, so the tail resumes past what the snapshot painted and
        // never replays a byte — the fix for cursor-agent's relative-redraw
        // stacking. The cursor is sampled with the capture to anchor the grid.
        let pipe = script.find("pipe-pane").expect("no pipe-pane arm");
        let cur = script.find("display-message").expect("no cursor sample");
        let cap = script.find("capture-pane").expect("no grid capture");
        let wc = script.find("wc -c").expect("no size probe");
        // pipe-pane armed first (folded into this exec to save a round trip),
        // then cursor+capture, then the size LAST (no-overlap ordering).
        assert!(pipe < cur, "pipe-pane not armed before capture: {script}");
        assert!(
            cap < wc,
            "size sampled before capture (would re-replay): {script}"
        );
        assert!(cur < cap, "cursor sampled after capture: {script}");
        assert!(
            script.contains("cursor_x") && script.contains("cursor_flag"),
            "cursor formats missing: {script}"
        );
        // Fallbacks keep the 3-section shape when the log or pane is absent.
        assert!(script.contains("echo 0"), "no size fallback: {script}");
        assert!(script.contains("echo X"), "no cursor fallback: {script}");
        assert!(script.contains("sess-4.log"), "log missing: {script}");
    }

    // ── external tmux session discovery ───────────────────────────────────

    #[test]
    fn parse_tmux_panes_groups_panes_and_skips_agentum_sessions() {
        // Three sessions on the host: a 2-pane dev session, an attached
        // detached-name edge case, and an agentum-managed one to exclude.
        let stdout = "dev\t1\t1718000000\tnvim\t/home/me/proj\n\
             dev\t1\t1718000000\tcargo\t/home/me/proj/crates\n\
             scratch\t0\t1718001234\tbash\t/tmp\n\
             agentum-alpha\t0\t1718002222\tclaude\t/home/me/proj\n";
        let sessions = parse_tmux_panes(stdout);
        assert_eq!(sessions.len(), 2, "agentum-* must be excluded");
        let dev = &sessions[0];
        assert_eq!(dev.name, "dev");
        assert!(dev.attached);
        assert_eq!(dev.created_at, Some(1718000000));
        assert_eq!(dev.panes.len(), 2);
        assert_eq!(dev.panes[1].command, "cargo");
        assert_eq!(dev.panes[1].cwd, "/home/me/proj/crates");
        let scratch = &sessions[1];
        assert!(!scratch.attached);
        assert_eq!(scratch.panes.len(), 1);
    }

    #[test]
    fn parse_tmux_panes_handles_empty_crlf_and_malformed_lines() {
        assert!(parse_tmux_panes("").is_empty());
        // CRLF endings and a short (malformed) line must not panic or leak in.
        let stdout = "dev\t2\t1718000000\tzsh\t/srv/app\r\n\nbroken-line\n";
        let sessions = parse_tmux_panes(stdout);
        assert_eq!(sessions.len(), 1);
        assert!(
            sessions[0].attached,
            "2 attached clients counts as attached"
        );
        assert_eq!(sessions[0].panes[0].cwd, "/srv/app");
    }

    #[test]
    fn parse_tmux_panes_managed_keeps_only_agentum_sessions() {
        // Inverse of the discovery view: external sessions drop, `agentum-*` stay.
        let stdout = "dev\t1\t1\tnvim\t/p\n\
             agentum-alpha\t0\t2\tclaude\t/p\n\
             agentum-beta\t1\t3\tcodex\t/q\n";
        let sessions = parse_tmux_panes_managed(stdout);
        assert_eq!(sessions.len(), 2, "only agentum-* kept");
        assert_eq!(sessions[0].name, "agentum-alpha");
        assert!(!sessions[0].attached);
        assert_eq!(sessions[1].name, "agentum-beta");
        assert!(sessions[1].attached);
    }

    fn tmux(name: &str, attached: bool) -> DiscoveredTmuxSession {
        DiscoveredTmuxSession {
            name: name.to_string(),
            attached,
            created_at: None,
            panes: Vec::new(),
        }
    }

    #[test]
    fn zombie_sweep_kills_only_orphaned_unattached_managed_panes() {
        let on_host = vec![
            tmux("agentum-live", false), // backed by a live session → keep
            tmux("agentum-dead", false), // orphaned + unattached → ZOMBIE
            tmux("agentum-busy", true),  // orphaned but attached → keep
            tmux("agentum-ext", false),  // EXTERNAL_TMUX_FLAG binding → keep
        ];
        let live: HashSet<String> = ["agentum-live".to_string()].into_iter().collect();
        let protected: HashSet<String> = ["agentum-ext".to_string()].into_iter().collect();

        let zombies = zombie_tmux_targets(&on_host, &live, &protected);
        assert_eq!(zombies, vec!["agentum-dead".to_string()]);
    }

    #[test]
    fn zombie_sweep_never_touches_external_or_attached_sessions() {
        // A non-managed (user) session is never a zombie even if unattached and
        // absent from the store; an attached managed orphan is never a zombie.
        let on_host = vec![
            tmux("dev", false),           // not agentum-* → never killed
            tmux("scratch", false),       // not agentum-* → never killed
            tmux("agentum-orphan", true), // attached → never killed
        ];
        let empty = HashSet::new();
        assert!(zombie_tmux_targets(&on_host, &empty, &empty).is_empty());
    }

    // ── host-aware git/fs, Local backend ──────────────────────────────────
    // The SSH backend can't be unit-tested without a live host (the only true
    // validation — see the plan), but the Local branch is the same code the
    // worktree/git routes hit for a local repo, so exercising it guards the
    // refactor that routed every git call through `git_in_dir`.

    fn local_host() -> Host {
        Host {
            id: agentum_core::LOCAL_HOST_ID,
            name: "local".into(),
            kind: HostKind::Local,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    #[tokio::test]
    async fn git_in_dir_local_reports_repo_and_propagates_exit_code() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let host = local_host();

        // Not a repo yet: rev-parse fails (non-zero), but the call itself Ok's
        // — git's exit code rides on `success`, not an Err.
        let before = git_in_dir(&host, &path, &["rev-parse", "--is-inside-work-tree"])
            .await
            .unwrap();
        assert!(!before.success);
        assert!(!is_git_repo(&host, &path).await);

        let init = git_in_dir(&host, &path, &["init"]).await.unwrap();
        assert!(init.success, "git init stderr: {}", init.stderr);

        let after = git_in_dir(&host, &path, &["rev-parse", "--is-inside-work-tree"])
            .await
            .unwrap();
        assert!(after.success);
        assert_eq!(after.stdout_string().trim(), "true");
        assert!(is_git_repo(&host, &path).await);
    }

    #[tokio::test]
    async fn mkdir_p_creates_nested_and_path_exists_tracks_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let nested = format!("{}/.claude/worktrees", dir.path().to_string_lossy());
        let host = local_host();

        assert!(!path_exists(&host, &nested).await.unwrap());
        mkdir_p(&host, &nested).await.unwrap();
        assert!(path_exists(&host, &nested).await.unwrap());
    }

    #[tokio::test]
    async fn read_file_bytes_local_reads_present_and_none_for_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = format!("{}/hello.txt", dir.path().to_string_lossy());
        let host = local_host();

        assert!(read_file_bytes(&host, &file).await.unwrap().is_none());
        tokio::fs::write(&file, b"hi there").await.unwrap();
        assert_eq!(
            read_file_bytes(&host, &file).await.unwrap().as_deref(),
            Some(&b"hi there"[..])
        );
    }

    // ── CDP forward-tunnel port selection ─────────────────────────────────
    // The forward (-L) tunnel binds a Mac loopback port; these pure helpers are
    // the moving parts of `ensure_forward_tunnel`'s scan. A live host validates
    // the I/O, but the range + bind-criterion are pure and tested here so a
    // regression can't silently collide with the MCP range or mis-read ssh.

    #[test]
    fn cdp_forward_range_is_24_ports_at_9200_disjoint_from_mcp() {
        let ports: Vec<u16> = forward_tunnel_ports().collect();
        assert_eq!(REMOTE_CDP_PORT_BASE, 9200, "CDP base moved");
        assert_eq!(ports.len(), 24, "must scan 24 candidate Mac ports");
        assert_eq!(
            ports[0], REMOTE_CDP_PORT_BASE,
            "scan must start at the base"
        );
        assert_eq!(*ports.last().unwrap(), REMOTE_CDP_PORT_BASE + 23);
        // The reverse MCP tunnel and this forward CDP tunnel ride one
        // ControlMaster — their port ranges must not overlap or arming one would
        // cancel/clobber the other.
        let mcp: std::collections::HashSet<u16> = (REMOTE_MCP_PORT_BASE
            ..REMOTE_MCP_PORT_BASE.saturating_add(REMOTE_MCP_PORT_TRIES))
            .collect();
        assert!(
            ports.iter().all(|p| !mcp.contains(p)),
            "CDP forward range overlaps the MCP reverse range"
        );
    }

    #[test]
    fn forward_arm_bound_treats_success_and_already_established_as_bound() {
        // A clean `-O forward` exit means the tunnel bound.
        assert!(forward_arm_bound(true, ""));
        // Cancel-then-arm can race a still-present forward; ssh reporting it as
        // already established is an idempotent success, not a failure.
        assert!(forward_arm_bound(false, "forwarding already in place"));
        assert!(
            forward_arm_bound(false, "remote forward EXISTS"),
            "must be case-insensitive"
        );
        // A generic forwarding failure means this port is unusable → scan the next.
        assert!(!forward_arm_bound(
            false,
            "could not request local forwarding"
        ));
    }

    #[test]
    fn marker_inner_script_is_home_relative_and_owner_only() {
        let script = marker_inner_script("hostbrowser/demo.port", "9222\n").unwrap();
        // $HOME must stay UNQUOTED-but-double-quoted so it expands on the host
        // (whose login shell may be fish/zsh) while the path stays one token.
        assert!(
            script.contains("\"$HOME/hostbrowser/demo.port\""),
            "marker path not home-relative: {script}"
        );
        assert!(
            script.contains("mkdir -p \"$HOME/hostbrowser\""),
            "parent dir not created: {script}"
        );
        // Content rides as base64 so a payload can't break the write.
        assert!(
            script.contains("base64 -d"),
            "content not base64-decoded: {script}"
        );
        // The marker is a per-user file; keep it owner-only.
        assert!(
            script.contains("umask 077") || script.contains("chmod 600"),
            "marker not owner-only: {script}"
        );
    }
}
