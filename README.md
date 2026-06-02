```
  █████╗  ██████╗ ███████╗███╗   ██╗████████╗██╗   ██╗███╗   ███╗
 ██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝██║   ██║████╗ ████║
 ███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║   ██║   ██║██╔████╔██║
 ██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║   ██║   ██║██║╚██╔╝██║
 ██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║   ╚██████╔╝██║ ╚═╝ ██║
 ╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝    ╚═════╝ ╚═╝     ╚═╝
               self-hosted AI agent control plane
```

> Rust control plane for AI coding agents. One binary, a fast TUI, and a native desktop app.

[![release](https://img.shields.io/github/v/release/mateocerquetella/agentum?display_name=tag)](https://github.com/mateocerquetella/agentum/releases)
[![ci](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml/badge.svg)](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml)
[![license](https://img.shields.io/github/license/mateocerquetella/agentum)](LICENSE)

## The Story

**Five `claude` agents. One closed MacBook lid. Half a day of work, gone.**

I had three `claude` sessions, a `codex` run, and an `opencode` review going in parallel on my Mac, each on a different part of the same project. I went to the supermarket. By the time I got back, the screen had slept and every one of them had died with it. The transcripts in `~/.claude/projects/` were still there. The state wasn't.

I tried the obvious fixes. `caffeinate` works until you actually have to take the laptop somewhere. `tmux` survives the lid, but the agents themselves don't survive losing their TTY. They notice.

So I dragged an old PC out of a closet, put Arch on it, tunnelled `tmux` through WireGuard, and wrote shell wrappers to spawn agents on that box over SSH. It worked — my agents kept running with the MacBook in my backpack. But I'd traded one problem for another: twenty tmux panes, one per agent, plus a `lazygit` pane per project to review AI-generated diffs. I was spending more time switching panes than reading what the agents had written.

**agentum is the weekend hack that ate a few weekends.** One Rust binary that wires `tmux` to a control plane: spawn Claude, Codex, Gemini, Cursor, or any CLI on a host you control; watch their terminals; kill them when they wander off. You drive it from a **terminal UI** (`agentum terminal`) or a **native desktop app** — both speak the same HTTP/WS API. Self-hosted, single binary, no subscriptions, no cloud lock-in.

**agentum is beta software, built by one developer who just wanted his AI agents to keep working when he closed his laptop.** If that resonates, you're exactly who this is for.

## TL;DR

One Rust binary spawns AI coding agents (Claude, Codex, Gemini, Cursor, Hermes, …) in tmux panes on a host you control and exposes an HTTP/WS API. Two clients drive it: a **TUI** (`agentum terminal`) and a **desktop app** (Tauri shell + React UI) that embeds the server in-process. On top you get a kanban board, notes, and cross-session channels.

## Quick start

**One install, on your machine.** It runs the agentum daemon (API server + TLS + tmux). To control *other* machines you don't install anything on them — you point agentum at them over SSH and it provisions them for you (see below).

```sh
# Install (interactive prompts for LAN exposure + autostart only)
curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh | sh

# From source
cargo install --git https://github.com/mateocerquetella/agentum agentum-cli
```

After install:

```sh
# Start the daemon (HTTP/WS API on :8822; loopback by default)
agentum serve

# Drive it from the terminal UI
agentum terminal

# …or spawn a session straight from the CLI
agentum new alpha --tool claude --dir ~/Developer/my-project --up
```

The **desktop app** is a separate download (or `cargo tauri build` from `crates/agentum-desktop`). It boots `agentum-server` in-process, so a desktop session and a TUI session on the same machine share one SQLite store.

### Control other machines (no second install)

agentum SSHes in, scans for what's missing, installs the required deps (tmux, git), and asks which agent CLIs to install there:

```sh
agentum hosts add omarchy --user me --hostname omarchy.local
# scans → installs tmux + git → asks which agents (claude, codex, …) to install
agentum hosts setup omarchy   # re-run the scan/install flow anytime
```

## What you get

| Feature      | Details |
|--------------|---------|
| Sessions     | Spawn agent CLIs in tmux. Live terminal stream over WS, input bar, watchdog auto-compacts on context-low. |
| Executors    | First-class adapters for Claude, Codex, Gemini, Hermes. Passthrough for any other binary on PATH. |
| Board        | Atomic-claim kanban for cross-agent task handoff. Drag-drop columns, optimistic updates, 409 on contention. |
| Notes        | Markdown notebook, auto-save on idle/blur, persisted to SQLite. |
| Channels     | 1:1 inter-session message channels with live broadcast over `/api/events`. |
| Watchdog     | Per-session monitor: `Context low.*<\s*50%` triggers `/compact`, crash signatures emit `session.crashed`. |
| Clients      | A fast **TUI** (`agentum terminal`) and a native **desktop app** (Tauri + React), both over the same HTTP/WS API. |
| Auth         | Single bearer token in `$XDG_DATA_HOME/agentum/auth_token` (chmod 0600). Rotate live with `agentum auth rotate`. Loopback binds default to no-auth. |
| TLS          | rustls + rcgen self-signed cert auto-generated on first boot. Plain-HTTP cert-server on `:8823` for trust-on-first-use. |
| Storage      | SQLite (WAL) at `$XDG_DATA_HOME/agentum/db.sqlite`. XDG-compliant on Linux + macOS. |
| Distribution | Single static binary. `cargo install`, `curl \| sh`, or download a tarball from GitHub Releases. |

## Architecture

The daemon is **API-only** — it serves no web UI. Clients connect over HTTP/WS.

```
        TUI  (agentum terminal)                Desktop app (Tauri + React)
              │  HTTP/WS                              │  HTTP/WS
              │                                       │  (embeds the server in-process)
              ▼                                       ▼
┌────────────────────────────────────────────────────────────────┐
│                  agentum-server (axum, tokio)                  │
│   /api/* + /api/events (WS)  ·  TLS (rustls)                   │
│   ├ sessions · tmux adapter · watchdog · event bus · store     │
│   └ plain HTTP :8823 → /api/cert  (trust-on-first-use)         │
└────────────────────────────────────────────────────────────────┘
                                │
                  ┌─────────────▼──────────────┐
                  │  tmux server (host)        │
                  │  $XDG_DATA_HOME/agentum/db │
                  └────────────────────────────┘
```

See [`docs/`](docs/) for the data model, HTTP API, and CLI reference.

## Repository layout

```
crates/
  agentum-cli/       # binary `agentum` + clap CLI; houses the TUI (commands/terminal/)
  agentum-server/    # axum HTTP(S) + WS API (API-only; no embedded web UI)
  agentum-desktop/   # Tauri 2 shell; embeds agentum-server in-process for the desktop app
  agentum-tmux/      # tokio process adapter for tmux
  agentum-watchdog/  # per-session pane monitor + event emitter
  agentum-executor/  # ToolAdapter trait + Claude/Codex/Gemini/Hermes adapters
  agentum-store/     # sqlx + SQLite (WAL) + XDG paths + migrations
  agentum-core/      # shared domain types
desktop-ui/          # React + Vite UI, loaded by the Tauri shell
web/                 # static marketing landing page (deployed separately, not served by the daemon)
docs/                # architecture, data model, API, CLI reference
```

## Development

```sh
# Daemon / TUI dev loop
cargo run -p agentum-cli -- serve --no-tls
cargo run -p agentum-cli -- terminal

# Desktop UI dev loop (Vite HMR)
npm --prefix desktop-ui run dev
# Desktop app (Tauri shell + embedded server)
cargo run -p agentum-desktop

# Lint + test
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --lib
npm --prefix desktop-ui run build
```

The `cc` crate's compiler-detect step gets confused by some `~/.local/bin/cc` shims, so the project's `.cargo/config.toml` defaults `CC=/usr/bin/gcc`. That's a non-overriding default; set `CC` in your shell to override.

## Security

- **Single-user**, **local-network**. No multi-tenant features. Don't expose `:8822` to the internet without a real reverse proxy + cert.
- Bearer token is a single value at `$XDG_DATA_HOME/agentum/auth_token`, generated from `rand::rng()` (32 bytes URL-safe base64). Rotate with `agentum auth rotate`; the running server picks it up on the next request. Loopback binds default to no-auth since only the local machine can reach them.
- TLS cert is self-signed. Browsers will warn. The plain-HTTP cert-server on `:8823` exists so you can pull the PEM and trust it out-of-band.
- All tmux invocations go through `tokio::process::Command` with `.arg(...)` per argument; no shell interpolation in our process invocation.
- WS authentication accepts both `Authorization: Bearer <token>` and a `?token=<token>` query parameter (browsers can't set custom headers on WebSocket upgrades).

## License

MIT. See [LICENSE](LICENSE).
