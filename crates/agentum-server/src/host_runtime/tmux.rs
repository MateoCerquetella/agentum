//! Tmux session lifecycle, pane capture/input, and remote pane streaming.
use super::*;

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
            // No `--`: `-t` consumes its own (shell-quoted) argument, getopt-safe
            // even if the target starts with `-`; a `--` would become the target.
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
/// This ALSO arms `pipe-pane` first — folding what used to be a separate
/// connect-time round-trip into this one. On a distant host each SSH exec is
/// ~450 ms even over the warm master, so doing arm-then-capture as two
/// sequential calls cost ~900 ms of blank screen before the first paint; one
/// combined exec halves that. The arm is guarded by a `#{pane_pipe}` probe
/// because `pipe-pane -o` TOGGLES (it is not the no-op-when-live the old
/// comment claimed — issue #270): a blind re-arm here disarmed the live
/// stream on every other connect. A pipe-pane failure is swallowed (the
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
pub(crate) fn snapshot_with_offset_script(target: &str, out_path: &Path) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    let pipe = q(&format!("cat >> {log}"))?.into_owned();
    let inner = format!(
        "mkdir -p \"{REMOTE_PANE_DIR}\" 2>/dev/null; \
         [ \"$(tmux display-message -p -t {t} '#{{pane_pipe}}' 2>/dev/null)\" = 1 ] || tmux pipe-pane -t {t} {pipe} 2>/dev/null; \
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

/// Set a session-scoped tmux environment variable on `target`'s pane.
/// Local → `agentum_tmux::set_environment`; SSH → `tmux set-environment` over
/// the pooled connection with every component shell-quoted.
///
/// Best-effort by contract: callers log-and-continue on error. tmux's session
/// environment is only read by *future* children of the pane — the already-running
/// agent does not see the change, so this never repairs a live process.
pub async fn set_pane_env(host: &Host, target: &str, key: &str, value: &str) -> Result<()> {
    match &host.kind {
        HostKind::Local => Ok(agentum_tmux::set_environment(target, key, value).await?),
        HostKind::Ssh { .. } => {
            let script = format!(
                "tmux set-environment -t {} {} {}",
                q(target)?,
                q(key)?,
                q(value)?
            );
            ssh_checked(host, &script).await
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
            for chunk in bytes.chunks(agentum_tmux::SEND_KEYS_HEX_CHUNK_BYTES) {
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
            let target = q(target)?;
            let script = format!(
                "tmux set-option -q -t {target} window-size manual; tmux resize-window -t {target} {flag} {count}"
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
            // terminal lag and flicker. Re-arming is made idempotent by a
            // `#{pane_pipe}` probe inside the script — NOT by `-o`, which
            // toggles the pipe off when one is live (issue #270).
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
pub(crate) fn remote_pane_log_expr(out_path: &Path) -> Result<String> {
    let name = out_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(HostRuntimeError::Quote)?;
    Ok(format!("\"{REMOTE_PANE_DIR}/{name}\""))
}

/// Build the `sh -c …` script that arms tmux pipe-pane on the remote, writing
/// raw pane output to the per-session log. Factored out so the (untestable
/// without a live host) quoting is at least covered by a string-shape unit test.
pub(crate) fn remote_pipe_script(target: &str, out_path: &Path) -> Result<String> {
    let log = remote_pane_log_expr(out_path)?;
    // tmux runs this command via `/bin/sh -c` on every flush; single-quoting it
    // keeps `$HOME` unexpanded through the outer shells so it resolves there.
    let pipe = format!("cat >> {log}");
    // Guard on `#{pane_pipe}` instead of trusting `pipe-pane -o`: `-o` toggles
    // a live pipe off (issue #270), so re-running this script against an armed
    // pane must be a true no-op. Arming uses plain `pipe-pane` so a lost race
    // still ends armed.
    let inner = format!(
        "mkdir -p \"{REMOTE_PANE_DIR}\" && [ \"$(tmux display-message -p -t {t} '#{{pane_pipe}}' 2>/dev/null)\" = 1 ] || tmux pipe-pane -t {t} {pipe}",
        t = q(target)?,
        pipe = q(&pipe)?
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
pub(crate) fn remote_tail_script(out_path: &Path, from_offset: Option<u64>) -> Result<String> {
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
///
/// `mux` selects the connection:
///   * [`SshMux::Streaming`] (the default connect path) — the shared `cms-`
///     master. One connection per host no matter how many sessions, so opening
///     the app can't storm sshd's `MaxStartups`; tails stay off the interactive
///     master's `MaxSessions` budget so keystrokes/execs aren't starved.
///   * [`SshMux::Off`] — a fresh, unmultiplexed connection. The reconnect
///     ([`reestablish_tail`]) escalates to this when the streaming master is
///     wedged or channel-saturated, so a dead pooled master can't permanently
///     freeze a session's output (it can't self-heal through the pool).
pub fn spawn_remote_pane_tail(
    host: &Host,
    out_path: &Path,
    from_offset: Option<u64>,
    mux: SshMux,
) -> Result<tokio::process::Child> {
    let script = remote_tail_script(out_path, from_offset)?;
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

/// Build the `sh -c …` script behind [`spawn_remote_input_writer`]: a remote
/// read-loop that turns each newline-terminated line of space-separated hex on
/// stdin into one `tmux send-keys -H` for the pane. `$line` is intentionally
/// unquoted so the hex pairs split into separate `send-keys` args. Errors per
/// line (e.g. the pane briefly gone) are swallowed so one bad write never ends
/// the loop. `exec` lets a kill of the ssh child reap it cleanly.
pub(crate) fn remote_input_script(target: &str) -> Result<String> {
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

/// Encode raw keystroke bytes as space-separated lowercase-hex lines
/// (newline-terminated) for the [`spawn_remote_input_writer`] remote loop,
/// split at [`agentum_tmux::SEND_KEYS_HEX_CHUNK_BYTES`] of input per line.
///
/// The split is load-bearing: each line becomes ONE remote `tmux send-keys`,
/// and tmux rejects an oversized marshalled command before it reaches the pane
/// — an error the remote loop deliberately swallows. Every byte is a separate
/// argv entry, so the safe input bound is substantially smaller than the
/// rendered hex text suggests. Use the same conservative bound as local
/// [`send_bytes`].
pub fn encode_input_hex_lines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 3 + 1);
    for chunk in bytes.chunks(agentum_tmux::SEND_KEYS_HEX_CHUNK_BYTES) {
        for (i, b) in chunk.iter().enumerate() {
            if i > 0 {
                out.push(b' ');
            }
            out.extend_from_slice(format!("{b:02x}").as_bytes());
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
            ssh_checked(host, &format!("tmux pipe-pane -t {}", q(target)?)).await
        }
    }
}
