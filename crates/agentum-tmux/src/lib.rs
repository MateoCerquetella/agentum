//! tmux subprocess adapter.
//!
//! Every method shells out to `tmux` via `tokio::process::Command` with one
//! `.arg()` per argument — no shell-string interpolation in our process
//! invocation. The single shell-command string we pass to
//! `tmux new-session` / `tmux pipe-pane` is safely quoted with [`shlex`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    #[error("invalid launch environment variable name")]
    InvalidEnvironmentName,
}

pub type Result<T> = std::result::Result<T, TmuxError>;

/// Returns the tmux session name for an agentum session.
pub fn target_for(name: &str) -> String {
    format!("agentum-{name}")
}

// tmux sanitizes control characters in format output when no UTF-8 locale is
// configured (a tab becomes `_`). Keep the wire delimiter printable instead.
// Session ids are always `$` plus decimal digits, so their first `_` safely
// separates the id from the otherwise-unrestricted session name.
const SESSION_LIST_FORMAT: &str = "#{session_id}_#{session_name}";

fn session_target_is_id(target: &str) -> Result<bool> {
    if !target.starts_with('$') {
        return Ok(false);
    }
    if target.strip_prefix('$').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return Ok(true);
    }
    Err(TmuxError::Parse(
        "invalid tmux session id (expected $ followed by decimal digits)".into(),
    ))
}

fn tmux_command(tmux_program: &Path) -> Command {
    #[cfg(test)]
    if tmux_program
        .extension()
        .is_some_and(|extension| extension == "sh")
    {
        let mut command = Command::new("/bin/sh");
        command.arg(tmux_program);
        return command;
    }
    Command::new(tmux_program)
}

/// Resolve a session name by exact string comparison and return tmux's
/// immutable `$N` id. tmux 3.7 rejects the former `=name` syntax, while a raw
/// `-t name` prefix-matches and can mutate the wrong Agentum session.
async fn resolve_session_id_with(tmux_program: &Path, target: &str) -> Result<Option<String>> {
    let target_is_id = session_target_is_id(target)?;
    let output = tmux_command(tmux_program)
        .args(["list-sessions", "-F", SESSION_LIST_FORMAT])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() == Some(1) && ssh::is_tmux_session_missing_error(&stderr) {
            return Ok(None);
        }
        check(&output)?;
        unreachable!("check returns an error for a non-zero tmux status");
    }

    let stdout = String::from_utf8(output.stdout)?;
    for row in stdout.lines() {
        let Some((session_id, session_name)) = row.split_once('_') else {
            continue;
        };
        let matches = if target_is_id {
            session_id == target
        } else {
            session_name == target
        };
        if !matches {
            continue;
        }
        if !session_id.strip_prefix('$').is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            return Err(TmuxError::Parse(format!(
                "tmux returned an invalid session id for {target}"
            )));
        }
        return Ok(Some(session_id.to_string()));
    }
    Ok(None)
}

async fn resolve_session_id(target: &str) -> Result<Option<String>> {
    resolve_session_id_with(Path::new("tmux"), target).await
}

async fn require_session_id(target: &str) -> Result<String> {
    resolve_session_id(target)
        .await?
        .ok_or_else(|| TmuxError::NonZero {
            status: 1,
            stderr: format!("can't find session: {target}"),
        })
}

/// Exact session existence check. Missing targets and an absent tmux server
/// return `false`; other tmux failures remain typed errors.
pub async fn has_session(target: &str) -> Result<bool> {
    Ok(resolve_session_id(target).await?.is_some())
}

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
/// tmux's own client/server command message is tighter than the OS `ARG_MAX`:
/// each byte becomes an argv entry plus framing, and a 1,000-byte batch already
/// fails on tmux 3.7b. Keep one conservative bound for local, SSH one-shot, and
/// persistent-writer input so large pastes are delivered losslessly.
pub const SEND_KEYS_HEX_CHUNK_BYTES: usize = 512;

/// Backstop for a launcher which tmux accepted but never opened. Ordinarily
/// `/bin/sh` unlinks the file immediately; this delayed cleanup handles a pane
/// failing before the wrapper starts without racing normal tmux startup.
const STAGED_LAUNCH_CLEANUP_DELAY: Duration = Duration::from_secs(60);

static STAGED_LAUNCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A private, one-shot shell launcher carrying the child environment.
///
/// While armed, dropping it cleans up a failed tmux launch. A successful tmux
/// handoff disarms the synchronous cleanup because the child may still be
/// opening the file; the wrapper self-deletes and a delayed task is retained as
/// a backstop.
struct StagedLaunch {
    script_path: PathBuf,
    launch_dir: PathBuf,
    cleanup_on_drop: bool,
}

impl StagedLaunch {
    fn shell_command(&self) -> Result<String> {
        let path = self
            .script_path
            .to_str()
            .ok_or_else(|| TmuxError::Parse("private launch path was not valid utf-8".into()))?;
        shlex::try_join(["/bin/sh", path]).map_err(|_| TmuxError::Quote)
    }

    fn handoff(mut self) {
        self.cleanup_on_drop = false;
        let script_path = self.script_path.clone();
        let launch_dir = self.launch_dir.clone();
        tokio::spawn(async move {
            sleep(STAGED_LAUNCH_CLEANUP_DELAY).await;
            cleanup_staged_launch(&script_path, &launch_dir);
        });
    }
}

impl Drop for StagedLaunch {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            cleanup_staged_launch(&self.script_path, &self.launch_dir);
        }
    }
}

fn cleanup_staged_launch(script_path: &Path, launch_dir: &Path) {
    let _ = std::fs::remove_file(script_path);
    let _ = std::fs::remove_dir(launch_dir);
}

fn private_launch_base_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        if runtime.is_absolute() {
            return runtime.join("agentum-tmux").join("launches");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if home.is_absolute() {
            return home.join(".agentum").join("runtime").join("tmux-launches");
        }
    }
    // `HOME` is present for normal agentum launches. This last-resort path is
    // process-namespaced and still created 0700, so secrets never land directly
    // in a shared temporary directory.
    std::env::temp_dir().join(format!("agentum-tmux-{}", std::process::id()))
}

fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "private tmux launch path is not a directory",
            ));
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(path)?;
    Ok(())
}

fn valid_environment_name(name: &str) -> bool {
    let mut chars = name.bytes();
    matches!(chars.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && chars.all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

fn render_staged_launch(cmd: &[String], env: &[(String, String)]) -> Result<String> {
    if cmd.is_empty() {
        return Err(TmuxError::Parse("launch command was empty".into()));
    }
    if env.iter().any(|(name, _)| !valid_environment_name(name)) {
        return Err(TmuxError::InvalidEnvironmentName);
    }

    let mut script = String::from(
        "#!/bin/sh\n\
         stage=$0\n\
         stage_dir=${stage%/*}\n\
         if ! rm -f \"$stage\"; then\n\
         \tprintf '%s\\n' 'agentum: could not remove private launch environment' >&2\n\
         \texit 70\n\
         fi\n\
         rmdir \"$stage_dir\" 2>/dev/null || :\n",
    );
    for (name, value) in env {
        let value = shlex::try_quote(value).map_err(|_| TmuxError::Quote)?;
        script.push_str(name);
        script.push('=');
        script.push_str(&value);
        script.push('\n');
        script.push_str("export ");
        script.push_str(name);
        script.push('\n');
    }
    let command = shlex::try_join(cmd.iter().map(String::as_str)).map_err(|_| TmuxError::Quote)?;
    script.push_str("exec ");
    script.push_str(&command);
    script.push('\n');
    Ok(script)
}

fn stage_launch_in(
    base_dir: &Path,
    cmd: &[String],
    env: &[(String, String)],
) -> Result<StagedLaunch> {
    let script = render_staged_launch(cmd, env)?;
    ensure_private_dir(base_dir)?;

    let launch_dir = loop {
        let sequence = STAGED_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = base_dir.join(format!("{}.{}", std::process::id(), sequence));
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&candidate)
        };
        #[cfg(not(unix))]
        let created = std::fs::create_dir(&candidate);
        match created {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    let script_path = launch_dir.join("launch.sh");

    let write_result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&script_path)?;
        file.write_all(script.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        cleanup_staged_launch(&script_path, &launch_dir);
        return Err(error.into());
    }

    Ok(StagedLaunch {
        script_path,
        launch_dir,
        cleanup_on_drop: true,
    })
}

fn tmux_new_session_command(
    tmux_program: &Path,
    target: &str,
    workdir: &Path,
    shell_command: &str,
) -> Command {
    let mut command = tmux_command(tmux_program);
    command
        .arg("new-session")
        .arg("-d")
        .arg("-s")
        .arg(target)
        .arg("-x")
        .arg(DEFAULT_PANE_COLS.to_string())
        .arg("-y")
        .arg(DEFAULT_PANE_ROWS.to_string())
        .arg("-c")
        .arg(workdir)
        .arg(shell_command);
    command
}

/// Spawn a detached tmux session running `cmd` (as argv) in `workdir`.
///
/// When `env` is non-empty, the command and environment are written to a
/// one-shot `0600` launcher inside a private `0700` directory. tmux receives
/// only that launcher's opaque path; environment values (which include MCP
/// bearer tokens) therefore never appear in the tmux client's argv or error
/// diagnostics. The launcher unlinks itself before exporting the environment
/// and `exec`ing the requested command. Workdir must exist on disk.
pub async fn new_session(
    target: &str,
    workdir: &Path,
    cmd: &[String],
    env: &[(String, String)],
) -> Result<()> {
    new_session_with_tmux(
        Path::new("tmux"),
        &private_launch_base_dir(),
        target,
        workdir,
        cmd,
        env,
    )
    .await
}

async fn new_session_with_tmux(
    tmux_program: &Path,
    launch_base_dir: &Path,
    target: &str,
    workdir: &Path,
    cmd: &[String],
    env: &[(String, String)],
) -> Result<()> {
    let staged = if env.is_empty() {
        None
    } else {
        Some(stage_launch_in(launch_base_dir, cmd, env)?)
    };
    let shell_command = match &staged {
        Some(staged) => staged.shell_command()?,
        None => shlex::try_join(cmd.iter().map(String::as_str)).map_err(|_| TmuxError::Quote)?,
    };
    let mut command = tmux_new_session_command(tmux_program, target, workdir, &shell_command);

    match run_checked(&mut command).await {
        Ok(()) => {
            if let Some(staged) = staged {
                staged.handoff();
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// `tmux kill-session -t <target>`. Idempotent — non-existent target returns Ok.
pub async fn kill_session(target: &str) -> Result<()> {
    kill_session_with_tmux(Path::new("tmux"), target).await
}

async fn kill_session_with_tmux(tmux_program: &Path, target: &str) -> Result<()> {
    let Some(session_id) = resolve_session_id_with(tmux_program, target).await? else {
        return Ok(());
    };
    kill_session_id_with_tmux(tmux_program, &session_id).await
}

async fn kill_session_id_with_tmux(tmux_program: &Path, session_id: &str) -> Result<()> {
    let mut c = tmux_command(tmux_program);
    // No `--` here: `-t` already consumes its own argument (getopt-safe even if
    // the target starts with `-`); a `--` would be taken AS the target name and
    // silently no-op the kill.
    c.arg("kill-session").arg("-t").arg(session_id);
    run_checked(&mut c).await
}

/// Capture last `lines` of pane content as plain text (no ANSI escapes —
/// suitable for regex matching by the watchdog).
pub async fn capture_pane(target: &str, lines: usize) -> Result<String> {
    let session_id = require_session_id(target).await?;
    let start = format!("-{lines}");
    let out = Command::new("tmux")
        .args(["capture-pane", "-p", "-S", &start, "-t"])
        .arg(session_id)
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
    let session_id = require_session_id(target).await?;
    let out = Command::new("tmux")
        .args(["capture-pane", "-p", "-S", "0", "-t"])
        .arg(session_id)
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
    let session_id = require_session_id(target).await?;
    // One tmux invocation, two commands (`;` separates command sequences at
    // the argv level): the cursor line and the grid come from the same server
    // pass, so the anchor can't drift from the content it anchors.
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(&session_id)
        .arg(CURSOR_SAMPLE_FORMAT)
        .arg(";")
        .args(["capture-pane", "-p", "-e", "-t"])
        .arg(&session_id)
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
    let session_id = require_session_id(target).await?;
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(session_id)
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
    let session_id = require_session_id(target).await?;
    let mut c = Command::new("tmux");
    c.arg("send-keys").arg("-t").arg(session_id).arg(keys);
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
/// Splits into chunks to stay under typical argv limits when a paste is huge.
pub async fn send_bytes(target: &str, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let session_id = require_session_id(target).await?;
    for chunk in bytes.chunks(SEND_KEYS_HEX_CHUNK_BYTES) {
        let mut c = Command::new("tmux");
        c.arg("send-keys").arg("-H").arg("-t").arg(&session_id);
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
    let session_id = require_session_id(target).await?;
    let cols = cols.max(20);
    let rows = rows.max(5);

    // window-size manual: tmux stops auto-fitting to attached clients and
    // honours our explicit size. -q suppresses "no current session" noise.
    let _ = Command::new("tmux")
        .args(["set-option", "-q", "-t"])
        .arg(&session_id)
        .args(["window-size", "manual"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    let mut c = Command::new("tmux");
    c.arg("resize-window")
        .arg("-t")
        .arg(&session_id)
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
    let session_id = require_session_id(target).await?;
    let _ = Command::new("tmux")
        .args(["set-option", "-q", "-t"])
        .arg(&session_id)
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
        .arg(&session_id)
        .arg(flag)
        .arg(count);
    run_checked(&mut c).await
}

/// Pipe the pane's output to `out_path` (append), idempotently: a pane whose
/// pipe is already live is left untouched.
///
/// IMPORTANT: `pipe-pane -o` toggles a live pipe off. tmux closes the existing
/// pipe before `-o` suppresses opening its replacement, so a blind re-arm on
/// every terminal attach made every other connection stop logging output.
/// Probe `#{pane_pipe}` and skip when it is already armed; use plain
/// `pipe-pane` when it is not.
///
/// The local log is prepared before tmux sees the pipe command: its parent is
/// a real, current-user-owned `0700` directory and the file is a singly-linked
/// regular `0600` file. The sink re-opens that exact device/inode and checks it
/// before copying any pane bytes, so replacing the path with a symlink or a
/// different file makes the pipe fail closed instead of writing elsewhere.
pub async fn pipe_pane(target: &str, out_path: &Path) -> Result<()> {
    let session_id = require_session_id(target).await?;
    let identity = prepare_private_pane_log(out_path)?;
    let sink = pane_log_sink_command(out_path, identity)?;
    // tmux executes pipe commands with `default-shell`, which may be fish or
    // zsh. The guarded sink deliberately uses POSIX syntax, so force it through
    // sh exactly like the SSH implementation does instead of letting a user's
    // tmux shell parse (and immediately kill) the output pipe.
    // `pipe-pane` treats `%` as a strftime token even inside a quoted shell
    // command. Double it so the stat format strings reach sh unchanged.
    let sink = sink.replace('%', "%%");
    let sink = shlex::try_quote(&sink).map_err(|_| TmuxError::Quote)?;
    let cmd_str = format!("exec sh -c {sink}");

    pipe_pane_with_tmux(Path::new("tmux"), &session_id, &cmd_str).await
}

async fn pipe_pane_with_tmux(
    tmux_program: &Path,
    session_id: &str,
    sink_command: &str,
) -> Result<()> {
    let probe = tmux_command(tmux_program)
        .args(["display-message", "-p", "-t"])
        .arg(session_id)
        .arg("#{pane_pipe}")
        .output()
        .await?;
    check(&probe)?;
    if String::from_utf8_lossy(&probe.stdout).trim() == "1" {
        return Ok(());
    }

    let mut c = tmux_command(tmux_program);
    c.arg("pipe-pane")
        .arg("-t")
        .arg(session_id)
        .arg(sink_command);
    run_checked(&mut c).await
}

#[derive(Debug, Clone, Copy)]
struct PaneLogIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    uid: u32,
}

fn pane_log_parent(out_path: &Path) -> std::io::Result<&Path> {
    out_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "pane log path must have an explicit parent directory",
            )
        })
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
struct PrivateDirIdentity {
    dev: u64,
    ino: u64,
    uid: u32,
}

/// Secure the pane-log directory without trusting `$UID` or following a final
/// path-component symlink. The owner uid comes from a freshly-created private
/// probe; descriptor/path identity checks catch replacement while preparing it.
#[cfg(unix)]
fn ensure_private_pane_log_dir(path: &Path) -> std::io::Result<PrivateDirIdentity> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;

    let initial = std::fs::symlink_metadata(path)?;
    if initial.file_type().is_symlink() || !initial.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "pane log parent is not a real directory: {}",
                path.display()
            ),
        ));
    }

    // Hold the directory itself open while repairing its mode. File methods
    // operate on the descriptor, so a path swap cannot redirect chmod outside
    // the directory that was just verified.
    let directory = std::fs::File::open(path)?;
    let opened = directory.metadata()?;
    if !opened.file_type().is_dir()
        || opened.dev() != initial.dev()
        || opened.ino() != initial.ino()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("pane log parent changed while opening: {}", path.display()),
        ));
    }

    let probe_path = loop {
        let sequence = STAGED_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = path.join(format!(
            ".agentum-owner-probe.{}.{}",
            std::process::id(),
            sequence
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(probe) => {
                let uid = probe.metadata()?.uid();
                drop(probe);
                std::fs::remove_file(&candidate)?;
                if opened.uid() != uid {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!(
                            "pane log parent {} is owned by uid {}, expected uid {uid}",
                            path.display(),
                            opened.uid()
                        ),
                    ));
                }
                break (candidate, uid);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let (_removed_probe, effective_uid) = probe_path;

    directory.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    let secured = directory.metadata()?;
    let current = std::fs::symlink_metadata(path)?;
    if !secured.file_type().is_dir()
        || secured.uid() != effective_uid
        || secured.mode() & 0o7777 != 0o700
        || current.file_type().is_symlink()
        || !current.file_type().is_dir()
        || current.dev() != secured.dev()
        || current.ino() != secured.ino()
        || current.uid() != effective_uid
        || current.mode() & 0o7777 != 0o700
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "pane log parent failed owner/mode verification: {}",
                path.display()
            ),
        ));
    }

    Ok(PrivateDirIdentity {
        dev: secured.dev(),
        ino: secured.ino(),
        uid: effective_uid,
    })
}

#[cfg(unix)]
fn private_pane_log_metadata(
    metadata: &std::fs::Metadata,
    expected_uid: u32,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_uid
        || metadata.nlink() != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "pane log must be a current-user-owned, singly-linked regular file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_pane_log_parent(path: &Path, expected: PrivateDirIdentity) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.dev() != expected.dev
        || metadata.ino() != expected.ino
        || metadata.uid() != expected.uid
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("pane log parent was replaced: {}", path.display()),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_private_pane_log(out_path: &Path) -> Result<PaneLogIdentity> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let parent = pane_log_parent(out_path)?;
    let parent_identity = ensure_private_pane_log_dir(parent)?;
    let initial = match std::fs::symlink_metadata(out_path) {
        Ok(metadata) => {
            private_pane_log_metadata(&metadata, parent_identity.uid)?;
            Some((metadata.dev(), metadata.ino()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    // `create_new` refuses a symlink at a previously-absent path. For an
    // existing file, opening performs no write; descriptor identity is checked
    // against both lstat snapshots before permissions are changed.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).append(true).mode(0o600);
    if initial.is_none() {
        options.create_new(true);
    }
    let file = options.open(out_path)?;
    let opened = file.metadata()?;
    private_pane_log_metadata(&opened, parent_identity.uid)?;
    if initial.is_some_and(|identity| identity != (opened.dev(), opened.ino())) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "pane log was replaced while opening: {}",
                out_path.display()
            ),
        )
        .into());
    }

    let current = std::fs::symlink_metadata(out_path)?;
    private_pane_log_metadata(&current, parent_identity.uid)?;
    if current.dev() != opened.dev() || current.ino() != opened.ino() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "pane log path changed while opening: {}",
                out_path.display()
            ),
        )
        .into());
    }
    verify_pane_log_parent(parent, parent_identity)?;

    // fchmod the already-verified descriptor, never a possibly-swapped path.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let secured = file.metadata()?;
    let secured_path = std::fs::symlink_metadata(out_path)?;
    private_pane_log_metadata(&secured, parent_identity.uid)?;
    private_pane_log_metadata(&secured_path, parent_identity.uid)?;
    if secured.mode() & 0o7777 != 0o600
        || secured_path.mode() & 0o7777 != 0o600
        || secured_path.dev() != secured.dev()
        || secured_path.ino() != secured.ino()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "pane log failed mode/identity verification: {}",
                out_path.display()
            ),
        )
        .into());
    }
    verify_pane_log_parent(parent, parent_identity)?;

    Ok(PaneLogIdentity {
        dev: secured.dev(),
        ino: secured.ino(),
        uid: secured.uid(),
    })
}

#[cfg(not(unix))]
fn prepare_private_pane_log(out_path: &Path) -> Result<PaneLogIdentity> {
    let parent = pane_log_parent(out_path)?;
    std::fs::create_dir_all(parent)?;
    std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .create(true)
        .open(out_path)?;
    Ok(PaneLogIdentity {})
}

fn pane_log_sink_command(out_path: &Path, identity: PaneLogIdentity) -> Result<String> {
    let path_str = out_path
        .to_str()
        .ok_or_else(|| TmuxError::Parse("non-utf8 log path".into()))?;
    let quoted_path = shlex::try_quote(path_str).map_err(|_| TmuxError::Quote)?;

    #[cfg(unix)]
    {
        // Opening with `>>` itself writes nothing. Only after the descriptor's
        // device/inode equals the securely-prepared file do pane bytes reach it.
        // `umask 077` also keeps any file created by a removal race private;
        // that fresh inode then fails the identity check before `/bin/cat` runs.
        Ok(format!(
            "umask 077; p={quoted_path}; \
             [ ! -L \"$p\" ] && [ -f \"$p\" ] || exit 73; \
             exec 3>>\"$p\" || exit 73; \
             if fd_actual=$(/usr/bin/stat -f '%i:%u:%l' /dev/fd/3 2>/dev/null); then \
               path_actual=$(/usr/bin/stat -f '%d:%i:%u:%Lp:%l' \"$p\" 2>/dev/null) || exit 73; \
               [ \"$fd_actual\" = '{}:{}:1' ] && [ \"$path_actual\" = '{}:{}:{}:600:1' ] || exit 73; \
             else \
               fd_actual=$(/usr/bin/stat -Lc '%d:%i:%u:%a:%h' /dev/fd/3 2>/dev/null) || exit 73; \
               path_actual=$(/usr/bin/stat -Lc '%d:%i:%u:%a:%h' \"$p\" 2>/dev/null) || exit 73; \
               [ \"$fd_actual\" = '{}:{}:{}:600:1' ] && [ \"$path_actual\" = '{}:{}:{}:600:1' ] || exit 73; \
             fi; \
             exec /bin/cat >&3",
            identity.ino,
            identity.uid,
            identity.dev,
            identity.ino,
            identity.uid,
            identity.dev,
            identity.ino,
            identity.uid,
            identity.dev,
            identity.ino,
            identity.uid
        ))
    }

    #[cfg(not(unix))]
    Ok(format!("umask 077; /bin/cat >> {quoted_path}"))
}

/// Disarm `pipe-pane` on a pane — running `tmux pipe-pane` with no shell
/// command closes the existing pipe. Used when detaching from an external
/// (non-agentum) tmux session so its output stops accumulating in our log.
pub async fn unpipe_pane(target: &str) -> Result<()> {
    let session_id = require_session_id(target).await?;
    let mut c = Command::new("tmux");
    c.arg("pipe-pane").arg("-t").arg(session_id);
    run_checked(&mut c).await
}

/// `tmux list-panes -a -F <format>` raw stdout across every session on the
/// server. Returns `Ok("")` when tmux is not installed or no tmux server is
/// running — for discovery both simply mean "no sessions", not an error.
pub async fn list_panes_all(format: &str) -> Result<String> {
    let out = match Command::new("tmux")
        .args(["list-panes", "-a", "-F", format])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e.into()),
    };
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8(out.stdout)?)
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
    let session_id = require_session_id(target).await?;
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(session_id)
        .arg("#{pane_current_command}")
        .output()
        .await?;
    check(&out)?;
    let s = String::from_utf8(out.stdout)?;
    Ok(s.trim().to_string())
}

/// PID of the foreground process inside the pane.
pub async fn pane_pid(target: &str) -> Result<u32> {
    let session_id = require_session_id(target).await?;
    pane_pid_for_session_id(&session_id).await
}

async fn pane_pid_for_session_id(session_id: &str) -> Result<u32> {
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t"])
        .arg(session_id)
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
    let Some(session_id) = resolve_session_id(target).await? else {
        return Ok(());
    };
    // pane gone between checks → just skip the signal phase.
    let pid = pane_pid_for_session_id(&session_id).await.ok();

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

    kill_session_id_with_tmux(Path::new("tmux"), &session_id).await
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

    fn unique_test_dir(label: &str) -> PathBuf {
        let sequence = STAGED_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "agentum-tmux-{label}-{}.{}",
            std::process::id(),
            sequence
        ))
    }

    fn unique_test_target(label: &str) -> String {
        let sequence = STAGED_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        format!(
            "tmux-test-agentum-{label}-{}-{sequence}",
            std::process::id()
        )
    }

    #[cfg(unix)]
    fn write_test_executable(path: &Path, contents: &str) {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;

        let sequence = STAGED_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staged = path.with_extension(format!("stage-{}-{sequence}", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file.sync_all().unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o700)).unwrap();
        drop(file);
        std::fs::rename(staged, path).unwrap();
    }

    #[test]
    fn target_format() {
        assert_eq!(target_for("alpha"), "agentum-alpha");
    }

    /// Regression for the attach-time output freeze: tmux's `-o` option
    /// toggles a live pane pipe off instead of leaving it armed. Repeated
    /// self-healing calls must keep the sink live and capture a multi-KiB
    /// output burst.
    #[cfg(unix)]
    #[tokio::test]
    async fn pipe_pane_is_idempotent_and_keeps_streaming_output() {
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

        let target = unique_test_target("pipe-idempotent");
        let cleanup = SessionCleanup(target.clone());
        let root = unique_test_dir("pipe-idempotent");
        let log = root.join("pane.log");
        std::fs::create_dir_all(&root).unwrap();
        new_session(&target, &root, &["/bin/sh".into()], &[])
            .await
            .unwrap();
        sleep(Duration::from_millis(100)).await;

        let mut states = Vec::new();
        for _ in 0..3 {
            pipe_pane(&target, &log).await.unwrap();
            let probe = Command::new("tmux")
                .args(["display-message", "-p", "-t"])
                .arg(&target)
                .arg("#{pane_pipe}")
                .output()
                .await
                .unwrap();
            states.push(String::from_utf8_lossy(&probe.stdout).trim().to_string());
        }

        sleep(Duration::from_millis(100)).await;
        send_bytes(&target, b"yes z | head -c 65536\r")
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
        let pane = capture_pane(&target, 10).await.unwrap_or_default();

        drop(cleanup);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(states, ["1", "1", "1"], "a re-arm toggled the pipe off");
        assert!(
            captured >= 64 * 1024,
            "live pipe captured only {captured} bytes after repeated re-arms; pane={pane:?}"
        );
    }

    #[cfg(unix)]
    fn exact_target_fake_tmux(root: &Path) -> (PathBuf, PathBuf) {
        let fake_tmux = root.join("tmux.sh");
        let captured = root.join("mutation-argv");
        let captured_quoted = shlex::try_quote(captured.to_str().unwrap()).unwrap();
        std::fs::create_dir_all(root).unwrap();
        write_test_executable(
            &fake_tmux,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  list-sessions) printf '%s_%s\\n%s_%s\\n' '$11' 'agentum-alpha' '$12' 'distinct-beta' ;;\n  kill-session) printf '%s\\0' \"$@\" > {captured_quoted} ;;\n  *) exit 99 ;;\nesac\n"
            ),
        );
        (fake_tmux, captured)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exact_session_resolver_rejects_prefixes_and_returns_tmux_id() {
        let root = unique_test_dir("exact-resolver");
        let (fake_tmux, _) = exact_target_fake_tmux(&root);

        assert_eq!(
            resolve_session_id_with(&fake_tmux, "agentum-alpha")
                .await
                .unwrap()
                .as_deref(),
            Some("$11")
        );
        assert_eq!(
            resolve_session_id_with(&fake_tmux, "distinct-beta")
                .await
                .unwrap()
                .as_deref(),
            Some("$12")
        );
        assert_eq!(
            resolve_session_id_with(&fake_tmux, "agentum-alph")
                .await
                .unwrap(),
            None,
            "a raw prefix resolved to another session"
        );
        assert_eq!(
            resolve_session_id_with(&fake_tmux, "$11")
                .await
                .unwrap()
                .as_deref(),
            Some("$11"),
            "a valid immutable control target was not accepted"
        );
        for malformed in ["$", "$x", "$11suffix", "$11; kill-server"] {
            assert!(matches!(
                resolve_session_id_with(&fake_tmux, malformed).await,
                Err(TmuxError::Parse(_))
            ));
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kill_session_never_mutates_a_prefix_match() {
        let root = unique_test_dir("exact-kill");
        let (fake_tmux, captured) = exact_target_fake_tmux(&root);

        kill_session_with_tmux(&fake_tmux, "agentum-alph")
            .await
            .unwrap();
        assert!(!captured.exists(), "prefix target reached kill-session");

        kill_session_with_tmux(&fake_tmux, "agentum-alpha")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(&captured).unwrap(),
            [
                b"kill-session\0".as_slice(),
                b"-t\0".as_slice(),
                b"$11\0".as_slice(),
            ]
            .concat(),
            "destructive command did not use the resolved immutable id"
        );

        kill_session_with_tmux(&fake_tmux, "$11").await.unwrap();
        assert!(std::fs::read(&captured).unwrap().ends_with(b"$11\0"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exact_session_resolver_distinguishes_absence_from_tmux_failure() {
        let root = unique_test_dir("exact-resolver-errors");
        std::fs::create_dir_all(&root).unwrap();
        let fake_tmux = root.join("tmux.sh");
        write_test_executable(
            &fake_tmux,
            "#!/bin/sh\nprintf '%s\\n' 'no server running on /tmp/tmux-test/default' >&2\nexit 1\n",
        );
        assert_eq!(
            resolve_session_id_with(&fake_tmux, "absent").await.unwrap(),
            None
        );

        write_test_executable(
            &fake_tmux,
            "#!/bin/sh\nprintf '%s\\n' 'error connecting to /tmp/tmux-test/default (Permission denied)' >&2\nexit 1\n",
        );
        match resolve_session_id_with(&fake_tmux, "absent").await {
            Err(TmuxError::NonZero { status, stderr }) => {
                assert_eq!(status, 1);
                assert!(stderr.contains("Permission denied"));
            }
            other => panic!("unexpected resolver result: {other:?}"),
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn staged_launch_is_private_preserves_env_and_self_cleans() {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_test_dir("private-launch");
        let base = root.join("launches");
        let output = root.join("result");
        std::fs::create_dir_all(&root).unwrap();
        let token = "sentinel token ' \" $HOME $(nope);\nsecond line";
        let cmd = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf '%s' \"$AGENTUM_SECRET_SENTINEL\" > \"$1\"".to_string(),
            "agentum-test".to_string(),
            output.to_string_lossy().into_owned(),
        ];
        let staged = stage_launch_in(
            &base,
            &cmd,
            &[("AGENTUM_SECRET_SENTINEL".to_string(), token.to_string())],
        )
        .unwrap();

        assert_eq!(
            std::fs::metadata(&base).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&staged.launch_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&staged.script_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(!staged.shell_command().unwrap().contains(token));

        let status = std::process::Command::new("/bin/sh")
            .arg(&staged.script_path)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(&output).unwrap(), token);
        assert!(!staged.script_path.exists(), "wrapper must unlink itself");
        assert!(
            !staged.launch_dir.exists(),
            "wrapper directory must be empty"
        );

        drop(staged);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_launch_secret_never_enters_tmux_argv_or_diagnostics() {
        let root = unique_test_dir("argv-sentinel");
        let launch_base = root.join("launches");
        let fake_tmux = root.join("tmux.sh");
        let captured = root.join("argv");
        std::fs::create_dir_all(&root).unwrap();
        let captured_quoted = shlex::try_quote(captured.to_str().unwrap()).unwrap();
        write_test_executable(
            &fake_tmux,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {captured_quoted}\nprintf '%s\\n' \"$@\" >&2\nexit 23\n"
            ),
        );

        let token = "AGENTUM_TOKEN_MUST_NOT_APPEAR_4f4fdf55";
        let error = new_session_with_tmux(
            &fake_tmux,
            &launch_base,
            "sentinel",
            &root,
            &["codex".to_string(), "--flag".to_string()],
            &[("AGENTUM_MCP_BEARER_TOKEN".to_string(), token.to_string())],
        )
        .await
        .unwrap_err();

        let argv = std::fs::read_to_string(&captured).unwrap_or_else(|read_error| {
            panic!("fake tmux did not capture argv ({read_error}); launch error was: {error:?}")
        });
        let diagnostic = error.to_string();
        assert!(!argv.contains(token), "secret leaked in tmux argv: {argv}");
        assert!(
            !diagnostic.contains(token),
            "secret leaked in tmux diagnostic: {diagnostic}"
        );
        assert!(
            !argv.lines().any(|arg| arg == "-e"),
            "local launches must not use tmux -e: {argv}"
        );
        assert!(argv.contains("/bin/sh"));
        assert!(argv.contains("launch.sh"));
        assert!(
            std::fs::read_dir(&launch_base).unwrap().next().is_none(),
            "a failed tmux command must remove the staged secret"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn invalid_environment_name_diagnostic_does_not_echo_value() {
        let token = "AGENTUM_TOKEN_MUST_NOT_APPEAR_5d0396dd";
        let error = render_staged_launch(
            &["codex".to_string()],
            &[("INVALID-NAME".to_string(), token.to_string())],
        )
        .unwrap_err();
        assert!(matches!(error, TmuxError::InvalidEnvironmentName));
        assert!(!error.to_string().contains(token));
    }

    #[cfg(unix)]
    #[test]
    fn pane_log_is_private_before_first_sink_write() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::process::Stdio;

        let root = unique_test_dir("private-pane-log");
        let parent = root.join("sessions");
        let log = parent.join("pane.log");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();

        let identity = prepare_private_pane_log(&log).unwrap();
        let parent_metadata = std::fs::symlink_metadata(&parent).unwrap();
        let log_metadata = std::fs::symlink_metadata(&log).unwrap();
        assert!(parent_metadata.file_type().is_dir());
        assert!(!parent_metadata.file_type().is_symlink());
        assert_eq!(parent_metadata.permissions().mode() & 0o7777, 0o700);
        assert!(log_metadata.file_type().is_file());
        assert!(!log_metadata.file_type().is_symlink());
        assert_eq!(log_metadata.permissions().mode() & 0o7777, 0o600);
        assert_eq!(log_metadata.nlink(), 1);
        assert_eq!(
            (log_metadata.dev(), log_metadata.ino()),
            (identity.dev, identity.ino)
        );

        let sink = pane_log_sink_command(&log, identity).unwrap();
        assert!(sink.starts_with("umask 077;"), "insecure sink: {sink}");
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&sink)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"first private pane bytes\n")
            .unwrap();
        assert!(child.wait().unwrap().success(), "sink failed: {sink}");
        assert_eq!(std::fs::read(&log).unwrap(), b"first private pane bytes\n");
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o7777,
            0o600
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pane_log_repairs_owner_controlled_mode_before_append() {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_test_dir("pane-log-mode");
        let parent = root.join("sessions");
        let log = parent.join("pane.log");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(&log, b"existing\n").unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o775)).unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();

        prepare_private_pane_log(&log).unwrap();

        assert_eq!(
            std::fs::metadata(&parent).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        assert_eq!(std::fs::read(&log).unwrap(), b"existing\n");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pane_log_rejects_symlink_parent_and_target_without_touching_destination() {
        use std::os::unix::fs::symlink;

        let root = unique_test_dir("pane-log-symlinks");
        let outside_dir = root.join("outside");
        let outside_file = root.join("outside.log");
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::fs::write(&outside_file, b"do not touch\n").unwrap();

        let parent_link = root.join("linked-sessions");
        symlink(&outside_dir, &parent_link).unwrap();
        let error = prepare_private_pane_log(&parent_link.join("pane.log")).unwrap_err();
        assert!(matches!(error, TmuxError::Io(_)));
        assert!(!outside_dir.join("pane.log").exists());

        let real_parent = root.join("sessions");
        std::fs::create_dir_all(&real_parent).unwrap();
        let log_link = real_parent.join("pane.log");
        symlink(&outside_file, &log_link).unwrap();
        let error = prepare_private_pane_log(&log_link).unwrap_err();
        assert!(matches!(error, TmuxError::Io(_)));
        assert_eq!(std::fs::read(&outside_file).unwrap(), b"do not touch\n");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pane_log_sink_rejects_replaced_file_before_copying_bytes() {
        use std::process::Stdio;

        let root = unique_test_dir("pane-log-replacement");
        let log = root.join("sessions").join("pane.log");
        let original = root.join("original.log");
        let identity = prepare_private_pane_log(&log).unwrap();
        let sink = pane_log_sink_command(&log, identity).unwrap();

        std::fs::rename(&log, &original).unwrap();
        std::fs::write(&log, b"replacement must remain unchanged\n").unwrap();
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&sink)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"pane bytes must be rejected\n")
            .unwrap();
        assert!(!child.wait().unwrap().success());
        assert_eq!(std::fs::read(&original).unwrap(), b"");
        assert_eq!(
            std::fs::read(&log).unwrap(),
            b"replacement must remain unchanged\n"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pane_log_rejects_hard_link_alias() {
        let root = unique_test_dir("pane-log-hardlink");
        let parent = root.join("sessions");
        let outside = root.join("outside.log");
        let log = parent.join("pane.log");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(&outside, b"outside\n").unwrap();
        std::fs::hard_link(&outside, &log).unwrap();

        let error = prepare_private_pane_log(&log).unwrap_err();
        assert!(matches!(error, TmuxError::Io(_)));
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside\n");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pane_log_sink_rejects_post_prepare_hard_link_alias() {
        use std::process::Stdio;

        let root = unique_test_dir("pane-log-late-hardlink");
        let log = root.join("sessions").join("pane.log");
        let outside = root.join("outside.log");
        let identity = prepare_private_pane_log(&log).unwrap();
        let sink = pane_log_sink_command(&log, identity).unwrap();

        std::fs::rename(&log, &outside).unwrap();
        std::fs::hard_link(&outside, &log).unwrap();
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&sink)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"must not reach hard-linked alias\n")
            .unwrap();
        assert!(!child.wait().unwrap().success());
        assert_eq!(std::fs::read(&outside).unwrap(), b"");

        std::fs::remove_dir_all(&root).unwrap();
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
        let target = unique_test_target("capture");
        let _ = kill_session(&target).await;
        let workdir = std::env::temp_dir();
        new_session(
            &target,
            &workdir,
            &["/bin/sleep".into(), "3600".into()],
            &[],
        )
        .await
        .unwrap();
        // Give tmux a beat to render the pane before capturing.
        sleep(Duration::from_millis(300)).await;
        let snap = capture_pane_ansi(&target).await.unwrap();
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
        kill_session(&target).await.unwrap();
    }

    #[tokio::test]
    async fn lifecycle_smoke() {
        // Skip if tmux isn't available in CI.
        if Command::new("tmux").arg("-V").status().await.is_err() {
            return;
        }
        let target = unique_test_target("smoke");
        let _ = kill_session(&target).await;

        let workdir = std::env::temp_dir();
        new_session(
            &target,
            &workdir,
            &["/bin/sleep".into(), "3600".into()],
            &[],
        )
        .await
        .unwrap();

        assert!(has_session(&target).await.unwrap());
        let pid = pane_pid(&target).await.unwrap();
        assert!(pid > 0);

        kill_session(&target).await.unwrap();
        assert!(!has_session(&target).await.unwrap());
    }
}
