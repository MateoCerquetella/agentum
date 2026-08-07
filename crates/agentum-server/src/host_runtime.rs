//! Local/SSH host execution helpers.
//!
//! The SSH backend intentionally drives only stock `ssh` + `tmux` on the
//! remote machine. The remote host never needs an `agentum` binary.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, OnceLock, Weak};
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
    SshMux, is_mux_transport_error, ssh_close_control_masters, ssh_command_opts,
    ssh_control_exit_cmd, ssh_control_forward_cmd, ssh_control_local_cancel_cmd,
    ssh_control_local_forward_cmd, ssh_output, ssh_output_on, ssh_retire_legacy_control_masters,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};
use uuid::Uuid;

const SSH_TIMEOUT: Duration = Duration::from_secs(12);

/// The launch transaction includes a cold (unmultiplexed) SSH handshake and a
/// short remote liveness window. Keep it bounded, but leave enough headroom for
/// password/key authentication on a high-latency link.
const REMOTE_LAUNCH_TIMEOUT: Duration = Duration::from_secs(18);
const SSH_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const SSH_CONTROL_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const SSH_CONTROL_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const SSH_REAP_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_DIAGNOSTIC_BYTES: usize = 8 * 1024;

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
    #[error("remote {stage} prerequisite failed: {message}")]
    RemotePrerequisite {
        stage: &'static str,
        message: String,
    },
    #[error("SSH {stage} failed for host `{host}`: {message}")]
    SshStage {
        stage: &'static str,
        host: String,
        message: String,
    },
    #[error("remote tmux setup failed during {stage} (status {status:?}): {stderr}")]
    TmuxSetup {
        stage: &'static str,
        status: Option<i32>,
        stderr: String,
    },
    #[error("remote `{program}` exited during startup with status {status:?}{diagnostic}")]
    EarlyExit {
        program: String,
        status: Option<i32>,
        /// Includes its own leading newline when non-empty, keeping the normal
        /// one-line error compact while making pane diagnostics readable.
        diagnostic: String,
    },
    #[error("SSH tunnel {operation} failed: {message}")]
    Tunnel {
        operation: &'static str,
        message: String,
    },
    #[error("{0}")]
    Bootstrap(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tmux(#[from] agentum_tmux::TmuxError),
}

impl HostRuntimeError {
    /// Whether tmux rejected an operation because its target disappeared.
    ///
    /// A selected session can be removed between a client's layout pass and
    /// the resize command reaching the host. That race is expected lifecycle
    /// churn, unlike an SSH transport, auth, or command failure. Keep the
    /// classifier narrow so callers can suppress only the stale-target case.
    pub fn is_tmux_target_missing(&self) -> bool {
        fn stderr_says_target_missing(stderr: &str) -> bool {
            let stderr = stderr.to_ascii_lowercase();
            [
                "can't find window:",
                "can't find session:",
                "can't find pane:",
                "no server running on ",
            ]
            .iter()
            .any(|needle| stderr.contains(needle))
        }

        match self {
            Self::NonZero { stderr, .. } => stderr_says_target_missing(stderr),
            Self::Tmux(agentum_tmux::TmuxError::NonZero { stderr, .. }) => {
                stderr_says_target_missing(stderr)
            }
            _ => false,
        }
    }
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
    let stdout = ssh_stdout(host, &remote_home_script()?).await?;
    let Some(home) = parse_remote_home(&stdout) else {
        return Err(HostRuntimeError::Bootstrap(
            "could not resolve remote $HOME".into(),
        ));
    };
    Ok(home)
}

fn remote_home_script() -> Result<String> {
    // A user's fish/zsh init may print to stdout even for `shell -c`. Prefix and
    // base64 the one machine-readable record so callers can ignore that noise
    // without corrupting a HOME containing spaces.
    let inner =
        "printf 'AGENTUM_HOME\\t'; printf %s \"$HOME\" | base64 | tr -d '\\r\\n'; printf '\\n'";
    Ok(format!("sh -c {}", q(inner)?))
}

fn parse_remote_home(stdout: &str) -> Option<String> {
    let encoded = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("AGENTUM_HOME\t"))?;
    let home = decode_protocol_field(encoded)?;
    (!home.is_empty() && home.starts_with('/')).then_some(home)
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

// ──────────────────── remote launch preflight / transaction ────────────────────

/// Host-derived launch inputs. These are deliberately resolved before tunnel
/// setup or tmux mutation so a missing directory/tool fails without leaving
/// partial remote state behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLaunchPreflight {
    pub home: String,
    pub workdir: PathBuf,
    /// Absolute executable for `argv[0]`. For `terminal`, this equals `shell`.
    pub executable: String,
    /// Absolute, usable remote login shell (falling back to `/bin/sh`).
    pub shell: String,
    /// PATH captured from the fresh SSH preflight. A long-lived tmux server can
    /// retain an older PATH, which also breaks an otherwise-absolute launcher
    /// whose shebang uses `/usr/bin/env` to find its interpreter.
    pub fresh_path: String,
    pub claude_transcript_exists: bool,
}

/// Apply Agentum's documented remote-workdir rules without consulting the
/// local machine: `~` and `~/...` use remote HOME, absolute paths stay
/// absolute, and every other path is relative to remote HOME.
fn resolve_remote_workdir(home: &str, requested: &str) -> PathBuf {
    if requested == "~" {
        return PathBuf::from(home);
    }
    if let Some(rest) = requested.strip_prefix("~/") {
        return Path::new(home).join(rest);
    }
    if Path::new(requested).is_absolute() {
        return PathBuf::from(requested);
    }
    Path::new(home).join(requested)
}

fn ssh_actionable_message(stderr: &str) -> String {
    let clean = bounded_text(stderr.trim(), STARTUP_DIAGNOSTIC_BYTES);
    let lower = clean.to_ascii_lowercase();
    let hint = if lower.contains("permission denied")
        || lower.contains("authentication failed")
        || lower.contains("no supported authentication")
    {
        "authentication was rejected; verify the saved SSH user and credential"
    } else if lower.contains("could not resolve hostname")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname")
    {
        "the hostname could not be resolved; verify the saved hostname"
    } else if lower.contains("connection refused") {
        "the SSH service refused the connection; verify the host and port"
    } else if lower.contains("timed out") || lower.contains("operation timed out") {
        "the SSH connection timed out; verify network reachability and the SSH port"
    } else if lower.contains("no route to host") || lower.contains("network is unreachable") {
        "the SSH host is unreachable from this machine"
    } else if lower.contains("host key verification failed") {
        "host-key verification failed; remove or update the stale known_hosts entry"
    } else {
        "the SSH transport or remote command failed"
    };
    if clean.is_empty() {
        hint.to_string()
    } else {
        format!("{hint} ({clean})")
    }
}

fn ssh_stage_error(
    host: &Host,
    stage: &'static str,
    status: Option<i32>,
    stderr: &str,
) -> HostRuntimeError {
    if status == Some(255) {
        HostRuntimeError::SshStage {
            stage,
            host: host.name.clone(),
            message: ssh_actionable_message(stderr),
        }
    } else {
        HostRuntimeError::TmuxSetup {
            stage,
            status,
            stderr: if stderr.trim().is_empty() {
                "remote command returned no diagnostic".to_string()
            } else {
                bounded_text(stderr.trim(), STARTUP_DIAGNOSTIC_BYTES)
            },
        }
    }
}

fn ssh_stage_io(host: &Host, stage: &'static str, error: std::io::Error) -> HostRuntimeError {
    let message = if error.kind() == std::io::ErrorKind::TimedOut {
        "the SSH operation timed out; verify network reachability and authentication".to_string()
    } else {
        format!("the SSH client could not complete the operation ({error})")
    };
    HostRuntimeError::SshStage {
        stage,
        host: host.name.clone(),
        message,
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn decode_protocol_field(value: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn preflight_validation_script(workdir: &Path, tool: &str, session_id: Uuid) -> Result<String> {
    let wd = workdir.to_str().ok_or(HostRuntimeError::Quote)?;
    let binary = if tool == "terminal" {
        ""
    } else {
        binary_for(tool)
    };
    let binary_error =
        format!("selected executable `{binary}` is not installed or executable on the remote PATH");
    let inner = format!(
        r#"encode() {{ printf %s "$1" | base64 | tr -d '\r\n'; }};
fail() {{ printf 'AGENTUM_PREFLIGHT_ERROR\t%s\t%s\n' "$1" "$(encode "$2")"; exit 64; }};
resolve_bin() {{ c=$1; case "$c" in /*|*/*) p=$c;; *) p=$(command -v "$c" 2>/dev/null) || return 1;; esac; [ -f "$p" ] && [ -x "$p" ] || return 1; case "$p" in /*) ;; *) d=$(dirname "$p") || return 1; b=$(basename "$p") || return 1; d=$(CDPATH= cd -P "$d" 2>/dev/null && pwd) || return 1; p=$d/$b;; esac; printf %s "$p"; }};
wd={wd};
[ -d "$wd" ] || fail workdir "$wd does not exist or is not a directory";
[ -x "$wd" ] || fail workdir "$wd is not accessible by the SSH user";
wd=$(CDPATH= cd -P "$wd" 2>/dev/null && pwd) || fail workdir "$wd could not be resolved";
tmux_bin=$(resolve_bin tmux) || fail tmux "tmux is not installed or executable";
commands=$("$tmux_bin" list-commands 2>/dev/null) || fail tmux "tmux could not report its supported commands";
for c in new-session list-sessions respawn-pane pipe-pane set-option capture-pane display-message kill-session; do case "$commands" in *"$c "*) ;; *) fail tmux "tmux is incompatible: required command $c is unavailable";; esac; done;
remote_shell=$(resolve_bin "${{SHELL:-/bin/sh}}" 2>/dev/null) || remote_shell=$(resolve_bin /bin/sh 2>/dev/null) || fail shell "neither the remote login shell nor /bin/sh is executable";
if [ {terminal} -eq 1 ]; then selected=$remote_shell; else selected=$(resolve_bin {binary}) || fail binary {binary_error}; fi;
encoded=$(printf %s "$wd" | tr '/' '-'); transcript=0; [ -f "$HOME/.claude/projects/$encoded/{session_id}.jsonl" ] && transcript=1;
printf 'AGENTUM_PREFLIGHT_OK\t%s\t%s\t%s\t%s\t%s\n' "$(encode "$wd")" "$(encode "$selected")" "$(encode "$remote_shell")" "$(encode "${{PATH-}}")" "$transcript""#,
        wd = q(wd)?,
        terminal = if tool == "terminal" { 1 } else { 0 },
        binary = q(binary)?,
        binary_error = q(&binary_error)?,
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

/// Resolve and validate all host-specific inputs needed to launch an SSH-owned
/// session. This function performs no remote mutation.
pub async fn preflight_remote_launch(
    host: &Host,
    requested_workdir: &Path,
    tool: &str,
    session_id: Uuid,
) -> Result<RemoteLaunchPreflight> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }

    let home_script = remote_home_script()?;
    let home_out = ssh_output(host, &home_script, SSH_TIMEOUT)
        .await
        .map_err(|e| ssh_stage_io(host, "home resolution", e))?;
    if !home_out.status.success() {
        return Err(ssh_stage_error(
            host,
            "home resolution",
            home_out.status.code(),
            &String::from_utf8_lossy(&home_out.stderr),
        ));
    }
    let home_stdout = String::from_utf8(home_out.stdout)?;
    let Some(home) = parse_remote_home(&home_stdout) else {
        return Err(HostRuntimeError::RemotePrerequisite {
            stage: "home",
            message: "remote $HOME did not return a valid absolute path".into(),
        });
    };

    let candidate = resolve_remote_workdir(
        &home,
        requested_workdir.to_str().ok_or(HostRuntimeError::Quote)?,
    );
    let script = preflight_validation_script(&candidate, tool, session_id)?;
    let out = ssh_output(host, &script, SSH_TIMEOUT)
        .await
        .map_err(|e| ssh_stage_io(host, "launch preflight", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout);

    if let Some(line) = stdout
        .lines()
        .rev()
        .find(|line| line.starts_with("AGENTUM_PREFLIGHT_ERROR\t"))
    {
        let mut fields = line.splitn(3, '\t');
        let _ = fields.next();
        let stage = match fields.next().unwrap_or("launch") {
            "home" => "home",
            "workdir" => "workdir",
            "tmux" => "tmux",
            "shell" => "shell",
            "binary" => "binary",
            _ => "launch",
        };
        let message = fields
            .next()
            .and_then(decode_protocol_field)
            .unwrap_or_else(|| "remote prerequisite check failed".into());
        return Err(HostRuntimeError::RemotePrerequisite { stage, message });
    }
    if !out.status.success() {
        return Err(ssh_stage_error(
            host,
            "launch preflight",
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ));
    }
    let line = stdout
        .lines()
        .rev()
        .find(|line| line.starts_with("AGENTUM_PREFLIGHT_OK\t"))
        .ok_or_else(|| HostRuntimeError::SshStage {
            stage: "launch preflight",
            host: host.name.clone(),
            message: "remote preflight returned an invalid response".into(),
        })?;
    let mut fields = line.split('\t');
    let _ = fields.next();
    let workdir = fields
        .next()
        .and_then(decode_protocol_field)
        .map(PathBuf::from)
        .ok_or_else(|| HostRuntimeError::SshStage {
            stage: "launch preflight",
            host: host.name.clone(),
            message: "remote workdir response was invalid".into(),
        })?;
    let executable = fields
        .next()
        .and_then(decode_protocol_field)
        .ok_or_else(|| HostRuntimeError::SshStage {
            stage: "launch preflight",
            host: host.name.clone(),
            message: "remote executable response was invalid".into(),
        })?;
    let shell = fields
        .next()
        .and_then(decode_protocol_field)
        .ok_or_else(|| HostRuntimeError::SshStage {
            stage: "launch preflight",
            host: host.name.clone(),
            message: "remote shell response was invalid".into(),
        })?;
    let fresh_path = fields
        .next()
        .and_then(decode_protocol_field)
        .ok_or_else(|| HostRuntimeError::SshStage {
            stage: "launch preflight",
            host: host.name.clone(),
            message: "remote PATH response was invalid".into(),
        })?;
    let claude_transcript_exists = fields.next() == Some("1");
    Ok(RemoteLaunchPreflight {
        home,
        workdir,
        executable,
        shell,
        fresh_path,
        claude_transcript_exists,
    })
}

const REMOTE_RUNTIME_DIR: &str = "$HOME/.agentum/runtime";

/// Owner-only final directory setup that refuses to traverse a symlink at the
/// directory itself. `$HOME`-relative expressions are expanded only by the
/// remote POSIX shell.
fn remote_private_dir_setup(dir: &str) -> String {
    format!(
        "{{ b=\"$HOME/.agentum\"; d=\"{dir}\"; \
         [ ! -L \"$b\" ] && mkdir -p \"$b\" && [ -d \"$b\" ] && [ ! -L \"$b\" ] && chmod 700 \"$b\" && \
         [ ! -L \"$d\" ] && {{ mkdir \"$d\" 2>/dev/null || [ -d \"$d\" ]; }} && \
         [ -d \"$d\" ] && [ ! -L \"$d\" ] && chmod 700 \"$d\"; }}"
    )
}

/// Portable GNU/BSD `stat` check used before a remote credential/log file is
/// opened. A regular file with more than one hard link can alias an unrelated
/// user file, so it is never safe as an append/read/write destination.
fn remote_regular_single_link_guard(path: &str) -> String {
    format!(
        "[ -f {path} ] && [ ! -L {path} ] && links=$(stat -c %h {path} 2>/dev/null || stat -f %l {path} 2>/dev/null) && [ \"$links\" = 1 ]"
    )
}

fn safe_remote_leaf(out_path: &Path) -> Result<&str> {
    let name = out_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(HostRuntimeError::Quote)?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(HostRuntimeError::Quote);
    }
    Ok(name)
}

// tmux replaces control characters in formatted output under a non-UTF-8
// locale. Put the constrained `$N` id first and use a printable separator;
// `read`'s final field preserves every `_` in the session name.
const TMUX_EXACT_SESSION_FORMAT: &str = "#{session_id}_#{session_name}";

/// Whether `target` is tmux's immutable session-id syntax (`$` plus one or
/// more ASCII digits). Callers may persist these IDs after an exact-name claim,
/// so the remote helpers must accept them without treating `$1` as a name.
fn is_tmux_session_id(target: &str) -> bool {
    target
        .strip_prefix('$')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Build the POSIX fragment that resolves either an immutable `$N` selector or
/// a human session name against `list-sessions`. Name lookup is exact string
/// equality: tmux's ordinary `-t name` lookup accepts prefixes, so it must never
/// receive a human-controlled name directly. The resulting `sid` is always the
/// immutable `#{session_id}` value reported by tmux.
fn remote_exact_tmux_session_resolution(target: &str) -> Result<String> {
    let compare = if is_tmux_session_id(target) {
        "session_id"
    } else if target.starts_with('$') {
        return Err(HostRuntimeError::RemotePrerequisite {
            stage: "tmux target",
            message: format!("malformed tmux session id `{target}`"),
        });
    } else {
        "session_name"
    };
    Ok(format!(
        "wanted={wanted}; sessions=$(tmux list-sessions -F {format}); list_rc=$?; sid=; \
         if [ \"$list_rc\" -eq 0 ]; then sid=$(printf '%s\\n' \"$sessions\" | while IFS=_ read -r session_id session_name; do \
         if [ \"${compare}\" = \"$wanted\" ]; then printf '%s\\n' \"$session_id\"; break; fi; done); fi",
        wanted = q(target)?,
        format = q(TMUX_EXACT_SESSION_FORMAT)?,
    ))
}

/// Wrap a remote tmux operation that refers only to `"$sid"`. The resolver and
/// operation run in one remote shell, eliminating both prefix selection and a
/// second SSH round trip. Exit 1 for a missing/no-server target retains tmux's
/// normal lifecycle semantics; other `list-sessions` failures propagate.
fn remote_exact_tmux_script(target: &str, operation: &str) -> Result<String> {
    let resolve = remote_exact_tmux_session_resolution(target)?;
    let inner = format!(
        "{resolve}; if [ \"$list_rc\" -ne 0 ] && [ \"$list_rc\" -ne 1 ]; then exit \"$list_rc\"; fi; \
         if [ -z \"$sid\" ]; then printf \"can't find session: %s\\n\" \"$wanted\" >&2; exit 1; fi; {operation}"
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

fn remote_exact_tmux_probe_script(target: &str) -> Result<String> {
    let resolve = remote_exact_tmux_session_resolution(target)?;
    let inner = format!(
        "{resolve}; if [ \"$list_rc\" -ne 0 ] && [ \"$list_rc\" -ne 1 ]; then exit \"$list_rc\"; fi; \
         if [ -n \"$sid\" ]; then printf 'AGENTUM_TMUX_EXACT\\t%s\\n' \"$sid\"; else printf 'AGENTUM_TMUX_MISSING\\n'; fi"
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

fn valid_env_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Serialize environment assignments as a POSIX source file. Values are kept
/// out of every SSH/tmux argument and exist only in the SSH stdin stream and
/// the remote owner-only staging file.
fn serialize_launch_env(env: &[(String, String)]) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    for (key, value) in env {
        if !valid_env_key(key) || value.as_bytes().contains(&0) {
            return Err(HostRuntimeError::RemotePrerequisite {
                stage: "environment",
                message: format!("invalid launch environment entry `{key}`"),
            });
        }
        let quoted = q(value)?;
        payload.extend_from_slice(format!("export {key}={quoted}\n").as_bytes());
    }
    Ok(payload)
}

fn redact_startup_output(
    output: &str,
    env: &[(String, String)],
    explicit_secrets: &[&str],
) -> String {
    let mut redacted = output.to_string();
    // Longest first prevents a short value that is a substring of a token from
    // obscuring only part of that token and leaving the remainder visible.
    let mut values: Vec<&str> = env
        .iter()
        .map(|(_, value)| value.as_str())
        .chain(explicit_secrets.iter().copied())
        .filter(|value| !value.is_empty())
        .collect();
    values.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    for value in values {
        redacted = redacted.replace(value, "[REDACTED]");
    }
    let clean: String = redacted
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    bounded_text(clean.trim(), STARTUP_DIAGNOSTIC_BYTES)
}

fn remote_launch_script(
    target: &str,
    workdir: &Path,
    argv: &[String],
    out_path: &Path,
) -> Result<String> {
    if argv.is_empty() {
        return Err(HostRuntimeError::RemotePrerequisite {
            stage: "binary",
            message: "launch argv is empty".into(),
        });
    }
    if !Path::new(&argv[0]).is_absolute() {
        return Err(HostRuntimeError::RemotePrerequisite {
            stage: "binary",
            message: format!(
                "remote executable `{}` was not resolved to an absolute path",
                argv[0]
            ),
        });
    }
    if !workdir.is_absolute() {
        return Err(HostRuntimeError::RemotePrerequisite {
            stage: "workdir",
            message: format!(
                "remote workdir `{}` was not resolved to an absolute path",
                workdir.display()
            ),
        });
    }
    let workdir = workdir.to_str().ok_or(HostRuntimeError::Quote)?;
    let leaf = safe_remote_leaf(out_path)?;
    let log = remote_pane_log_expr(out_path)?;
    let runtime_setup = remote_private_dir_setup(REMOTE_RUNTIME_DIR);
    let pane_setup = remote_private_pane_log_setup(&log);
    let log_guard = remote_regular_single_link_guard(&log);
    let resolve = remote_exact_tmux_session_resolution(target)?;
    // One file per launch attempt avoids concurrent starts of the same session
    // clobbering or unlinking each other's staged environment.
    let attempt = Uuid::new_v4().simple();
    let env_file = format!("\"{REMOTE_RUNTIME_DIR}/{leaf}-{attempt}.env\"");
    let command =
        shlex::try_join(argv.iter().map(String::as_str)).map_err(|_| HostRuntimeError::Quote)?;
    let wrapper_inner = format!(
        "f={env_file}; . \"$f\"; rc=$?; rm -f \"$f\"; [ \"$rc\" -eq 0 ] || exit \"$rc\"; exec {command}"
    );
    // tmux evaluates shell-command through its `default-shell`, which may be
    // fish. Keep that outer command trivial and force all POSIX syntax into sh.
    let wrapper = format!("exec sh -c {}", q(&wrapper_inner)?);
    let dormant = "exec sh -c 'while :; do sleep 3600; done'";
    let pipe_inner =
        escape_tmux_pipe_command(&format!("umask 077; {log_guard} || exit 73; cat >> {log}"));
    let pipe = format!("exec sh -c {}", q(&pipe_inner)?);
    let env_guard = remote_regular_single_link_guard(&env_file);
    let inner = format!(
        "umask 077; created=0; committed=0; created_sid=; sid=; \
         cleanup() {{ rc=$?; trap - EXIT HUP INT TERM; rm -f {env_file}; \
           if [ \"$created\" -eq 1 ] && [ \"$committed\" -ne 1 ] && [ -n \"$created_sid\" ]; then tmux kill-session -t \"$created_sid\" >/dev/null 2>&1 || true; fi; exit \"$rc\"; }}; \
         trap cleanup EXIT HUP INT TERM; \
         if ! {runtime_setup}; then printf 'AGENTUM_SETUP_ERROR\\tprivate-directories\\n'; exit 70; fi; \
         if [ -e {env_file} ] || [ -L {env_file} ] || ! (set -C; umask 077; : > {env_file}) || ! {{ {env_guard}; }} || ! cat > {env_file} || ! chmod 600 {env_file} || ! {{ {env_guard}; }}; then printf 'AGENTUM_SETUP_ERROR\\tenvironment-staging\\n'; exit 70; fi; \
         if ! {pane_setup}; then printf 'AGENTUM_SETUP_ERROR\\tpane-log\\n'; exit 70; fi; \
         if ! created_sid=$(tmux new-session -d -P -F '#{{session_id}}' -s {target} -x {cols} -y {rows} -c {workdir} {dormant}); then printf 'AGENTUM_SETUP_ERROR\\tnew-session\\n'; exit 70; fi; created=1; \
         {resolve}; if [ \"$list_rc\" -ne 0 ] || [ -z \"$sid\" ] || [ \"$sid\" != \"$created_sid\" ]; then printf 'AGENTUM_SETUP_ERROR\\texact-session-resolution\\n'; exit 70; fi; \
         if ! tmux set-option -p -t \"$sid\" remain-on-exit on; then printf 'AGENTUM_SETUP_ERROR\\tremain-on-exit\\n'; exit 70; fi; \
         if ! tmux pipe-pane -t \"$sid\" {pipe}; then printf 'AGENTUM_SETUP_ERROR\\tpipe-pane\\n'; exit 70; fi; \
         if ! tmux respawn-pane -k -t \"$sid\" -c {workdir} {wrapper}; then printf 'AGENTUM_SETUP_ERROR\\trespawn-pane\\n'; exit 70; fi; \
         sleep 1; \
         state=$(tmux display-message -p -t \"$sid\" '#{{pane_dead}}\t#{{pane_dead_status}}' 2>/dev/null) || {{ printf 'AGENTUM_SETUP_ERROR\\tstartup-inspection\\n'; exit 70; }}; \
         dead=${{state%%	*}}; status=${{state#*	}}; \
         if [ \"$dead\" = 1 ]; then diagnostic=$(tmux capture-pane -p -S -200 -t \"$sid\" 2>/dev/null | tail -c 16384 | base64 | tr -d '\\r\\n'); printf 'AGENTUM_EARLY_EXIT\\t%s\\t%s\\n' \"$status\" \"$diagnostic\"; exit 71; fi; \
         if ! tmux set-option -p -t \"$sid\" remain-on-exit off; then printf 'AGENTUM_SETUP_ERROR\\tdisable-remain-on-exit\\n'; exit 70; fi; \
         rm -f {env_file}; committed=1; printf 'AGENTUM_LAUNCH_OK\\n'",
        target = q(target)?,
        cols = agentum_tmux::DEFAULT_PANE_COLS,
        rows = agentum_tmux::DEFAULT_PANE_ROWS,
        workdir = q(workdir)?,
        dormant = q(dormant)?,
        pipe = q(&pipe)?,
        wrapper = q(&wrapper)?,
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

enum LaunchProtocol {
    Ok,
    Setup(&'static str),
    EarlyExit {
        status: Option<i32>,
        diagnostic: String,
    },
    Invalid,
}

fn parse_launch_protocol(stdout: &[u8]) -> LaunchProtocol {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines().rev() {
        if line == "AGENTUM_LAUNCH_OK" {
            return LaunchProtocol::Ok;
        }
        if let Some(stage) = line.strip_prefix("AGENTUM_SETUP_ERROR\t") {
            let stage = match stage {
                "private-directories" => "private directories",
                "environment-staging" => "environment staging",
                "pane-log" => "pane log creation",
                "new-session" => "new-session",
                "exact-session-resolution" => "exact session resolution",
                "remain-on-exit" => "remain-on-exit",
                "pipe-pane" => "pipe-pane",
                "respawn-pane" => "respawn-pane",
                "startup-inspection" => "startup inspection",
                "disable-remain-on-exit" => "remain-on-exit cleanup",
                _ => "launch transaction",
            };
            return LaunchProtocol::Setup(stage);
        }
        if let Some(rest) = line.strip_prefix("AGENTUM_EARLY_EXIT\t") {
            let mut fields = rest.splitn(2, '\t');
            let status = fields.next().and_then(|s| s.parse().ok());
            let diagnostic = fields
                .next()
                .and_then(decode_protocol_field)
                .unwrap_or_default();
            return LaunchProtocol::EarlyExit { status, diagnostic };
        }
    }
    LaunchProtocol::Invalid
}

async fn ssh_output_with_stdin(
    host: &Host,
    script: &str,
    stdin: &[u8],
    dur: Duration,
    stage: &'static str,
) -> Result<Output> {
    // A launch transaction cannot safely be replayed after an ambiguous mux
    // failure. Use one fresh connection instead of the retryable pooled runner.
    let mut cmd = ssh_command_opts(host, script, SshMux::Off);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| ssh_stage_io(host, stage, e))?;
    let mut input = child.stdin.take();
    let stdout = child.stdout.take().ok_or_else(|| {
        ssh_stage_io(
            host,
            stage,
            std::io::Error::other("SSH stdout pipe was not available"),
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ssh_stage_io(
            host,
            stage,
            std::io::Error::other("SSH stderr pipe was not available"),
        )
    })?;
    // Drain both pipes while stdin is being written and the process is alive;
    // otherwise a verbose remote command can fill a pipe and deadlock before
    // it consumes the rest of the private stdin payload.
    let mut stdout_task = spawn_ssh_reader(stdout);
    let mut stderr_task = spawn_ssh_reader(stderr);
    // One deadline covers BOTH feeding stdin and waiting. Bounding only
    // `wait_with_output` leaves `write_all` able to hang forever if the remote
    // side never reads (especially relevant for a large private config file).
    let operation = async {
        let write_error = if let Some(mut writer) = input.take() {
            let result = writer.write_all(stdin).await;
            let _ = writer.shutdown().await;
            result.err()
        } else {
            None
        };
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, write_error))
    };
    let (status, write_error) = match timeout(dur, operation).await {
        Ok(result) => result.map_err(|e| ssh_stage_io(host, stage, e))?,
        Err(_) => {
            // Dropping a timed-out `wait_with_output` only relies on
            // kill-on-drop and does not guarantee the OS child is reaped. Kill
            // explicitly, wait for it, and drain both pipes before returning.
            let cleanup =
                cleanup_timed_out_ssh_child(&mut child, &mut stdout_task, &mut stderr_task).await;
            return Err(HostRuntimeError::SshStage {
                stage,
                host: host.name.clone(),
                message: if cleanup.is_empty() {
                    format!("the SSH {stage} timed out")
                } else {
                    format!(
                        "the SSH {stage} timed out (cleanup: {})",
                        cleanup.join("; ")
                    )
                },
            });
        }
    };
    let (stdout, stderr) = tokio::join!(
        collect_ssh_reader(stdout_task),
        collect_ssh_reader(stderr_task)
    );
    let output = Output {
        status,
        stdout: stdout.map_err(|e| ssh_stage_io(host, stage, e))?,
        stderr: stderr.map_err(|e| ssh_stage_io(host, stage, e))?,
    };
    // A zero exit is trustworthy only when the complete payload reached the
    // remote reader. Login-shell banners or even a success protocol line must
    // never mask a short stdin write: that could silently install a truncated
    // private config/environment file. For a non-zero child, retain its more
    // specific remote diagnostic instead of replacing it with the consequent
    // broken pipe from the writer.
    if output.status.success()
        && let Some(error) = write_error
    {
        return Err(ssh_stage_io(host, stage, error));
    }
    Ok(output)
}

type SshReaderTask = tokio::task::JoinHandle<std::io::Result<Vec<u8>>>;

fn spawn_ssh_reader<R>(mut reader: R) -> SshReaderTask
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(bytes)
    })
}

async fn collect_ssh_reader(mut task: SshReaderTask) -> std::io::Result<Vec<u8>> {
    match timeout(SSH_REAP_TIMEOUT, &mut task).await {
        Ok(joined) => joined.map_err(|error| std::io::Error::other(error.to_string()))?,
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "SSH output pipe did not close after child exit",
            ))
        }
    }
}

async fn drain_ssh_reader_after_timeout(task: &mut SshReaderTask) -> Option<String> {
    match timeout(SSH_REAP_TIMEOUT, &mut *task).await {
        Ok(Ok(Ok(_))) => None,
        Ok(Ok(Err(error))) => Some(format!("pipe drain failed: {error}")),
        Ok(Err(error)) => Some(format!("pipe drain task failed: {error}")),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Some("pipe drain timed out".into())
        }
    }
}

async fn cleanup_timed_out_ssh_child(
    child: &mut tokio::process::Child,
    stdout_task: &mut SshReaderTask,
    stderr_task: &mut SshReaderTask,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    if let Err(error) = child.start_kill() {
        diagnostics.push(format!("kill failed: {error}"));
    }
    match timeout(SSH_REAP_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => diagnostics.push(format!("reap failed: {error}")),
        Err(_) => diagnostics.push("reap timed out".into()),
    }
    let (stdout_diagnostic, stderr_diagnostic) = tokio::join!(
        drain_ssh_reader_after_timeout(stdout_task),
        drain_ssh_reader_after_timeout(stderr_task)
    );
    if let Some(diagnostic) = stdout_diagnostic {
        diagnostics.push(format!("stdout {diagnostic}"));
    }
    if let Some(diagnostic) = stderr_diagnostic {
        diagnostics.push(format!("stderr {diagnostic}"));
    }
    diagnostics
}

/// Atomically-ish launch an SSH-owned session: privately stage environment via
/// stdin, create a retained dormant pane, arm logging before the real process,
/// inspect a bounded startup window, then either commit or return the captured
/// and redacted first failure. SSH hosts only.
pub async fn launch_remote_session(
    host: &Host,
    target: &str,
    workdir: &Path,
    argv: &[String],
    env: &[(String, String)],
    redaction_secrets: &[&str],
    out_path: &Path,
) -> Result<()> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    let payload = serialize_launch_env(env)?;
    let script = remote_launch_script(target, workdir, argv, out_path)?;
    let output = ssh_output_with_stdin(
        host,
        &script,
        &payload,
        REMOTE_LAUNCH_TIMEOUT,
        "launch transaction",
    )
    .await?;
    let stderr = redact_startup_output(
        &String::from_utf8_lossy(&output.stderr),
        env,
        redaction_secrets,
    );
    match parse_launch_protocol(&output.stdout) {
        LaunchProtocol::Ok if output.status.success() => Ok(()),
        LaunchProtocol::Setup(stage) => Err(HostRuntimeError::TmuxSetup {
            stage,
            status: output.status.code(),
            stderr: if stderr.is_empty() {
                "tmux rejected the operation; verify the remote tmux version and permissions".into()
            } else {
                stderr
            },
        }),
        LaunchProtocol::EarlyExit { status, diagnostic } => {
            let diagnostic = redact_startup_output(&diagnostic, env, redaction_secrets);
            Err(HostRuntimeError::EarlyExit {
                program: Path::new(&argv[0])
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&argv[0])
                    .to_string(),
                status,
                diagnostic: if diagnostic.is_empty() {
                    " (the pane produced no diagnostic output)".into()
                } else {
                    format!("\n{diagnostic}")
                },
            })
        }
        LaunchProtocol::Ok | LaunchProtocol::Invalid => Err(ssh_stage_error(
            host,
            "launch transaction",
            output.status.code(),
            &stderr,
        )),
    }
}

pub async fn has_session(host: &Host, target: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::has_session(target).await?),
        HostKind::Ssh { .. } => {
            let output = ssh_output(host, &remote_exact_tmux_probe_script(target)?, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            if !output.status.success() {
                return Err(HostRuntimeError::NonZero {
                    status: output.status.code(),
                    stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                });
            }
            let stdout = String::from_utf8(output.stdout)?;
            for line in stdout.lines().rev() {
                if line == "AGENTUM_TMUX_MISSING" {
                    return Ok(false);
                }
                if line
                    .strip_prefix("AGENTUM_TMUX_EXACT\t")
                    .is_some_and(is_tmux_session_id)
                {
                    return Ok(true);
                }
            }
            Err(HostRuntimeError::SshStage {
                stage: "exact tmux lookup",
                host: host.name.clone(),
                message: "remote tmux lookup returned an invalid response".into(),
            })
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
            if !env.is_empty() {
                return Err(HostRuntimeError::RemotePrerequisite {
                    stage: "environment",
                    message: "remote launches with environment entries must use the private launch transaction"
                        .into(),
                });
            }
            let cmd_str = shlex::try_join(cmd.iter().map(String::as_str))
                .map_err(|_| HostRuntimeError::Quote)?;
            let parts = vec![
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
                q(&cmd_str)?.into_owned(),
            ];
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
            ssh_checked(
                host,
                &remote_exact_tmux_script(target, "tmux kill-session -t \"$sid\"")?,
            )
            .await
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
            let operation = format!(
                "tmux display-message -p -t \"$sid\" {fmt} 2>/dev/null || echo X; tmux capture-pane -p -e -t \"$sid\"",
                fmt = q(agentum_tmux::CURSOR_SAMPLE_FORMAT)?,
            );
            let out = ssh_stdout(host, &remote_exact_tmux_script(target, &operation)?).await?;
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
    let setup = remote_private_pane_log_setup(&log);
    let guard = remote_regular_single_link_guard(&log);
    let sink = escape_tmux_pipe_command(&format!("umask 077; {guard} || exit 73; cat >> {log}"));
    let pipe = q(&format!("exec sh -c {}", q(&sink)?))?.into_owned();
    let arm = remote_pipe_pane_arm_operation(&pipe);
    let operation = format!(
        "umask 077; {setup} || exit 73; ({arm}) 2>/dev/null || true; \
         c=$(tmux display-message -p -t \"$sid\" {fmt} 2>/dev/null || echo X); \
         f=$(mktemp \"{REMOTE_PANE_DIR}/.snapshot.XXXXXX\" 2>/dev/null) || {{ printf \"0\\nX\\n\"; exit 0; }}; \
         tmux capture-pane -p -e -t \"$sid\" > \"$f\" 2>/dev/null || true; \
         o=$({{ {guard} && wc -c < {log}; }} 2>/dev/null || echo 0); \
         printf \"%s\\n%s\\n\" \"$o\" \"$c\"; cat \"$f\" 2>/dev/null; rm -f \"$f\"",
        fmt = q(agentum_tmux::CURSOR_SAMPLE_FORMAT)?,
    );
    remote_exact_tmux_script(target, &operation)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReverseTunnelKey {
    /// Saved-host identity. SSH ControlPaths use a compatible identity plus a
    /// connection-only fingerprint, so duplicate records for one endpoint do
    /// not borrow one another's authenticated forward cache.
    host_id: Uuid,
    /// PID reported by `ssh -O check` for the exact interactive master. A new
    /// process at the same ControlPath has no forwards even when the socket is
    /// healthy, so its generation must never hit the prior master's cache.
    master_generation: String,
    /// Stable loopback listen port on the saved SSH host.
    host_listen_port: u16,
    /// Current embedded Agentum server port on the Mac.
    mac_destination_port: u16,
}

impl ReverseTunnelKey {
    fn same_forward(&self, other: &Self) -> bool {
        self.host_id == other.host_id
            && self.host_listen_port == other.host_listen_port
            && self.mac_destination_port == other.mac_destination_port
    }
}

fn remember_armed_reverse_tunnel(state: &mut TunnelControlState, key: ReverseTunnelKey) {
    state
        .reverse_tunnels
        .retain(|existing| !existing.same_forward(&key));
    state.reverse_tunnels.insert(key);
}

#[derive(Default)]
struct TunnelControlState {
    /// Desired reverse forwards paired with the master generation on which
    /// each was last established. A prior-generation entry is deliberately
    /// retained as desired state until the warmer re-arms it on the replacement
    /// master; an exact current-generation entry is the only valid cache hit.
    reverse_tunnels: HashSet<ReverseTunnelKey>,
    /// Host record revision for which the pre-namespacing `cm-%C`/`cms-%C`
    /// sockets have been explicitly retired. Upgrade cleanup must precede the
    /// first new master: a legacy reverse forward can otherwise keep the stable
    /// remote MCP port bound to an obsolete Mac endpoint.
    legacy_retired_revisions: HashMap<Uuid, i128>,
}

fn tunnel_control_lock() -> &'static Mutex<TunnelControlState> {
    static LOCK: OnceLock<Mutex<TunnelControlState>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(TunnelControlState::default()))
}

/// Per-host master/tunnel lifecycle lock.
///
/// The working Agentum server warms hosts independently. This fork previously
/// used one process-wide mutex here, so an unreachable host could hold every
/// other host behind its 10-second SSH probe. Weak entries keep the registry
/// bounded while preserving same-host serialization for warm, invalidate, and
/// forwarding operations.
fn ssh_master_warm_lock(host_id: Uuid) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<Uuid, Weak<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&host_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(host_id, Arc::downgrade(&lock));
    lock
}

async fn acquire_ssh_master_warm(host_id: Uuid) -> tokio::sync::OwnedMutexGuard<()> {
    ssh_master_warm_lock(host_id).lock_owned().await
}

async fn retire_legacy_ssh_masters_once(host: &Host) -> Result<()> {
    let revision = host.updated_at.unix_timestamp_nanos();
    let _master_guard = acquire_ssh_master_warm(host.id).await;
    {
        let state = tunnel_control_lock().lock().await;
        if state.legacy_retired_revisions.get(&host.id) == Some(&revision) {
            return Ok(());
        }
    }
    ssh_retire_legacy_control_masters(host, SSH_CONTROL_TIMEOUT)
        .await
        .map_err(|error| ssh_stage_io(host, "legacy SSH connection retirement", error))?;
    tunnel_control_lock()
        .lock()
        .await
        .legacy_retired_revisions
        .insert(host.id, revision);
    Ok(())
}

/// Build a non-mutating `ssh -O check` for the exact private ControlPath used
/// by `mux`. The lower crate deliberately keeps path construction private;
/// derive the option from its role-specific exit command so this file cannot
/// drift to another socket template.
fn ssh_control_check_cmd(host: &Host, mux: SshMux) -> Option<Command> {
    let exit = ssh_control_exit_cmd(host, mux)?;
    let control_path = exit
        .as_std()
        .get_args()
        .filter_map(|arg| arg.to_str())
        .find(|arg| arg.starts_with("ControlPath="))?
        .to_string();
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    // This operation addresses an already-running private ControlMaster. Avoid
    // reparsing user SSH config here: `Match exec` hooks can otherwise make a
    // local socket health check take seconds and monopolize the interactive
    // stream's title-poll loop.
    let mut cmd = agentum_tmux::ssh::ssh_existing_control_command();
    cmd.arg("-T")
        .arg("-o")
        .arg(control_path)
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("check")
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

fn parse_control_master_generation(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let marker = "pid=";
    let tail = text.rsplit_once(marker)?.1;
    let pid: String = tail.chars().take_while(char::is_ascii_digit).collect();
    (!pid.is_empty()).then(|| format!("pid:{pid}"))
}

async fn verified_control_master_generation(host: &Host) -> Result<String> {
    let Some(check) = ssh_control_check_cmd(host, SshMux::Interactive) else {
        return Err(HostRuntimeError::SshStage {
            stage: "ControlMaster generation check",
            host: host.name.clone(),
            message: "no private ControlPath is available for this SSH host".into(),
        });
    };
    let checked = bounded_command_output(check, SSH_CONTROL_TIMEOUT).await?;
    if !checked.status.success() {
        return Err(ssh_stage_error(
            host,
            "ControlMaster generation check",
            checked.status.code(),
            &String::from_utf8_lossy(&checked.stderr),
        ));
    }
    parse_control_master_generation(&checked.stdout, &checked.stderr).ok_or_else(|| {
        HostRuntimeError::SshStage {
            stage: "ControlMaster generation check",
            host: host.name.clone(),
            message: "OpenSSH confirmed the master but did not report its process generation"
                .into(),
        }
    })
}

/// Re-arm every reverse forward this process still desires for `host` when the
/// interactive ControlMaster PID changes. This is called by the periodic
/// warmer as well as session start, so an otherwise-idle running agent does not
/// lose MCP indefinitely after a transparent mux replacement.
/// Reconcile desired forwards while the caller holds [`ssh_master_warm_lock`].
/// Keeping the generation probe, cache mutation, and `-O forward` operations
/// under the same lease as both interactive and streaming master creation
/// prevents invalidation from interleaving between the two warm legs.
async fn reconcile_desired_reverse_tunnels_locked(host: &Host) -> Result<()> {
    let generation = verified_control_master_generation(host).await?;
    let mut state = tunnel_control_lock().lock().await;
    let mut desired: Vec<(u16, u16)> = state
        .reverse_tunnels
        .iter()
        .filter(|key| key.host_id == host.id)
        .map(|key| (key.host_listen_port, key.mac_destination_port))
        .collect();
    desired.sort_unstable();
    desired.dedup();

    for (host_port, mac_port) in desired {
        let current = ReverseTunnelKey {
            host_id: host.id,
            master_generation: generation.clone(),
            host_listen_port: host_port,
            mac_destination_port: mac_port,
        };
        if state.reverse_tunnels.contains(&current) {
            continue;
        }
        let Some(cmd) = ssh_control_forward_cmd(host, host_port, mac_port) else {
            return Err(HostRuntimeError::Tunnel {
                operation: "reverse-forward rearm",
                message: "no private ControlPath is available for this SSH host".into(),
            });
        };
        let output = bounded_command_output(cmd, SSH_CONTROL_TIMEOUT)
            .await
            .map_err(|error| HostRuntimeError::Tunnel {
                operation: "reverse-forward rearm",
                message: error.to_string(),
            })?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !tunnel_arm_bound(output.status.success(), &stderr) {
            return Err(HostRuntimeError::Tunnel {
                operation: "reverse-forward rearm",
                message: format!(
                    "could not restore host loopback port {host_port}: {}",
                    bounded_text(stderr.trim(), STARTUP_DIAGNOSTIC_BYTES)
                ),
            });
        }
        remember_armed_reverse_tunnel(&mut state, current);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PooledProbeDisposition {
    Healthy,
    Evict,
    Busy,
    Failed,
}

/// Classify only outcomes whose timing/replay boundary is known. In particular,
/// [`is_mux_transport_error`] accepts a narrow set of diagnostics emitted before
/// OpenSSH sends the remote command. A channel-pressure refusal means the master
/// is alive and shared, so evicting it would disrupt other sessions.
fn classify_pooled_probe(
    status: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> PooledProbeDisposition {
    if status == Some(0) {
        return PooledProbeDisposition::Healthy;
    }
    if status == Some(255)
        && stdout.is_empty()
        && std::str::from_utf8(stderr).is_ok_and(is_mux_transport_error)
    {
        return PooledProbeDisposition::Evict;
    }
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if stderr.contains("session open refused by peer")
        || stderr.contains("open failed: administratively prohibited")
    {
        PooledProbeDisposition::Busy
    } else {
        PooledProbeDisposition::Failed
    }
}

fn pooled_master_stage(mux: SshMux) -> &'static str {
    match mux {
        SshMux::Interactive => "interactive ControlMaster health probe",
        SshMux::Streaming => "streaming ControlMaster health probe",
        SshMux::Observer => "observer ControlMaster health probe",
        SshMux::Off => "unpooled SSH health probe",
    }
}

/// `ssh -O check` is local-socket-only. It does not prove connectivity; it only
/// tells the real remote probe whether a timed-out child attached to an existing
/// master (which can be evicted) or was a cold connection to an unavailable
/// host (where a second full attempt would only double the wait).
async fn pooled_master_was_running_locked(host: &Host, mux: SshMux) -> bool {
    let Some(check) = ssh_control_check_cmd(host, mux) else {
        return false;
    };
    match bounded_command_output(check, SSH_CONTROL_CHECK_TIMEOUT).await {
        Ok(output) => output.status.success(),
        // A local control process that cannot answer its own socket is suspect;
        // let the real probe's timeout take the repair branch.
        Err(_) => true,
    }
}

/// Best-effort exact-role eviction while the caller owns the master warm lock.
/// This uses only Agentum's validated private ControlPath and is bounded even if
/// the local master process itself is wedged.
async fn evict_ssh_master_locked(host: &Host, mux: SshMux) {
    let Some(command) = ssh_control_exit_cmd(host, mux) else {
        return;
    };
    if let Err(error) = bounded_command_output(command, SSH_CONTROL_EXIT_TIMEOUT).await {
        tracing::warn!(host = %host.name, ?mux, %error, "could not evict wedged SSH master");
    }
}

/// Evict one pooled role without racing host credential invalidation or the
/// periodic warmer. Missing sockets and non-SSH hosts are harmless no-ops.
pub async fn evict_ssh_master(host: &Host, mux: SshMux) {
    if !matches!(host.kind, HostKind::Ssh { .. }) || mux == SshMux::Off {
        return;
    }
    let _master_guard = acquire_ssh_master_warm(host.id).await;
    evict_ssh_master_locked(host, mux).await;
}

/// Prove and, when transport evidence warrants it, repair one exact pooled SSH
/// role. Silent pane-tail recovery uses this for `Streaming` so it cannot evict
/// or add contention to the interactive master that carries accepted input.
pub async fn repair_ssh_master_role(host: &Host, mux: SshMux) -> Result<()> {
    if !matches!(host.kind, HostKind::Ssh { .. }) || mux == SshMux::Off {
        return Ok(());
    }
    let _master_guard = acquire_ssh_master_warm(host.id).await;
    probe_or_repair_pooled_master_locked(host, mux).await
}

async fn open_pooled_master_locked(host: &Host, mux: SshMux) -> Result<()> {
    let output =
        bounded_command_output(ssh_command_opts(host, "true", mux), SSH_CONTROL_TIMEOUT).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ssh_stage_error(
            host,
            pooled_master_stage(mux),
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        ))
    }
}

/// Prove that one pooled role reaches the remote host. A successful probe is
/// itself the warmup, so recovery never follows a successful remote no-op with
/// a redundant second one. Only an existing timed-out master or a recognized
/// pre-session mux failure is evicted and reopened once.
async fn probe_or_repair_pooled_master_locked(host: &Host, mux: SshMux) -> Result<()> {
    debug_assert!(mux != SshMux::Off);
    let was_running = pooled_master_was_running_locked(host, mux).await;
    let probe =
        bounded_command_output(ssh_command_opts(host, "true", mux), SSH_CONTROL_TIMEOUT).await;

    match probe {
        Ok(output) => {
            match classify_pooled_probe(output.status.code(), &output.stdout, &output.stderr) {
                PooledProbeDisposition::Healthy => Ok(()),
                PooledProbeDisposition::Evict => {
                    evict_ssh_master_locked(host, mux).await;
                    open_pooled_master_locked(host, mux).await
                }
                PooledProbeDisposition::Busy if was_running => {
                    tracing::debug!(host = %host.name, ?mux, "preserving busy shared SSH master");
                    Ok(())
                }
                PooledProbeDisposition::Busy | PooledProbeDisposition::Failed => {
                    Err(ssh_stage_error(
                        host,
                        pooled_master_stage(mux),
                        output.status.code(),
                        &String::from_utf8_lossy(&output.stderr),
                    ))
                }
            }
        }
        Err(HostRuntimeError::Timeout) if was_running => {
            evict_ssh_master_locked(host, mux).await;
            open_pooled_master_locked(host, mux).await
        }
        // No local master existed, so this was already a cold connection. Do
        // not make an unreachable host pay a redundant second handshake.
        Err(error) => Err(error),
    }
}

/// Prove (and, when necessary, repair) all pooled SSH masters for `host`.
/// Boot/periodic calls keep interactive operations, the first tail, and the
/// independent observer off the cold TCP+auth path. Streaming and observer
/// warmup remain best-effort; the interactive role is required.
pub async fn warm_ssh_master(host: &Host) -> Result<()> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Ok(());
    }
    retire_legacy_ssh_masters_once(host).await?;
    // Close/invalidate and both probe legs share this lease. In particular, a
    // concurrent credential edit cannot retire an old revision and have this
    // task recreate it after the close.
    let _master_guard = acquire_ssh_master_warm(host.id).await;
    let (interactive, streaming, observer) = tokio::join!(
        probe_or_repair_pooled_master_locked(host, SshMux::Interactive),
        probe_or_repair_pooled_master_locked(host, SshMux::Streaming),
        probe_or_repair_pooled_master_locked(host, SshMux::Observer),
    );
    if let Err(error) = streaming {
        tracing::debug!(host = %host.name, %error, "streaming SSH master warmup deferred");
    }
    if let Err(error) = observer {
        tracing::debug!(host = %host.name, %error, "observer SSH master warmup deferred");
    }
    interactive?;
    reconcile_desired_reverse_tunnels_locked(host).await
}

/// Tear down both pooled connections before boot-time MCP tunnel re-arming.
///
/// The remote MCP listen port is stable across embedded-server generations,
/// while its Mac-side destination port can change. Closing the old interactive
/// master atomically removes its stale `-R` forwarding table; clearing the
/// matching in-process cache makes a second embedded boot in the same process
/// arm the new destination instead of treating the prior mapping as current.
/// This intentionally does not warm a replacement: [`ensure_reverse_tunnel`]
/// does that immediately before it arms the fresh forward.
pub async fn reset_ssh_master_for_mcp_rearm(host: &Host) -> Result<()> {
    forget_ssh_control_masters(host).await
}

/// Close both master generations and discard every desired reverse-forward for
/// this saved host. Use when deleting the host, changing its destination with
/// no bound sessions, or rotating the embedded server's Mac endpoint.
pub async fn forget_ssh_control_masters(host: &Host) -> Result<()> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    // Lock order matches warm_ssh_master (master, then tunnel)
    // and blocks both a concurrent warm and a concurrent -O forward mutation.
    let _master_guard = acquire_ssh_master_warm(host.id).await;
    let mut tunnel_state = tunnel_control_lock().lock().await;
    ssh_close_control_masters(host, SSH_CONTROL_TIMEOUT)
        .await
        .map_err(|error| ssh_stage_io(host, "SSH connection retirement", error))?;
    tunnel_state
        .reverse_tunnels
        .retain(|key| key.host_id != host.id);
    tunnel_state.legacy_retired_revisions.remove(&host.id);
    Ok(())
}

/// Discard only in-process tunnel/legacy bookkeeping after the caller has
/// successfully committed a host destination change or deletion.
///
/// Transport invalidation happens before the database mutation via
/// [`invalidate_ssh_control_masters`], which deliberately preserves desired
/// forwards so a failed Store write can safely continue using revision A. This
/// infallible post-commit step makes that transaction ordering explicit without
/// performing another SSH operation using a row that no longer exists.
pub async fn discard_ssh_control_state(host_id: Uuid) {
    let _master_guard = acquire_ssh_master_warm(host_id).await;
    let mut tunnel_state = tunnel_control_lock().lock().await;
    tunnel_state
        .reverse_tunnels
        .retain(|key| key.host_id != host_id);
    tunnel_state.legacy_retired_revisions.remove(&host_id);
}

/// Close both master generations after a saved connection/authentication
/// change while retaining desired reverse-forward specs. The next periodic
/// warm (or explicit session operation) recreates the new namespaced master and
/// reconciles those forwards. Boot-time MCP endpoint rotation uses
/// [`reset_ssh_master_for_mcp_rearm`] instead because it intentionally discards
/// the old Mac destination.
pub async fn invalidate_ssh_control_masters(host: &Host) -> Result<()> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Ok(());
    }
    let _master_guard = acquire_ssh_master_warm(host.id).await;
    ssh_close_control_masters(host, SSH_CONTROL_TIMEOUT)
        .await
        .map_err(|error| ssh_stage_io(host, "SSH connection invalidation", error))
}

async fn bounded_command_output(mut cmd: Command, dur: Duration) -> Result<Output> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(HostRuntimeError::Io)?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        HostRuntimeError::Io(std::io::Error::other("command stdout pipe unavailable"))
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        HostRuntimeError::Io(std::io::Error::other("command stderr pipe unavailable"))
    })?;

    let operation = async {
        let read_stdout = async {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        };
        let read_stderr = async {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        };
        let (status, stdout, stderr) = tokio::join!(child.wait(), read_stdout, read_stderr);
        Ok::<_, std::io::Error>(Output {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        })
    };

    match timeout(dur, operation).await {
        Ok(result) => result.map_err(HostRuntimeError::Io),
        Err(_) => {
            // `kill_on_drop` is only a backstop. Explicitly terminate, drain,
            // and wait so repeated unreachable-host probes cannot accumulate
            // zombie ssh children or blocked pipe readers. Bound cleanup too:
            // a descendant can inherit either pipe, and a pathological OS
            // reap must not turn the operation timeout into an infinite wait.
            let kill_error = child.start_kill().err().and_then(|error| {
                (error.kind() != std::io::ErrorKind::InvalidInput).then_some(error)
            });
            let mut remaining_stdout = Vec::new();
            let mut remaining_stderr = Vec::new();
            let cleanup = timeout(SSH_REAP_TIMEOUT, async {
                tokio::join!(
                    child.wait(),
                    stdout.read_to_end(&mut remaining_stdout),
                    stderr.read_to_end(&mut remaining_stderr)
                )
            })
            .await;
            if let Some(error) = kill_error {
                tracing::warn!(%error, "timed-out SSH command could not be killed");
            }
            match cleanup {
                Ok((Ok(_), Ok(_), Ok(_))) => {}
                Ok((wait, stdout, stderr)) => tracing::warn!(
                    wait_error = ?wait.err(),
                    stdout_error = ?stdout.err(),
                    stderr_error = ?stderr.err(),
                    "timed-out SSH command cleanup was incomplete"
                ),
                Err(_) => tracing::warn!("timed-out SSH command cleanup timed out"),
            }
            Err(HostRuntimeError::Timeout)
        }
    }
}

/// First port in the IANA dynamic/private range used for Agentum's stable
/// per-host MCP listener. Every port in 49152..=65535 is unprivileged.
const REMOTE_MCP_PORT_BASE: u16 = 49_152;
const REMOTE_MCP_PORT_COUNT: u128 = 16_384;

/// Deterministically map a saved host UUID to its remote MCP listen port.
///
/// The mapping deliberately does not use the embedded Mac server's ephemeral
/// port: an agent's `http://127.0.0.1:<port>/mcp` URL must remain valid across
/// Agentum restarts. UUID v4's random low bits spread saved hosts uniformly
/// over the complete dynamic/private range. As with every 16-bit mapping,
/// different saved records can collide; the loopback bind then fails visibly
/// instead of silently targeting the wrong service.
fn reverse_tunnel_host_port(host_id: Uuid) -> u16 {
    let offset = (host_id.as_u128() % REMOTE_MCP_PORT_COUNT) as u16;
    REMOTE_MCP_PORT_BASE + offset
}

/// Ensure a **reverse** SSH tunnel so this host can reach the Mac's embedded
/// agentum MCP server: the stable host-side `127.0.0.1:<host_port>` (derived
/// from the saved host UUID) → over SSH → the current Mac-side
/// `127.0.0.1:<mac_port>`. Returns the stable **host port** that the caller puts
/// in agent configuration.
///
pub async fn ensure_reverse_tunnel(host: &Host, mac_port: u16) -> Result<u16> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    // `-O forward` attaches to an existing master, so the master must be up first.
    warm_ssh_master(host).await?;

    // Keep the generation probe, cache decision, and control operation in one
    // master lifecycle critical section. Reset/rearm and the background warmer
    // take this same lock before replacing a pooled connection.
    let _master_guard = acquire_ssh_master_warm(host.id).await;
    let master_generation = verified_control_master_generation(host).await?;

    let host_port = reverse_tunnel_host_port(host.id);
    let key = ReverseTunnelKey {
        host_id: host.id,
        master_generation,
        host_listen_port: host_port,
        mac_destination_port: mac_port,
    };
    // OpenSSH control operations mutate one master's forwarding table. Keep
    // the arm serialized and remember exact successful specs on this master.
    // Multiple sessions share the same embedded MCP port, so cancelling an
    // already-healthy identical forward here would briefly disconnect every
    // running remote agent using it.
    let mut state = tunnel_control_lock().lock().await;
    if state.reverse_tunnels.contains(&key) {
        return Ok(host_port);
    }
    let Some(cmd) = ssh_control_forward_cmd(host, host_port, mac_port) else {
        return Err(HostRuntimeError::Tunnel {
            operation: "reverse-forward arm",
            message: "no private ControlPath is available for this SSH host".into(),
        });
    };
    let out = bounded_command_output(cmd, SSH_CONTROL_TIMEOUT)
        .await
        .map_err(|e| HostRuntimeError::Tunnel {
            operation: "reverse-forward arm",
            message: e.to_string(),
        })?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if tunnel_arm_bound(out.status.success(), &stderr) {
        remember_armed_reverse_tunnel(&mut state, key);
        Ok(host_port)
    } else {
        Err(HostRuntimeError::Tunnel {
            operation: "reverse-forward arm",
            message: format!(
                "could not bind host loopback port {host_port}: {}",
                bounded_text(stderr.trim(), STARTUP_DIAGNOSTIC_BYTES)
            ),
        })
    }
}

/// First Mac-loopback port scanned for the **forward** (CDP screencast) tunnel.
pub const REMOTE_CDP_PORT_BASE: u16 = 9200;
/// How many consecutive Mac ports to try before giving up (another local app
/// may already hold some, or a stale forward may linger).
const REMOTE_CDP_PORT_TRIES: u16 = 24;

/// The Mac-loopback port range scanned by [`ensure_forward_tunnel`]. Pure so the
/// range (and its disjointness from the MCP range) is unit-testable.
fn forward_tunnel_ports() -> std::ops::Range<u16> {
    REMOTE_CDP_PORT_BASE..REMOTE_CDP_PORT_BASE.saturating_add(REMOTE_CDP_PORT_TRIES)
}

/// Did an `ssh -O forward` attempt bind (or confirm) its exact `-L`/`-R` spec?
/// A clean exit means bound; ssh explicitly reporting that exact forwarding as
/// already established is idempotent success. Any other non-zero exit remains
/// a failure (in particular, never confuse a foreign occupied port with ours).
fn tunnel_arm_bound(status_success: bool, stderr: &str) -> bool {
    if status_success {
        return true;
    }
    let s = stderr.to_ascii_lowercase();
    // Do NOT accept a generic "already" / "exists": an occupied local port is
    // reported as "Address already in use" and must make us scan onward.
    s.contains("forwarding already in place")
        || s.contains("forward already exists")
        || s.contains("forward is already established")
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

    let _guard = tunnel_control_lock().lock().await;
    let mut last_err = String::new();
    for mac_port in forward_tunnel_ports() {
        // Cancel any forward already bound to this Mac→host pair (e.g. a stale
        // one left after a Mac sleep, when re-attaching to the same browser),
        // then arm fresh. OpenSSH needs the full spec to cancel a -L, so this
        // only clears a forward to the SAME host port — exactly the re-attach
        // case; a foreign holder of the Mac port instead fails the arm below and
        // we scan on. No-op when none; best-effort, so failures are ignored.
        if let Some(cancel) = ssh_control_local_cancel_cmd(host, mac_port, host_port) {
            let _ = bounded_command_output(cancel, SSH_CONTROL_TIMEOUT).await;
        }
        let Some(cmd) = ssh_control_local_forward_cmd(host, mac_port, host_port) else {
            return Err(HostRuntimeError::Tunnel {
                operation: "forward arm",
                message: "no private ControlPath is available for this SSH host".into(),
            });
        };
        let out = bounded_command_output(cmd, SSH_CONTROL_TIMEOUT)
            .await
            .map_err(|e| HostRuntimeError::Tunnel {
                operation: "forward arm",
                message: e.to_string(),
            })?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if tunnel_arm_bound(out.status.success(), &stderr) {
            return Ok(mac_port);
        }
        // Port busy (local service or unbindable stale forward) → try the next.
        last_err = stderr.trim().to_string();
    }
    Err(HostRuntimeError::Tunnel {
        operation: "forward arm",
        message: format!(
            "no free CDP forward-tunnel port on Mac in {REMOTE_CDP_PORT_BASE}..; last: {}",
            bounded_text(&last_err, STARTUP_DIAGNOSTIC_BYTES)
        ),
    })
}

fn remote_atomic_write_script(abs_path: &str) -> Result<String> {
    let parent = Path::new(abs_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    let dst = q(abs_path)?;
    let existing_guard = remote_regular_single_link_guard(&dst);
    let inner = format!(
        "umask 077; set -f; dir={dir}; dst={path}; committed=0; tmp=; \
         ensure_dir_tree() {{ path=$1; case \"$path\" in /*) ;; *) return 1;; esac; current=/; rest=${{path#/}}; \
           while [ -n \"$rest\" ]; do case \"$rest\" in */*) part=${{rest%%/*}}; rest=${{rest#*/}};; *) part=$rest; rest=;; esac; \
             case \"$part\" in ''|.) continue;; ..) return 1;; esac; current=${{current%/}}/$part; \
             if [ -L \"$current\" ]; then return 1; elif [ -e \"$current\" ]; then [ -d \"$current\" ] || return 1; \
             else (umask 077; mkdir \"$current\") || {{ [ -d \"$current\" ] && [ ! -L \"$current\" ]; }} || return 1; fi; \
             [ -d \"$current\" ] && [ ! -L \"$current\" ] || return 1; done; }}; \
         cleanup() {{ rc=$?; trap - EXIT HUP INT TERM; [ -n \"$tmp\" ] && [ \"$committed\" -ne 1 ] && rm -f \"$tmp\"; exit \"$rc\"; }}; \
         trap cleanup EXIT HUP INT TERM; ensure_dir_tree \"$dir\" || {{ printf '%s\\n' 'destination parent is not a real directory tree' >&2; exit 72; }}; \
         if [ -L \"$dst\" ] || {{ [ -e \"$dst\" ] && ! {{ {existing_guard}; }}; }}; then printf '%s\\n' 'destination is not a single-link regular file' >&2; exit 72; fi; \
         tmp=$(mktemp \"$dir/.agentum-write.XXXXXX\") || exit 72; cat > \"$tmp\" || exit 72; chmod 600 \"$tmp\" || exit 72; \
         if [ -L \"$dst\" ] || {{ [ -e \"$dst\" ] && ! {{ {existing_guard}; }}; }}; then printf '%s\\n' 'destination became unsafe before replacement' >&2; exit 72; fi; \
         mv -f \"$tmp\" \"$dst\" || exit 72; {existing_guard} || exit 72; committed=1",
        dir = q(&parent)?,
        path = dst,
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

fn remote_file_read_script(abs_path: &str) -> Result<String> {
    let parent = Path::new(abs_path)
        .parent()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    let path = q(abs_path)?;
    let guard = remote_regular_single_link_guard(&path);
    let inner = format!(
        "set -f; dir={dir}; dst={path}; validate_dir_tree() {{ path=$1; case \"$path\" in /*) ;; *) return 1;; esac; current=/; rest=${{path#/}}; \
           while [ -n \"$rest\" ]; do case \"$rest\" in */*) part=${{rest%%/*}}; rest=${{rest#*/}};; *) part=$rest; rest=;; esac; \
             case \"$part\" in ''|.) continue;; ..) return 1;; esac; current=${{current%/}}/$part; \
             if [ -L \"$current\" ]; then return 2; elif [ ! -e \"$current\" ]; then return 3; elif [ ! -d \"$current\" ]; then return 2; fi; done; }}; \
         validate_dir_tree \"$dir\"; dir_rc=$?; \
         if [ \"$dir_rc\" -eq 3 ]; then printf 'AGENTUM_FILE_MISSING\\n'; \
         elif [ \"$dir_rc\" -ne 0 ]; then printf '%s\\n' 'source parent is not a real directory tree' >&2; exit 73; \
         elif [ -L \"$dst\" ]; then printf '%s\\n' 'source is a symlink' >&2; exit 73; \
         elif [ ! -e \"$dst\" ]; then printf 'AGENTUM_FILE_MISSING\\n'; \
         elif ! {{ {guard}; }}; then printf '%s\\n' 'source is not a single-link regular file' >&2; exit 73; \
         elif [ ! -r \"$dst\" ]; then printf '%s\\n' 'file is not readable by the SSH user' >&2; exit 73; \
         else size=$({guard} && wc -c < \"$dst\") || exit 73; printf 'AGENTUM_FILE_PRESENT\\t%s\\n' \"$size\"; {guard} && cat \"$dst\" || exit 73; fi",
        dir = q(&parent)?,
        path = path,
    );
    Ok(format!("sh -c {}", q(&inner)?))
}

enum RemoteFileProbe {
    Missing,
    Present(Vec<u8>),
}

fn parse_remote_file_probe(stdout: &[u8]) -> Option<RemoteFileProbe> {
    const MISSING: &[u8] = b"AGENTUM_FILE_MISSING\n";
    const PRESENT: &[u8] = b"AGENTUM_FILE_PRESENT\t";
    let find = |needle: &[u8]| {
        stdout
            .windows(needle.len())
            .position(|window| window == needle)
    };
    if let Some(p) = find(PRESENT) {
        let size_start = p + PRESENT.len();
        let newline = stdout[size_start..].iter().position(|b| *b == b'\n')?;
        let content_start = size_start + newline + 1;
        let size = std::str::from_utf8(&stdout[size_start..size_start + newline])
            .ok()?
            .trim()
            .parse::<usize>()
            .ok()?;
        let content_end = content_start.checked_add(size)?;
        if content_end <= stdout.len() {
            return Some(RemoteFileProbe::Present(
                stdout[content_start..content_end].to_vec(),
            ));
        }
    }
    find(MISSING).map(|_| RemoteFileProbe::Missing)
}

fn unsafe_local_path(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

/// Create missing parent components without ever accepting a symlink as a
/// directory component. Existing real directories are left untouched; new
/// ones are owner-only on Unix because they may hold credential-bearing files.
fn local_real_directory_tree(parent: &Path, create_missing: bool) -> std::io::Result<()> {
    use std::path::Component;

    let mut current = PathBuf::new();
    for component in parent.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(unsafe_local_path(format!(
                    "credential parent contains `..`: {}",
                    parent.display()
                )));
            }
            Component::Normal(part) => current.push(part),
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    // macOS exposes stable system aliases such as
                    // `/var -> private/var` and `/tmp -> private/tmp`. They are
                    // root-owned and may legitimately precede the user's
                    // private tree, but the actual requested parent must always
                    // be a real directory. User-owned intermediate links remain
                    // fail-closed as well.
                    #[cfg(unix)]
                    let trusted_system_alias = {
                        use std::os::unix::fs::MetadataExt;
                        current != parent
                            && metadata.uid() == 0
                            && std::fs::metadata(&current)
                                .is_ok_and(|target| target.is_dir() && target.uid() == 0)
                    };
                    #[cfg(not(unix))]
                    let trusted_system_alias = false;
                    if trusted_system_alias {
                        continue;
                    }
                    return Err(unsafe_local_path(format!(
                        "credential parent component is not a real directory: {}",
                        current.display()
                    )));
                }
                if !metadata.is_dir() {
                    return Err(unsafe_local_path(format!(
                        "credential parent component is not a real directory: {}",
                        current.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create_missing {
                    return Err(error);
                }
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                match builder.create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                let metadata = std::fs::symlink_metadata(&current)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(unsafe_local_path(format!(
                        "credential parent changed into an unsafe path: {}",
                        current.display()
                    )));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn ensure_local_real_directory_tree(parent: &Path) -> std::io::Result<()> {
    local_real_directory_tree(parent, true)
}

fn validate_local_real_directory_tree(parent: &Path) -> std::io::Result<()> {
    local_real_directory_tree(parent, false)
}

/// A destination may be absent or an ordinary single-link file. Refusing
/// symlinks, devices, directories, and hardlinks gives callers a fail-closed
/// signal instead of silently replacing an attacker-prepared path.
fn validate_local_replace_destination(destination: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(unsafe_local_path(format!(
                    "credential destination is not a regular file: {}",
                    destination.display()
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(unsafe_local_path(format!(
                        "credential destination has {} hard links: {}",
                        metadata.nlink(),
                        destination.display()
                    )));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn open_local_safe_regular_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    let current = std::fs::symlink_metadata(path)?;
    if current.file_type().is_symlink() || !current.is_file() || !opened.is_file() {
        return Err(unsafe_local_path(format!(
            "credential source is not a regular file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if current.nlink() != 1 || opened.nlink() != 1 {
            return Err(unsafe_local_path(format!(
                "credential source is not a single-link regular file: {}",
                path.display()
            )));
        }
        if current.dev() != opened.dev() || current.ino() != opened.ino() {
            return Err(unsafe_local_path(format!(
                "credential source changed while it was opened: {}",
                path.display()
            )));
        }
    }
    Ok(file)
}

/// Write raw `content` to `abs_path` on `host` (local fs, or on the SSH host)
/// with owner-only (0600) permissions.
///
/// Security: the file must be unreadable to other users on the host (the token
/// is a credential). We write with `umask 077` to a `mktemp` file and `mv` it
/// into place atomically — so the final path can't be a pre-planted symlink we'd
/// follow, and the file is never briefly world-readable. Parent components and
/// any existing destination are checked fail-closed; content travels only over
/// process stdin, never as plaintext or encoded data in SSH argv.
async fn write_host_file_bytes_with_timeout(
    host: &Host,
    abs_path: &str,
    content: &[u8],
    remote_timeout: Duration,
    stage: &'static str,
) -> Result<()> {
    let destination = Path::new(abs_path);
    if !destination.is_absolute() {
        return Err(HostRuntimeError::RemotePrerequisite {
            stage: "file path",
            message: format!("destination `{abs_path}` is not absolute"),
        });
    }
    match &host.kind {
        HostKind::Local => {
            use std::io::Write as _;
            let parent = destination.parent().unwrap_or_else(|| Path::new("/"));
            ensure_local_real_directory_tree(parent).map_err(map_ssh_io)?;
            validate_local_replace_destination(destination).map_err(map_ssh_io)?;
            let tmp = parent.join(format!(".agentum-write-{}.tmp", Uuid::new_v4().simple()));
            let write_result = (|| -> std::io::Result<()> {
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&tmp)?;
                file.write_all(content)?;
                file.sync_all()?;
                drop(file);
                // Recheck immediately before replacement. `rename` itself does
                // not follow a final symlink, but surfacing the unsafe state is
                // preferable to silently accepting a planted destination.
                ensure_local_real_directory_tree(parent)?;
                validate_local_replace_destination(destination)?;
                std::fs::rename(&tmp, destination)
            })();
            if let Err(error) = write_result {
                let _ = std::fs::remove_file(&tmp);
                return Err(map_ssh_io(error));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o600))
                    .map_err(map_ssh_io)?;
                let metadata = std::fs::symlink_metadata(destination).map_err(map_ssh_io)?;
                if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1
                {
                    return Err(map_ssh_io(unsafe_local_path(
                        "credential destination became unsafe after replacement",
                    )));
                }
            }
            Ok(())
        }
        HostKind::Ssh { .. } => {
            // The script contains metadata only. The possibly-secret content is
            // fed through SSH stdin into a same-directory private temp file.
            let script = remote_atomic_write_script(abs_path)?;
            let out = ssh_output_with_stdin(host, &script, content, remote_timeout, stage).await?;
            if out.status.success() {
                Ok(())
            } else if out.status.code() == Some(255) {
                Err(ssh_stage_error(
                    host,
                    stage,
                    out.status.code(),
                    &String::from_utf8_lossy(&out.stderr),
                ))
            } else {
                Err(HostRuntimeError::NonZero {
                    status: out.status.code(),
                    stderr: bounded_text(
                        String::from_utf8_lossy(&out.stderr).trim(),
                        STARTUP_DIAGNOSTIC_BYTES,
                    ),
                })
            }
        }
    }
}

/// Write UTF-8 configuration/credential content with the established short
/// launch-time bound. Kept as the compatibility entry point for existing MCP
/// provisioning callers.
pub async fn write_remote_file(host: &Host, abs_path: &str, content: &str) -> Result<()> {
    write_host_file_bytes_with_timeout(
        host,
        abs_path,
        content.as_bytes(),
        REMOTE_LAUNCH_TIMEOUT,
        "private file write",
    )
    .await
}

/// Write arbitrary private file bytes on `host`. Uploads can be as large as 25
/// MiB, so their SSH stdin transfer receives a bounded two-minute window rather
/// than the short agent-launch configuration deadline.
pub async fn write_remote_file_bytes(host: &Host, abs_path: &str, content: &[u8]) -> Result<()> {
    write_host_file_bytes_with_timeout(
        host,
        abs_path,
        content,
        Duration::from_secs(120),
        "private binary file write",
    )
    .await
}

/// Read `abs_path` from `host` (local fs or SSH), or `None` when it doesn't
/// exist. Used to merge agentum into an existing agent config file (Cursor,
/// Gemini, OpenCode) without clobbering the user's other servers. Only stdout is
/// read, so the host's login-shell noise (fnm, etc.) on stderr is ignored.
pub async fn read_remote_file(host: &Host, abs_path: &str) -> Result<Option<String>> {
    let destination = Path::new(abs_path);
    if !destination.is_absolute() {
        return Err(HostRuntimeError::RemotePrerequisite {
            stage: "file path",
            message: format!("source `{abs_path}` is not absolute"),
        });
    }
    match &host.kind {
        HostKind::Local => {
            use std::io::Read as _;

            let parent = destination.parent().unwrap_or_else(|| Path::new("/"));
            match validate_local_real_directory_tree(parent) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(map_ssh_io(error)),
            }
            match std::fs::symlink_metadata(destination) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            validate_local_replace_destination(destination).map_err(map_ssh_io)?;
            let mut file = open_local_safe_regular_file(destination).map_err(map_ssh_io)?;
            let mut content = String::new();
            file.read_to_string(&mut content).map_err(map_ssh_io)?;
            Ok(Some(content))
        }
        HostKind::Ssh { .. } => {
            let script = remote_file_read_script(abs_path)?;
            let out = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(|e| ssh_stage_io(host, "private file read", e))?;
            if !out.status.success() {
                if out.status.code() == Some(255) {
                    return Err(ssh_stage_error(
                        host,
                        "private file read",
                        out.status.code(),
                        &String::from_utf8_lossy(&out.stderr),
                    ));
                }
                return Err(HostRuntimeError::NonZero {
                    status: out.status.code(),
                    stderr: bounded_text(
                        String::from_utf8_lossy(&out.stderr).trim(),
                        STARTUP_DIAGNOSTIC_BYTES,
                    ),
                });
            }
            match parse_remote_file_probe(&out.stdout) {
                Some(RemoteFileProbe::Missing) => Ok(None),
                Some(RemoteFileProbe::Present(content)) => {
                    Ok(Some(String::from_utf8_lossy(&content).into_owned()))
                }
                None => Err(HostRuntimeError::SshStage {
                    stage: "private file read",
                    host: host.name.clone(),
                    message: "remote file probe returned an invalid response".into(),
                }),
            }
        }
    }
}

/// Read the deterministic Claude transcript for an Agentum session from the
/// host that owns it. SSH paths are anchored at the remote user's `$HOME`;
/// they are never interpreted on the daemon filesystem.
pub async fn read_claude_transcript(
    host: &Host,
    workdir: &str,
    session_id: uuid::Uuid,
) -> Result<(String, Option<String>)> {
    match &host.kind {
        HostKind::Local => {
            let path =
                agentum_core::transcript::transcript_path_for(Path::new(workdir), session_id)
                    .ok_or(HostRuntimeError::Unsupported)?;
            let display = path.to_string_lossy().into_owned();
            let content = match std::fs::read_to_string(&path) {
                Ok(s) => Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            Ok((display, content))
        }
        HostKind::Ssh { .. } => {
            if !Path::new(workdir).is_absolute() {
                return Err(HostRuntimeError::Unsupported);
            }
            let encoded = workdir.replace('/', "-");
            let relative = format!(".claude/projects/{encoded}/{session_id}.jsonl");
            let script = claude_transcript_read_script(&relative)?;
            let out = ssh_output(host, &script, SSH_TIMEOUT)
                .await
                .map_err(map_ssh_io)?;
            let display = format!("$HOME/{relative}");
            if out.status.success() {
                Ok((display, Some(String::from_utf8(out.stdout)?)))
            } else if out.status.code() == Some(44) {
                Ok((display, None))
            } else {
                Err(HostRuntimeError::NonZero {
                    status: out.status.code(),
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                })
            }
        }
    }
}

fn claude_transcript_read_script(relative: &str) -> Result<String> {
    let relative = q(relative)?;
    Ok(format!(
        "path=\"$HOME\"/{relative}; if [ ! -e \"$path\" ]; then exit 44; fi; cat \"$path\""
    ))
}

pub async fn capture_pane_visible(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::capture_pane_visible(target).await?),
        HostKind::Ssh { .. } => {
            ssh_stdout(
                host,
                &remote_exact_tmux_script(target, "tmux capture-pane -p -S 0 -t \"$sid\"")?,
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
                &remote_exact_tmux_script(
                    target,
                    "tmux display-message -p -t \"$sid\" '#{pane_title}'",
                )?,
            )
            .await?;
            Ok(out.trim_matches(|c| c == '\n' || c == '\r').to_string())
        }
    }
}

/// Low-frequency state used to prove that a persistent remote pane tail is
/// still making progress. Keeping the title in the same response replaces the
/// old, separate title exec on the latency-sensitive interactive master.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePaneStreamState {
    pub log_size: u64,
    pub title: String,
}

/// Read the pane log size and title in one bounded round trip on the independent
/// observer ControlMaster. The operation neither touches the pane nor changes
/// the pipe. Callers compare `log_size` with bytes already received from their
/// `tail -f` child to distinguish a healthy idle pane from a silent channel.
pub async fn remote_pane_stream_state(
    host: &Host,
    target: &str,
    out_path: &Path,
) -> Result<RemotePaneStreamState> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Err(HostRuntimeError::Unsupported);
    }
    let script = remote_pane_stream_state_script(target, out_path)?;
    let output = ssh_output_on(host, &script, SSH_TIMEOUT, remote_pane_stream_mux())
        .await
        .map_err(map_ssh_io)?;
    if !output.status.success() {
        return Err(HostRuntimeError::NonZero {
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let stdout = String::from_utf8(output.stdout)?;
    let (size, title) = stdout.split_once('\n').unwrap_or((stdout.as_str(), ""));
    let log_size = size
        .trim()
        .parse::<u64>()
        .map_err(|_| HostRuntimeError::SshStage {
            stage: "pane stream state",
            host: host.name.clone(),
            message: "remote pane log size was not an unsigned integer".into(),
        })?;
    Ok(RemotePaneStreamState {
        log_size,
        title: title.trim_matches(|c| c == '\n' || c == '\r').to_string(),
    })
}

fn remote_pane_stream_mux() -> SshMux {
    SshMux::Observer
}

fn remote_pane_stream_state_script(target: &str, out_path: &Path) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    let operation = format!(
        "o=$(wc -c < {log} 2>/dev/null || echo 0); \
         t=$(tmux display-message -p -t \"$sid\" '#{{pane_title}}' 2>/dev/null || true); \
         printf '%s\\n%s' \"$o\" \"$t\""
    );
    remote_exact_tmux_script(target, &operation)
}

pub async fn send_keys(host: &Host, target: &str, keys: &str, append_enter: bool) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::send_keys(target, keys, append_enter).await?),
        HostKind::Ssh { .. } => {
            let mut operation = format!("tmux send-keys -t \"$sid\" {}", q(keys)?);
            if append_enter {
                operation.push_str(" Enter");
            }
            ssh_checked(host, &remote_exact_tmux_script(target, &operation)?).await
        }
    }
}

pub async fn send_bytes(host: &Host, target: &str, bytes: &[u8]) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::send_bytes(target, bytes).await?),
        HostKind::Ssh { .. } => {
            for chunk in bytes.chunks(agentum_tmux::SEND_KEYS_HEX_CHUNK_BYTES) {
                let mut operation = "tmux send-keys -H -t \"$sid\"".to_string();
                for b in chunk {
                    operation.push(' ');
                    operation.push_str(&format!("{b:02x}"));
                }
                ssh_checked(host, &remote_exact_tmux_script(target, &operation)?).await?;
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
            let operation = format!(
                "tmux set-option -q -t \"$sid\" window-size manual && tmux resize-window -t \"$sid\" -x {cols} -y {rows}"
            );
            ssh_checked(host, &remote_exact_tmux_script(target, &operation)?).await
        }
    }
}

/// Relative height nudge (see [`agentum_tmux::resize_window_relative`]). Used by
/// the remote redraw heal, which doesn't learn the pane's absolute size at
/// connect, to provoke a SIGWINCH with a shrink-then-restore toggle.
pub async fn resize_window_relative(host: &Host, target: &str, rows_delta: i16) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::resize_window_relative(target, rows_delta).await?),
        HostKind::Ssh { .. } => {
            if rows_delta == 0 {
                return Ok(());
            }
            let flag = if rows_delta > 0 { "-U" } else { "-D" };
            let count = rows_delta.unsigned_abs();
            let operation = format!(
                "tmux set-option -q -t \"$sid\" window-size manual && tmux resize-window -t \"$sid\" {flag} {count}"
            );
            ssh_checked(host, &remote_exact_tmux_script(target, &operation)?).await
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
    let name = safe_remote_leaf(out_path)?;
    Ok(format!("\"{REMOTE_PANE_DIR}/{name}\""))
}

/// POSIX command group that makes the remote pane directory/log private and
/// rejects legacy symlink/non-regular targets before any append/tail/chmod can
/// follow them. The directory becomes 0700 before a missing log is created.
fn remote_private_pane_log_setup(log: &str) -> String {
    let dir_setup = remote_private_dir_setup(REMOTE_PANE_DIR);
    let guard = remote_regular_single_link_guard(log);
    format!(
        "{{ {dir_setup} && \
         if [ -L {log} ]; then false; elif [ -e {log} ]; then {guard}; else (set -C; umask 077; : > {log}); fi && \
         {guard} && chmod 600 {log} && {guard}; }}"
    )
}

/// Build the `sh -c …` script that arms tmux pipe-pane on the remote, writing
/// raw pane output to the per-session log. Factored out so the (untestable
/// without a live host) quoting is at least covered by a string-shape unit test.
fn remote_pipe_script(target: &str, out_path: &Path) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    let setup = remote_private_pane_log_setup(&log);
    // tmux runs this command via `/bin/sh -c` on every flush; single-quoting it
    // keeps `$HOME` unexpanded through the outer shells so it resolves there.
    let guard = remote_regular_single_link_guard(&log);
    let sink = escape_tmux_pipe_command(&format!("umask 077; {guard} || exit 73; cat >> {log}"));
    let pipe = format!("exec sh -c {}", q(&sink)?);
    let pipe = q(&pipe)?;
    let arm = remote_pipe_pane_arm_operation(&pipe);
    let operation = format!("umask 077; {setup} || exit 73; {arm}");
    remote_exact_tmux_script(target, &operation)
}

/// Return a truly idempotent remote pipe arm for an already-resolved `$sid`.
///
/// `tmux pipe-pane -o` is deceptively named: tmux closes a live pipe first and
/// then `-o` suppresses opening its replacement. Blindly using it to "re-arm"
/// on attach therefore toggles the pane log off. Probe `#{pane_pipe}` instead;
/// plain `pipe-pane` makes a concurrent lost race end armed as well.
fn remote_pipe_pane_arm_operation(quoted_pipe_command: &str) -> String {
    format!(
        r#"if [ "$(tmux display-message -p -t "$sid" '#{{pane_pipe}}' 2>/dev/null)" != 1 ]; then tmux pipe-pane -t "$sid" {quoted_pipe_command}; fi"#
    )
}

/// Escape strftime placeholders before a shell command crosses tmux's command
/// parser. `pipe-pane` expands `%d`, `%u`, and friends even inside shell quotes;
/// the pane-log guards use those tokens as `stat` formats and must receive them
/// literally. tmux turns each doubled `%%` back into one `%` for the shell.
fn escape_tmux_pipe_command(command: &str) -> String {
    command.replace('%', "%%")
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
/// The private setup creates a missing log without following a planted link;
/// `exec` lets a kill of the ssh child reap the remote tail cleanly.
fn remote_tail_script(out_path: &Path, from_offset: Option<u64>) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    let setup = remote_private_pane_log_setup(&log);
    let mode = match from_offset {
        Some(n) => format!("-c +{}", n.saturating_add(1)),
        None => "-n 0".to_string(),
    };
    let inner = format!("umask 077; {setup} || exit 73; exec tail {mode} -f {log}");
    Ok(format!("sh -c {}", q(&inner)?))
}

/// Spawn a long-lived `tail -f` of the remote pane log over a single persistent
/// SSH channel. The caller reads `child.stdout` for raw pane bytes and kills the
/// child on disconnect (also guarded by `kill_on_drop`). SSH hosts only — local
/// sessions tail the on-disk log directly via [`stream_session`].
///
/// `mux` is normally [`SshMux::Streaming`], keeping every tail for one host on a
/// single pooled TCP connection. Recovery may select [`SshMux::Off`] after it
/// evicts a wedged or saturated streaming master, providing a fresh-connection
/// escape hatch instead of reconnecting forever through the same dead socket.
pub fn spawn_remote_pane_tail(
    host: &Host,
    out_path: &Path,
    from_offset: Option<u64>,
    mux: SshMux,
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
    let mut cmd = ssh_command_opts(host, &script, mux);
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

/// Build the `sh -c …` script behind [`spawn_remote_input_writer`]. This keeps
/// the desktop implementation's deliberately simple newline-framed hex
/// protocol, while resolving the target once to its immutable tmux id. Large
/// input is split and lightly paced: tmux's pane PTY can accept only about 1 KiB
/// at once and otherwise reports success while silently dropping whole chunks.
fn remote_input_script(target: &str) -> Result<String> {
    let operation = "exec sh -c 'sid=$1; while IFS= read -r l; do tmux send-keys -H -t \"$sid\" $l 2>/dev/null; if [ \"${#l}\" -gt 128 ]; then sleep 0.01; fi; done' sh \"$sid\"";
    remote_exact_tmux_script(target, operation)
}

/// Stay far below the pane PTY's roughly 1 KiB input queue. The remote loop
/// paces these multi-byte records; normal key events are one tiny un-delayed
/// record and retain the low-latency persistent-channel path.
const REMOTE_INPUT_FRAME_BYTES: usize = 64;

/// Spawn a long-lived keystroke writer over one persistent SSH channel: the
/// caller writes newline-framed hex bytes to `child.stdin`, and a remote
/// read-loop feeds them to the pane.
///
/// Why this exists: the old path ran one `ssh … tmux send-keys` *exec per
/// keystroke*. Each exec opens a fresh ControlMaster channel and round-trips a
/// command — ~450 ms against a distant host (measured to a 150 ms-RTT box) —
/// so typing into a remote agent was unusable. With a persistent channel a
/// keystroke is just a one-way write down an already-open stream: ~1 RTT
/// (~150 ms) to delivery, no per-key channel setup, no master-channel churn.
///
/// Rides the interactive ControlMaster, unlike the tail. The
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

/// Encode raw terminal input as newline-framed, space-separated lowercase hex
/// for the persistent remote writer. This is the proven desktop protocol, with
/// a smaller lossless frame bound so successive tmux injections cannot overrun
/// the pane PTY.
pub fn encode_remote_input_lines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len().saturating_mul(3) + 1);
    for chunk in bytes.chunks(REMOTE_INPUT_FRAME_BYTES) {
        for (index, byte) in chunk.iter().enumerate() {
            if index > 0 {
                out.push(b' ');
            }
            out.extend_from_slice(format!("{byte:02x}").as_bytes());
        }
        out.push(b'\n');
    }
    out
}

/// Disarm `pipe-pane` on a pane (a bare `tmux pipe-pane` closes the pipe).
/// Used when detaching from an external tmux session: the underlying
/// session must stay alive, but its output should stop feeding our log.
pub async fn unpipe_pane(host: &Host, target: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::unpipe_pane(target).await?),
        HostKind::Ssh { .. } => {
            ssh_checked(
                host,
                &remote_exact_tmux_script(target, "tmux pipe-pane -t \"$sid\"")?,
            )
            .await
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

/// Like [`list_tmux_sessions`] but returns ALL sessions (external + managed).
/// Used by the host-level tmux browser in the desktop UI.
pub async fn list_all_tmux_sessions(host: &Host) -> Result<Vec<DiscoveredTmuxSession>> {
    let raw = tmux_discover_raw(host).await?;
    Ok(parse_tmux_panes_all(&raw))
}

/// Parse [`TMUX_DISCOVER_FORMAT`] pane lines into ALL sessions regardless of
/// the `agentum-*` naming convention.
fn parse_tmux_panes_all(stdout: &str) -> Vec<DiscoveredTmuxSession> {
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
        let pane = DiscoveredPane {
            command: command.to_string(),
            cwd: cwd.to_string(),
        };
        match sessions.iter_mut().find(|s| s.name == name) {
            Some(s) => s.panes.push(pane),
            None => sessions.push(DiscoveredTmuxSession {
                name: name.to_string(),
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

/// Kill a tmux session on the host by name. Errors if the session doesn't
/// exist or the transport fails.
pub async fn kill_tmux_session(host: &Host, name: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::kill_session(name).await?),
        HostKind::Ssh { .. } => {
            let script = remote_exact_tmux_script(name, "tmux kill-session -t \"$sid\"")?;
            ssh_checked(host, &script).await
        }
    }
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

/// Playwright's browser install can download ~150 MB; the readiness `SSH_TIMEOUT`
/// is far too short, and even `BOOTSTRAP_TIMEOUT` (180s) is tight on a slow link.
const BROWSER_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// POSIX script that prints the first of `candidates` found on `PATH` (or
/// nothing). `command -v` keeps it portable across the host's login shell
/// (fish/zsh/bash). Pure so the probe shape is unit-testable.
fn which_first_script(candidates: &[&str]) -> String {
    let names = candidates.join(" ");
    format!(
        "for b in {names}; do if command -v \"$b\" >/dev/null 2>&1; then printf %s \"$b\"; exit 0; fi; done"
    )
}

/// Return the first of `candidates` on the host's `PATH`, or `None`. One round
/// trip. `candidates` must be plain binary names (no shell metacharacters) — they
/// are embedded directly in the probe loop.
pub async fn which_first(host: &Host, candidates: &[&str]) -> Result<Option<String>> {
    let script = which_first_script(candidates);
    let out = match &host.kind {
        HostKind::Local => {
            let o = Command::new("sh")
                .arg("-c")
                .arg(&script)
                .output()
                .await
                .map_err(map_ssh_io)?;
            String::from_utf8_lossy(&o.stdout).into_owned()
        }
        HostKind::Ssh { .. } => ssh_stdout(host, &format!("sh -c {}", q(&script)?)).await?,
    };
    let name = out.trim();
    Ok((!name.is_empty()).then(|| name.to_string()))
}

/// Best-effort install of Chromium on the host via Playwright (`npx playwright
/// install chromium`). Login shell (`sh -lc`) so node/npx on a user PATH (nvm,
/// fnm) resolve. Returns the combined output tail; errors on a non-zero exit so
/// the caller can surface a stated reason. Needs node/npx on the host.
pub async fn install_host_chromium(host: &Host) -> Result<String> {
    let cmd = "npx --yes playwright install chromium";
    match &host.kind {
        HostKind::Local => {
            let o = Command::new("sh")
                .arg("-lc")
                .arg(cmd)
                .output()
                .await
                .map_err(map_ssh_io)?;
            let tail = String::from_utf8_lossy(&o.stdout).into_owned();
            if o.status.success() {
                Ok(tail)
            } else {
                Err(HostRuntimeError::NonZero {
                    status: o.status.code(),
                    stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
                })
            }
        }
        HostKind::Ssh { .. } => {
            let out = ssh_output(
                host,
                &format!("sh -lc {}", q(cmd)?),
                BROWSER_INSTALL_TIMEOUT,
            )
            .await
            .map_err(map_ssh_io)?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                Err(HostRuntimeError::NonZero {
                    status: out.status.code(),
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                })
            }
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

    fn ssh_host() -> Host {
        Host {
            id: Uuid::nil(),
            name: "remote-test".into(),
            kind: HostKind::Ssh {
                user: "alice".into(),
                hostname: "example.test".into(),
                port: 2222,
                auth: agentum_core::SshAuth::Agent,
            },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    #[test]
    fn target_missing_classifies_only_tmux_disappearance_errors() {
        for stderr in [
            "can't find window: agentum-alpha",
            "can't find session: agentum-alpha",
            "can't find pane: agentum-alpha.0",
            "no server running on /tmp/tmux-1000/default",
        ] {
            let error = HostRuntimeError::NonZero {
                status: Some(1),
                stderr: stderr.to_string(),
            };
            assert!(
                error.is_tmux_target_missing(),
                "expected target-missing classification for: {stderr}"
            );
        }

        for stderr in [
            "ssh: connect to host box port 22: Connection refused",
            "tmux: command not found",
            "permission denied",
        ] {
            let error = HostRuntimeError::NonZero {
                status: Some(1),
                stderr: stderr.to_string(),
            };
            assert!(
                !error.is_tmux_target_missing(),
                "must preserve actionable resize error: {stderr}"
            );
        }
    }

    #[test]
    fn reverse_tunnel_port_is_stable_per_saved_host_in_private_range() {
        let first = Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap();
        let second = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let top = Uuid::parse_str("00000000-0000-0000-0000-000000003fff").unwrap();

        assert_eq!(reverse_tunnel_host_port(first), 49_152);
        assert_eq!(reverse_tunnel_host_port(second), 49_153);
        assert_eq!(reverse_tunnel_host_port(top), 65_535);
        assert_eq!(
            reverse_tunnel_host_port(second),
            reverse_tunnel_host_port(second),
            "saved host UUID must map identically across server boots"
        );

        let arbitrary = Uuid::parse_str("be2aed4e-f38c-4cc4-99e1-75b8e88a99fe").unwrap();
        let port = reverse_tunnel_host_port(arbitrary);
        assert!((49_152..=65_535).contains(&port));
    }

    #[test]
    fn control_master_check_targets_each_role_without_opening_a_connection() {
        let host = ssh_host();
        let mut paths = Vec::new();
        for mux in [SshMux::Interactive, SshMux::Streaming, SshMux::Observer] {
            let command = ssh_control_check_cmd(&host, mux).expect("private ControlPath");
            let args: Vec<String> = command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert_eq!(command.as_std().get_program(), "ssh");
            assert!(
                args.windows(2).any(|pair| pair == ["-O", "check"]),
                "warmup must inspect the pooled master: {args:?}"
            );
            let path = args
                .iter()
                .find(|arg| arg.starts_with("ControlPath="))
                .expect("control path")
                .clone();
            paths.push(path);
            assert!(
                !args.iter().any(|arg| arg == "forward" || arg == "cancel"),
                "verification must not mutate shared forwards: {args:?}"
            );
            assert_eq!(args.last().map(String::as_str), Some("alice@example.test"));
        }
        assert_ne!(paths[0], paths[1], "roles must inspect distinct sockets");
        assert!(ssh_control_check_cmd(&host, SshMux::Off).is_none());
    }

    #[test]
    fn pooled_probe_evicts_only_known_pre_session_failures() {
        assert_eq!(
            classify_pooled_probe(Some(0), b"", b""),
            PooledProbeDisposition::Healthy
        );
        assert_eq!(
            classify_pooled_probe(
                Some(255),
                b"",
                b"mux_client_request_session: master alive request failed\n"
            ),
            PooledProbeDisposition::Evict
        );
        assert_eq!(
            classify_pooled_probe(
                Some(255),
                b"",
                b"mux_client_request_session: session request failed: Session open refused by peer\n"
            ),
            PooledProbeDisposition::Busy
        );
        assert_eq!(
            classify_pooled_probe(Some(255), b"", b"Permission denied (publickey).\n"),
            PooledProbeDisposition::Failed
        );
        // Output means the remote command may have run; never classify it as a
        // replay-safe mux failure even when stderr contains a familiar line.
        assert_eq!(
            classify_pooled_probe(
                Some(255),
                b"remote output",
                b"mux_client_request_session: master alive request failed\n"
            ),
            PooledProbeDisposition::Failed
        );
    }

    #[test]
    fn control_master_generation_is_parsed_from_openssh_check_output() {
        assert_eq!(
            parse_control_master_generation(b"", b"Master running (pid=4812)\r\n").as_deref(),
            Some("pid:4812")
        );
        assert_eq!(parse_control_master_generation(b"ok", b""), None);
    }

    #[test]
    fn replacement_master_generation_cannot_hit_or_preserve_stale_forward_cache() {
        let host_id = Uuid::new_v4();
        let old = ReverseTunnelKey {
            host_id,
            master_generation: "pid:10".into(),
            host_listen_port: 50_001,
            mac_destination_port: 61_001,
        };
        let replacement = ReverseTunnelKey {
            master_generation: "pid:11".into(),
            ..old.clone()
        };
        let mut state = TunnelControlState::default();
        state.reverse_tunnels.insert(old.clone());

        assert!(!state.reverse_tunnels.contains(&replacement));
        remember_armed_reverse_tunnel(&mut state, replacement.clone());
        assert!(state.reverse_tunnels.contains(&replacement));
        assert!(!state.reverse_tunnels.contains(&old));
        assert_eq!(state.reverse_tunnels.len(), 1);
    }

    #[tokio::test]
    async fn post_commit_discard_removes_only_the_mutated_hosts_control_state() {
        let removed_id = Uuid::new_v4();
        let retained_id = Uuid::new_v4();
        let key = |host_id| ReverseTunnelKey {
            host_id,
            master_generation: "pid:test".into(),
            host_listen_port: 50_111,
            mac_destination_port: 61_111,
        };
        {
            let _master_guard = acquire_ssh_master_warm(removed_id).await;
            let mut state = tunnel_control_lock().lock().await;
            state.reverse_tunnels.insert(key(removed_id));
            state.reverse_tunnels.insert(key(retained_id));
            state.legacy_retired_revisions.insert(removed_id, 1);
            state.legacy_retired_revisions.insert(retained_id, 2);
        }

        discard_ssh_control_state(removed_id).await;

        let _master_guard = acquire_ssh_master_warm(removed_id).await;
        let mut state = tunnel_control_lock().lock().await;
        assert!(
            !state
                .reverse_tunnels
                .iter()
                .any(|key| key.host_id == removed_id)
        );
        assert!(!state.legacy_retired_revisions.contains_key(&removed_id));
        assert!(
            state
                .reverse_tunnels
                .iter()
                .any(|key| key.host_id == retained_id)
        );
        assert_eq!(state.legacy_retired_revisions.get(&retained_id), Some(&2));
        state
            .reverse_tunnels
            .retain(|key| key.host_id != retained_id);
        state.legacy_retired_revisions.remove(&retained_id);
    }

    #[tokio::test]
    async fn ssh_master_lifecycle_serializes_one_host_without_blocking_others() {
        let first_host = Uuid::new_v4();
        let other_host = Uuid::new_v4();
        let held = acquire_ssh_master_warm(first_host).await;

        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                acquire_ssh_master_warm(first_host)
            )
            .await
            .is_err(),
            "the same host must remain serialized"
        );
        let other = tokio::time::timeout(
            Duration::from_millis(20),
            acquire_ssh_master_warm(other_host),
        )
        .await
        .expect("an unrelated host must warm concurrently");

        drop(other);
        drop(held);
        tokio::time::timeout(
            Duration::from_millis(20),
            acquire_ssh_master_warm(first_host),
        )
        .await
        .expect("the same host lock must release");
    }

    #[test]
    fn remote_workdir_resolution_uses_only_remote_home() {
        let home = "/srv/remote user";
        assert_eq!(resolve_remote_workdir(home, "~"), PathBuf::from(home));
        assert_eq!(
            resolve_remote_workdir(home, "~/repo"),
            PathBuf::from("/srv/remote user/repo")
        );
        assert_eq!(
            resolve_remote_workdir(home, "/opt/repo"),
            PathBuf::from("/opt/repo")
        );
        assert_eq!(
            resolve_remote_workdir(home, "repo/subdir"),
            PathBuf::from("/srv/remote user/repo/subdir")
        );
        assert_eq!(resolve_remote_workdir("/", "repo"), PathBuf::from("/repo"));
    }

    #[test]
    fn remote_home_protocol_ignores_login_shell_stdout_noise() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("/home/remote user");
        let stdout = format!("fish init banner\ncolors and MOTD\nAGENTUM_HOME\t{encoded}\n");
        assert_eq!(
            parse_remote_home(&stdout).as_deref(),
            Some("/home/remote user")
        );
        assert!(remote_home_script().unwrap().contains("AGENTUM_HOME"));
    }

    #[test]
    fn preflight_script_validates_directory_tmux_binary_shell_and_transcript() {
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000042").unwrap();
        let script = preflight_validation_script(Path::new("/srv/my repo"), "claude", id)
            .expect("preflight script");
        assert!(
            script.contains("[ -d"),
            "directory existence is not checked"
        );
        assert!(script.contains("[ -x"), "directory access is not checked");
        assert!(
            script.contains("list-commands"),
            "tmux capability probe missing"
        );
        assert!(script.contains("command -v"), "PATH resolution missing");
        assert!(script.contains("${SHELL:-/bin/sh}"), "remote shell missing");
        assert!(
            script.contains("${PATH-}"),
            "fresh SSH PATH is not returned to the launcher"
        );
        assert!(
            script.contains("00000000-0000-0000-0000-000000000042.jsonl"),
            "deterministic remote Claude transcript is not checked"
        );
        assert!(script.contains("AGENTUM_PREFLIGHT_OK"));
    }

    #[test]
    fn launch_env_is_shell_safe_and_never_enters_remote_command() {
        let secret = "sentinel '$TOKEN' with spaces\nand newline";
        let env = vec![("AGENTUM_MCP_BEARER_TOKEN".into(), secret.into())];
        let payload = String::from_utf8(serialize_launch_env(&env).unwrap()).unwrap();
        let script = remote_launch_script(
            "agentum-test",
            Path::new("/srv/repo"),
            &["/usr/bin/codex".into(), "--help".into()],
            Path::new("session-42.log"),
        )
        .unwrap();
        assert!(payload.contains("export AGENTUM_MCP_BEARER_TOKEN="));
        assert!(payload.contains("sentinel"));
        assert!(!script.contains(secret), "secret leaked into ssh/tmux argv");
        assert!(!script.contains("new-session -e"));
        assert!(script.contains("cat >"), "stdin is not staged remotely");
        assert!(script.contains("chmod 600"), "env file is not private");
        assert!(script.contains("rm -f"), "env file is not unlinked");
    }

    #[test]
    fn launch_transaction_arms_pipe_before_real_process_and_checks_liveness() {
        let script = remote_launch_script(
            "agentum-test",
            Path::new("/srv/repo"),
            &["/usr/local/bin/agent".into(), "go".into()],
            Path::new("session-99.log"),
        )
        .unwrap();
        let new = script.find("new-session").unwrap();
        let retain = script.find("remain-on-exit on").unwrap();
        let pipe = script.find("pipe-pane -t").unwrap();
        let respawn = script.find("respawn-pane -k").unwrap();
        let inspect = script.find("pane_dead").unwrap();
        let commit = script.find("remain-on-exit off").unwrap();
        assert!(new < retain && retain < pipe && pipe < respawn);
        assert!(respawn < inspect && inspect < commit);
        assert!(script.contains("created=0"));
        assert!(
            script.contains("kill-session"),
            "partial pane cleanup missing"
        );
        assert!(script.contains("chmod 700"));
        assert!(script.contains("session-99.log"));
        assert!(script.contains("-P -F"), "new session ID is not captured");
        assert!(script.contains("list-sessions"), "exact lookup is missing");
        assert!(script.contains("$created_sid"));
        assert!(script.contains("$sid"));
        assert!(script.contains("stat -c %h") && script.contains("stat -f %l"));
        assert!(!script.contains("touch "), "pane log must not use touch");
    }

    #[test]
    fn launch_protocol_preserves_early_exit_status_and_output() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("bad config\ntry again");
        match parse_launch_protocol(
            format!("noise\nAGENTUM_EARLY_EXIT\t78\t{encoded}\n").as_bytes(),
        ) {
            LaunchProtocol::EarlyExit { status, diagnostic } => {
                assert_eq!(status, Some(78));
                assert_eq!(diagnostic, "bad config\ntry again");
            }
            _ => panic!("early-exit protocol was not parsed"),
        }
        assert!(matches!(
            parse_launch_protocol(b"AGENTUM_LAUNCH_OK\n"),
            LaunchProtocol::Ok
        ));
    }

    #[test]
    fn startup_output_is_secret_redacted_sanitized_and_bounded() {
        let token = "sentinel-secret-token";
        let env = vec![("TOKEN".into(), token.into())];
        let raw = format!("error: {token}\u{1b}[31m{}", "x".repeat(20_000));
        let diagnostic = redact_startup_output(&raw, &env, &[]);
        assert!(!diagnostic.contains(token));
        assert!(diagnostic.contains("[REDACTED]"));
        assert!(!diagnostic.contains('\u{1b}'));
        assert!(diagnostic.len() <= STARTUP_DIAGNOSTIC_BYTES + 3);
    }

    #[test]
    fn startup_output_redacts_config_only_secret_not_present_in_environment() {
        let token = "config-only-stable-token";
        let raw = format!("invalid Authorization header: Bearer {token}");
        let diagnostic = redact_startup_output(&raw, &[], &[token]);
        assert!(!diagnostic.contains(token));
        assert_eq!(
            diagnostic,
            "invalid Authorization header: Bearer [REDACTED]"
        );
    }

    #[test]
    fn launch_env_rejects_non_posix_names_without_echoing_values() {
        let error = serialize_launch_env(&[("BAD-NAME".into(), "do-not-print".into())])
            .unwrap_err()
            .to_string();
        assert!(error.contains("BAD-NAME"));
        assert!(!error.contains("do-not-print"));
    }

    #[cfg(unix)]
    fn run_exact_tmux_resolver(target: &str) -> std::process::Output {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let fake_tmux = dir.path().join("tmux");
        std::fs::write(
            &fake_tmux,
            "#!/bin/sh\nif [ \"$1\" = list-sessions ]; then printf '$11_agent\\n$12_agentum-real\\n'; exit 0; fi\nexit 97\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
        let script =
            remote_exact_tmux_script(target, "printf 'RESOLVED\\t%s\\n' \"$sid\"").unwrap();
        std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .output()
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn exact_tmux_resolution_never_selects_a_name_prefix() {
        let exact = run_exact_tmux_resolver("agent");
        assert!(exact.status.success());
        assert_eq!(String::from_utf8(exact.stdout).unwrap(), "RESOLVED\t$11\n");

        let prefix = run_exact_tmux_resolver("age");
        assert_eq!(prefix.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&prefix.stderr).contains("can't find session: age"));
    }

    #[cfg(unix)]
    #[test]
    fn exact_tmux_resolution_accepts_only_well_formed_immutable_ids() {
        let exact = run_exact_tmux_resolver("$12");
        assert!(exact.status.success());
        assert_eq!(String::from_utf8(exact.stdout).unwrap(), "RESOLVED\t$12\n");
        for malformed in ["$", "$12oops", "$-1"] {
            let error = remote_exact_tmux_script(malformed, "true").unwrap_err();
            assert!(error.to_string().contains("malformed tmux session id"));
        }
    }

    #[tokio::test]
    async fn timed_out_ssh_cleanup_kills_reaps_and_drains_without_hanging() {
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let mut stdout_task = spawn_ssh_reader(child.stdout.take().unwrap());
        let mut stderr_task = spawn_ssh_reader(child.stderr.take().unwrap());
        let started = Instant::now();
        let diagnostics =
            cleanup_timed_out_ssh_child(&mut child, &mut stdout_task, &mut stderr_task).await;
        assert!(diagnostics.is_empty(), "cleanup failed: {diagnostics:?}");
        assert!(started.elapsed() < SSH_REAP_TIMEOUT);
        assert!(child.try_wait().unwrap().is_some(), "child was not reaped");
    }

    #[test]
    fn remote_private_file_write_is_stdin_only_and_atomic() {
        let script = remote_atomic_write_script("/tmp/agentum-secret.json").unwrap();
        assert!(script.starts_with("sh -c "));
        assert!(script.contains("umask 077"));
        assert!(script.contains("mktemp"));
        assert!(script.contains("cat >"), "payload must arrive on stdin");
        assert!(script.contains("chmod 600"));
        assert!(script.contains("mv -f"), "replacement must be atomic");
        assert!(script.contains("trap cleanup"));
        assert!(script.contains("ensure_dir_tree"));
        assert!(script.contains("[ -L"), "symlink checks missing: {script}");
        assert!(script.contains("stat -c %h") && script.contains("stat -f %l"));
        assert!(
            !script.contains("mkdir -p"),
            "recursive mkdir could follow a planted parent symlink: {script}"
        );
        assert!(
            !script.contains("base64"),
            "encoded secrets must not enter argv"
        );
    }

    #[cfg(unix)]
    #[test]
    fn generated_remote_security_scripts_are_valid_posix_shell() {
        let scripts = [
            remote_launch_script(
                "agentum-shell-check",
                Path::new("/srv/project"),
                &["/usr/bin/codex".into()],
                Path::new("shell-check.log"),
            )
            .unwrap(),
            remote_atomic_write_script("/home/alice/.claude/mcp-shell-check.json").unwrap(),
            remote_file_read_script("/home/alice/.claude/mcp-shell-check.json").unwrap(),
            remote_pipe_script("agentum-shell-check", Path::new("shell-check.log")).unwrap(),
            snapshot_with_offset_script("agentum-shell-check", Path::new("shell-check.log"))
                .unwrap(),
            remote_input_script("agentum-shell-check").unwrap(),
            remote_tail_script(Path::new("shell-check.log"), Some(7)).unwrap(),
        ];
        for script in scripts {
            let argv = shlex::split(&script).expect("generated outer command is shell-parseable");
            assert_eq!(&argv[..2], ["sh", "-c"]);
            let checked = std::process::Command::new("sh")
                .arg("-n")
                .arg("-c")
                .arg(&argv[2])
                .output()
                .unwrap();
            assert!(
                checked.status.success(),
                "invalid generated shell:\n{}\n{}",
                argv[2],
                String::from_utf8_lossy(&checked.stderr)
            );
        }
    }

    #[test]
    fn remote_file_read_distinguishes_missing_from_unreadable() {
        let script = remote_file_read_script("/tmp/agentum.json").unwrap();
        assert!(script.contains("[ ! -e"));
        assert!(script.contains("AGENTUM_FILE_MISSING"));
        // The outer shell-quoting layer may split the literal test tokens, so
        // assert the stable diagnostic that proves the unreadable branch is
        // present instead of depending on shlex's representation.
        assert!(script.contains("file is not readable by the SSH user"));
        assert!(script.contains("AGENTUM_FILE_PRESENT"));
        assert!(script.contains("cat"));
        assert!(script.contains("validate_dir_tree"));
        assert!(script.contains("[ -L"));
        assert!(script.contains("stat -c %h") && script.contains("stat -f %l"));

        match parse_remote_file_probe(
            b"fish init noise\nAGENTUM_FILE_PRESENT\t2\n{}trailing shell noise",
        ) {
            Some(RemoteFileProbe::Present(content)) => assert_eq!(content, b"{}"),
            _ => panic!("present file protocol was not parsed through shell noise"),
        }
        assert!(matches!(
            parse_remote_file_probe(b"banner\nAGENTUM_FILE_MISSING\n"),
            Some(RemoteFileProbe::Missing)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_private_file_write_refuses_symlink_destination() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim.json");
        let destination = dir.path().join("config.json");
        std::fs::write(&victim, "keep-me").unwrap();
        symlink(&victim, &destination).unwrap();

        let error = write_remote_file(
            &local_host(),
            destination.to_str().unwrap(),
            "new-private-content",
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("not a regular file"));
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep-me");
        assert!(
            std::fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_private_binary_write_preserves_every_byte_and_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let destination = dir.path().join("uploads").join("image.bin");
        let content = [0x00, 0xff, 0x89, b'P', b'N', b'G', 0x0a, 0x80];
        write_remote_file_bytes(&local_host(), destination.to_str().unwrap(), &content)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), content);
        assert_eq!(
            std::fs::symlink_metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_private_file_write_refuses_symlink_parent_and_hardlink_destination() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let real_parent = dir.path().join("real");
        std::fs::create_dir(&real_parent).unwrap();
        let linked_parent = dir.path().join("linked");
        symlink(&real_parent, &linked_parent).unwrap();
        let via_link = linked_parent.join("config.json");
        let error = write_remote_file(&local_host(), via_link.to_str().unwrap(), "must-not-write")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not a real directory"));
        assert!(!real_parent.join("config.json").exists());

        let victim = dir.path().join("victim.json");
        let hardlink = dir.path().join("config-hardlink.json");
        std::fs::write(&victim, "keep-me").unwrap();
        std::fs::hard_link(&victim, &hardlink).unwrap();
        let error = write_remote_file(&local_host(), hardlink.to_str().unwrap(), "must-not-write")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("hard links"));
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep-me");
        assert_eq!(std::fs::read_to_string(&hardlink).unwrap(), "keep-me");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_private_file_write_creates_real_private_parents_and_file() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let dir = tempfile::TempDir::new().unwrap();
        let parent = dir.path().join("new").join("nested");
        let destination = parent.join("config.json");
        write_remote_file(
            &local_host(),
            destination.to_str().unwrap(),
            "private-content",
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "private-content"
        );
        let parent_meta = std::fs::symlink_metadata(&parent).unwrap();
        assert!(parent_meta.is_dir());
        assert!(!parent_meta.file_type().is_symlink());
        assert_eq!(parent_meta.permissions().mode() & 0o777, 0o700);
        let file_meta = std::fs::symlink_metadata(&destination).unwrap();
        assert!(file_meta.is_file());
        assert_eq!(file_meta.nlink(), 1);
        assert_eq!(file_meta.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_private_file_read_rejects_symlinks_and_hardlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim.json");
        std::fs::write(&victim, "{\"secret\":true}").unwrap();

        let symlink_source = dir.path().join("symlink.json");
        symlink(&victim, &symlink_source).unwrap();
        let error = read_remote_file(&local_host(), symlink_source.to_str().unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not a regular file"));

        let hardlink_source = dir.path().join("hardlink.json");
        std::fs::hard_link(&victim, &hardlink_source).unwrap();
        let error = read_remote_file(&local_host(), hardlink_source.to_str().unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("hard links"));

        let real_parent = dir.path().join("real-parent");
        std::fs::create_dir(&real_parent).unwrap();
        std::fs::write(real_parent.join("config.json"), "{}").unwrap();
        let linked_parent = dir.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        let error = read_remote_file(
            &local_host(),
            linked_parent.join("config.json").to_str().unwrap(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("not a real directory"));

        assert_eq!(
            read_remote_file(
                &local_host(),
                dir.path().join("missing.json").to_str().unwrap()
            )
            .await
            .unwrap(),
            None
        );
    }

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
    fn remote_pane_stream_state_combines_size_and_title_without_touching_pane() {
        assert_eq!(remote_pane_stream_mux(), SshMux::Observer);
        let p = std::path::Path::new("/x/sessions/sess-1.log");
        let script = remote_pane_stream_state_script("agentum-demo", p).unwrap();
        assert!(script.starts_with("sh -c "), "not sh-wrapped: {script}");
        assert!(script.contains("wc -c"), "log size missing: {script}");
        assert!(script.contains("#{pane_title}"), "title missing: {script}");
        assert!(
            script.contains("list-sessions"),
            "exact target lookup missing"
        );
        assert!(script.contains("$HOME/.agentum/panes/sess-1.log"));
        assert!(!script.contains("capture-pane"));
        assert!(!script.contains("send-keys"));
        assert!(!script.contains("pipe-pane"));
    }

    #[test]
    fn remote_pipe_script_arms_pipe_pane_to_session_log() {
        let p = std::path::Path::new("/x/sessions/sess-1.log");
        let script = remote_pipe_script("agentum-demo", p).unwrap();
        // Wrapped for fish/zsh logins, makes the dir, idempotently arms the
        // pipe, and routes raw pane output into the home-relative session log.
        assert!(script.starts_with("sh -c "), "not sh-wrapped: {script}");
        assert!(script.contains("mkdir -p"), "no mkdir: {script}");
        // `-o` toggles an existing pipe off. Agentum's refactor probes the
        // pane state and only uses plain pipe-pane when no sink is live.
        assert!(
            script.contains("#{pane_pipe}"),
            "no pane_pipe guard: {script}"
        );
        assert!(
            script.contains("tmux pipe-pane -t"),
            "no pipe-pane arm: {script}"
        );
        assert!(
            !script.contains("pipe-pane -o"),
            "toggling -o arm crept back in: {script}"
        );
        assert!(script.contains("agentum-demo"), "target missing: {script}");
        assert!(
            script.contains("sess-1.log"),
            "log basename missing: {script}"
        );
        assert!(script.contains("cat >>"), "not an append sink: {script}");
        assert!(script.contains("$HOME"), "log not home-relative: {script}");
        assert!(script.contains("umask 077"), "pipe umask is not private");
        assert!(script.contains("chmod 700"), "pane dir is not private");
        assert!(script.contains("chmod 600"), "pane log is not private");
        assert!(script.contains("list-sessions"), "exact lookup missing");
        assert!(script.contains("$sid"), "pipe does not use immutable ID");
        assert!(script.contains("stat -c %h") && script.contains("stat -f %l"));
    }

    /// Execute the generated remote script against a real local tmux server.
    /// This catches both attach-time `-o` toggling and tmux's expansion of the
    /// `%` tokens inside the hardened stat guards before an SSH host is needed.
    #[cfg(unix)]
    #[tokio::test]
    async fn remote_pipe_protocol_stays_armed_and_streams_multi_kib_output() {
        if Command::new("tmux").arg("-V").output().await.is_err() {
            return;
        }

        struct SessionCleanup(String);
        impl Drop for SessionCleanup {
            fn drop(&mut self) {
                let _ = std::process::Command::new("tmux")
                    .args(["kill-session", "-t", &self.0])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }

        let target = format!("agentum-remote-pipe-{}", Uuid::new_v4().simple());
        let cleanup = SessionCleanup(target.clone());
        let temp = tempfile::TempDir::new().unwrap();
        let log_leaf = format!("remote-pane-{}.log", Uuid::new_v4().simple());
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME for live tmux test"));
        let log = home.join(".agentum").join("panes").join(&log_leaf);
        agentum_tmux::new_session(&target, temp.path(), &["/bin/sh".into()], &[])
            .await
            .unwrap();
        sleep(Duration::from_millis(100)).await;

        // A tmux server retains the HOME it started with, so use that same
        // owner-only pane directory here (with a unique leaf) instead of
        // overriding HOME only for the client process.
        let script = remote_pipe_script(&target, Path::new(&log_leaf)).unwrap();
        let argv = shlex::split(&script).expect("generated pipe command");
        let mut states = Vec::new();
        for _ in 0..3 {
            let output = Command::new(&argv[0])
                .args(&argv[1..])
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "remote pipe script failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            sleep(Duration::from_millis(50)).await;
            let probe = Command::new("tmux")
                .args(["display-message", "-p", "-t"])
                .arg(&target)
                .arg("#{pane_pipe}")
                .output()
                .await
                .unwrap();
            states.push(String::from_utf8_lossy(&probe.stdout).trim().to_string());
        }

        agentum_tmux::send_bytes(&target, b"yes r | head -c 65536\r")
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let captured = loop {
            let length = tokio::fs::metadata(&log)
                .await
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if length >= 64 * 1024 || tokio::time::Instant::now() >= deadline {
                break length;
            }
            sleep(Duration::from_millis(20)).await;
        };

        drop(cleanup);
        let _ = std::fs::remove_file(&log);
        assert_eq!(states, ["1", "1", "1"], "a remote re-arm toggled off");
        assert!(
            captured >= 64 * 1024,
            "remote pane pipe captured only {captured} bytes"
        );
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
        assert!(script.contains("umask 077"), "tail umask is not private");
        assert!(script.contains("chmod 700"), "pane dir is not private");
        assert!(script.contains("chmod 600"), "pane log is not private");
        assert!(script.contains("stat -c %h") && script.contains("stat -f %l"));
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
    fn remote_input_script_uses_paced_hex_records() {
        let script = remote_input_script("agentum-demo").unwrap();
        assert!(script.starts_with("sh -c "), "not sh-wrapped: {script}");
        assert!(
            script.contains("while IFS= read -r l"),
            "no read loop: {script}"
        );
        assert!(
            script.contains("send-keys -H -t"),
            "input lost the desktop hex path: {script}"
        );
        assert!(
            script.contains("sleep 0.01"),
            "large records are not paced: {script}"
        );
        assert!(script.contains("agentum-demo"), "target missing: {script}");
        assert!(script.contains("exec sh -c"), "loop not exec'd: {script}");
        assert!(script.contains("list-sessions"), "exact lookup missing");
        assert!(script.contains("$sid"), "writer does not pin immutable ID");
    }

    #[test]
    fn encode_remote_input_lines_preserves_small_keys_on_the_fast_path() {
        assert_eq!(encode_remote_input_lines(b"hi"), b"68 69\n");
        assert_eq!(encode_remote_input_lines(b"\r"), b"0d\n");
        assert_eq!(
            encode_remote_input_lines(&[0x00, 0x1b, 0x5b, 0x41]),
            b"00 1b 5b 41\n"
        );
        assert_eq!(encode_remote_input_lines(b""), b"");
    }

    #[test]
    fn encode_remote_input_lines_splits_long_pastes_losslessly() {
        let chunk = REMOTE_INPUT_FRAME_BYTES;
        let paste: Vec<u8> = (0..chunk * 2 + 17)
            .map(|index| (index % 251) as u8)
            .collect();
        let encoded = encode_remote_input_lines(&paste);
        let lines: Vec<&[u8]> = encoded
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(lines.len(), 3, "expected three tmux-safe commands");

        let mut decoded = Vec::new();
        for line in lines {
            for word in line.split(|byte| *byte == b' ') {
                decoded.push(
                    u8::from_str_radix(std::str::from_utf8(word).unwrap(), 16)
                        .expect("valid hex byte"),
                );
            }
        }
        assert_eq!(decoded, paste);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_input_protocol_delivers_multi_kib_paste_through_tmux() {
        if Command::new("tmux").arg("-V").output().await.is_err() {
            return;
        }

        struct SessionCleanup(String);
        impl Drop for SessionCleanup {
            fn drop(&mut self) {
                let _ = std::process::Command::new("tmux")
                    .args(["kill-session", "-t", &self.0])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }

        let target = format!("agentum-hex-input-{}", Uuid::new_v4().simple());
        let cleanup = SessionCleanup(target.clone());
        let temp = tempfile::TempDir::new().unwrap();
        let received = temp.path().join("received.bin");
        let received_quoted = shlex::try_quote(received.to_str().unwrap()).unwrap();
        let pane_command = format!("stty raw -echo; exec cat > {received_quoted}");
        agentum_tmux::new_session(
            &target,
            temp.path(),
            &["sh".into(), "-c".into(), pane_command],
            &[],
        )
        .await
        .unwrap();
        sleep(Duration::from_millis(100)).await;

        let script = remote_input_script(&target).unwrap();
        let argv = shlex::split(&script).expect("generated writer command");
        let mut writer = Command::new(&argv[0]);
        writer
            .args(&argv[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut writer = writer.spawn().unwrap();
        let mut stdin = writer.stdin.take().unwrap();

        let payload: Vec<u8> = (0..512 * 8 + 137)
            .map(|index| b'a' + (index % 26) as u8)
            .collect();
        let frames = encode_remote_input_lines(&payload);
        stdin.write_all(&frames).await.unwrap();
        stdin.shutdown().await.unwrap();
        // The production writer is intentionally long-lived and is killed when
        // its WebSocket closes. Drop the pipe here so EOF may reap it, but test
        // the data plane rather than coupling delivery to child-exit timing.
        drop(stdin);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let delivered = loop {
            match tokio::fs::read(&received).await {
                Ok(bytes) if bytes.len() >= payload.len() => break bytes,
                _ if tokio::time::Instant::now() < deadline => {
                    sleep(Duration::from_millis(20)).await;
                }
                Ok(bytes) => {
                    let mismatch = bytes
                        .iter()
                        .zip(&payload)
                        .position(|(actual, expected)| actual != expected);
                    panic!(
                        "multi-kilobyte tmux input did not arrive: received {} of {} bytes; first mismatch: {mismatch:?}",
                        bytes.len(),
                        payload.len()
                    );
                }
                Err(error) => panic!("multi-kilobyte tmux input did not arrive: {error}"),
            }
        };
        assert_eq!(delivered, payload);
        if timeout(Duration::from_millis(500), writer.wait())
            .await
            .is_err()
        {
            writer.kill().await.unwrap();
        }
        drop(cleanup);
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
        let guard = script.find("#{pane_pipe}").expect("no pane_pipe guard");
        let pipe = script.find("pipe-pane").expect("no pipe-pane arm");
        // The guard also uses display-message; identify the cursor sample by
        // its format instead of accidentally selecting the guard probe.
        let cur = script.find("cursor_x").expect("no cursor sample");
        let cap = script.find("capture-pane").expect("no grid capture");
        let wc = script.find("wc -c").expect("no size probe");
        // pipe-pane is guarded and armed first (folded into this exec to save a
        // round trip), then cursor+capture, then size LAST.
        assert!(guard < pipe, "arm not guarded by pane_pipe probe: {script}");
        assert!(
            !script.contains("pipe-pane -o"),
            "toggling -o arm crept back in: {script}"
        );
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
        assert!(
            script.contains("$HOME/.agentum/panes/.snapshot.XXXXXX"),
            "snapshot temp must be exclusive and private: {script}"
        );
        assert!(
            !script.contains("/tmp/agentum-snap"),
            "predictable shared-/tmp snapshot is unsafe: {script}"
        );
        assert!(script.contains("sess-4.log"), "log missing: {script}");
        assert!(script.contains("list-sessions"), "exact lookup missing");
        assert!(
            script.contains("$sid"),
            "snapshot does not use immutable ID"
        );
        assert!(script.contains("stat -c %h") && script.contains("stat -f %l"));
        assert!(!script.contains("touch "), "snapshot must not use touch");
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

    #[test]
    fn which_first_script_probes_candidates_in_order() {
        let s = which_first_script(&["chromium", "chromium-browser", "google-chrome"]);
        assert!(
            s.contains("for b in chromium chromium-browser google-chrome"),
            "candidates not probed in order: {s}"
        );
        assert!(s.contains("command -v"), "not a portable PATH probe: {s}");
        assert!(s.contains("printf"), "first hit not printed: {s}");
    }

    #[test]
    fn claude_transcript_script_is_home_anchored_and_shell_quoted() {
        let script = claude_transcript_read_script(
            ".claude/projects/-srv-project/00000000-0000-0000-0000-000000000001.jsonl",
        )
        .unwrap();
        assert!(script.starts_with("path=\"$HOME\"/"));
        assert!(
            script.contains("exit 44"),
            "missing must have a distinct status"
        );
        assert!(script.ends_with("cat \"$path\""));

        let quoted = claude_transcript_read_script(".claude/projects/a'b/session.jsonl").unwrap();
        assert!(
            quoted.contains("\".claude/projects/a'b/session.jsonl\""),
            "path was not kept inside one quoted shell word: {quoted}"
        );
    }

    #[tokio::test]
    async fn which_first_finds_present_binary_locally() {
        let host = local_host();
        // `sh` is on PATH everywhere; the bogus name precedes it to prove order.
        assert_eq!(
            which_first(&host, &["definitely-not-a-real-bin-xyz", "sh"])
                .await
                .unwrap()
                .as_deref(),
            Some("sh")
        );
        assert!(
            which_first(&host, &["definitely-not-a-real-bin-xyz"])
                .await
                .unwrap()
                .is_none()
        );
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
    // regression can't silently move the scan or mis-read ssh.

    #[test]
    fn cdp_forward_range_is_24_ports_at_9200() {
        let ports: Vec<u16> = forward_tunnel_ports().collect();
        assert_eq!(REMOTE_CDP_PORT_BASE, 9200, "CDP base moved");
        assert_eq!(ports.len(), 24, "must scan 24 candidate Mac ports");
        assert_eq!(
            ports[0], REMOTE_CDP_PORT_BASE,
            "scan must start at the base"
        );
        assert_eq!(*ports.last().unwrap(), REMOTE_CDP_PORT_BASE + 23);
    }

    #[test]
    fn tunnel_arm_bound_treats_success_and_already_established_as_bound() {
        // A clean `-O forward` exit means the tunnel bound.
        assert!(tunnel_arm_bound(true, ""));
        // ssh reporting an exact forward as already established is an
        // idempotent success, not a failure.
        assert!(tunnel_arm_bound(false, "forwarding already in place"));
        assert!(
            tunnel_arm_bound(false, "remote forward ALREADY EXISTS"),
            "must be case-insensitive"
        );
        // A generic forwarding failure means this port is unusable → scan the next.
        assert!(!tunnel_arm_bound(
            false,
            "could not request local forwarding"
        ));
        assert!(
            !tunnel_arm_bound(false, "bind [127.0.0.1]:9200: Address already in use"),
            "a busy local port must be scanned past"
        );
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
