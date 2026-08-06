# Plan: Repair and Accelerate SSH Connectivity

## 1. Add deterministic recovery policy seams

- Add pure decisions for pooled-master probe outcomes, tail reconnect backoff and
  mux escalation, and idle title polling.
- Add regression tests before wiring the async paths.
- Preserve the existing exact ControlPath, mux-off, and authentication tests.

Evidence: focused `agentum-tmux` and `agentum-server --lib` tests fail for the
missing behavior, then pass after implementation.

## 2. Repair pooled-master warmup

- Import the exact-role ControlMaster exit builder into `host_runtime`.
- Probe interactive and streaming masters with concurrent bounded remote no-ops.
- On a timed-out or recognized pre-session mux failure, evict only that role and
  perform one pooled reopen; preserve healthy/busy shared masters.
- Treat the successful probe as warm and retain interactive PID-generation plus
  desired reverse-forward reconciliation.

Evidence: classification/role tests and the existing ControlMaster security
suite; review confirms the healthy path has one remote command rather than two.

## 3. Make remote output self-healing

- Thread `SshMux` through `spawn_remote_pane_tail`.
- Factor tail child/stdout/stderr tasks into an explicitly cleaned pump.
- Replace initial-spawn and EOF teardown with bounded recovery that reloads the
  current host, re-snapshots with its log offset, and repaints before resumed
  bytes.
- Escalate from pooled streaming attempts to one streaming-master eviction and
  fresh mux-off tails; honor cancellation throughout.

Evidence: reconnect backoff/escalation tests plus existing stream cancellation
and child-reap tests.

## 4. Make remote input bounded, restorable, and lossless

- Replace the single-line encoder with shared-bound chunked hex lines.
- Add boundary and lossless round-trip tests including oversized paste data.
- Bound persistent write/flush to three seconds, explicitly clean a failed child,
  send the same bytes through the resilient per-exec fallback, and recreate the
  persistent channel on later batches under a freshly loaded host revision.
- Log queue saturation for both binary and text frames.

Evidence: encoder tests, input policy tests, server suite, and code review of
every failure/cancellation cleanup branch.

## 5. Reduce idle SSH channel contention

- Gate remote title queries on pane-byte activity.
- Keep a five-second safety query and immediate next-tick polling after output.
- Unit-test inactive, active, and safety-tick transitions.

Evidence: focused pure tests and review showing idle ticks issue no SSH command.

## 6. Verify and review the complete change

- Run `cargo fmt --all -- --check`.
- Run `cargo test -p agentum-tmux --all-targets` and focused server tests.
- Run `cargo check --workspace --all-targets`.
- Execute the Policy v2 `package-test-checkpoints` command for immutable test
  evidence covering all crates.
- Run the opt-in saved-host test seam when safely configured, recording only
  redacted outcome/timing.
- Conduct an independent acceptance, concurrency, cleanup, performance, and
  secret-safety review; fix every actionable finding.

Evidence: executed test receipt(s), optional live receipt, and collected review
receipt mapped to AC-1 through AC-9.

## 7. Complete and integrate

- Mark implementation complete only after the exact diff matches the contract.
- Pass immutable evidence receipt ids through verify and review.
- Create an independent clean target worktree from the baseline commit.
- Call `empirical_integrate` against that target, verify the receipt and living
  capability projection, and stop at the authorized integrated ceiling.

Evidence: immutable Empirical integration receipt.
