# Design: SSH Remote Session Reliability

## Overview

Keep `agentum-tmux::ssh` as the single OpenSSH transport and move all session
lifecycle decisions into the host-aware server path. A remote launch becomes a
bounded transaction:

1. resolve the remote workdir and selected executable on the target host;
2. derive host-specific launch details (remote shell and remote Claude resume);
3. stage launch environment over SSH stdin so secrets never enter argv;
4. create a dormant pane, enable temporary `remain-on-exit`, arm its private
   output pipe, and respawn the pane with the real command in one SSH script;
5. inspect a short startup window, returning captured output if the child died;
6. disable `remain-on-exit` and persist `Running` only after liveness succeeds.

Local launch behavior remains on the direct tmux adapter, except that the Codex
MCP contract is corrected for both local and remote sessions.

## Component changes

### `agentum-executor`

- Extend the MCP adapter contract with environment entries.
- Codex emits `mcp_servers.<name>.bearer_token_env_var="<stable env name>"`.
- The literal token is returned only as launch environment, never as a `-c`
  argument.
- Add explicit remote launch helpers so terminal sessions use the target's
  `${SHELL:-/bin/sh}` and Claude can select `--resume` from server-supplied
  remote transcript state.

### `agentum-tmux::ssh`

- Add `-T` to machine-oriented SSH calls and support password plus
  keyboard-interactive authentication through askpass.
- Mark SSH children `kill_on_drop` before applying timeouts.
- Classify only canonical tmux missing-session output as `false`; propagate
  auth, transport, command-not-found, and unexpected tmux failures.
- Add bounded helpers for ControlMaster shutdown so host edits/removals cannot
  reuse stale authenticated masters.
- Expand stale-mux classification with tested OpenSSH variants.

### `agentum-server::host_runtime`

- Resolve `~`, `~/...`, absolute, and relative remote paths against the remote
  home; reject missing/inaccessible directories before tunnel or tmux mutation.
- Resolve the selected executable to an absolute remote path, avoiding a stale
  tmux-server PATH. Terminal is special and resolves the target shell.
- Check the remote deterministic Claude transcript path before choosing
  `--session-id` versus `--resume`.
- Introduce an SSH-only `launch_session` transaction. Environment is written
  through SSH stdin to a mode-0600, per-session file. The pane wrapper sources
  and immediately unlinks it before `exec`.
- Create remote runtime/pane directories under `umask 077`, enforce directory
  mode 0700 and log mode 0600.
- Return stage-specific error variants for prerequisites, SSH transport, tmux
  setup, and early agent exit. Captured output is bounded and secret-redacted.
- Bound ControlMaster warmup and reverse-forward operations; serialize tunnel
  cancel/arm operations.

### `agentum-server::routes`

- Map missing prerequisites to a client-actionable response and remote runtime
  failures to `502 Bad Gateway`, keeping the normal `{"error":"..."}` envelope.
- Factor session start into a host-aware callable used by both HTTP and boot
  resume.
- On host update/delete, close old interactive and streaming ControlMasters.

### `agentum-tui`

- Start the embedded server first, then auto-resume only idle sessions through
  the server's host-aware start function. Explicitly stopped sessions stay
  stopped; the legacy local-only boot helper is not used for SSH records.
- Preserve the owning profile and host in the plain-terminal shortcut.
- Decode the API error envelope into a clean message.
- Convert error entries to wrapped visual lines and make `errors_scroll` count
  those rendered rows, preserving j/k, page, top/bottom, clear, and close.

## Failure model

| Stage | Detection | User result |
|---|---|---|
| SSH connect/auth | exit 255 or timeout | Names target and auth/network hint |
| Remote workdir | resolved `test -d/-x` | Names missing/inaccessible path |
| Remote tool | absolute `command -v` result | Names missing selected executable |
| tmux session lookup | canonical stderr classifier | Absent only for known no-session output |
| tmux setup/pipe | nonzero transaction stage | Names tmux setup stage and stderr |
| Agent startup | dead-pane status plus captured pane | Names tool, exit status, and bounded output |
| TUI presentation | decoded JSON plus wrapping | Complete readable diagnostic |

No password, MCP bearer token, hook token, or staged environment content is
included in argv, tracing fields, API errors, or test snapshots.

## Compatibility and cleanup

- Remote launch no longer depends on tmux `new-session -e`; environment staging
  keeps older supported tmux versions usable.
- All generated remote scripts execute POSIX logic through explicit `sh -c`, so
  fish/zsh login shells do not parse the inner control flow.
- Any transaction failure kills the placeholder pane and removes staged env.
- A successful wrapper removes env before executing the selected tool.
- A dead diagnostic pane is captured and killed before the API returns.
- Existing local session creation and non-Codex MCP behavior remain unchanged.

## Verification design

- Executor tests assert the supported Codex key, stable env reference, and token
  absence from argv.
- SSH command tests cover `-T`, auth modes, missing-session classification,
  stale-mux variants, control-master exit commands, and timeout cleanup.
- Host-runtime pure tests cover path expansion, env-file serialization,
  transaction scripts, redaction, early-exit parsing, and private modes.
- Server route tests cover failure status/message mapping and host-aware resume.
- TUI tests cover error-envelope decoding, multiline wrapping at narrow widths,
  and remote plain-terminal routing.
- Focused crate suites run before full workspace fmt/check/test.
- A credential-redacted live test uses the saved host to validate Codex's old
  configuration fails, the new env-var configuration parses, and a real remote
  session starts, streams, accepts input, stops, and cleans up.
