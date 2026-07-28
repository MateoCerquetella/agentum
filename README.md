<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="resources/brand/lockup-on-dark.svg">
  <img alt="agentum" src="resources/brand/lockup-on-light.svg" width="300">
</picture>

### Self-hosted control plane for AI coding agents

Run Claude, Codex, Gemini & Cursor in tmux on a host you own — they survive a closed lid.<br>
Then use one durable, approval-aware workflow to move a local specification to Ready.

[![release](https://img.shields.io/github/v/release/mateocerquetella/agentum?display_name=tag&color=111)](https://github.com/mateocerquetella/agentum/releases)
[![ci](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml/badge.svg)](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml)
[![license](https://img.shields.io/github/license/mateocerquetella/agentum?color=111)](LICENSE)
![platforms](https://img.shields.io/badge/macOS%20·%20Windows%20·%20Linux-111)

[**Download**](https://github.com/mateocerquetella/agentum/releases/latest) · [Documentation](docs/) · [Website](https://agentum.sh) · [Changelog](CHANGELOG.md)

<br>

<img src="https://agentum.sh/assets/app-hero.png" alt="agentum desktop app" width="820">

</div>

---

## Why agentum

- **Survive the lid.** Agents run in tmux on a host *you* control, not in a laptop session that dies when the screen sleeps. Close the lid, walk away, come back to finished work.
- **Work from one durable spec.** New Spec creates a stable `SPC-<ULID>` and Run Center drives specification, design, planning, implementation, verification, and independent review. Standard + Guarded asks once for spec approval and then stops at Ready.
- **One pane for the whole fleet.** Drive everything from a fast **terminal UI** or a **native desktop app** — both speak the same HTTP/WS API and share one store.
- **Any agent.** First-class SDD adapters for Claude, Codex, Cursor/Agent,
  Gemini, Hermes, OpenCode, and Aider; declared custom adapters pass the same
  conformance contract. Agentum enables an adapter only when its executable,
  authentication, conformance evidence, and required OS isolation are
  available, then launches it through a bounded provider-neutral interface.
- **Self-hosted, single binary.** Your machine, your hosts. No subscriptions, no cloud lock-in. One Rust binary wires `tmux` to a control plane.

## The workflow

Agentum owns the state machine, artifacts, attempts, approvals, and evidence. Providers submit typed artifacts or bounded changes; they do not own run state or write ambient configuration into your project.

```
  New Spec → specification → design → planning → implementation
                                                   │
                                                   ▼
                      Ready ← review ← verification
                        │
                        └── explicit Deliver preview → selected side effects
```

Ready means locally implemented, verified, and independently reviewed. It does not mean committed, pushed, merged, released, or synchronized to a tracker. Those actions require an explicit, hash-bound Deliver confirmation.

## Install

Two ways to run it — pick one or use both. On the same machine they boot `agentum-server` in-process and share one SQLite store.

### Desktop app

Native **Tauri 2** (no Electron), with signed in-app auto-update. macOS ships
arm64 and x86_64 builds; Windows and Linux currently ship x86_64 builds.

> **The CLI/TUI (`agentum terminal`) now lives in a separate repo:** [`github.com/mateocerquetella/agentum-tui`](https://github.com/mateocerquetella/agentum-tui). This repo is the **desktop app** plus the shared backend crates. CLI install/run commands below point at that repo.

```sh
# macOS — Homebrew (Developer ID signed and notarized)
brew install --cask mateocerquetella/tap/agentum

# Linux — verified AppImage (use --format deb, rpm, or raw if preferred)
curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh | sh
```

| Platform | Get it |
|----------|--------|
| **macOS** | `brew install --cask mateocerquetella/tap/agentum` · or the `.dmg` |
| **Windows** | `agentum-<ver>-windows-x64-setup.exe` |
| **Linux** | `.AppImage`, `.deb`, `.rpm`, or the standalone desktop executable |

All native installers live on the [latest release](https://github.com/mateocerquetella/agentum/releases/latest).
The release installer requires the published SHA-256 manifest; macOS also
validates Developer ID signing, Gatekeeper acceptance, and notarization before
replacing the app. The release `uninstall.sh` preserves user data by default;
pass `--purge-data` explicitly to remove Agentum's known desktop data roots.

### Terminal UI / CLI

The CLI/TUI now lives in its own repo, [`agentum-tui`](https://github.com/mateocerquetella/agentum-tui) — one static binary that boots its own server in-process (no daemon to start).

```sh
# from source (installs the `agentum` command) — from the CLI repo:
cargo install --git https://github.com/mateocerquetella/agentum-tui agentum-tui
```

Then:

```sh
agentum terminal                                              # open the TUI
agentum new alpha --tool claude --dir ~/code/my-project --up  # …or spawn from the CLI
```

### Control other machines — no second install

Point agentum at a box over SSH; it scans for what's missing, installs the deps
(tmux, git), and asks which agent CLIs to set up for interactive sessions.
The sequential remote SDD worker is a separate, version-matched,
administrator-installed subsystem and remains capability-gated; see
[`docs/AGENTUM_SDD.md`](docs/AGENTUM_SDD.md#remote-ssh-worker-deployment).

```sh
agentum hosts add omarchy --user me --hostname omarchy.local
agentum hosts setup omarchy   # re-run the scan/install flow anytime
```

## What you get

| | |
|---|---|
| **Sessions** | Agent CLIs in tmux, live terminal stream over WS, input bar, auto-`/compact` on context-low. |
| **Executors** | Claude · Codex · Gemini · Hermes · OpenCode adapters, plus passthrough for any binary on `PATH`. |
| **Agentum SDD** | New Spec + Run Center, stable artifact identity, isolated attempts, typed task DAGs, evidence, review, approval digests, crash recovery, and explicit delivery. |
| **Coordination** | Atomic-claim kanban board, markdown notes, and 1:1 inter-session channels for cross-agent hand-off. |
| **Watchdog** | Per-session monitor that compacts on context-low and emits `session.crashed` on known crash signatures. |
| **MCP server** | agentum's own capabilities as MCP tools at `/mcp` — any agent gets list-sessions, worktrees & the orchestration mailbox. Local Claude/Codex launches are auto-wired. |
| **SSH hosts** | Provision remote machines (tmux, git, agent CLIs) over SSH, surviving `ControlMaster`. |
| **Clients** | Native desktop app (voice dictation, command palette, GitHub Projects, skills) **+** keyboard-driven TUI — same API. |

## Architecture

The daemon is **API-only** — no web UI. Both clients connect over HTTP/WS; the desktop app embeds the server in-process.

```
  TUI  ─┐                                  ┌─ Desktop app (Tauri 2 + React, native)
        │  HTTP / WS                       │  (embeds the server in-process)
        ▼                                  ▼
  ┌──────────────────────────────────────────────┐
  │        agentum-server  (axum · tokio)         │
  │ sessions · SDD runs · tmux · event bus · DB   │
  │  TLS (rustls) · /api/* · /api/events (WS)      │
  └──────────────────────────────────────────────┘
                        │
              tmux server + SQLite (WAL) on the host
```

Deep dives live in [`docs/`](docs/): [Architecture](docs/ARCHITECTURE.md) · [HTTP API](docs/API.md) · [CLI](docs/CLI.md) · [Data model](docs/DATA-MODEL.md) · [Design system](docs/DESIGN-SYSTEM.md). Desktop is fully native — no Electron bridge, no Node.js; the React UI calls Tauri `invoke`/`listen` directly via a typed client.

## Repository layout

```
crates/
  agentum-desktop/   # desktop app: Tauri 2 Rust shell in src/ (embeds agentum-server in-process)
                      #   + React/Vite UI in ui/ (native, no Electron bridge)
  agentum-server/    # axum HTTP(S) + WS API (API-only; no embedded web UI)
  agentum-tmux/      # tokio process adapter for tmux
  agentum-watchdog/  # per-session pane monitor + event emitter
  agentum-executor/  # ToolAdapter trait + Claude/Codex/Gemini/Hermes/Opencode adapters
  agentum-store/     # sqlx + SQLite (WAL) + XDG paths + migrations
  agentum-core/      # shared domain types
docs/                # architecture, data model, API, CLI reference
examples/sdd-demo/   # zero-pollution repository used by the SDD release gate

# The CLI/TUI (binary `agentum`, package agentum-tui; the TUI in
# commands/terminal/) moved to its own repo:
#   github.com/mateocerquetella/agentum-tui
```

## Development

```sh
# Desktop UI dev loop (Vite HMR)
npm --prefix crates/agentum-desktop/ui run dev
# Desktop app (Tauri shell + embedded server)
cargo run -p agentum-desktop

# TUI dev loop (`cargo run -p agentum-tui -- terminal`) lives in the
# separate CLI repo: github.com/mateocerquetella/agentum-tui

# Lint + test
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --lib
```

Repository layout, the new-agent checklist, and gotchas are in [`CLAUDE.md`](CLAUDE.md).

## Security

Single-user, local-network by design — no multi-tenant features. Networked
servers use database-backed bearer sessions. The embedded desktop server is
also authenticated: each boot mints a high-entropy, memory-only bearer exposed
only to the trusted main Tauri webview. Provider and agent processes never
receive it. The MCP endpoint uses a distinct, explicitly rotated bearer.
`agentum serve --no-auth` remains available for non-SDD local automation, but
the complete `/api/sdd` HTTP/WebSocket namespace still requires an
authenticated human session. TLS uses a self-signed rustls certificate with a
plain-HTTP cert endpoint on `:8823` for trust-on-first-use. **Don't expose
`:8822` to the internet without a real reverse proxy + cert.** Release, signing,
renderer, and artifact boundaries are documented in
[`docs/SECURITY.md`](docs/SECURITY.md).

## License

MIT — see [LICENSE](LICENSE). It started with one developer who just wanted his agents to keep working after he closed the laptop — now it runs the whole loop.
