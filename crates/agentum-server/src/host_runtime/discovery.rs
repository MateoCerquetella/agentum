//! External (non-agentum) tmux session discovery and parsing.
use std::collections::HashSet;

use agentum_core::Host;

use super::*;

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
pub(crate) fn parse_tmux_panes_all(stdout: &str) -> Vec<DiscoveredTmuxSession> {
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
            // No `--`: `-t` consumes its own (shell-quoted) argument, getopt-safe.
            let script = format!("tmux kill-session -t {}", q(name)?);
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
pub(crate) fn parse_tmux_panes(stdout: &str) -> Vec<DiscoveredTmuxSession> {
    parse_tmux_panes_filtered(stdout, false)
}

/// Parse [`TMUX_DISCOVER_FORMAT`] pane lines into the agentum-MANAGED
/// (`agentum-*`) sessions — the zombie-sweep view.
pub(crate) fn parse_tmux_panes_managed(stdout: &str) -> Vec<DiscoveredTmuxSession> {
    parse_tmux_panes_filtered(stdout, true)
}

/// Parse [`TMUX_DISCOVER_FORMAT`] pane lines into sessions, preserving tmux's
/// order. `managed = false` keeps only EXTERNAL sessions (discovery); `managed =
/// true` keeps only agentum-MANAGED (`agentum-*`) ones (zombie sweep). Tolerant
/// of trailing `\r` and malformed lines.
pub(crate) fn parse_tmux_panes_filtered(stdout: &str, managed: bool) -> Vec<DiscoveredTmuxSession> {
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
