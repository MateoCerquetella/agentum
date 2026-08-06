# Project Overview

## Purpose

- `agentum` is a Rust terminal UI and embedded HTTP/WebSocket server for
  creating, supervising, and interacting with AI coding-agent sessions.
- Sessions run inside tmux either locally or on a saved SSH host. The remote
  host needs stock OpenSSH, tmux, git, and the selected agent CLI; it does not
  need an Agentum binary.

## Boundaries

- `agentum-tui` owns terminal interaction and API clients.
- `agentum-server` owns session lifecycle, host readiness, and remote execution.
- `agentum-tmux` owns local tmux operations and the shared OpenSSH runner.
- `agentum-executor` translates tool names into launch argv and MCP settings.
- `agentum-store` persists hosts, sessions, events, and settings in SQLite.

## Evidence

- Workspace boundaries: `Cargo.toml` and `crates/*/Cargo.toml`.
- Product entrypoint and prerequisites: `README.md` and
  `crates/agentum-tui/src/main.rs`.
- Embedded-server startup: `crates/agentum-tui/src/commands/terminal/mod.rs`.
- Remote execution: `crates/agentum-server/src/host_runtime.rs` and
  `crates/agentum-tmux/src/ssh.rs`.
