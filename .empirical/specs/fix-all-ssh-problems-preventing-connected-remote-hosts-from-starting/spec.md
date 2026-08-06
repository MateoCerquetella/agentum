# Fix All Ssh Problems Preventing Connected Remote Hosts From Starting

## Request

> Fix all SSH problems preventing connected remote hosts from starting/using sessions in the Agentum TUI. Reproduce the ssh/tmux 500 error, identify root causes across TUI/server/tmux layers, implement robust fixes and actionable errors, add regression tests, and verify the remote-session workflow.

## Goal

An SSH host that passes Agentum readiness can start and use supported remote
agent sessions from the TUI. Remote Codex receives valid authenticated MCP
configuration without exposing its token in argv, auto-resume remains
host-aware, startup races preserve the real failing-program diagnostic, and the
TUI presents complete errors instead of clipped JSON lines.

## Acceptance Criteria

- [ ] [AC-1] Starting a `test2`-equivalent Codex session on a reachable SSH
  host with compatible tmux, git, Codex, and an existing workdir creates a live
  remote tmux session and returns success.
- [ ] [AC-2] Authenticated HTTP MCP configuration for Codex uses its supported
  bearer-token environment-variable setting; the token is present in the child
  environment but absent from process argv and rendered errors.
- [ ] [AC-3] Agentum distinguishes SSH transport/auth/timeout failures, an
  incompatible tmux, missing remote workdirs or binaries, tmux setup failures,
  and an agent that exits during startup, returning an actionable message.
- [ ] [AC-4] A fast-exiting remote agent cannot erase useful startup output
  behind a later `pipe-pane` target-missing error; partial tmux state is cleaned
  up after its bounded diagnostic is captured.
- [ ] [AC-5] Startup and automatic resume of SSH-owned sessions always use the
  saved remote host and never test or launch the remote path on the local Mac.
- [ ] [AC-6] Remote terminal sessions select a shell on the remote host rather
  than copying the daemon machine's `$SHELL` path.
- [ ] [AC-7] The TUI error overlay makes full multi-line start errors readable
  within the terminal width while preserving scrolling, clearing, and closing.
- [ ] [AC-8] Agent, key-file, and password SSH construction remains
  non-interactive, timeout-bounded, safely quoted, and resilient to stale pooled
  connections; local-session behavior remains unchanged.
- [ ] [AC-9] Deterministic tests cover MCP env wiring, remote validation and
  diagnostics, host-aware resume, remote shell selection, and wrapped errors;
  all workspace checks pass.

## Scope

- Remote create/start/resume across executor, server, tmux, and TUI layers.
- The reproduced Codex failure plus adjacent supported SSH lifecycle defects
  identified by code, history, logs, and live credential-redacted probes.
- Secret-safe MCP auth, tmux capability/workdir/tool validation, remote terminal
  shell selection, startup diagnostics/cleanup, and readable error rendering.

## Non-goals

- Configuring sshd, VPN/Tailscale, DNS, firewalls, or user SSH keys.
- Adding arbitrary OpenSSH directives, ProxyJump UI, file transfer, or a remote
  Agentum daemon.
- Installing dependencies without the existing explicit confirmation.
- Supporting non-POSIX remote operating systems or every historical agent CLI.

## Verification

- Focused tests for `agentum-executor`, `agentum-tmux`, `agentum-server`, and
  `agentum-tui`.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, and
  `cargo test --workspace`.
- Secret-scan generated Codex argv and captured errors.
- Credential-redacted saved-host smoke test: validate Codex config, then
  start/stream/type/stop a disposable or requested remote session.
- Independent code review of the final diff and evidence.

## Capability Deltas

- `deltas/remote-session-reliability.md`
