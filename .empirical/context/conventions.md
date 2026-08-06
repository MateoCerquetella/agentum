# Conventions

## Code and structure

- Rust 2024 edition with workspace dependency/version inheritance.
- Async process and network work uses Tokio.
- Shell fragments quote untrusted components with `shlex`; local subprocesses
  use one `.arg()` per argument.
- SSH flags/authentication belong in `agentum-tmux::ssh`, not server call sites.
- Host-aware behavior branches explicitly on local versus SSH hosts.

## Testing and delivery

- Factor pure command construction/parsing so it is unit-testable without live
  tmux or SSH.
- Add regressions in the owning crate, then run focused and workspace suites.
- Preserve actionable stderr while keeping passwords and bearer tokens out of
  argv, logs, snapshots, and error responses.

## Repository-specific constraints

- Remote hosts do not run Agentum; remote work uses stock SSH and tmux.
- SSH operations have timeouts and separate interactive/streaming
  ControlMasters when a safe socket path is available.
- Preserve unrelated worktree changes and avoid unrelated rewrites.
