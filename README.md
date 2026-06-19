<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="resources/brand/lockup-on-dark.svg">
  <img alt="agentum" src="resources/brand/lockup-on-light.svg" width="300">
</picture>

### Self-hosted control plane for AI coding agents

Run Claude, Codex, Gemini & Cursor in tmux on a host you own — they survive a closed lid.<br>
Then let a verification-gated loop drive them from spec to shipped, one feature at a time.

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
- **Ship on autopilot.** A verification-gated loop drives agents through spec-driven development — build a feature, run the gate, QA it, hand off, repeat — moving tickets to done on their own and pinging you only when it's genuinely stuck.
- **One pane for the whole fleet.** Drive everything from a fast **terminal UI** or a **native desktop app** — both speak the same HTTP/WS API and share one store.
- **Any agent.** First-class adapters for Claude, Codex, Gemini, Hermes & OpenCode; passthrough for anything else on `PATH`. agentum probes what's installed and launches each one natively.
- **Self-hosted, single binary.** Your machine, your hosts. No subscriptions, no cloud lock-in. One Rust binary wires `tmux` to a control plane.

## The loop

agentum doesn't just *run* agents — it *drives* them. Point the **Harness Engine** at a backlog (or a chat, or your Linear / GitHub issues) and it works autonomously, one feature at a time:

```
  backlog / chat / tickets
          │
          ▼
   spec ─→ agent builds ─→ verify.sh ─→ QA gate ─→ handoff ─→ next feature
                               │
                     red = retry · green = advance
```

The QA gate is a script or a spawned **browser-QA agent**. Tickets move **Todo → In Progress → Ready to Test → Done** on their own and escalate to **Needs Human** only when the loop is genuinely stuck — so you babysit the exceptions, not every step.

## Install

Two ways to run it — pick one or use both. On the same machine they boot `agentum-server` in-process and share one SQLite store.

### Desktop app

Native **Tauri 2** (no Electron), with in-app auto-update. arm64 + x86_64.

```sh
# macOS — Homebrew (recommended; warning-free even though not yet notarized)
brew install --cask mateocerquetella/tap/agentum
```

| Platform | Get it |
|----------|--------|
| **macOS** | `brew install --cask mateocerquetella/tap/agentum` · or the `.dmg` |
| **Windows** | `agentum-<ver>-windows-x64-setup.exe` |
| **Linux** | `.deb`, `.rpm`, or tarball |

All native installers live on the [latest release](https://github.com/mateocerquetella/agentum/releases/latest).

### Terminal UI / CLI

One static binary that boots its own server in-process — no daemon to start.

```sh
# Install script (prompts for LAN exposure + autostart only)
curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh | sh

# …or from source
cargo install --git https://github.com/mateocerquetella/agentum agentum-tui
```

Then:

```sh
agentum terminal                                              # open the TUI
agentum new alpha --tool claude --dir ~/code/my-project --up  # …or spawn from the CLI
```

### Control other machines — no second install

Point agentum at a box over SSH; it scans for what's missing, installs the deps (tmux, git), and asks which agent CLIs to set up. Nothing to install on the remote by hand.

```sh
agentum hosts add omarchy --user me --hostname omarchy.local
agentum hosts setup omarchy   # re-run the scan/install flow anytime
```

## What you get

| | |
|---|---|
| **Sessions** | Agent CLIs in tmux, live terminal stream over WS, input bar, auto-`/compact` on context-low. |
| **Executors** | Claude · Codex · Gemini · Hermes · OpenCode adapters, plus passthrough for any binary on `PATH`. |
| **Harness Engine** | Verification-gated autonomy: drives agents one feature at a time behind a `verify.sh` gate — red blocks & retries, green writes a handoff and advances. |
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
  │  sessions · tmux · watchdog · event bus · DB  │
  │  TLS (rustls) · /api/* · /api/events (WS)      │
  └──────────────────────────────────────────────┘
                        │
              tmux server + SQLite (WAL) on the host
```

Deep dives live in [`docs/`](docs/): [Architecture](docs/ARCHITECTURE.md) · [HTTP API](docs/API.md) · [CLI](docs/CLI.md) · [Data model](docs/DATA-MODEL.md) · [Design system](docs/DESIGN-SYSTEM.md).

## Development

```sh
cargo run -p agentum-tui -- terminal              # TUI dev loop (embedded server)
npm --prefix crates/agentum-desktop/ui run dev    # desktop UI (Vite HMR)
cargo run -p agentum-desktop                      # desktop app (shell + embedded server)

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --lib
```

Repository layout, the new-agent checklist, and gotchas are in [`CLAUDE.md`](CLAUDE.md).

## Security

Single-user, local-network by design — no multi-tenant features. A single bearer token (`$XDG_DATA_HOME/agentum/auth_token`, rotate with `agentum auth rotate`) guards networked daemons; loopback binds default to no-auth. TLS is a self-signed rustls cert with a plain-HTTP cert endpoint on `:8823` for trust-on-first-use. **Don't expose `:8822` to the internet without a real reverse proxy + cert.**

## License

MIT — see [LICENSE](LICENSE). It started with one developer who just wanted his agents to keep working after he closed the laptop — now it runs the whole loop.
