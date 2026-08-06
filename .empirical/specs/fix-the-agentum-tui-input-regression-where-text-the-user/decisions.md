# Decisions: Fix The Agentum Tui Input Regression Where Text The User

Record concise, externally reviewable evidence and choices here. Do not store
private chain-of-thought, prompts, credentials, secrets, or scratchpad text.

## D-001: Preserve the binary terminal-input protocol

Status: Accepted

### Evidence

- `agentum-tui` currently maps `TermOut::Bytes(Vec<u8>)` directly to a binary
  tungstenite message.
- Both local and SSH session routes treat binary frames as raw terminal input;
  recognized text frames carry resize/refresh controls.
- Local and remote tmux delivery already use hexadecimal `send-keys -H`.
- The source contains no intentional literal `BYTES` terminal payload.

### Options

1. Preserve binary input frames and locate the first byte mismatch with
   boundary-level tests.
2. Convert printable input to WebSocket text frames.
3. Replace the tmux hex protocol with shell-quoted text commands.

### Chosen approach

Choose option 1. Binary frames are unambiguous and support control bytes,
Unicode UTF-8, paste, and terminal escape sequences without JSON or shell
escaping. The fix will be applied only at the first reproduced mismatch.

### Trade-offs and risks

- Boundary tests add small pure helpers if necessary, but avoid a protocol
  redesign.
- Text frames would collide with resize/refresh JSON and require ambiguous
  escaping.
- Shell-quoted text cannot faithfully carry arbitrary terminal bytes.
- Live verification distinguishes byte delivery from terminal echo artifacts.

### Verification

- Compare exact byte arrays at the TUI/WebSocket boundary.
- Decode SSH hex frames and compare the concatenated bytes to the source.
- Capture bytes delivered through disposable local and SSH tmux targets.
- Confirm resize JSON is still handled only as control.
