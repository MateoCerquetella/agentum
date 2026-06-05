//! Local/SSH host execution helpers.
//!
//! The SSH backend intentionally drives only stock `ssh` + `tmux` on the
//! remote machine. The remote host never needs an `agentum` binary.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use agentum_core::{AgentDepCheck, DepCheck, Host, HostKind, HostReadiness, HostSystemInfo};
use agentum_executor::{binary_for, probed_tools};
// The SSH connection builder lives in the shared lower crate so the watchdog
// (which can't depend on agentum-server) shares one source of truth for the
// ssh/sshpass flags. Every `ssh_command(host, …)` call site below is unchanged.
use agentum_tmux::ssh::ssh_command;
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
    let mut report = match &host.kind {
        HostKind::Local => assemble_readiness(probe_local()),
        HostKind::Ssh { .. } => match probe_ssh(host).await {
            Ok(probe) => assemble_readiness(probe),
            // Connection / auth / timeout failure: surface the error
            // verbatim and report everything as missing so the UI shows
            // the full (unverifiable) dependency list rather than a bare
            // error with no guidance.
            Err(e) => unreachable_readiness(e.to_string()),
        },
    };
    crate::host_install_hints::fill_hints(&mut report);
    report
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
    let output = timeout(dur, ssh_command(host, script).output())
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
                &format!("tmux display-message -p -t {} '#{{pane_title}}'", q(target)?),
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
            let out = timeout(GIT_TIMEOUT, ssh_command(host, &script).output())
                .await
                .map_err(|_| HostRuntimeError::Timeout)??;
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
            let out = timeout(GIT_TIMEOUT, ssh_command(host, &script).output())
                .await
                .map_err(|_| HostRuntimeError::Timeout)??;
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
            let out = timeout(SSH_TIMEOUT, ssh_command(host, &script).output())
                .await
                .map_err(|_| HostRuntimeError::Timeout)??;
            Ok(out.status.success())
        }
    }
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
}
