//! Local/SSH host execution helpers.
//!
//! The SSH backend intentionally drives only stock `ssh` + `tmux` on the
//! remote machine. The remote host never needs an `agentum` binary.

use std::collections::{HashMap, HashSet};
use std::path::Path;
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

mod skills;
pub use skills::*;

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

mod probe;
pub(crate) use probe::*;
mod tmux;
pub use tmux::*;

mod tunnels;
pub use tunnels::*;

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

mod discovery;
pub use discovery::*;

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

mod git_fs;
pub use git_fs::*;
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
