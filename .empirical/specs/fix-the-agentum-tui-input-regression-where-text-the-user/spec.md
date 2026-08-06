# Fix The Agentum Tui Input Regression Where Text The User

## Request

> Fix the Agentum TUI input regression where text the user sends to a session is transmitted or rendered as the literal token BYTES instead of the actual typed text. Reproduce the failure, trace the complete TUI-to-API/WebSocket-to-session/SSH/tmux serialization path, preserve the user exact UTF-8 payload without debug or type-name substitution, add focused regression coverage, and verify the fix in the real TUI without losing or corrupting input.

## Goal

Text accepted by an Agentum terminal pane reaches the selected local or SSH
session exactly once as the same ordered UTF-8 bytes. Transport framing,
serializer variants, and implementation labels such as `Bytes` or `BYTES`
never replace or contaminate the user's payload.

## Acceptance Criteria

- [ ] [AC-1] A normal printable key sequence entered in a focused Agentum
  terminal pane arrives at the selected process with the same UTF-8 bytes and
  does not contain `Bytes`, `BYTES`, or another transport/debug label.
- [ ] [AC-2] Bracketed paste preserves spaces, punctuation, newlines, and
  non-ASCII characters in order without loss, duplication, or substitution.
- [ ] [AC-3] The invariant holds for both local tmux sessions and SSH-backed
  sessions, including the persistent remote input writer and its per-exec
  fallback.
- [ ] [AC-4] Resize/control envelopes remain distinguishable from user input;
  fixing text input does not forward resize JSON into the pane or reinterpret
  arbitrary user text as a control message.
- [ ] [AC-5] Focused regression tests exercise the exact failing boundary and
  prove the emitted tmux input bytes equal the accepted terminal payload.
- [ ] [AC-UI-1] [UI] In the real TUI, sending an Empirical invocation containing
  a unique Unicode marker renders that exact text in the target session, with
  no literal `BYTES` substitution.

## Scope

- Crossterm key and paste conversion in `agentum-tui`.
- Binary WebSocket input framing and reconnect queue behavior.
- Local and SSH session WebSocket handlers.
- Persistent SSH input encoding, remote decoding, and tmux byte delivery.
- Deterministic regression tests plus one real-TUI verification artifact.

## Non-goals

- Redesigning terminal rendering, keyboard shortcuts, or the session protocol.
- Changing output/screencast binary frames unrelated to terminal input.
- Adding support for arbitrary invalid UTF-8 typed through crossterm.
- Broad SSH performance work beyond what is necessary for input fidelity.

## Verification

- Run focused `agentum-tui`, `agentum-server`, and `agentum-tmux` tests covering
  typed text, paste, WebSocket framing, and remote hex-line encoding/decoding.
- Run formatting, compilation, and the affected crate test suites.
- Launch the current TUI against an isolated disposable session, send an exact
  marker such as `empirical-λ-🛠-BYTES-check`, and capture the pane output
  proving byte-for-byte delivery without the unwanted token substitution.
- Review the final diff for control-frame confusion, duplicate delivery, input
  logging, and secret exposure.

## Capability Deltas

- `deltas/remote-session-reliability.md` modifies self-healing session transport
  to require lossless, label-free input delivery across local and SSH paths.
