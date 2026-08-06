# Review — revision 8

## Scope reviewed

- `crates/agentum-tmux/src/ssh.rs`: existing-ControlMaster command construction,
  hot/cold selection, control operations, and regression tests.
- `crates/agentum-server/src/host_runtime.rs`: persistent remote-input framing,
  pacing, exact tmux target resolution, fallback behavior, and real-tmux tests.
- Accepted decision D-001 and acceptance criteria AC-1 through AC-UI-1.

## Findings

No blocking or non-blocking correctness, security, or maintainability findings
remain in the reviewed change.

## Criterion review

- AC-1/AC-2: raw bytes are encoded as lowercase hex and reconstructed by
  `tmux send-keys -H`; normal input is unmodified and long input is split without
  reordering. Unicode is carried as its original UTF-8 bytes.
- AC-3: local delivery is unchanged; SSH uses the persistent writer, while a
  failed/stale pooled socket retains the existing per-exec fallback through a
  cold command that still loads user SSH configuration.
- AC-4: the binary WebSocket data path remains separate from text control
  envelopes; no resize/control serialization was changed.
- AC-5: boundary tests cover fast-path bytes, arbitrary control bytes,
  multi-frame reconstruction, exact tmux target resolution, hot/cold SSH config
  handling, and real tmux delivery of a multi-kilobyte payload.
- AC-UI-1: the installed release was exercised through a real TUI/PTY and the
  exact marker `empirical-λ-🛠-BYTES-check` appeared remotely.

## Safety review

- Cold SSH connections preserve aliases, ProxyJump, authentication, and other
  user configuration. `-F /dev/null` is selected only for an existing private
  Unix-domain ControlPath or for an explicit `ssh -O` control operation.
- Stale socket/channel failure remains bounded and routes to the existing
  no-mux retry; the remote operation cannot execute twice under the retry
  classifier.
- Input data is carried on stdin and is not logged or added to process argv.
- The shell expands only encoder-produced hexadecimal words; the tmux session
  is first resolved to its immutable exact ID.

## Verification considered

- Empirical package checkpoint: passed across all seven workspace crates.
- `agentum-tmux`: 76 passed.
- `agentum-server --lib`: 413 passed, 5 ignored.
- `agentum-tui --lib`: 113 passed.
- Real-tmux multi-kilobyte delivery: passed ten consecutive focused runs.
- Formatting, workspace all-target compilation, release build, and diff
  whitespace checks: passed.
- Live pooled SSH timing: approximately 5.25s before and 0.75s after.

Verdict: PASS.
