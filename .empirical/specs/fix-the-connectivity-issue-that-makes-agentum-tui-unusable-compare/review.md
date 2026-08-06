# Review: SSH Connectivity and Latency Repair

## Verdict

PASS. No blocking correctness, safety, or scope findings remain.

The implementation follows accepted decision D-001: it ports the sibling
project's proven recovery behavior without replacing this tree's newer
credential, ControlPath, exact-target, and lifecycle protections. The reviewed
source diff is limited to `agentum-tmux` input chunking and `agentum-server`
pooled-master, remote-tail, input, and title-poll behavior, plus Empirical
feature records.

## Acceptance review

- **AC-1 — pass.** `RemoteTailPump` turns SSH stdout EOF into channel closure;
  the WebSocket loop explicitly reaps the ended child and calls
  `reestablish_remote_tail`. Each replacement captures the pane and log offset
  together, starts the new tail at that offset, then sends RIS plus the
  snapshot before consuming buffered tail bytes.
- **AC-2 — pass.** Six attempts use capped 250/500/1000/2000/3000/3000 ms
  backoff. Attempts 0–2 use `SshMux::Streaming`; attempt 3 evicts only that
  role and switches to `SshMux::Off`; later attempts remain unmultiplexed.
  SSH capture, control exit, and command paths are independently bounded.
- **AC-3 — pass.** Both role-specific ControlPaths receive concurrent real
  remote `true` probes. Success is itself the warmup. Only timeout on a known
  existing master or the lower layer's narrow, pre-session mux classifier can
  trigger exact-role eviction and one reopen. A live master with channel
  pressure is retained, and streaming failure remains best-effort.
- **AC-4 — pass.** Persistent input is pre-opened, every write/flush has a
  three-second deadline, and failure kills/reaps the child before the same raw
  buffer is passed to bounded per-exec `send_bytes`. The next batch reloads the
  current host and attempts to restore the persistent writer. The accepted
  D-001 at-least-once ambiguity remains unavoidable because the remote writer
  protocol has no acknowledgement.
- **AC-5 — pass.** Local, remote one-shot, and persistent input share the
  conservative 512-byte raw-input bound. Persistent frames produce ordered
  newline-delimited hex commands; the lossless long-paste regression decodes
  them back to the original bytes. A bounded queue now backpressures instead of
  silently discarding input.
- **AC-6 — pass.** Output marks title state dirty, the next 2.5-second tick
  polls promptly, the first idle tick is skipped, and the second idle tick is a
  five-second safety poll.
- **AC-7 — pass.** Every recovery, input restoration/fallback, title, resize,
  and refresh operation reloads the persisted host under the shared lifecycle
  lease through a cancellation-aware helper. Tail and input children are
  explicitly killed/reaped before the stream registry publishes cleanup ACK.
- **AC-8 — pass.** The patch does not weaken the SSH builder. Existing tests
  still cover noninteractive password/key/agent auth, accept-new host keys,
  private revision-scoped ControlPaths and secret files, replay-safe mux
  classification, exact immutable tmux IDs, host revision rotation, and local
  tmux behavior.
- **AC-9 — pass.** Receipt `executed-b5381d828ebb33d6c9c1dc69` records a
  successful, non-timed-out serial `--all-targets` test checkpoint for every
  workspace crate. `cargo fmt --all -- --check`, `cargo check --workspace
  --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `git diff --check` also pass. The credential-presence check found no configured
  `AGENTUM_LIVE_SSH_*` seam, so the contract's conditional live smoke was not
  available and no secret or host value was read into evidence.

## Comparative review

The behavior matches the relevant sibling fixes (`ec9e7d39`, `925dea0c`,
`80ecfb3f`, and `3f22a5a4`). The sibling's later module extraction
(`67bd0c06`) was used only to identify boundaries; copying it wholesale would
have regressed protections already present here. No accepted decision is
contradicted and no superseding decision is required.

## Residual risk

No live SSH endpoint was configured in this environment. Transport behavior is
therefore proven by deterministic seams, command construction tests, lifecycle
tests, full workspace regressions, and source comparison, but not by a new WAN
latency measurement in this run. The opt-in ignored live smoke remains available
at `crates/agentum-server/tests/ssh_agents_live.rs` when redacted credentials are
provided.
