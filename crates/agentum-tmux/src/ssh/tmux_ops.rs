//! tmux session/pane operations executed over SSH (the remote counterpart of
//! the local tmux wrapper): session existence, pane capture, send-keys, sampling.
use super::*;

/// `tmux has-session` on `host`. Non-zero remote exit means "no such session".
pub async fn has_session(host: &Host, target: &str) -> Result<bool> {
    match &host.kind {
        HostKind::Local => crate::has_session(target).await,
        HostKind::Ssh { .. } => {
            let script = format!("tmux has-session -t {}", q(&format!("={target}"))?);
            Ok(ssh_status(host, &script).await?.success())
        }
    }
}

/// Capture the last `lines` of `target`'s pane (incl. scrollback) as plain
/// text on `host`.
pub async fn capture_pane(host: &Host, target: &str, lines: usize) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::capture_pane(target, lines).await,
        HostKind::Ssh { .. } => {
            ssh_stdout(
                host,
                &format!("tmux capture-pane -p -S -{lines} -t {}", q(target)?),
            )
            .await
        }
    }
}

/// Capture only the currently-visible viewport of `target` (no scrollback)
/// as plain text on `host`.
pub async fn capture_pane_visible(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::capture_pane_visible(target).await,
        HostKind::Ssh { .. } => {
            ssh_stdout(
                host,
                &format!("tmux capture-pane -p -S 0 -t {}", q(target)?),
            )
            .await
        }
    }
}

/// Send `keys` (a tmux key spec or text) to `target` on `host`, optionally
/// appending Enter.
pub async fn send_keys(host: &Host, target: &str, keys: &str, append_enter: bool) -> Result<()> {
    match &host.kind {
        HostKind::Local => crate::send_keys(target, keys, append_enter).await,
        HostKind::Ssh { .. } => {
            let mut script = format!("tmux send-keys -t {} {}", q(target)?, q(keys)?);
            if append_enter {
                script.push_str(" Enter");
            }
            ssh_checked(host, &script).await
        }
    }
}

/// Everything the watchdog needs about a pane, gathered in one round trip.
/// See [`sample_pane`].
#[derive(Debug)]
pub struct PaneSample {
    /// Last N lines including scrollback (crash + context-low matching).
    pub pane: String,
    /// Currently-visible viewport only (activity classification).
    pub viewport: String,
    /// Foreground process basename (`#{pane_current_command}`), trimmed.
    pub current_command: String,
}

/// Boundary line separating the three sections of [`sample_pane`]'s combined
/// remote output. High-entropy so rendered pane text essentially can't
/// collide with it; a collision parses as a section-count mismatch and
/// surfaces as an `Err` (one skipped watchdog tick), never as wrong data
/// silently attributed to the wrong section.
const SAMPLE_BOUNDARY: &str = ":::agentum-pane-sample-7f3a9c:::";

/// Exit code the sample script uses for "session is gone" — distinguishable
/// from ssh's own 255 (transport) and tmux's 1 (generic error).
const SAMPLE_GONE_EXIT: i32 = 43;

/// One watchdog sample of `target` on `host`: session existence, a
/// scrollback capture (`lines` deep), the visible viewport, and the
/// foreground command. Returns `Ok(None)` when the session no longer exists.
///
/// On SSH hosts this is ONE remote exec instead of the four the watchdog
/// previously issued per tick (`has-session` + two `capture-pane`s +
/// `display-message`). At a 1 s tick with several remote sessions open, those
/// per-call channel open/closes were the dominant load on the shared
/// ControlMaster — the same master that carries interactive keystrokes — so
/// batching directly reduces input latency, not just probe overhead.
/// Local hosts keep the four direct tmux calls (process spawns are cheap and
/// there is no channel contention to relieve).
pub async fn sample_pane(host: &Host, target: &str, lines: usize) -> Result<Option<PaneSample>> {
    match &host.kind {
        HostKind::Local => {
            if !crate::has_session(target).await? {
                return Ok(None);
            }
            Ok(Some(PaneSample {
                pane: crate::capture_pane(target, lines).await?,
                viewport: crate::capture_pane_visible(target).await?,
                current_command: crate::pane_current_command(target).await?,
            }))
        }
        HostKind::Ssh { .. } => {
            let exact_target = format!("={target}");
            let exact = q(&exact_target)?;
            let t = q(target)?;
            // `2>/dev/null` on the captures: if the session dies between the
            // has-session gate and a capture, the malformed output parses as
            // a boundary mismatch (skipped tick) and the next tick exits 43.
            let inner = format!(
                "tmux has-session -t {exact} 2>/dev/null || exit {SAMPLE_GONE_EXIT}\n\
                 tmux display-message -p -t {t} '#{{pane_current_command}}' 2>/dev/null\n\
                 echo {SAMPLE_BOUNDARY}\n\
                 tmux capture-pane -p -S -{lines} -t {t} 2>/dev/null\n\
                 echo {SAMPLE_BOUNDARY}\n\
                 tmux capture-pane -p -S 0 -t {t} 2>/dev/null"
            );
            let script = format!("sh -c {}", q(&inner)?);
            // Ride the STREAMING master, not the interactive one: this exec fires
            // every watchdog tick per session, and on the interactive master it
            // starved keystrokes (and, via remote tmux load, pane throughput).
            let output = ssh_output_on(host, &script, SSH_TIMEOUT, SshMux::Streaming)
                .await
                .map_err(TmuxError::Io)?;
            if output.status.code() == Some(SAMPLE_GONE_EXIT) {
                return Ok(None);
            }
            if !output.status.success() {
                return Err(TmuxError::NonZero {
                    status: output.status.code().unwrap_or(-1),
                    stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                });
            }
            let stdout = String::from_utf8(output.stdout)?;
            parse_pane_sample(&stdout)
                .map(Some)
                .ok_or_else(|| TmuxError::NonZero {
                    status: 0,
                    stderr: "pane sample output did not contain the expected sections".to_string(),
                })
        }
    }
}

/// Split the combined sample stdout into its three sections. `None` when the
/// boundary count is off (remote race or a pathological pane collision).
fn parse_pane_sample(stdout: &str) -> Option<PaneSample> {
    let sep = format!("\n{SAMPLE_BOUNDARY}\n");
    let mut parts = stdout.splitn(3, &sep);
    let current_command = parts.next()?.trim().to_string();
    let pane = parts.next()?.to_string();
    let viewport = parts.next()?.to_string();
    Some(PaneSample {
        pane,
        viewport,
        current_command,
    })
}

/// Basename of the foreground process inside `target`'s pane
/// (`#{pane_current_command}`) on `host`, trimmed.
pub async fn pane_current_command(host: &Host, target: &str) -> Result<String> {
    match &host.kind {
        HostKind::Local => crate::pane_current_command(target).await,
        HostKind::Ssh { .. } => {
            let out = ssh_stdout(
                host,
                &format!(
                    "tmux display-message -p -t {} '#{{pane_current_command}}'",
                    q(target)?
                ),
            )
            .await?;
            Ok(out.trim().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_sample_parses_three_sections() {
        let stdout = format!(
            "claude\n{SAMPLE_BOUNDARY}\nline one\nline two\n{SAMPLE_BOUNDARY}\nviewport line\n"
        );
        let s = parse_pane_sample(&stdout).expect("well-formed sample");
        assert_eq!(s.current_command, "claude");
        // The pane section's trailing newline is consumed by the boundary
        // separator; the watchdog only does substring matching, so that's
        // contractually fine.
        assert_eq!(s.pane, "line one\nline two");
        assert_eq!(s.viewport, "viewport line\n");
    }

    #[test]
    fn pane_sample_rejects_missing_boundary() {
        // A capture race (session died mid-script) yields truncated output —
        // that must surface as a parse failure (skipped tick), never as
        // sections silently mis-attributed.
        assert!(parse_pane_sample("claude\nonly one section\n").is_none());
    }
}
