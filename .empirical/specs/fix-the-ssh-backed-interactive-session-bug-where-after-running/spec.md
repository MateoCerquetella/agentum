# Fix The Ssh Backed Interactive Session Bug Where After Running

## Request

> Fix the SSH-backed interactive session bug where, after running for a while, the connection appears stalled until the user presses a key, and continue optimizing remote-session responsiveness/latency without compromising reliability, input fidelity, or pooled-connection safety.

## Goal

Keep an attached SSH-backed terminal visibly live during long-running and idle
sessions without requiring a keypress to wake it, while reducing avoidable SSH
round trips and preserving exact input and pooled-connection isolation.

## Acceptance Criteria

- [ ] [AC-1] A remote pane stream that becomes half-open or stops forwarding
  output is detected without user input and is recovered in place within a
  bounded interval; the existing WebSocket remains usable when recovery
  succeeds.
- [ ] [AC-2] Recovery re-synchronizes from an atomic pane snapshot/log offset so
  output is neither silently lost nor replayed across the recovery boundary.
- [ ] [AC-3] An idle but healthy remote pane does not reconnect or perform
  high-frequency SSH probes merely because it has produced no output.
- [ ] [AC-4] Keyboard and paste input remain ordered, lossless, and responsive;
  output-liveness work does not share or evict a healthy interactive SSH master.
- [ ] [AC-5] A genuinely wedged streaming master is retired only after a bounded
  health check identifies transport failure, and recovery can escape through a
  fresh connection without disrupting unrelated interactive channels.
- [ ] [AC-6] Normal remote output is forwarded promptly without adding a polling
  delay, and periodic title/status work is suppressed or moved off the input
  critical path when it would add avoidable contention.
- [ ] [AC-7] Deterministic tests cover healthy idle behavior, a silent/wedged
  output channel, bounded recovery, snapshot/offset continuity, and preservation
  of the interactive master/input path.

## Scope

- SSH pane-output streaming and its liveness/recovery state machine.
- Streaming-versus-interactive ControlMaster health checks and eviction rules.
- Remote title/status polling and other background work that competes with
  terminal input or output.
- Focused instrumentation and deterministic tests for timing/state decisions.

## Non-goals

- Replacing OpenSSH, tmux, WebSockets, or the terminal renderer.
- Changing local-session streaming behavior except for shared helpers/tests.
- Guaranteeing recovery during a sustained host/network outage; the requirement
  is bounded detection, honest reconnect state, and automatic recovery once the
  path is usable.
- Changing terminal byte semantics, decorating user input, or weakening saved
  host revision/lifecycle locking.

## Verification

- Run focused `agentum-server` and `agentum-tmux` tests for stream liveness,
  reconnect planning, SSH pool classification, and exact byte framing.
- Run `cargo fmt --all -- --check`.
- Run `cargo check --workspace --all-targets` and `cargo test --workspace`.
- When an SSH test host is available, attach through an idle period longer than
  the liveness bound and inject a controlled streaming-channel interruption;
  record credential-redacted timestamps showing output resumes without input.

## Capability Deltas

See `deltas/remote-session-reliability.md`.
