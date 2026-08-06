# Plan

## 1. Pin progress and probe behavior

- [ ] Add pure pane-log progress state/classification for caught-up, first lag,
  persistent lag, intervening delivery, and log truncation.
- [ ] Add focused unit tests for every transition and the bounded lag threshold.
- [ ] Add a combined remote log-size/pane-title script and parser with tests for
  valid, empty, malformed, and special-character title output.

## 2. Move maintenance off the input-critical path

- [ ] Run the combined probe through `SshMux::Streaming` with the existing SSH
  deadline and saved-host lifecycle lease.
- [ ] Replace inline interactive-master title polling with at most one background
  probe task per WebSocket stream.
- [ ] Preserve activity-sensitive title cadence and low-frequency healthy-idle
  safety checks while keeping tail and socket branches dispatchable.

## 3. Recover confirmed silent tails

- [ ] Track the next log byte expected from the persistent tail, including bytes
  coalesced into WebSocket frames.
- [ ] On persistent remote-size lag, kill/reap only the tail child and enter the
  existing bounded re-establishment path without waiting for input.
- [ ] Return the atomic replacement offset from recovery, reset progress state,
  and preserve the WebSocket, input writer, and interactive master.
- [ ] Keep EOF recovery and pooled-to-unmultiplexed escape behavior intact.

## 4. Verify reliability and latency invariants

- [ ] Run focused `agentum-server` and `agentum-tmux` tests.
- [ ] Run formatting, workspace compile checks, and workspace tests.
- [ ] Review the diff for host-revision locking, exact-role eviction, child
  cleanup, byte continuity, and absence of new input-path SSH round trips.
- [ ] Collect immutable evidence receipts for configured verification commands
  and complete the exact Empirical revisions.
