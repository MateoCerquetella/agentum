```
  █████╗  ██████╗ ███████╗███╗   ██╗████████╗██╗   ██╗███╗   ███╗
 ██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝██║   ██║████╗ ████║
 ███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║   ██║   ██║██╔████╔██║
 ██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║   ██║   ██║██║╚██╔╝██║
 ██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║   ╚██████╔╝██║ ╚═╝ ██║
 ╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝    ╚═════╝ ╚═╝     ╚═╝
               self-hosted AI agent control plane
```

> Rust backend, Svelte frontend, single binary, themeable.

[![release](https://img.shields.io/github/v/release/mateocerquetella/agentum?display_name=tag)](https://github.com/mateocerquetella/agentum/releases)
[![ci](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml/badge.svg)](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml)
[![license](https://img.shields.io/github/license/mateocerquetella/agentum)](LICENSE)

## The Story

I was running five AI coding agents on my Mac. Then I closed the lid to go to the supermarket, and they all died.

So I grabbed an old PC, installed Arch Linux on it, and connected everything through tmux and WireGuard. I set up bidirectional folder sync so any change on my Mac instantly mirrored to the machine that would keep my agents running. Then I wrote shell scripts (`cc --remote`, `codex --remote`, `opencode --remote`) to spawn Claude Code, Codex, and OpenCode in tmux sessions on that old PC.

It worked. My agents kept running even with the MacBook in my backpack.

But now I had a new problem: twenty terminal windows. One per agent. One for lazygit to review AI-generated diffs. Plus my editor, builds, and logs. I was spending more time managing terminals than writing code.

**agentum started as a weekend hack to get my life back.** One Rust binary that turns tmux into a real control plane. Spawn, watch, and message between parallel AI agents from a single dashboard. Two weekends of nights-and-weekends coding. An old PC as a server. No subscriptions, no cloud lock-in.

Then I wanted to check my agents from my phone. Claude Code has `/remote`, but OpenCode doesn't. Codex doesn't. So I built a PWA dashboard that streams live terminals over WebSocket, installable on iOS and Android, self-hosted TLS, zero recurring costs. My agents, my machine, still running when I get home.

**agentum is beta software, built by one developer who just wanted his AI agents to keep working when he closed his laptop.** If that resonates, you're exactly who this is for.

## TL;DR

One Rust binary. Spawns AI coding agents (Claude, Codex, Gemini, Cursor, Hermes, …) in tmux panes on a host you control, streams their terminals over WebSocket to a Svelte PWA, and gives you a kanban board, notes, and cross-session channels on top.

## Quick start

The installer is interactive. It asks whether you want the full **Control Plane** (server + dashboard + TLS) or just the lightweight **Terminal CLI** for managing tmux sessions. Both install the same binary; the choice tailors the post-install guidance.

```sh
# Interactive install (recommended)
curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh | sh

# Non-interactive
curl -fsSL https://.../install.sh | INSTALL_MODE=server sh   # Control Plane
curl -fsSL https://.../install.sh | INSTALL_MODE=cli    sh   # CLI only

# From source
cargo install --git https://github.com/mateocerquetella/agentum agentum
```

After install:

```sh
# Control Plane: start the server, open the dashboard
agentum serve
# https://127.0.0.1:8822  (paste the bearer from `agentum auth show`)

# Terminal CLI: spawn an agent session right away
agentum new alpha --tool claude --dir ~/Developer/my-project --up
```

## What you get

| Feature      | Details |
|--------------|---------|
| Sessions     | Spawn agent CLIs in tmux. Live terminal stream over WS, input bar, watchdog auto-compacts on context-low. |
| Executors    | First-class adapters for Claude, Codex, Gemini, Hermes. Passthrough for any other binary on PATH. |
| Board        | Atomic-claim kanban for cross-agent task handoff. Drag-drop columns, optimistic updates, 409 on contention. |
| Notes        | Markdown notebook with CodeMirror 6, auto-save on idle/blur, persisted to SQLite. |
| Channels     | 1:1 inter-session message channels with live broadcast over `/api/events`. |
| Watchdog     | Per-session monitor: `Context low.*<\s*50%` triggers `/compact`, crash signatures emit `session.crashed`. |
| Themes       | Pure-CSS theme engine. **Terminal Dark** + **Paperlight** ship in v0.1; system theme follows OS. |
| PWA          | Installable on iOS Safari / Chrome Android. Service worker pre-caches the SPA shell for offline read. |
| Auth         | Single bearer token in `$XDG_DATA_HOME/agentum/auth_token` (chmod 0600). Rotate live with `agentum auth rotate`. |
| TLS          | rustls + rcgen self-signed cert auto-generated on first boot. Plain-HTTP cert-server on `:8823` for trust-on-first-use from a phone. |
| Storage      | SQLite (WAL) at `$XDG_DATA_HOME/agentum/db.sqlite`. XDG-compliant on Linux + macOS. |
| Distribution | Single static binary. `cargo install`, `curl \| sh`, or download tarball from GitHub Releases. |

## Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                  agentum (single binary)                       │
│                                                                │
│   axum HTTPS :8822  ◄──────  embedded SvelteKit (rust-embed)   │
│   tokio runtime                                                │
│   ├ sessions, tmux adapter, watchdog, event bus, store         │
│   └ /api/events broadcast → UI toasts + channel messages       │
│                                                                │
│   plain HTTP :8823  →  /api/cert  (trust-on-first-use)         │
└────────────────────────────────────────────────────────────────┘
                                │
                  ┌─────────────▼──────────────┐
                  │  tmux server (host)        │
                  │  $XDG_DATA_HOME/agentum/db │
                  └────────────────────────────┘
```

See [`docs/`](docs/) for architecture, data model, HTTP API, and CLI reference.

## Repository layout

```
crates/
  agentum/         # binary + clap CLI
  agentum-server/  # axum HTTP(S) + WS + rust-embed of dashboard build
  agentum-tmux/    # tokio process adapter for tmux
  agentum-watchdog/# per-session pane monitor + event emitter
  agentum-executor/# ToolAdapter trait + Claude/Codex/Gemini/Hermes adapters
  agentum-store/   # sqlx + SQLite (WAL) + XDG paths + migrations
  agentum-core/    # shared domain types
dashboard/         # SvelteKit 2 + Svelte 5 SPA, embedded into binary
docs/              # architecture, data model, API, CLI reference
```

## Development

```sh
# Backend dev loop
cargo run -- serve --no-tls

# Frontend dev loop (vite, proxies /api → :8822)
npm --prefix dashboard run dev

# Production bundle (frontend + cargo release)
npm --prefix dashboard run build && cargo build --release

# Lint + test
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
npm --prefix dashboard run check
```

The `cc` crate's compiler-detect step gets confused by some `~/.local/bin/cc` shims, so the project's `.cargo/config.toml` defaults `CC=/usr/bin/gcc`. That's a non-overriding default; set `CC` in your shell to override.

## Security

- **Single-user**, **local-network**. No multi-tenant features. Don't expose `:8822` to the internet without a real reverse proxy + cert.
- Bearer token is a single value at `$XDG_DATA_HOME/agentum/auth_token`, generated from `rand::rng()` (32 bytes URL-safe base64). Rotate with `agentum auth rotate`; the running server picks it up on the next request.
- TLS cert is self-signed. Browsers will warn. The plain-HTTP cert-server on `:8823` exists so you can pull the PEM and trust it on a phone.
- All tmux invocations go through `tokio::process::Command` with `.arg(...)` per argument; no shell interpolation in our process invocation.
- WS authentication accepts both `Authorization: Bearer <token>` and a `?token=<token>` query parameter (browsers can't set custom headers on WebSocket upgrades).

## License

MIT. See [LICENSE](LICENSE).
