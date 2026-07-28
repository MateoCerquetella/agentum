//! tmux subprocess adapter.
//!
//! Every method shells out to `tmux` via `tokio::process::Command` with one
//! `.arg()` per argument — no shell-string interpolation in our process
//! invocation. The single shell-command string we pass to
//! `tmux new-session` / `tmux pipe-pane` is safely quoted with [`shlex`].

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::time::sleep;

/// Host-aware tmux ops (Local or SSH) + the shared SSH connection builder.
pub mod ssh;

#[derive(Debug, thiserror::Error)]
pub enum TmuxError {
    #[error("tmux exited with status {status:?} (stderr: {stderr})")]
    NonZero { status: i32, stderr: String },
    #[error("tmux output was not valid utf-8")]
    NotUtf8(#[from] std::string::FromUtf8Error),
    #[error("could not parse tmux output: {0}")]
    Parse(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("could not shell-quote command components")]
    Quote,
}

pub type Result<T> = std::result::Result<T, TmuxError>;

/// Returns the tmux session name for an agentum session.
pub fn target_for(name: &str) -> String {
    format!("agentum-{name}")
}

/// `tmux has-session -t <target>` → bool. Non-zero exit means "no such session".
pub async fn has_session(target: &str) -> Result<bool> {
    let status = Command::new("tmux")
        .arg("has-session")
        .arg("-t")
        .arg(format!("={target}")) // exact-match to avoid prefix collisions
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;
    Ok(status.success())
}

/// Spawn a detached tmux session running `cmd` (as argv) in `workdir`.
///
/// `env` entries are forwarded as `-e KEY=VAL` to tmux so the spawned shell
/// inherits them. Workdir must exist on disk.
/// Initial tmux pane size for newly-created sessions. Without explicit
/// `-x/-y` flags, `tmux new-session -d` clamps to its default 80×24,
/// which means embedded TUIs (claude code, codex, opencode) launch and
/// render their first frame at 80 cols. When a wider client later
/// connects we tell tmux to `resize-window` and the embedded process
/// gets SIGWINCH — but ratatui-based agents don't always reflow stale
/// chat history past their viewport, so the user sees text wrapped at
/// 80 cols stranded inside a much wider visible pane. Pre-sizing to a
/// roomy default (132×40 — fits a 13" laptop in landscape and is the
/// classic VT220 wide mode) means the very first rendered frame uses a
/// width any modern client can comfortably display.
pub const DEFAULT_PANE_COLS: u16 = 132;
pub const DEFAULT_PANE_ROWS: u16 = 40;

/// Maximum raw-input bytes carried by one `tmux send-keys -H` invocation.
///
/// This is bounded by tmux's own client-to-server command message, not the OS
/// `ARG_MAX`. Each byte becomes a separate two-character argv entry plus tmux's
/// per-argument framing; on tmux 3.7b a 1,000-byte batch already fails with
/// `command too long`. Keep a conservative shared bound for local sends and the
/// server's SSH encoders so large agent prompts/pastes are delivered losslessly.
pub const SEND_KEYS_HEX_CHUNK_BYTES: usize = 512;

pub async fn new_session(
    target: &str,
    workdir: &Path,
    cmd: &[String],
    env: &[(String, String)],
) -> Result<()> {
    let cmd_str = shlex::try_join(cmd.iter().map(String::as_str)).map_err(|_| TmuxError::Quote)?;

    let mut c = Command::new("tmux");
    c.arg("new-session")
        .arg("-d")
        .arg("-s")
        .arg(target)
        .arg("-x")
        .arg(DEFAULT_PANE_COLS.to_string())
        .arg("-y")
        .arg(DEFAULT_PANE_ROWS.to_string())
        .arg("-c")
        .arg(workdir);
    for (k, v) in env {
        c.arg("-e").arg(format!("{k}={v}"));
    }
    c.arg(cmd_str);

    run_checked(&mut c).await
}

/// `tmux kill-session -t <target>`. Idempotent — non-existent target returns Ok.
pub async fn kill_session(target: &str) -> Result<()> {
    if !has_session(target).await? {
        return Ok(());
    }
    let mut c = Command::new("tmux");
    // No `--` here: `-t` already consumes its own argument (getopt-safe even if
    // the target starts with `-`); a `--` would be taken AS the target name and
    // silently no-op the kill.
    c.arg("kill-session").arg("-t").arg(target);
    run_checked(&mut c).await
}

/// Capture last `lines` of pane content as plain text (no ANSI escapes —
/// suitable for regex matching by the watchdog).
pub async fn capture_pane(target: &str, lines: usize) -> Result<String> {
    let start = format!("-{lines}");
    let out = Command::new("tmux")
        .args(["capture-pane", "-p", "-S", &start, "-t"])
        .arg(target)
        .output()
        .await?;
    check(&out)?;
    Ok(String::from_utf8(out.stdout)?)
}

/// Capture only the currently-visible viewport (no scrollback) as plain
/// text. Critical for the watchdog's activity classification: Claude's
/// "esc to interrupt" footer lingers in scrollback after a turn ends,
/// so a scrollback-inclusive capture matches the busy signature forever
/// and the dot stays a misleading "live" green long after the agent
/// went idle. `-S 0` pins the start to the top of the visible pane so
/// only what's currently on-screen counts.
pub async fn capture_pane_visible(target: &str) -> Result<String> {
    let out = Command::new("tmux")
        .args(["capture-pane", "-p", "-S", "0", "-t"])
        .arg(target)
        .output()
        .await?;
    check(&out)?;
    Ok(String::from_utf8(out.stdout)?)
}

/// The watchdog's three per-tick reads — foreground command, `lines`-deep
/// scrollback, and the visible viewport — pulled by ONE tmux client as a
/// `;`-separated command sequence, with `boundary` printed between sections so
/// the caller can split them apart (see `ssh::parse_pane_sample`).
///
/// Why: the watchdog samples every running session on a 1 s tick. Issued as
/// separate `pane_current_command` + `capture_pane` + `capture_pane_visible`
/// calls, that is three `tmux` client fork/exec + server-socket round trips per
/// session per second — continuous, N-scaling CPU for data that fits in one
/// client invocation. tmux runs the whole sequence against the server in a
/// single connection, so this cuts the spawn count without changing the data.
/// Session existence is folded into this same invocation: targets use tmux's
/// exact-match form (`=name`, no prefix collisions), so a vanished session
/// fails every command with a stderr [`tmux_stderr_means_target_gone`]
/// classifies — callers need no separate `has-session` fork/exec pre-gate.
///
/// The `;` tokens are passed as standalone argv elements (no shell in the
/// loop), which is how tmux recognises a command separator when exec'd directly.
pub async fn capture_pane_sample_combined(
    target: &str,
    lines: usize,
    boundary: &str,
) -> Result<String> {
    let start = format!("-{lines}");
    // `=name` alone only parses as an exact match for target-SESSION commands;
    // these take a target-pane, where a bare `=name` is tried as a pane and
    // fails ("can't find pane"). The trailing `:` marks the session part, so
    // `=name:` = exact session, its active window/pane — verified on tmux 3.6.
    let exact = format!("={target}:");
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(&exact)
        .arg("#{pane_current_command}")
        .arg(";")
        .args(["display-message", "-p"])
        .arg(boundary)
        .arg(";")
        .args(["capture-pane", "-p", "-S", &start, "-t"])
        .arg(&exact)
        .arg(";")
        .args(["display-message", "-p"])
        .arg(boundary)
        .arg(";")
        .args(["capture-pane", "-p", "-S", "0", "-t"])
        .arg(&exact)
        .output()
        .await?;
    check(&out)?;
    Ok(String::from_utf8(out.stdout)?)
}

/// tmux format string yielding a [`CursorSample`] line — must be sampled in
/// the same tmux command sequence (or remote shell) as the capture it anchors.
pub const CURSOR_SAMPLE_FORMAT: &str = "#{cursor_x} #{cursor_y} #{cursor_flag}";

/// Pane cursor state sampled atomically with a `capture-pane` snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSample {
    /// 0-based column within the visible pane.
    pub x: u32,
    /// 0-based row within the visible pane.
    pub y: u32,
    /// tmux `cursor_flag` — false when the program hid the cursor (DECTCEM).
    pub visible: bool,
}

/// Parse one line of [`CURSOR_SAMPLE_FORMAT`] output. `None` (rather than a
/// 0,0 guess) when it doesn't parse: anchoring to a wrong position is worse
/// than the legacy no-anchor behavior, because erase-up redraw cycles would
/// then chew through the top of the snapshot.
pub fn parse_cursor_sample(line: &str) -> Option<CursorSample> {
    let mut it = line.split_whitespace();
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    let visible = it.next().is_none_or(|f| f != "0");
    Some(CursorSample { x, y, visible })
}

/// Turn raw `capture-pane -p -e` stdout (LF-separated rows) into bytes safe
/// to replay into a freshly-reset client parser, anchored so the live byte
/// stream that follows renders where the pane really drew it.
///
/// Two invariants:
///
/// 1. Rows are *separated* by CRLF, never terminated. capture-pane emits
///    every visible row (trailing blanks included), so a terminator on a
///    full-height pane scrolls the client viewport one row — desyncing all
///    painted content from the absolute coordinates in the live stream.
/// 2. The pane's cursor position is restored with an absolute CUP. Agent
///    TUIs (Claude Code, Cursor's composer) redraw in place via
///    cursor-relative erase/rewrite cycles anchored on wherever the previous
///    frame left the cursor. Painting the grid leaves the client cursor
///    below the last row instead, so without this anchor every replayed
///    frame lands rows too low and stale spinner lines pile up (the
///    "Composing… Composing…" corruption).
///
/// A hidden cursor is re-hidden: the RIS that precedes the snapshot made it
/// visible, and a TUI that hid it at startup never repeats the hide.
pub fn assemble_anchored_snapshot(grid: &[u8], cursor: Option<CursorSample>) -> Vec<u8> {
    // capture-pane terminates its last row with `\n`; drop exactly that one
    // so blank trailing rows (empty splits) still paint.
    let grid = grid.strip_suffix(b"\n").unwrap_or(grid);
    if grid.is_empty() {
        return Vec::new();
    }
    let mut buf = Vec::with_capacity(grid.len() + 32);
    for (i, line) in grid.split(|b| *b == b'\n').enumerate() {
        if i > 0 {
            buf.extend_from_slice(b"\r\n");
        }
        buf.extend_from_slice(line);
    }
    if let Some(c) = cursor {
        // CUP is 1-based; the tmux formats are 0-based.
        buf.extend_from_slice(format!("\x1b[{};{}H", c.y + 1, c.x + 1).as_bytes());
        if !c.visible {
            buf.extend_from_slice(b"\x1b[?25l");
        }
    }
    buf
}

/// Capture the current visible pane state with ANSI escapes (`-e`), plus the
/// pane's cursor sampled in the same tmux command sequence, assembled for
/// replay into a fresh client parser — see [`assemble_anchored_snapshot`].
pub async fn capture_pane_ansi(target: &str) -> Result<Vec<u8>> {
    // One tmux invocation, two commands (`;` separates command sequences at
    // the argv level): the cursor line and the grid come from the same server
    // pass, so the anchor can't drift from the content it anchors.
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(target)
        .arg(CURSOR_SAMPLE_FORMAT)
        .arg(";")
        .args(["capture-pane", "-p", "-e", "-t"])
        .arg(target)
        .output()
        .await?;
    check(&out)?;
    let (first, rest) = match out.stdout.iter().position(|b| *b == b'\n') {
        Some(i) => (&out.stdout[..i], &out.stdout[i + 1..]),
        None => (&out.stdout[..], &[][..]),
    };
    let cursor = parse_cursor_sample(&String::from_utf8_lossy(first));
    Ok(assemble_anchored_snapshot(rest, cursor))
}

/// Read the pane's current title (`#{pane_title}`). tmux captures the program's
/// OSC 0/2 title sequences into this property and — with `set-titles off` —
/// never forwards the raw sequences to an attached client. Agent CLIs announce
/// working/idle/permission in that title, so the session stream re-injects this
/// value as a synthetic `\x1b]0;…\x07` so the desktop's title-derived
/// agent-status pipeline can follow the state. Returns the trimmed title.
pub async fn pane_title(target: &str) -> Result<String> {
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(target)
        .arg("#{pane_title}")
        .output()
        .await?;
    check(&out)?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim_matches(|c| c == '\n' || c == '\r')
        .to_string())
}

/// Send raw key spec (e.g. "C-c", "Enter") or text to a pane.
/// `append_enter` adds a trailing Enter, useful for chat-style input bars.
pub async fn send_keys(target: &str, keys: &str, append_enter: bool) -> Result<()> {
    let mut c = Command::new("tmux");
    c.arg("send-keys").arg("-t").arg(target).arg(keys);
    if append_enter {
        c.arg("Enter");
    }
    run_checked(&mut c).await
}

/// Send raw bytes verbatim to a pane via `tmux send-keys -H` (hex pairs).
/// This bypasses tmux's key-name parsing — every byte is delivered literally,
/// including control chars and escape sequences. Used by the interactive WS
/// terminal so xterm.js keystrokes round-trip into the running pty.
///
/// Splits into chunks to stay under tmux's command-message limit when a paste
/// is huge.
pub async fn send_bytes(target: &str, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    for chunk in bytes.chunks(SEND_KEYS_HEX_CHUNK_BYTES) {
        let mut c = Command::new("tmux");
        c.arg("send-keys").arg("-H").arg("-t").arg(target);
        for b in chunk {
            c.arg(format!("{b:02x}"));
        }
        run_checked(&mut c).await?;
    }
    Ok(())
}

/// Resize the tmux window (and therefore its single pane) so the running
/// process redraws into `cols × rows`. Required when no client is attached
/// — without an attached client tmux clamps the size to the default 80×24,
/// which is why embedded TUIs render at the wrong width and overflow when
/// the agentum TUI / web dashboard pane is bigger than that.
///
/// Tmux ≥ 3.0 honours `resize-window` for unattached sessions when the
/// `window-size` option is `manual`. We force that mode on the first call
/// (idempotent) so the resize sticks.
pub async fn resize_window(target: &str, cols: u16, rows: u16) -> Result<()> {
    let cols = cols.max(20);
    let rows = rows.max(5);

    // window-size manual: tmux stops auto-fitting to attached clients and
    // honours our explicit size. -q suppresses "no current session" noise.
    let _ = Command::new("tmux")
        .args(["set-option", "-q", "-t"])
        .arg(target)
        .args(["window-size", "manual"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    let mut c = Command::new("tmux");
    c.arg("resize-window")
        .arg("-t")
        .arg(target)
        .arg("-x")
        .arg(cols.to_string())
        .arg("-y")
        .arg(rows.to_string());
    run_checked(&mut c).await
}

/// Adjust the window height by `rows_delta` rows *relative* to its current
/// size (positive = taller via `-U`, negative = shorter via `-D`). Unlike
/// [`resize_window`], the caller needn't know the absolute size — used by the
/// redraw heal to provoke a SIGWINCH (shrink then restore) when only a row
/// delta, not the current geometry, is on hand. Forces `window-size manual`
/// first for the same unattached-session reason as [`resize_window`].
pub async fn resize_window_relative(target: &str, rows_delta: i16) -> Result<()> {
    if rows_delta == 0 {
        return Ok(());
    }
    let _ = Command::new("tmux")
        .args(["set-option", "-q", "-t"])
        .arg(target)
        .args(["window-size", "manual"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    // `-U`/`-D` take a positive count; map the signed delta onto the flag.
    let flag = if rows_delta > 0 { "-U" } else { "-D" };
    let count = rows_delta.unsigned_abs().to_string();
    let mut c = Command::new("tmux");
    c.arg("resize-window")
        .arg("-t")
        .arg(target)
        .arg(flag)
        .arg(count);
    run_checked(&mut c).await
}

/// Set a session-scoped environment variable on a tmux target via
/// `tmux set-environment -t <target> KEY VAL`. Each argument is passed
/// individually (no shell interpolation), so KEY/VAL with spaces or special
/// chars are delivered literally.
///
/// IMPORTANT: tmux's session environment is read by *future* processes spawned
/// in the pane (new windows/panes/`respawn`), NOT by the process already running
/// there. The live agent does not pick this up — it only affects new children.
pub async fn set_environment(target: &str, key: &str, value: &str) -> Result<()> {
    let mut c = Command::new("tmux");
    c.arg("set-environment")
        .arg("-t")
        .arg(target)
        .arg(key)
        .arg(value);
    run_checked(&mut c).await
}

/// Pipe the pane's output to `out_path` (append), idempotently: a pane whose
/// pipe is already live is left untouched.
///
/// IMPORTANT: `pipe-pane -o` is NOT "no-op if a pipe exists" — it TOGGLES.
/// tmux always closes the existing pipe first; `-o` then merely skips opening
/// the replacement. A blind `-o` re-arm therefore DISARMED live streams every
/// other call (issue #270): a fresh agent session spawned armed, and the tab's
/// connect-time re-arm switched the pipe off before the agent printed a byte —
/// a permanently blank terminal. So probe `#{pane_pipe}` and skip when armed;
/// when arming, use plain `pipe-pane` (no `-o`) so a lost race between two
/// concurrent connects still ends with a live pipe rather than a toggled-off
/// one.
///
/// tmux interprets the shell-command via `/bin/sh -c`, so `>>` is the shell's
/// append-redirect operator. Only the path is shell-quoted.
pub async fn pipe_pane(target: &str, out_path: &Path) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        // Async create: this runs on the tokio worker on every local WS connect,
        // so keep the (usually no-op) mkdir off the blocking path.
        tokio::fs::create_dir_all(parent).await?;
    }
    let path_str = out_path
        .to_str()
        .ok_or_else(|| TmuxError::Parse("non-utf8 log path".into()))?;
    let quoted_path = shlex::try_quote(path_str).map_err(|_| TmuxError::Quote)?;
    let cmd_str = format!("cat >> {quoted_path}");

    let probe = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(target)
        .arg("#{pane_pipe}")
        .output()
        .await?;
    check(&probe)?;
    if String::from_utf8_lossy(&probe.stdout).trim() == "1" {
        return Ok(());
    }

    let mut c = Command::new("tmux");
    c.arg("pipe-pane").arg("-t").arg(target).arg(cmd_str);
    run_checked(&mut c).await
}

/// Disarm `pipe-pane` on a pane — running `tmux pipe-pane` with no shell
/// command closes the existing pipe. Used when detaching from an external
/// (non-agentum) tmux session so its output stops accumulating in our log.
pub async fn unpipe_pane(target: &str) -> Result<()> {
    let mut c = Command::new("tmux");
    c.arg("pipe-pane").arg("-t").arg(target);
    run_checked(&mut c).await
}

/// `tmux list-panes -a -F <format>` raw stdout across every session on the
/// server, with a small retry. Returns `Ok("")` when tmux is not installed or
/// there is genuinely no server/sessions — for discovery both mean "nothing to
/// list", not an error.
///
/// `list-panes -a` scans the whole server in one shot. It has been observed to
/// fail *transiently* on macOS inside a long-running app process — a fast
/// non-zero exit with an EMPTY stderr — while targeted commands (`capture-pane
/// -t <name>`) on the *same* socket keep working. The old code masked that as
/// "no sessions" (the `stderr.is_empty()` branch), which froze the host tmux
/// panel into a false "0 sessions" until the app restarted (issue #203). So:
///   - a stderr that *names* a genuine no-server / no-sessions returns `Ok("")`
///     immediately (no retry — that state is stable);
///   - any other non-zero exit (incl. the empty-stderr anomaly) is retried a
///     few times, and if it still fails is surfaced as an error — the UI then
///     shows "couldn't list sessions", never a misleading "none".
pub async fn list_panes_all(format: &str) -> Result<String> {
    const ATTEMPTS: u32 = 3;
    let mut last: Option<TmuxError> = None;
    for attempt in 1..=ATTEMPTS {
        match list_panes_all_once(format).await {
            Ok(out) => return Ok(out),
            Err(e) => {
                tracing::warn!(attempt, error = %e, "tmux list-panes -a failed; retrying");
                last = Some(e);
                if attempt < ATTEMPTS {
                    // Brief backoff: the failure is transient by hypothesis, so
                    // give the tmux client a moment before re-scanning.
                    sleep(Duration::from_millis(75 * attempt as u64)).await;
                }
            }
        }
    }
    // Every attempt failed with a real (non "no sessions") error — surface it
    // rather than masking it as an empty list.
    Err(last.expect("the retry loop runs at least once"))
}

/// One `tmux list-panes -a` pass. `Ok("")` for "tmux missing" or a genuine
/// no-server/no-sessions; `Err` for any other non-zero exit (retried by the
/// caller). Split out so [`list_panes_all`] can retry without re-implementing
/// the classification.
async fn list_panes_all_once(format: &str) -> Result<String> {
    let out = match Command::new("tmux")
        .args(["list-panes", "-a", "-F", format])
        .output()
        .await
    {
        Ok(o) => o,
        // tmux not on PATH → "no sessions" for discovery, but log it: an empty
        // list would otherwise read as "0 sessions" even though the binary is
        // missing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("tmux binary not found on PATH; reporting no sessions");
            return Ok(String::new());
        }
        Err(e) => return Err(e.into()),
    };
    if out.status.success() {
        return Ok(String::from_utf8(out.stdout)?);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    let code = out.status.code().unwrap_or(-1);
    if tmux_stderr_means_no_sessions(stderr) {
        tracing::info!(code, stderr, "tmux list-panes -a returned no sessions");
        return Ok(String::new());
    }
    // A real (or anomalous) failure — surface it; the caller retries first.
    tracing::warn!(code, stderr, "tmux list-panes -a failed");
    Err(TmuxError::NonZero {
        status: code,
        stderr: stderr.to_string(),
    })
}

/// True when tmux's stderr means "there simply is no server / no sessions" — a
/// legitimate empty result for discovery — rather than a real failure.
///
/// tmux names this case explicitly: "no server running on <socket>" /
/// "no current session" / "no sessions" (Linux), or, when the socket file is
/// absent, "error connecting to <socket> (No such file or directory)" (macOS).
/// An EMPTY stderr is deliberately NOT here: tmux always names a genuine
/// no-server, so a non-zero exit with no stderr is an *anomaly* (retried and
/// surfaced by [`list_panes_all`]), not "no sessions" — masking it as empty was
/// the root of the frozen "0 sessions" panel (issue #203). A different connect
/// error (permission denied, connection refused) stays a real, surfaced
/// failure. Kept pure so the benign-vs-error split is unit-testable without a
/// live tmux server.
fn tmux_stderr_means_no_sessions(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("no server running")
        || s.contains("no current session")
        || s.contains("no sessions")
        // macOS: an absent server socket → the client can't connect because the
        // socket file doesn't exist → nothing to list.
        || (s.contains("error connecting") && s.contains("no such file"))
}

/// True when tmux's stderr means "this specific target no longer exists" — a
/// legitimate "session gone" for a targeted command — rather than a real
/// failure. Superset of [`tmux_stderr_means_no_sessions`] (no server at all ⇒
/// the target is gone too) plus tmux's named-target misses: current tmux says
/// "can't find session/window/pane: <t>", pre-2.2 said "session not found".
/// Lets the watchdog fold session existence into its batched capture instead
/// of paying a separate `has-session` fork/exec per session per tick. Kept
/// pure so the gone-vs-error split is unit-testable without a live server.
pub(crate) fn tmux_stderr_means_target_gone(stderr: &str) -> bool {
    if tmux_stderr_means_no_sessions(stderr) {
        return true;
    }
    let s = stderr.to_ascii_lowercase();
    s.contains("can't find session")
        || s.contains("can't find window")
        || s.contains("can't find pane")
        || s.contains("session not found")
}

/// Basename of the foreground process inside the pane (tmux's
/// `pane_current_command` format token). Useful for figuring out which
/// adapter the user is currently running — e.g. tells us "codex" vs
/// "claude" vs "bash" without us having to scrape pane output.
///
/// Returns the trimmed string straight from tmux. On freshly-spawned
/// panes this can briefly be the shell binary even when the intended
/// adapter is mid-launch — callers that want stability should debounce
/// across a few ticks rather than reacting to a single observation.
pub async fn pane_current_command(target: &str) -> Result<String> {
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(target)
        .arg("#{pane_current_command}")
        .output()
        .await?;
    check(&out)?;
    let s = String::from_utf8(out.stdout)?;
    Ok(s.trim().to_string())
}

/// PID of the foreground process inside the pane.
pub async fn pane_pid(target: &str) -> Result<u32> {
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(target)
        .arg("#{pane_pid}")
        .output()
        .await?;
    check(&out)?;
    let s = String::from_utf8(out.stdout)?;
    s.trim()
        .parse()
        .map_err(|e: std::num::ParseIntError| TmuxError::Parse(e.to_string()))
}

/// Send SIGTERM to the pane's process; if it's still alive after `timeout`,
/// SIGKILL it. Then `kill-session` cleans up tmux state. Idempotent.
pub async fn graceful_stop(target: &str, timeout: Duration) -> Result<()> {
    if !has_session(target).await? {
        return Ok(());
    }
    // pane gone between checks → just skip the signal phase.
    let pid = pane_pid(target).await.ok();

    if let Some(pid) = pid {
        let _ = signal(pid, "TERM").await;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !is_alive(pid).await {
                break;
            }
            sleep(Duration::from_millis(200)).await;
        }
        if is_alive(pid).await {
            tracing::warn!(pid, "process did not exit after SIGTERM; sending SIGKILL");
            let _ = signal(pid, "KILL").await;
        }
    }

    kill_session(target).await
}

async fn signal(pid: u32, sig: &str) -> Result<()> {
    let mut c = Command::new("kill");
    c.arg(format!("-{sig}")).arg(pid.to_string());
    run_checked(&mut c).await
}

async fn is_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn run_checked(c: &mut Command) -> Result<()> {
    let out = c.output().await?;
    check(&out)
}

fn check(out: &std::process::Output) -> Result<()> {
    if out.status.success() {
        Ok(())
    } else {
        Err(TmuxError::NonZero {
            status: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_format() {
        assert_eq!(target_for("alpha"), "agentum-alpha");
    }

    /// Regression for issue #270: `tmux pipe-pane -o` TOGGLES — tmux closes a
    /// live pipe first, and `-o` merely skips opening the replacement — so a
    /// blind `-o` re-arm disarmed the stream on every other call. Freshly
    /// spawned agent sessions went permanently blank: spawn armed the pipe,
    /// the tab's connect-time self-heal toggled it off, and the agent's
    /// output had nowhere to go. `pipe_pane` must be truly idempotent:
    /// re-arming an armed pane leaves the pipe live.
    ///
    /// Drives a REAL tmux server (same convention as the other live tests
    /// here: default socket, `agentum-test-*` session, kill on both ends);
    /// skips where tmux isn't installed so bare CI runners stay green.
    #[tokio::test]
    async fn pipe_pane_is_idempotent_against_live_tmux() {
        // Skip if tmux isn't available in CI.
        if Command::new("tmux").arg("-V").status().await.is_err() {
            return;
        }
        let target = "agentum-test-pipe-toggle";
        let _ = kill_session(target).await;
        let workdir = std::env::temp_dir();
        new_session(target, &workdir, &["sleep".into(), "3600".into()], &[])
            .await
            .unwrap();

        let log = std::env::temp_dir().join("agentum-test-pipe-toggle.log");
        let mut states = Vec::new();
        // Arm, then re-arm twice (spawn + connect + reconnect in real life).
        for _ in 0..3 {
            pipe_pane(target, &log).await.unwrap();
            let probe = Command::new("tmux")
                .args(["display-message", "-p", "-t", target, "#{pane_pipe}"])
                .output()
                .await
                .unwrap();
            states.push(String::from_utf8_lossy(&probe.stdout).trim().to_string());
        }
        // Tear down before asserting so a red run doesn't leak the session.
        kill_session(target).await.unwrap();
        let _ = std::fs::remove_file(&log);

        assert_eq!(
            states,
            ["1", "1", "1"],
            "a re-arm toggled the pipe off (issue #270)"
        );
    }

    #[test]
    fn tmux_stderr_no_sessions_classification() {
        // Benign — a genuinely empty result, must NOT surface as an error.
        assert!(tmux_stderr_means_no_sessions(
            "no server running on /tmp/tmux-501/default"
        ));
        assert!(tmux_stderr_means_no_sessions("no current session"));
        assert!(tmux_stderr_means_no_sessions("No sessions")); // case-insensitive
        // macOS phrases an absent server socket as an "error connecting …
        // (No such file or directory)" — still just "no server", i.e. empty.
        assert!(tmux_stderr_means_no_sessions(
            "error connecting to /private/tmp/tmux-501/default (No such file or directory)"
        ));
        // Real / anomalous failures — must be surfaced (not swallowed into an
        // empty list), else the discovery panel shows a misleading "0"
        // (issue #203). An EMPTY stderr is such an anomaly: tmux always names a
        // genuine no-server, so no message on a non-zero exit is NOT "none".
        assert!(!tmux_stderr_means_no_sessions(""));
        assert!(!tmux_stderr_means_no_sessions(
            "protocol version mismatch (client 8, server 7)"
        ));
        assert!(!tmux_stderr_means_no_sessions(
            "error connecting to /tmp/tmux-501/default (Permission denied)"
        ));
        assert!(!tmux_stderr_means_no_sessions("too many open files"));
    }

    #[test]
    fn tmux_stderr_target_gone_classification() {
        // "Gone" — the watchdog's batched sample must map these to Ok(None),
        // matching what the old has-session pre-gate would have concluded.
        // tmux 3.x names a missing exact-match target per command in the
        // sequence; pre-2.2 used "session not found".
        assert!(tmux_stderr_means_target_gone(
            "can't find session: =agentum-alpha"
        ));
        assert!(tmux_stderr_means_target_gone("can't find pane: %5"));
        assert!(tmux_stderr_means_target_gone("can't find window: @2"));
        assert!(tmux_stderr_means_target_gone("session not found: alpha"));
        // The whole sequence fails once per command — still classifiable.
        assert!(tmux_stderr_means_target_gone(
            "can't find session: =agentum-a\ncan't find session: =agentum-a\ncan't find session: =agentum-a"
        ));
        // No server at all ⇒ the target is gone too (superset of no-sessions).
        assert!(tmux_stderr_means_target_gone(
            "no server running on /tmp/tmux-501/default"
        ));
        // Real failures must still surface as errors, not "gone" — a wrongly
        // swallowed error would make the watchdog mark a live agent finished.
        assert!(!tmux_stderr_means_target_gone(""));
        assert!(!tmux_stderr_means_target_gone(
            "protocol version mismatch (client 8, server 7)"
        ));
        assert!(!tmux_stderr_means_target_gone(
            "error connecting to /tmp/tmux-501/default (Permission denied)"
        ));
        assert!(!tmux_stderr_means_target_gone("too many open files"));
    }

    #[test]
    fn cursor_sample_parses_and_rejects() {
        assert_eq!(
            parse_cursor_sample("12 3 0"),
            Some(CursorSample {
                x: 12,
                y: 3,
                visible: false
            })
        );
        // Missing flag defaults to visible (older tmux without cursor_flag).
        assert_eq!(
            parse_cursor_sample("5 7"),
            Some(CursorSample {
                x: 5,
                y: 7,
                visible: true
            })
        );
        assert_eq!(parse_cursor_sample(""), None);
        assert_eq!(parse_cursor_sample("X"), None);
        assert_eq!(parse_cursor_sample("3 nope 1"), None);
    }

    #[test]
    fn anchored_snapshot_separates_rows_and_restores_cursor() {
        let snap = assemble_anchored_snapshot(
            b"hello\nworld\n",
            Some(CursorSample {
                x: 5,
                y: 1,
                visible: true,
            }),
        );
        // No trailing CRLF (would scroll a full-height pane); CUP is 1-based.
        assert_eq!(snap, b"hello\r\nworld\x1b[2;6H");
    }

    #[test]
    fn anchored_snapshot_preserves_blank_trailing_rows() {
        // Only capture-pane's own final `\n` is stripped — a blank last row
        // (empty split) still paints, keeping absolute coordinates aligned.
        let snap = assemble_anchored_snapshot(b"a\n\n\n", None);
        assert_eq!(snap, b"a\r\n\r\n");
    }

    #[test]
    fn anchored_snapshot_rehides_hidden_cursor() {
        let snap = assemble_anchored_snapshot(
            b"x\n",
            Some(CursorSample {
                x: 0,
                y: 0,
                visible: false,
            }),
        );
        assert_eq!(snap, b"x\x1b[1;1H\x1b[?25l");
    }

    #[test]
    fn anchored_snapshot_empty_grid_yields_empty() {
        // Callers treat an empty snapshot as "nothing to paint" — a bare CUP
        // with no content must not flip that gate.
        assert_eq!(
            assemble_anchored_snapshot(
                b"",
                Some(CursorSample {
                    x: 1,
                    y: 1,
                    visible: true
                })
            ),
            Vec::<u8>::new()
        );
        assert_eq!(assemble_anchored_snapshot(b"\n", None), Vec::<u8>::new());
    }

    #[tokio::test]
    async fn capture_pane_ansi_smoke_anchors_cursor() {
        // Skip if tmux isn't available in CI.
        if Command::new("tmux").arg("-V").status().await.is_err() {
            return;
        }
        let target = "agentum-test-capture";
        let _ = kill_session(target).await;
        let workdir = std::env::temp_dir();
        new_session(target, &workdir, &["sleep".into(), "3600".into()], &[])
            .await
            .unwrap();
        // Give tmux a beat to render the pane before capturing.
        sleep(Duration::from_millis(300)).await;
        let snap = capture_pane_ansi(target).await.unwrap();
        let s = String::from_utf8_lossy(&snap);
        // The cursor anchor (absolute CUP) must be present and the rows must
        // not be newline-terminated — both are what keeps replayed redraw
        // cycles aligned with the painted grid.
        assert!(
            s.rsplit('\x1b')
                .next()
                .is_some_and(|t| t.ends_with('H') || t.ends_with('l')),
            "no cursor anchor suffix: {s:?}"
        );
        assert!(!s.ends_with("\r\n"), "row-terminated snapshot: {s:?}");
        kill_session(target).await.unwrap();
    }

    #[tokio::test]
    async fn lifecycle_smoke() {
        // Skip if tmux isn't available in CI.
        if Command::new("tmux").arg("-V").status().await.is_err() {
            return;
        }
        let target = "agentum-test-smoke";
        let _ = kill_session(target).await;

        let workdir = std::env::temp_dir();
        new_session(target, &workdir, &["sleep".into(), "3600".into()], &[])
            .await
            .unwrap();

        assert!(has_session(target).await.unwrap());
        let pid = pane_pid(target).await.unwrap();
        assert!(pid > 0);

        kill_session(target).await.unwrap();
        assert!(!has_session(target).await.unwrap());
    }

    #[tokio::test]
    async fn send_bytes_chunks_role_prompt_sized_payloads_below_tmux_limit() {
        // A long structured authoring prompt that exposed this regression is ~3.3 KiB.
        // Sending it as one `send-keys -H` command fails with "command too
        // long" on tmux 3.7b; the helper must split it into accepted commands.
        if Command::new("tmux").arg("-V").status().await.is_err() {
            return;
        }
        let target = "agentum-test-send-bytes-large";
        let _ = kill_session(target).await;
        let workdir = std::env::temp_dir();
        new_session(target, &workdir, &["sleep".into(), "3600".into()], &[])
            .await
            .unwrap();

        send_bytes(target, &vec![b'a'; 3_281]).await.unwrap();

        kill_session(target).await.unwrap();
    }

    #[tokio::test]
    async fn combined_sample_batches_three_sections() {
        // The batched watchdog read must produce the same three-section,
        // boundary-delimited output the SSH path does — proving tmux accepts the
        // `;`-separated command sequence when exec'd directly (the win: one
        // client round trip, not three) and keeps the sections in order.
        if Command::new("tmux").arg("-V").status().await.is_err() {
            return;
        }
        let target = "agentum-test-combined-sample";
        let _ = kill_session(target).await;
        let workdir = std::env::temp_dir();
        new_session(target, &workdir, &["sleep".into(), "3600".into()], &[])
            .await
            .unwrap();
        // Some interactive shell profiles render a startup banner (for example
        // fastfetch) before handing the pane to `sleep`. Let that output settle
        // so the combined and immediately-following individual captures compare
        // the same static viewport rather than racing shell initialization.
        sleep(Duration::from_secs(10)).await;

        let boundary = ":::agentum-test-boundary:::";
        let stdout = capture_pane_sample_combined(target, 50, boundary)
            .await
            .unwrap();

        // Exactly two boundaries → three sections (command / scrollback /
        // viewport). More/fewer means the sequence didn't run as expected.
        assert_eq!(
            stdout.matches(boundary).count(),
            2,
            "combined sample must delimit exactly three sections: {stdout:?}"
        );
        let sep = format!("\n{boundary}\n");
        let sections: Vec<&str> = stdout.splitn(3, &sep).collect();
        assert_eq!(sections.len(), 3, "combined sample sections: {stdout:?}");

        // The batched read must be equivalent to the three separate calls it
        // replaces. The command section uses the identical `#{pane_current_command}`
        // format, so it must agree with the standalone helper (whatever the shell
        // wrapper reports); the viewport is static for a `sleep` pane, so it must
        // match `capture_pane_visible` byte-for-byte.
        let individual_command = pane_current_command(target).await.unwrap();
        assert_eq!(sections[0].trim(), individual_command);
        let individual_viewport = capture_pane_visible(target).await.unwrap();
        assert_eq!(sections[2], individual_viewport);

        kill_session(target).await.unwrap();

        // With the session gone, the same batched call must fail with a stderr
        // the target-gone classifier recognises — this is what lets the
        // watchdog skip the separate has-session fork/exec per tick, so verify
        // it against the real tmux binary's wording, not just our fixtures.
        match capture_pane_sample_combined(target, 50, boundary).await {
            Err(TmuxError::NonZero { stderr, .. }) => {
                assert!(
                    tmux_stderr_means_target_gone(&stderr),
                    "killed session's stderr must classify as gone: {stderr:?}"
                );
            }
            other => panic!("expected NonZero for killed session, got {other:?}"),
        }
    }
}
