# Architecture

## Components and ownership

- `agentum-core`: shared serialized domain types.
- `agentum-store`: SQLite access, migrations, and platform-specific paths.
- `agentum-executor`: agent adapters and launch/MCP configuration.
- `agentum-tmux`: tmux adapter, OpenSSH command builder, pooling, and retries.
- `agentum-server`: Axum routes, lifecycle, host runtime, and streaming.
- `agentum-tui`: Ratatui app, overlays, clients, PTY, and embedded startup.

## Data and control flow

1. The TUI creates a session and optionally selects a saved SSH host.
2. `POST /api/sessions/{id}/start` resolves its host and tool adapter.
3. The server prepares remote MCP configuration and delegates tmux creation and
   pane piping to `host_runtime`.
4. `host_runtime` uses `agentum_tmux::ssh` for canonical OpenSSH options and a
   one-time unmultiplexed retry after stale ControlMaster failures.
5. Pane logs stream over SSH/WebSocket; keystrokes use a persistent SSH writer.

## External dependencies

- Rust 1.85+ and Cargo; OpenSSH on the Agentum machine.
- tmux, git, and the selected agent CLI on each managed host.
- SQLite through `sqlx` for persistence.
