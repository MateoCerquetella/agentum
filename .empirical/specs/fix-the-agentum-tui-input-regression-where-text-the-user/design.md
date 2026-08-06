# Design: Lossless Agentum Terminal Input

## Context

The terminal input data plane has four transformations:

1. crossterm key/paste events become `Vec<u8>`;
2. `TermOut::Bytes` becomes a binary WebSocket frame;
3. the session route selects the local tmux or SSH input path;
4. SSH input is newline-framed hexadecimal and decoded by remote
   `tmux send-keys -H`.

The current source contains no intentional literal `BYTES` payload. The defect
must therefore be reproduced at a boundary and fixed at the first point where
the accepted payload differs.

## Data contract

For an accepted payload `P`, each data-bearing boundary MUST observe the same
ordered bytes `P`. Control frames are separate:

- terminal input: binary WebSocket frame containing exactly `P`;
- resize/refresh: recognized JSON text envelopes;
- local delivery: `tmux send-keys -H` with two-digit hex arguments for `P`;
- SSH delivery: one or more newline-terminated, space-separated hex frames that
  decode and concatenate to exactly `P`.

No boundary may use `Debug`, variant names, type names, or display formatting
as the data payload.

## Investigation and implementation

1. Reproduce with a distinctive payload containing ASCII, whitespace,
   punctuation, Unicode, and the sentinel word: `empirical-λ-🛠-BYTES-check`.
2. Add focused pure-boundary tests for key/paste conversion and
   `TermOut::Bytes` WebSocket framing.
3. Exercise local tmux delivery and the persistent SSH hex-line protocol with
   the same payload, decoding/capturing the target pane bytes.
4. Locate the first mismatch. Make the smallest change at that boundary and
   route both normal and fallback paths through the tested conversion.
5. Retain resize JSON as text frames and input as binary frames.

## Failure handling

- A failed persistent SSH write may retry the same original payload once through
  the existing per-exec path; formatted error text never becomes pane input.
- A closed WebSocket reports a stream error and does not synthesize input.
- Tests compare byte arrays, not rendered terminal strings, so Unicode and
  control characters cannot pass through lossy display conversion unnoticed.

## Verification

- Focused TUI framing/key/paste tests.
- Local tmux byte-delivery test.
- SSH encoder and persistent/fallback byte-delivery tests.
- A real TUI smoke using an isolated disposable target and the sentinel payload.
- Formatting, compilation, affected crate suites, and final security/code review.

## Risks

- Terminal echo/rendering can make correct input look duplicated; verification
  captures target bytes and pane output separately.
- Retrying after an ambiguous partial SSH write can duplicate input. Existing
  delivery semantics must not be broadened without evidence that a write failed
  before delivery.
- Test instrumentation must never log arbitrary user payloads or secrets.
