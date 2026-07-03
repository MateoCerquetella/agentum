//! SSH reverse/forward tunnel management.
use super::*;

/// How long a muxed health probe waits before declaring a pooled master wedged.
/// A cold-but-healthy connect pays TCP+auth (~1–3s, ConnectTimeout=8), so 10s
/// clears a legitimate fresh open; a master whose TCP has silently died answers
/// neither (its own ServerAlive takes ~15s to notice), so the probe times out
/// and we evict rather than let the next op attach and hang.
const MASTER_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Evict a pooled ControlMaster (`ssh -O exit`) — best-effort, bounded. Talks to
/// the master over its local control socket, so it reaps even a TCP-dead master.
/// A missing socket just exits non-zero; either way the next `ControlMaster=auto`
/// op opens a clean master instead of attaching to a wedged/orphaned one.
pub async fn evict_ssh_master(host: &Host, mux: SshMux) {
    let Some(mut cmd) = ssh_control_exit_cmd(host, mux) else {
        return;
    };
    // 5s is generous for a local-socket request; bound it so a truly stuck ssh
    // process can't wedge the warmer/reestablish path that called us.
    let _ = timeout(Duration::from_secs(5), cmd.output()).await;
}

/// Probe one pooled master with a bounded, **muxed** no-op and evict it ONLY if
/// it is genuinely dead. Unlike [`ssh_output`] (which retries unmuxed and so
/// *masks* a dead master), this rides only the pooled socket.
///
/// Eviction is deliberately conservative — a healthy master may be SHARED with a
/// concurrent agentum instance, and reaping it out from under that instance is
/// worse than leaving a rare stale one:
///   * clean success → healthy (or freshly opened by this probe) → keep;
///   * timeout / spawn error → the far end is gone (wedged TCP) → evict;
///   * non-zero WITH a mux-transport error (broken pipe / "failed to connect to
///     new control master") → the master socket is dead → evict;
///   * any other non-zero (a `MaxSessions` channel refusal on a BUSY master, a
///     connect timeout to a genuinely-down host with no socket to reap) → leave
///     it: the master is alive-but-busy, or there is nothing to evict anyway.
async fn probe_or_evict_master(host: &Host, mux: SshMux) {
    match timeout(
        MASTER_PROBE_TIMEOUT,
        ssh_command_opts(host, "true", mux).output(),
    )
    .await
    {
        Ok(Ok(out)) if out.status.success() => {}
        Ok(Ok(out)) => {
            if is_mux_transport_error(&String::from_utf8_lossy(&out.stderr)) {
                evict_ssh_master(host, mux).await;
            }
        }
        // Timed out attaching (wedged master) or the ssh process failed to spawn.
        Ok(Err(_)) | Err(_) => evict_ssh_master(host, mux).await,
    }
}

/// Open (or refresh) BOTH pooled SSH masters for `host`, **evicting a wedged one
/// first**. The boot-time/periodic warmer calls this so interactive remote ops
/// AND the first stream tail find a *live* master instead of attaching to a dead
/// pooled socket (an orphan from a prior app instance, or one whose TCP died on
/// a slept laptop / network change) and hanging — the "SSH never reconnects
/// after a while or an app reload" freeze.
///
/// Each master is first health-probed on its own pooled socket; only a probe
/// *failure* evicts, so a healthy master (including one shared with a concurrent
/// agentum instance) is preserved and not churned. After the probe, both are
/// warmed fresh so a cold or just-evicted host still ends hot. No-op for local
/// hosts; the streaming leg is best-effort — only interactive failure is fatal.
pub async fn warm_ssh_master(host: &Host) -> Result<()> {
    if !matches!(host.kind, HostKind::Ssh { .. }) {
        return Ok(());
    }
    // Reap a wedged interactive/streaming master before warming (only a
    // genuinely-dead one — see `probe_or_evict_master`). Concurrent: one
    // master's probe must not gate the other's.
    tokio::join!(
        probe_or_evict_master(host, SshMux::Interactive),
        probe_or_evict_master(host, SshMux::Streaming),
    );
    // Warm both fresh: the probe already reopened a healthy master (its `true`
    // ran over it) or evicted a wedged one that this reopens. Interactive rides
    // `ssh_output` (unmuxed retry) so a racing reopen still lands; streaming is
    // best-effort.
    let stream_warm = ssh_command_opts(host, "true", SshMux::Streaming).output();
    let (interactive, _) = tokio::join!(ssh_output(host, "true", SSH_TIMEOUT), stream_warm);
    interactive.map_err(map_ssh_io)?;
    Ok(())
}

/// First port of the loopback range scanned for the reverse tunnel on a host.
pub const REMOTE_MCP_PORT_BASE: u16 = 8990;
/// How many consecutive ports to try before giving up (host services or stale
/// forwards may already hold some).
pub(crate) const REMOTE_MCP_PORT_TRIES: u16 = 24;

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
pub(crate) fn forward_tunnel_ports() -> std::ops::Range<u16> {
    REMOTE_CDP_PORT_BASE..REMOTE_CDP_PORT_BASE.saturating_add(REMOTE_CDP_PORT_TRIES)
}

/// Did an `ssh -O forward -L` attempt bind the Mac port? A clean exit means
/// bound; ssh reporting the forward as already established is an idempotent
/// success (cancel-then-arm can race a still-present forward). Any other
/// non-zero exit means the port is unusable → scan the next. Mirrors the
/// reverse-tunnel predicate in [`ensure_reverse_tunnel`].
pub(crate) fn forward_arm_bound(status_success: bool, stderr: &str) -> bool {
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
