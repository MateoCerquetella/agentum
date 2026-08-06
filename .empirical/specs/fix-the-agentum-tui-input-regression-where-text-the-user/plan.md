# Plan: Lossless Agentum Terminal Input

## 1. Establish the failing boundary

- Inventory the current key, paste, WebSocket, local-session, remote-session,
  persistent-writer, fallback, and tmux-delivery paths.
- Use one sentinel payload containing ASCII, whitespace, punctuation, Unicode,
  and `BYTES`.
- Add or run the smallest deterministic probes needed to compare input bytes at
  each boundary.
- Reproduce the user-visible corruption in the current installed/current-source
  TUI or identify an installed-binary/runtime mismatch that explains it.

Evidence: exact failing payload, boundary, observed bytes, and reproduction
command/artifact without unrelated prompt contents.

## 2. Fix the first mismatching transformation

- Apply the smallest code change at the confirmed boundary.
- Keep user input as binary WebSocket frames.
- Keep resize/refresh controls as recognized text JSON.
- Preserve the original payload through SSH persistent and per-exec fallback
  delivery without logging it.

Evidence: focused regression that fails before the change and passes after it.

## 3. Cover adjacent input modes

- Test printable ASCII and multi-byte Unicode key conversion.
- Test bracketed paste with spaces, punctuation, and newlines.
- Test binary WebSocket framing separately from control text framing.
- Test local tmux byte delivery.
- Test SSH hex-line encode/decode and persistent/fallback behavior with the same
  payload, including chunk boundaries.

Evidence: focused test outputs mapped to AC-1 through AC-5.

## 4. Verify the real workflow

- Build the current TUI with the pinned toolchain.
- Launch against an isolated disposable local target and, if the failing path is
  SSH-specific, a disposable remote tmux target.
- Send `empirical-λ-🛠-BYTES-check` and capture evidence that the target receives
  that exact text once.
- Run formatting, compilation, affected crate suites, and diff checks.

Evidence: TUI screenshot/artifact for AC-UI-1 and immutable command receipts.

## 5. Review and integrate

- Review for duplicate delivery, control/input ambiguity, payload logging,
  command injection, and regressions to reconnect behavior.
- Address actionable findings and rerun impacted checks.
- Complete the exact Empirical revision with receipt ids and archive the reviewed
  capability delta into `remote-session-reliability`.
