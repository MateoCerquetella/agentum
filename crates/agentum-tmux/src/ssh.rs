//! Host-aware tmux operations + the shared SSH connection builder.
//!
//! The watchdog and the server both need to sample tmux panes on a session's
//! host, which may be `Local` (run tmux directly) or `Ssh` (run tmux over an
//! `ssh` connection). The connection builder ([`ssh_command`]) and the small
//! set of read/poll ops the watchdog uses live here, in the shared lower
//! crate, so the watchdog can be host-aware without depending on
//! `agentum-server` (which depends on the watchdog — the dependency only runs
//! one way).
//!
//! `agentum-server`'s `host_runtime` imports [`ssh_command`] from here rather
//! than re-deriving the argv, so there is a single source of truth for the
//! `ssh` flags (BatchMode / ConnectTimeout / StrictHostKeyChecking / auth
//! handling — key, agent, and SSH_ASKPASS-based password, no external
//! `sshpass`).

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use agentum_core::{Host, HostKind, SshAuth};
use tokio::process::Command;
use tokio::time::timeout;

use crate::{Result, TmuxError};

/// Matches `host_runtime`'s probe budget so a hung remote can't wedge the
/// watchdog's tick loop (`tokio::time::timeout` bounds every SSH round trip).
const SSH_TIMEOUT: Duration = Duration::from_secs(12);

/// Private base dir for ControlMaster sockets: `$XDG_RUNTIME_DIR/agentum-ssh`
/// (preferred — short and user-private on Linux) else `$HOME/.agentum/ssh`.
/// Never the world-writable temp dir: the socket backs an *authenticated* SSH
/// channel, so a hijackable location is a real risk (and macOS's `$TMPDIR` is
/// long enough to blow the unix-socket path cap once `%C` is appended).
fn control_socket_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p.join("agentum-ssh"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".agentum").join("ssh"))
}

/// Which pooled ControlMaster a connection attaches to. Two separate masters per
/// host keep the bursty interactive traffic and the long-lived stream tails from
/// starving each other on one connection's `MaxSessions` channel budget.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SshMux {
    /// No pooling — a fresh TCP+auth connection. Used by the stale-master retry.
    Off,
    /// The interactive master (`cm-`): tmux/git/fs execs, keystrokes, input loop.
    Interactive,
    /// The streaming master (`cms-`): persistent pane-log `tail -f` channels.
    /// One shared connection for ALL a host's tails — so opening the app fires
    /// ONE connection per host instead of one-per-session (which overran sshd's
    /// `MaxStartups` and timed tails out as "[session stream closed]").
    Streaming,
}

/// `ControlPath` template for OpenSSH multiplexing, or `None` when no safe,
/// short-enough socket dir exists (then we skip multiplexing and connect fresh
/// rather than risk a too-long path or an unsafe socket). `prefix` namespaces
/// the socket so the interactive and streaming masters never collide. Created
/// `0700` on demand. `%C` is a fixed-length host+port+user hash; we bail if the
/// expanded path would breach the ~104-byte unix socket cap.
fn control_path_template_for(prefix: &str) -> Option<String> {
    let dir = control_socket_dir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        // recursive(true) ⇒ Ok if it already exists; 0700 keeps it owner-only.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .ok()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir).ok()?;
    }
    let template = dir
        .join(format!("{prefix}-%C"))
        .to_string_lossy()
        .into_owned();
    // `%C` (2 chars) expands to a 40-char hex hash at connect time.
    if template.len() - 2 + 40 > 100 {
        return None;
    }
    Some(template)
}

/// The interactive master path — a named helper kept for the tests.
#[cfg(test)]
fn control_path_template() -> Option<String> {
    control_path_template_for("cm")
}

fn control_path_for(mux: SshMux) -> Option<String> {
    match mux {
        SshMux::Off => None,
        SshMux::Interactive => control_path_template_for("cm"),
        SshMux::Streaming => control_path_template_for("cms"),
    }
}

/// Env var the askpass helper reads the password from. It lives only in the
/// child process environment (same-user-readable) and — unlike `sshpass -p
/// <pw>` — never appears on a command line where `ps` could read it.
const ASKPASS_PW_ENV: &str = "AGENTUM_SSH_ASKPASS_PW";

/// Path to a tiny SSH_ASKPASS helper that prints the password from
/// [`ASKPASS_PW_ENV`] on stdout — OpenSSH's askpass protocol. This is how we
/// feed a password to `ssh` non-interactively *without* the external `sshpass`
/// binary: the stock `ssh` on every modern macOS/Linux runs this helper when
/// `SSH_ASKPASS_REQUIRE=force` is set (OpenSSH 8.4+, 2020). Created `0700` on
/// demand in the same private dir as the ControlMaster sockets. Returns `None`
/// (caller then skips askpass wiring) when no safe dir exists or the write
/// fails — mirroring how [`control_path_template`] degrades.
#[cfg(unix)]
fn askpass_script_path() -> Option<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    let dir = control_socket_dir()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let path = dir.join("askpass.sh");
    // The helper carries no secret itself — it just echoes the env var ssh
    // inherits. `\\n` in the Rust source is a literal backslash-n for printf.
    let script = format!("#!/bin/sh\nprintf '%s\\n' \"${ASKPASS_PW_ENV}\"\n");
    // Skip the write when the helper is already present and current.
    if std::fs::read_to_string(&path)
        .map(|c| c == script)
        .unwrap_or(false)
    {
        return Some(path);
    }
    // Write to a uniquely-named temp then atomically rename into place, so a
    // concurrent ssh always sees either the old helper or the fully-written new
    // one — never a truncated/half-written file. The unique suffix keeps two
    // concurrent writers from clobbering each other's temp.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        "askpass.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o700)
        .open(&tmp)
        .ok()?;
    f.write_all(script.as_bytes()).ok()?;
    f.sync_all().ok()?;
    drop(f);
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// Non-unix: the askpass helper is a POSIX shell script, so SSH_ASKPASS-based
/// password auth isn't wired there (`sshpass` was essentially never present on
/// Windows either). The caller falls back to a plain `ssh` with no helper.
#[cfg(not(unix))]
fn askpass_script_path() -> Option<PathBuf> {
    None
}

/// Build the `ssh` argv for running `script` on `host`. Returns a plain tokio
/// [`Command`]; the caller drives `.output()` / `.status()`.
///
/// This is the single source of truth for our SSH connection flags. Password
/// auth is fed through OpenSSH's own SSH_ASKPASS helper (see
/// [`askpass_script_path`]) rather than an external `sshpass` binary, so the
/// watchdog (which can't depend on the server) and the server share one
/// builder and password hosts need nothing installed.
pub fn ssh_command(host: &Host, script: &str) -> Command {
    ssh_command_opts(host, script, SshMux::Interactive)
}

/// Like [`ssh_command`] but selects which pooled master (or none) the
/// connection uses. [`ssh_output`]'s retry rebuilds with [`SshMux::Off`] so a
/// stale/racing pooled master (broken-pipe / "failed to connect to new control
/// master") can't keep failing an op — the replay connects fresh instead.
pub fn ssh_command_opts(host: &Host, script: &str, mux: SshMux) -> Command {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        auth,
    } = &host.kind
    else {
        return Command::new("false");
    };

    // Password auth feeds the secret through OpenSSH's SSH_ASKPASS helper (set
    // up in the `match auth` block below), so ssh must actually prompt for it —
    // that requires BatchMode=no (BatchMode=yes suppresses the prompt entirely).
    // Key/agent auth keeps BatchMode=yes so it never blocks waiting on a prompt.
    let password = matches!(
        auth,
        SshAuth::Password { password } if !password.trim().is_empty()
    );

    // Always invoke `ssh` directly now — no external `sshpass`.
    let mut cmd = Command::new("ssh");

    cmd.arg("-o")
        .arg(if password {
            "BatchMode=no"
        } else {
            "BatchMode=yes"
        })
        .arg("-o")
        .arg("ConnectTimeout=8")
        .arg("-o")
        .arg("ConnectionAttempts=1")
        .arg("-o")
        .arg("ServerAliveInterval=5")
        .arg("-o")
        // CountMax=3 (≈15s grace), not 1: with ControlMaster pooling a single
        // missed keepalive used to tear down the *shared* master on any transient
        // stall, orphaning its socket — the next op then hit "read from master
        // failed: Broken pipe" / "Failed to connect to new control master" and
        // ssh exited 255. Tolerating a few missed beats keeps the master alive;
        // ssh_output still retries unmultiplexed if it dies anyway.
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        // GSSAPI is never used for these hosts, but when the server advertises
        // it the client-side attempt can stall a cold connect for seconds
        // (Kerberos/DNS lookups). Disabling it shaves that off every handshake.
        .arg("GSSAPIAuthentication=no")
        .arg("-o")
        // TCPKeepAlive: surface a dead peer (slept laptop, dropped link) at the
        // TCP layer, not only via the app-level ServerAlive probe. A pooled
        // master whose far end vanished is then reset by the OS rather than
        // lingering as a half-open socket the next op stalls on. Cheap, and it
        // pairs with the ssh_output stale-master retry to keep reconnects clean.
        .arg("TCPKeepAlive=yes")
        // Compression: terminal/agent output is highly repetitive (redraw
        // escape sequences, whitespace, repeated status lines) and compresses
        // ~10x. On a bandwidth-limited link (e.g. a Tailscale-relayed host at
        // ~0.6 MB/s) that is the difference between ~0.5 MB/s and ~6 MB/s of
        // effective pane throughput — an ~11x win measured against Freebee. The
        // gzip CPU cost is trivial at these data rates, and agentum's SSH
        // traffic (pane streams, git, small execs) is all compressible and
        // never the fast-link bulk transfer where compression would hurt.
        .arg("-o")
        .arg("Compression=yes")
        .arg("-p")
        .arg(port.to_string());

    // ControlMaster connection pooling: the first op authenticates and opens a
    // master socket; subsequent ops within ControlPersist reuse it, skipping the
    // TCP+auth handshake entirely — the big remote-latency win (each tmux/git/fs
    // round trip would otherwise pay a full SSH handshake). Applies to both
    // key/agent and password (sshpass) auth. Only enabled when we have a private,
    // short-enough socket dir; otherwise we connect fresh rather than risk a
    // too-long ControlPath (ssh exits 255) or an unsafe socket location.
    if let Some(control_path) = control_path_for(mux) {
        cmd.arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg(format!("ControlPath={control_path}"))
            .arg("-o")
            // 10m, not 30s: the master idles cheaply (one ssh process, a
            // keepalive every 5s), but re-establishing it costs a full
            // TCP+auth handshake — 1-3s on WAN. 30s expired between normal
            // user interactions, so nearly every sidebar click paid that
            // handshake again. ssh_output's unmuxed retry still covers a
            // master that dies mid-window.
            .arg("ControlPersist=600s");
    }

    match auth {
        SshAuth::Key { path } if !path.trim().is_empty() => {
            cmd.arg("-i").arg(path);
            // Pin auth to THIS key. Without IdentitiesOnly, ssh first offers
            // every ssh-agent/default identity; on a server with a low
            // `MaxAuthTries` that burns the attempt budget before our key is
            // tried and the connection is refused ("Too many authentication
            // failures"). Forcing the publickey method also stops a failed key
            // from falling through to a method that can't succeed under
            // BatchMode. Agent auth deliberately omits both (it must let the
            // agent offer its keys).
            cmd.arg("-o")
                .arg("IdentitiesOnly=yes")
                .arg("-o")
                .arg("PreferredAuthentications=publickey");
        }
        SshAuth::Password { password } if !password.trim().is_empty() => {
            // Force password auth so ssh doesn't silently try a key first
            // (which would bypass the askpass prompt and fail confusingly).
            cmd.arg("-o")
                .arg("PreferredAuthentications=password")
                .arg("-o")
                .arg("PubkeyAuthentication=no");
            // Feed the password via OpenSSH's SSH_ASKPASS protocol: ssh runs the
            // helper and reads the password from its stdout. `force` makes ssh
            // use the helper even when a tty is present (OpenSSH 8.4+). The
            // secret travels in the child env, never on the argv.
            if let Some(askpass) = askpass_script_path() {
                cmd.env("SSH_ASKPASS", &askpass)
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env(ASKPASS_PW_ENV, password);
                // Pre-8.4 ssh only consults askpass when DISPLAY is set; a
                // placeholder is harmless (our helper never touches X).
                if std::env::var_os("DISPLAY").is_none() {
                    cmd.env("DISPLAY", ":0");
                }
            }
        }
        _ => {}
    }

    cmd.arg(format!("{user}@{hostname}")).arg(script);
    cmd
}

/// Build an `ssh -O forward -R …` control command that adds a **reverse** port
/// forward to the host's *already-established* interactive ControlMaster — no
/// new connection. On the host, `127.0.0.1:<host_port>` then tunnels back to the
/// Mac's `127.0.0.1:<mac_port>` (the embedded agentum MCP server). Bound to
/// loopback on BOTH ends, so it's never exposed to either machine's network.
///
/// Returns `None` for a non-SSH host or when no ControlPath is available (the
/// master must exist first — warm it via the normal path). `-O forward` matches
/// the running master purely by the `%C` ControlPath hash, so only the host
/// identity (`-p`, `user@host`) needs to agree with how the master was opened.
pub fn ssh_control_forward_cmd(host: &Host, host_port: u16, mac_port: u16) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(SshMux::Interactive)?;
    // Explicit loopback bind on the host side (the leading `127.0.0.1:`); without
    // it a host with `GatewayPorts yes` could bind the wildcard and expose the
    // tunnel to the host's network. Mac side is loopback too.
    let spec = format!("127.0.0.1:{host_port}:127.0.0.1:{mac_port}");
    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("forward")
        .arg("-R")
        .arg(spec)
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// `ssh -O cancel -R 127.0.0.1:<host_port>` — tears down a reverse forward on the
/// host's master, matched by its loopback bind side only (so it cancels a stale
/// forward regardless of what Mac port it used to target). Best-effort cleanup
/// run before re-arming, so the tunnel always points at the *current* Mac port
/// and a leftover forward from a prior app instance can't block the new bind.
pub fn ssh_control_cancel_cmd(host: &Host, host_port: u16) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(SshMux::Interactive)?;
    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("cancel")
        .arg("-R")
        .arg(format!("127.0.0.1:{host_port}"))
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// Build an `ssh -O forward -L …` control command that adds a **local** port
/// forward to the host's *already-established* interactive ControlMaster — no
/// new connection. On the Mac, `127.0.0.1:<mac_port>` then tunnels to the host's
/// `127.0.0.1:<host_port>` (where headless Chromium binds its CDP debugger).
/// This is the mirror of [`ssh_control_forward_cmd`]'s reverse (-R) forward:
/// the MCP server lives on the Mac (reverse), but the browser/CDP lives on the
/// host, so the Mac reaches it with a forward (-L). Bound to loopback on BOTH
/// ends, so it's never exposed to either machine's network.
///
/// Returns `None` for a non-SSH host or when no ControlPath is available (the
/// master must exist first — warm it via the normal path). `-O forward` matches
/// the running master purely by the `%C` ControlPath hash, so only the host
/// identity (`-p`, `user@host`) needs to agree with how the master was opened.
pub fn ssh_control_local_forward_cmd(
    host: &Host,
    mac_port: u16,
    host_port: u16,
) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(SshMux::Interactive)?;
    // Explicit `127.0.0.1:` listen bind on the Mac side so the forwarded port is
    // never offered on the Mac's network; the host side is loopback too (that's
    // where Chromium's CDP binds, via `--remote-debugging-address=127.0.0.1`).
    let spec = format!("127.0.0.1:{mac_port}:127.0.0.1:{host_port}");
    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("forward")
        .arg("-L")
        .arg(spec)
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// `ssh -O cancel -L 127.0.0.1:<mac_port>:127.0.0.1:<host_port>` — tears down a
/// local forward on the host's master. Unlike the reverse [`ssh_control_cancel_cmd`]
/// (which cancels by listen side alone), OpenSSH rejects a listen-side-only
/// `-O cancel -L` ("Bad local forwarding specification"); a local-forward cancel
/// must pass the SAME full spec used to arm it (verified on OpenSSH 10.0p2).
/// Best-effort cleanup run before re-arming so a re-attach refreshes the forward
/// instead of colliding with its own still-present one.
pub fn ssh_control_local_cancel_cmd(host: &Host, mac_port: u16, host_port: u16) -> Option<Command> {
    let HostKind::Ssh {
        user,
        hostname,
        port,
        ..
    } = &host.kind
    else {
        return None;
    };
    let control_path = control_path_for(SshMux::Interactive)?;
    let spec = format!("127.0.0.1:{mac_port}:127.0.0.1:{host_port}");
    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg(format!("ControlPath={control_path}"))
        .arg("-p")
        .arg(port.to_string())
        .arg("-O")
        .arg("cancel")
        .arg("-L")
        .arg(spec)
        .arg(format!("{user}@{hostname}"));
    Some(cmd)
}

/// True when ssh's stderr says the *pooled ControlMaster* socket was stale or
/// racing (the shared master died mid-op), not that the remote command failed.
/// Such a failure happens at the multiplex layer *before* the script runs, so
/// the op never executed remotely and is safe to replay on a fresh, unpooled
/// connection — even a mutating one (e.g. `git worktree add`).
pub fn is_mux_transport_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("mux_client")              // mux_client_request_session: …
        || s.contains("control master")  // Failed to connect to new control master
        || s.contains("controlpath")
        || s.contains("read from master") // read from master failed: Broken pipe
        || s.contains("multiplexing")
}

/// Run `script` over SSH with `.output()`, bounded by `dur`, transparently
/// retrying ONCE on a stale/racing-ControlMaster transport failure with
/// multiplexing disabled. The pooled master can die mid-flight (keepalive
/// timeout on a transient stall, or its ControlPersist window expiring exactly
/// as a new op connects), leaving a dead socket; the next op then exits 255 at
/// the mux layer *without having run the remote script*, which makes a replay
/// on a fresh connection safe. Returns the raw [`Output`] so each caller keeps
/// its own non-zero-exit semantics; only transport/timeout failures are `Err`.
///
/// [`Output`]: std::process::Output
pub async fn ssh_output(
    host: &Host,
    script: &str,
    dur: Duration,
) -> std::io::Result<std::process::Output> {
    ssh_output_on(host, script, dur, SshMux::Interactive).await
}

/// Like [`ssh_output`] but rides the caller-chosen pooled ControlMaster.
///
/// Why this exists: the watchdog's per-session pane sample ([`sample_pane`]) is
/// an `ssh` exec every tick. Riding the *interactive* master (`cm-`) meant those
/// execs shared one TCP connection's channel budget with — and starved — the
/// keystroke writer and other interactive execs. With several remote sessions
/// open that contention throttled typing AND pane output (the remote tmux server
/// was also busy servicing N `capture-pane`s/tick). Routing the sample onto the
/// *streaming* master (`cms-`, otherwise just long-lived `tail -f` channels)
/// takes it off the keystroke path entirely. Same stale-master retry as
/// [`ssh_output`]: a 255 mux-transport failure replays once unmultiplexed.
pub async fn ssh_output_on(
    host: &Host,
    script: &str,
    dur: Duration,
    mux: SshMux,
) -> std::io::Result<std::process::Output> {
    let first = run_ssh_once(host, script, dur, mux).await?;
    if first.status.code() == Some(255) {
        let stderr = String::from_utf8_lossy(&first.stderr);
        if is_mux_transport_error(&stderr) {
            return run_ssh_once(host, script, dur, SshMux::Off).await;
        }
    }
    Ok(first)
}

/// One `.output()` attempt, bounded by `dur`. A timeout surfaces as an
/// `io::Error` of kind `TimedOut` so callers can map it to their own variant.
async fn run_ssh_once(
    host: &Host,
    script: &str,
    dur: Duration,
    mux: SshMux,
) -> std::io::Result<std::process::Output> {
    match timeout(dur, ssh_command_opts(host, script, mux).output()).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "ssh timed out",
        )),
    }
}

/// shell-quote `s`, mapping a quoting failure to [`TmuxError::Quote`].
fn q(s: &str) -> Result<Cow<'_, str>> {
    shlex::try_quote(s).map_err(|_| TmuxError::Quote)
}

/// Run `script` over SSH and return its stdout, erroring on a non-zero exit
/// or timeout. Mirrors `host_runtime::ssh_stdout`.
async fn ssh_stdout(host: &Host, script: &str) -> Result<String> {
    let output = ssh_output(host, script, SSH_TIMEOUT)
        .await
        .map_err(TmuxError::Io)?;
    if !output.status.success() {
        return Err(TmuxError::NonZero {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Run `script` over SSH for its exit status only, erroring on transport
/// failure or timeout (a non-zero remote exit is reported via the bool/`()`
/// the caller maps from `success`).
async fn ssh_status(host: &Host, script: &str) -> Result<std::process::ExitStatus> {
    Ok(ssh_output(host, script, SSH_TIMEOUT)
        .await
        .map_err(TmuxError::Io)?
        .status)
}

/// Run `script` over SSH and error on a non-zero exit (or transport
/// failure / timeout). Mirrors `host_runtime::ssh_checked`.
async fn ssh_checked(host: &Host, script: &str) -> Result<()> {
    let output = ssh_output(host, script, SSH_TIMEOUT)
        .await
        .map_err(TmuxError::Io)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(TmuxError::NonZero {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

// ───────────────────────── host-aware tmux ops ─────────────────────────
// Each branches on `host.kind`: Local calls the existing `crate::<fn>`
// (identical behaviour to before the refactor); Ssh runs the same tmux
// command wrapped in `sh -c` (the remote login shell may be fish/zsh, which
// reject the POSIX `for`/`case`/quoting the SSH branches build).

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

    fn ssh_host(auth: SshAuth) -> Host {
        Host {
            id: agentum_core::LOCAL_HOST_ID,
            name: "t".into(),
            kind: HostKind::Ssh {
                user: "me".into(),
                hostname: "box.local".into(),
                port: 2222,
                auth,
            },
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
            last_seen_at: None,
        }
    }

    // `ssh_command` returns a tokio Command; `.as_std()` exposes the inner
    // std Command for introspecting program + args.
    fn arg_strings(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    // Env vars explicitly set on the Command (vars only cleared/inherited are
    // skipped) — lets the tests assert the SSH_ASKPASS wiring. Only the askpass
    // tests (unix-gated) use it, so gate it too to avoid a dead-code warning on
    // Windows.
    #[cfg(unix)]
    fn env_map(cmd: &Command) -> std::collections::HashMap<String, String> {
        cmd.as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    #[test]
    fn ssh_command_key_uses_plain_ssh_with_batchmode() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
        let args = arg_strings(&cmd);
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.iter().any(|a| a == "me@box.local"));
        // Key/agent must never reach for sshpass options.
        assert!(!args.contains(&"PreferredAuthentications=password".to_string()));
    }

    /// ControlMaster pooling must be present on every connection so repeated
    /// remote ops reuse one authenticated socket. Shared assertion for both auth
    /// paths (key/agent and password).
    fn assert_control_master(args: &[String]) {
        assert!(
            args.contains(&"ControlMaster=auto".to_string()),
            "missing ControlMaster=auto: {args:?}"
        );
        assert!(
            args.contains(&"ControlPersist=600s".to_string()),
            "missing ControlPersist=600s: {args:?}"
        );
        let control_path = args
            .iter()
            .find(|a| a.starts_with("ControlPath="))
            .unwrap_or_else(|| panic!("missing ControlPath=: {args:?}"));
        // `%C` is a fixed-length host+port+user hash; keep the socket name short
        // so the unix socket path stays under the ~104-char cap.
        assert!(
            control_path.ends_with("cm-%C"),
            "unexpected ControlPath: {control_path}"
        );
        // The socket dir must exist (we create it on demand) — strip the
        // `ControlPath=` prefix and the `/cm-%C` leaf.
        let path = control_path.trim_start_matches("ControlPath=");
        let dir = std::path::Path::new(path).parent().expect("control dir");
        assert!(dir.is_dir(), "control dir not created: {}", dir.display());
    }

    #[test]
    fn ssh_command_key_enables_control_master_pooling() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert_control_master(&arg_strings(&cmd));
    }

    #[test]
    fn ssh_command_no_mux_omits_control_master() {
        // The retry connection must NOT reuse the (stale) pooled socket, so the
        // ControlMaster flags are absent for `SshMux::Off`.
        let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", SshMux::Off);
        let args = arg_strings(&cmd);
        assert!(
            !args.iter().any(|a| a == "ControlMaster=auto"),
            "ControlMaster must be off on the unmultiplexed retry: {args:?}"
        );
        assert!(!args.iter().any(|a| a.starts_with("ControlPath=")));
    }

    #[test]
    fn streaming_and_interactive_masters_use_distinct_sockets() {
        // Tails (Streaming) must pool on a SEPARATE socket from interactive ops
        // so they don't share one connection's MaxSessions channel budget.
        let path_of = |mux| {
            arg_strings(&ssh_command_opts(&ssh_host(SshAuth::Agent), "x", mux))
                .into_iter()
                .find(|a| a.starts_with("ControlPath="))
        };
        let interactive = path_of(SshMux::Interactive).expect("interactive path");
        let streaming = path_of(SshMux::Streaming).expect("streaming path");
        assert_ne!(interactive, streaming, "masters share a socket");
        assert!(
            interactive.contains("/cm-"),
            "interactive not cm-: {interactive}"
        );
        assert!(
            streaming.contains("/cms-"),
            "streaming not cms-: {streaming}"
        );
    }

    #[test]
    fn keepalive_tolerates_a_few_missed_beats() {
        // CountMax=1 orphaned the shared master on any transient stall; 3 gives
        // ~15s grace so the pooled socket survives a blip.
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert!(arg_strings(&cmd).contains(&"ServerAliveCountMax=3".to_string()));
    }

    #[test]
    fn ssh_command_enables_compression_for_throughput() {
        // Pane streams compress ~10x; on a bandwidth-limited link that is an
        // ~11x effective-throughput win. Every mux mode must carry it.
        for mux in [SshMux::Off, SshMux::Interactive, SshMux::Streaming] {
            let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", mux);
            assert!(
                arg_strings(&cmd).contains(&"Compression=yes".to_string()),
                "compression missing (mux={mux:?})"
            );
        }
    }

    #[test]
    fn ssh_command_key_pins_to_that_identity() {
        // An explicit key must pin auth to THAT key. IdentitiesOnly stops ssh
        // from offering every ssh-agent/default key BEFORE the configured one —
        // which trips a hardened server's `MaxAuthTries` and gets the whole
        // connection refused ("Too many authentication failures") before our key
        // is ever tried. Forcing the publickey method keeps a failed key from
        // silently falling through to a method that can't work under BatchMode.
        let cmd = ssh_command(
            &ssh_host(SshAuth::Key {
                path: "/home/me/.ssh/id_ed25519".into(),
            }),
            "echo hi",
        );
        let args = arg_strings(&cmd);
        assert!(
            args.contains(&"IdentitiesOnly=yes".to_string()),
            "key auth must set IdentitiesOnly=yes: {args:?}"
        );
        assert!(
            args.contains(&"PreferredAuthentications=publickey".to_string()),
            "key auth must force the publickey method: {args:?}"
        );
        // The configured key is still passed.
        assert!(args.iter().any(|a| a == "/home/me/.ssh/id_ed25519"));
    }

    #[test]
    fn ssh_command_agent_omits_identities_only() {
        // IdentitiesOnly=yes with no `-i` would stop ssh-agent keys from being
        // offered at all, breaking agent auth. The flag belongs ONLY on the
        // explicit-key path.
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        assert!(
            !arg_strings(&cmd).contains(&"IdentitiesOnly=yes".to_string()),
            "agent auth must not pin IdentitiesOnly (would suppress agent keys)"
        );
    }

    #[test]
    fn ssh_command_enables_tcp_keepalive() {
        // Detect a dead peer (slept laptop, dropped link) at the TCP layer, not
        // only via the app-level ServerAlive probe — so a stale pooled master is
        // torn down and replaced instead of lingering. Every mux mode carries it.
        for mux in [SshMux::Off, SshMux::Interactive, SshMux::Streaming] {
            let cmd = ssh_command_opts(&ssh_host(SshAuth::Agent), "echo hi", mux);
            assert!(
                arg_strings(&cmd).contains(&"TCPKeepAlive=yes".to_string()),
                "TCPKeepAlive missing (mux={mux:?})"
            );
        }
    }

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

    #[test]
    fn detects_stale_control_master_stderr() {
        // The exact stderr from the reported `/api/fs/list` 400.
        let bug = "mux_client_request_session: read from master failed: Broken pipe\r\n\
                   Failed to connect to new control master";
        assert!(is_mux_transport_error(bug));
        // An ordinary remote failure must NOT trigger a replay (it really ran).
        assert!(!is_mux_transport_error("not a directory: /home/x/nope"));
        assert!(!is_mux_transport_error("Permission denied (publickey)."));
    }

    /// Password auth feeds the secret through OpenSSH's own SSH_ASKPASS helper
    /// (no external `sshpass`): plain `ssh`, the password method forced, and the
    /// secret carried in the child env — never on the argv, where `ps` could
    /// read it (a strict improvement over `sshpass -p <pw>`).
    #[cfg(unix)]
    #[test]
    fn ssh_command_password_uses_askpass_not_sshpass() {
        let cmd = ssh_command(
            &ssh_host(SshAuth::Password {
                password: "s3cret".into(),
            }),
            "echo hi",
        );
        // Plain ssh now — sshpass is gone.
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
        let args = arg_strings(&cmd);
        assert!(
            !args.iter().any(|a| a.contains("sshpass")),
            "must not shell through sshpass: {args:?}"
        );
        // BatchMode=no so ssh actually prompts (firing the askpass helper);
        // force the password method so it never silently tries a key first.
        assert!(args.contains(&"BatchMode=no".to_string()));
        assert!(!args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"PreferredAuthentications=password".to_string()));
        assert!(args.contains(&"PubkeyAuthentication=no".to_string()));
        assert!(args.iter().any(|a| a == "me@box.local"));
        // The password rides in the env for the askpass helper — NOT in argv.
        assert!(
            !args.iter().any(|a| a == "s3cret"),
            "password must never reach the argv: {args:?}"
        );
        let envs = env_map(&cmd);
        assert_eq!(
            envs.get("SSH_ASKPASS_REQUIRE").map(String::as_str),
            Some("force")
        );
        assert_eq!(envs.get(ASKPASS_PW_ENV).map(String::as_str), Some("s3cret"));
        let askpass = envs
            .get("SSH_ASKPASS")
            .expect("SSH_ASKPASS set for password auth");
        assert!(
            askpass.ends_with("askpass.sh"),
            "unexpected askpass path: {askpass}"
        );
        // Pooling still applies on the password path.
        assert_control_master(&args);
    }

    /// Key/agent auth must NOT wire up the askpass env (it never prompts, so
    /// there is no password to feed).
    #[cfg(unix)]
    #[test]
    fn ssh_command_key_sets_no_askpass_env() {
        let cmd = ssh_command(&ssh_host(SshAuth::Agent), "echo hi");
        let envs = env_map(&cmd);
        assert!(
            !envs.contains_key("SSH_ASKPASS"),
            "key/agent auth must not set SSH_ASKPASS"
        );
        assert!(!envs.contains_key(ASKPASS_PW_ENV));
    }

    /// The askpass helper is written executable (0700) and idempotently — a
    /// static POSIX script that echoes the password env var to ssh on stdout.
    #[cfg(unix)]
    #[test]
    fn askpass_script_written_executable_and_idempotent() {
        use std::os::unix::fs::PermissionsExt;
        let path = askpass_script_path().expect("askpass path available with HOME set");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.starts_with("#!/bin/sh"),
            "missing shebang: {content}"
        );
        assert!(
            content.contains(ASKPASS_PW_ENV),
            "script must echo the pw env var: {content}"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "askpass must be owner-only executable");
        // Idempotent: a second call returns the same path without error.
        assert_eq!(askpass_script_path(), Some(path));
    }

    #[test]
    fn control_path_template_creates_dir_idempotently() {
        // Calling twice must not panic even though the dir already exists.
        let first = control_path_template();
        let second = control_path_template();
        assert_eq!(first, second);
        // HOME is set in the test environment, so a path is available.
        let path = first.expect("control path available with HOME set");
        assert!(path.ends_with("cm-%C"));
        let dir = std::path::Path::new(&path).parent().expect("dir");
        assert!(dir.is_dir());
    }

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

    #[test]
    fn local_forward_cmd_builds_dash_l_to_host_loopback() {
        // CDP lives on the host, so the Mac reaches it via a *local* (-L)
        // forward: the Mac listens on `mac_port` and ssh tunnels to the host's
        // `127.0.0.1:host_port`. Mirror of the reverse (-R) MCP tunnel.
        let cmd = ssh_control_local_forward_cmd(&ssh_host(SshAuth::Agent), 7000, 9222)
            .expect("ssh host yields a command");
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "ssh");
        let args = arg_strings(&cmd);
        assert!(args.contains(&"-O".to_string()), "missing -O: {args:?}");
        assert!(
            args.contains(&"forward".to_string()),
            "missing forward: {args:?}"
        );
        assert!(args.contains(&"-L".to_string()), "missing -L: {args:?}");
        // listen on Mac loopback:mac_port → host loopback:host_port.
        assert!(
            args.contains(&"127.0.0.1:7000:127.0.0.1:9222".to_string()),
            "wrong -L spec: {args:?}"
        );
        // Must NOT be a reverse forward.
        assert!(
            !args.contains(&"-R".to_string()),
            "-L builder emitted -R: {args:?}"
        );
        // Rides the warm Interactive master (cm-, not the streaming cms-).
        let control_path = args
            .iter()
            .find(|a| a.starts_with("ControlPath="))
            .expect("ControlPath present");
        assert!(
            control_path.contains("/cm-"),
            "not interactive master: {control_path}"
        );
        assert!(
            !control_path.contains("/cms-"),
            "must not use streaming master: {control_path}"
        );
        // Host identity must agree with how the master was opened.
        assert!(
            args.iter().any(|a| a == "2222"),
            "host port missing: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "me@box.local"),
            "user@host missing: {args:?}"
        );
    }

    #[test]
    fn local_cancel_cmd_uses_full_local_forward_spec() {
        // OpenSSH rejects a listen-side-only `-O cancel -L` ("Bad local
        // forwarding specification") — unlike `-R`, a local-forward cancel needs
        // the SAME full spec used to arm it. Verified against OpenSSH 10.0p2.
        let cmd = ssh_control_local_cancel_cmd(&ssh_host(SshAuth::Agent), 7000, 9222)
            .expect("ssh host yields a command");
        let args = arg_strings(&cmd);
        assert!(args.contains(&"-O".to_string()), "missing -O: {args:?}");
        assert!(
            args.contains(&"cancel".to_string()),
            "missing cancel: {args:?}"
        );
        assert!(args.contains(&"-L".to_string()), "missing -L: {args:?}");
        assert!(
            args.contains(&"127.0.0.1:7000:127.0.0.1:9222".to_string()),
            "cancel must pass the full local-forward spec: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "me@box.local"),
            "user@host missing: {args:?}"
        );
    }

    #[test]
    fn local_forward_and_cancel_none_for_local_host() {
        // No tunnel for a local host — there is no ssh master to attach to.
        assert!(ssh_control_local_forward_cmd(&local_host(), 7000, 9222).is_none());
        assert!(ssh_control_local_cancel_cmd(&local_host(), 7000, 9222).is_none());
    }
}
