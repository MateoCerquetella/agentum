# Architecture

## Components and ownership

- `agentum-core`: shared domain types.
- `agentum-store`: SQLite/WAL persistence and migrations.
- `agentum-tmux`, `agentum-watchdog`, and `agentum-executor`: process,
  observation, and provider-adapter boundaries.
- `agentum-server`: Axum HTTP/WebSocket API, authentication, SDD lifecycle,
  filesystem isolation, remote workers, and event bus.
- `agentum-desktop`: Tauri 2 shell embedding `agentum-server`; `ui/` is the
  React 19/Vite desktop surface.
- `agentum-jira-broker`: separately deployable Jira OAuth broker.

## Data and control flow

- The desktop webview calls typed Tauri operations and the authenticated,
  loopback embedded server. Both local and remote workflows persist through
  `agentum-store`.
- New Spec source adapters normalize bounded read-only input before any durable
  aggregate or worktree allocation. Agentum then owns artifacts, attempts,
  approval digests, evidence, review, Ready, and Deliver.
- Providers run through declared non-shell argv contracts and OS isolation.
  Remote SDD uses a version-matched sequential SSH worker and never silently
  falls back to desktop-local execution.

## External dependencies

- Rust workspace with Tokio, Axum, SQLx/SQLite, Tauri, tmux integration, and
  provider CLIs discovered at runtime.
- Desktop UI uses Bun, TypeScript, React, Vite, Vitest, Tailwind, and Tauri APIs.
- Git and GitHub (`gh`) support repository/tracker delivery; release builds use
  protected GitHub Actions workflows and platform signing credentials.
