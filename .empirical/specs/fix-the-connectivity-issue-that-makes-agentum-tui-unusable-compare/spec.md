# Fix The Connectivity Issue That Makes Agentum Tui Unusable Compare

## Request

> Fix the connectivity issue that makes agentum-tui unusable. Compare the working Rust SSH connectivity implementation in the sibling agentum project, including its refactored version if present, port the appropriate behavior into agentum-tui, and make SSH connection startup faster while preserving correctness. Verify the fix and integrate it locally.

## Goal

Remote sessions remain usable across stale pooled SSH connections, transient
channel failures, application reloads, and large input. Opening a session uses
the existing warm connection when healthy, output and input recover in place
when it is not, and idle sessions stop consuming avoidable SSH channel capacity.

## Acceptance Criteria

- [ ] [AC-1] When a remote pane tail exits because the streaming SSH channel is
  dropped or refused, Agentum keeps the WebSocket alive and performs a bounded
  reconnect that re-snapshots the pane and resumes from the paired log offset.
- [ ] [AC-2] Tail recovery initially reuses the shared streaming master, then
  evicts that exact master once and switches to a fresh unmultiplexed connection
  after bounded pooled failures; it never loops without a wall-clock bound.
- [ ] [AC-3] The periodic/boot SSH warmer verifies both pooled masters with a
  real remote no-op, preserves a healthy or merely busy shared master, evicts a
  timed-out or recognized pre-session mux failure, and leaves a successful probe
  warm without paying a redundant handshake.
- [ ] [AC-4] A persistent input write that breaks or stalls is bounded, its SSH
  child is terminated and reaped, the same input falls back through the bounded
  per-exec path, and later input attempts to restore the fast persistent path.
- [ ] [AC-5] Arbitrarily large WebSocket input is losslessly split at the shared
  tmux-safe byte bound before the remote read loop invokes `tmux send-keys`.
- [ ] [AC-6] Idle remote streams skip avoidable title-poll SSH round trips while
  active output still triggers prompt title refresh and a periodic safety poll
  covers title-only changes.
- [ ] [AC-7] Recovery observes host lifecycle cancellation and reloads the
  current saved host revision, so a host edit cannot recreate an old-credential
  stream and stream cleanup still acknowledges killed/reaped SSH children.
- [ ] [AC-8] Existing noninteractive authentication, host-key, connection
  revision, private ControlPath, secret cleanup, exact tmux targeting, and local
  session behavior do not regress.
- [ ] [AC-9] Focused SSH/server regressions, format, workspace compile, and
  workspace tests pass; a credential-redacted saved-host smoke test is run when
  a configured reachable host is available.

## Scope

- `agentum-tmux` pooled-master control commands and transport tests.
- `agentum-server` SSH warmer, pane-tail/input helpers, and remote WebSocket
  streaming behavior.
- The proven recovery, input chunking, and idle-poll behavior from the sibling
  `agentum` Rust project, adapted to this tree's stricter host-revision and
  secret-safety invariants.
- Deterministic regressions and a safe live SSH smoke check where configured.

## Non-goals

- Replacing stock OpenSSH with an in-process SSH library.
- Changing sshd, VPN, DNS, firewall, user keys, or remote package configuration.
- Weakening password/askpass handling, ControlPath ownership checks, or secret
  redaction to match an older sibling implementation.
- Copying unrelated desktop, browser, SDD, or release changes from `agentum`.
- Publishing a build, opening a pull request, or changing a remote branch.

## Verification

- `cargo test -p agentum-tmux --all-targets`.
- Focused `agentum-server` tests for reconnect planning/backoff, input timeout
  behavior seams, long-paste chunking, title polling, and host cancellation.
- `cargo fmt --all -- --check` and `cargo check --workspace --all-targets`.
- Policy-configured package test checkpoints covering every workspace crate.
- Independent review against AC-1 through AC-9 and the sibling implementation.
- Credential-redacted saved-host connect/stream/input/reconnect smoke test when
  the repository's own configured test seam can run without exposing secrets.

## Capability Deltas

- `deltas/remote-session-reliability.md`
