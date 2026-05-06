# agentum

> A self-hosted control plane for AI coding agents.
> Rust backend · Svelte frontend · single binary · themeable.

[![release](https://img.shields.io/github/v/release/mateocerquetella/agentum?display_name=tag)](https://github.com/mateocerquetella/agentum/releases)
[![ci](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml/badge.svg)](https://github.com/mateocerquetella/agentum/actions/workflows/ci.yml)
[![license](https://img.shields.io/github/license/mateocerquetella/agentum)](LICENSE)

agentum gives you a single dashboard to spawn, watch, and message between
parallel AI coding agents (Claude Code, Codex, Gemini, Hermes, or any CLI you
want) — running in tmux on your own machine, browseable from your laptop or
phone. Rust + axum on the backend, Svelte 5 with a real theme system on the
front, one binary, `cargo install`.

## Quick start

The installer is interactive — it asks whether you want the full **Control Plane**
(server + dashboard + TLS) or just the lightweight **Terminal CLI** for managing
tmux sessions.  Both install the same binary; the choice tailors the
post-install guidance.

```sh
# Interactive install (recommended)
curl -fsSL https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh | sh

# Or download and run directly for the interactive prompts:
#   curl -fsSLO https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh
#   sh install.sh
```

When you run it, you'll see:

```
  █████╗  ██████╗ ███████╗███╗   ██╗████████╗██╗   ██╗███╗   ███╗
 ██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝██║   ██║████╗ ████║
 ███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║   ██║   ██║██╔████╔██║
 ██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║   ██║   ██║██║╚██╔╝██║
 ██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║   ╚██████╔╝██║ ╚═╝ ██║
 ╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝    ╚═════╝ ╚═╝     ╚═╝
               self-hosted AI agent control plane

  ● platform x86_64-unknown-linux-gnu
  ● version  v0.6.3
  ● install  /home/you/.local/bin

  Choose your install:

  🖥️   [1] Control Plane
       Server · Dashboard · TLS · tmux — full web UI
       › agentum serve on your LAN, dashboard from any device

  ⌨️   [2] Terminal CLI
       CLI-only tmux session manager, no server/TLS
       › agentum new/up/down/ls/tail from your terminal

  Choice [1-2] (1):
```

**Non-interactive / CI usage:**

```sh
# Control Plane (server + dashboard)
curl -fsSL https://.../install.sh | INSTALL_MODE=server sh

# Terminal CLI only
curl -fsSL https://.../install.sh | INSTALL_MODE=cli sh

# Or with CLI flags (download first, then run directly):
sh install.sh --mode server
sh install.sh --mode cli --no-interactive

# Install from source
cargo install --git https://github.com/mateocerquetella/agentum agentum
```

After install, get started:

```sh
# Control Plane — start the server and open the dashboard
agentum serve
# → https://127.0.0.1:8822  (paste the bearer from `agentum auth show`)

# Terminal CLI — spawn an agent session right away
agentum new alpha --tool claude --dir ~/Developer/my-project --up
```

## What you get

| Feature        | Details                                                                                                    |
|----------------|------------------------------------------------------------------------------------------------------------|
| Sessions       | Spawn agent CLIs in tmux. Live terminal stream over WS, input bar, watchdog auto-compacts on context-low.  |
| Executors      | First-class adapters for Claude, Codex, Gemini, Hermes — passthrough for any other binary on PATH.          |
| Board          | Atomic-claim kanban for cross-agent task handoff. Drag-drop columns, optimistic updates, 409 on contention. |
| Notes          | Markdown notebook with CodeMirror 6, auto-save on idle/blur, persisted to SQLite.                           |
| Channels       | 1:1 inter-session message channels with live broadcast over `/api/events`.                                  |
| Watchdog       | Per-session monitor — `Context low.*<\s*50%` → `/compact`, crash signatures → `session.crashed` event.       |
| Themes         | Pure-CSS theme engine. **Terminal Dark** + **Paperlight** ship in v0.1; system theme follows OS.            |
| PWA            | Installable on iOS Safari / Chrome Android. Service worker pre-caches the SPA shell for offline read.       |
| Auth           | Single bearer token in `$XDG_DATA_HOME/agentum/auth_token` (chmod 0600). Rotate live with `agentum auth rotate`. |
| TLS            | rustls + rcgen self-signed cert auto-generated on first boot. Plain-HTTP cert-server on `:8823` for trust-on-first-use from a phone. |
| Storage        | SQLite (WAL) at `$XDG_DATA_HOME/agentum/db.sqlite`. XDG-compliant on Linux + macOS.                          |
| Distribution   | Single static binary. `cargo install`, `curl \| sh`, or download tarball from GitHub Releases.              |

## Screenshots

> Drop PNGs at `docs/screenshots/` and the README will pick them up.

| Sessions list | Live terminal | Kanban |
|---|---|---|
| ![](docs/screenshots/sessions-dark.png) | ![](docs/screenshots/terminal-dark.png) | ![](docs/screenshots/board-dark.png) |

## CLI surface

```
agentum new <name> --tool <cli> --dir <path> [--model <m>] [--arg KEY=VAL]… [--up]
agentum up   <name>
agentum down <name>             # SIGTERM → SIGKILL after 5s → kill-session
agentum kill <name>             # immediate kill-session
agentum rm   <name> [--force]
agentum ls   [--running] [--tool <t>]
agentum ps                      # alias for `ls --running`
agentum open <name>             # tmux attach passthrough
agentum tail <name> [-n 30] [-f]
agentum send <name> <text>
agentum keys <name> <key-spec>  # raw tmux keys, e.g. 'C-c'
agentum serve [--port 8822] [--cert-port 8823] [--no-tls] [--no-resume]
agentum auth show | rotate
agentum config get | set | edit
agentum doctor                  # check tmux, XDG dirs, db, cert, port
```

Run `agentum --help` for full details.

## Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                  agentum (single binary)                       │
│                                                                │
│   axum HTTPS :8822  ◄──────  embedded SvelteKit (rust-embed)   │
│   tokio runtime                                                │
│   ├ sessions ─ tmux adapter ─ watchdog ─ event bus ─ store     │
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
  agentum-server/  # axum HTTP(S) + WS + rust-embed of web/build
  agentum-tmux/    # tokio process adapter for tmux
  agentum-watchdog/# per-session pane monitor + event emitter
  agentum-executor/# ToolAdapter trait + Claude/Codex/Gemini/Hermes adapters
  agentum-store/   # sqlx + SQLite (WAL) + XDG paths + migrations
  agentum-core/    # shared domain types
web/               # SvelteKit 2 + Svelte 5 SPA, embedded into binary
docs/              # architecture, data model, API, CLI reference
```

## Development

```sh
# Backend dev loop (auto-reload via cargo-watch)
cargo run -- serve --no-tls

# Frontend dev loop (vite, proxies /api → :8822)
pnpm --dir web dev

# Build production bundle (web + cargo release)
pnpm --dir web build && cargo build --release

# Lint + test
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
pnpm --dir web check
```

The `cc` crate's compiler-detect step gets confused by some `~/.local/bin/cc`
shims, so the project's `.cargo/config.toml` defaults `CC=/usr/bin/gcc`.
That's a non-overriding default — set `CC` in your shell to override.

## Security

- **Single-user**, **local-network**. No multi-tenant features. Don't expose
  `:8822` to the internet without a real reverse proxy + cert.
- Bearer token is a single value at `$XDG_DATA_HOME/agentum/auth_token`,
  generated from `rand::rng()` (32 bytes URL-safe base64). Rotate with
  `agentum auth rotate` — the running server picks it up on the next request.
- TLS cert is self-signed. Browsers will warn. The plain-HTTP cert-server
  on `:8823` exists so you can pull the PEM and trust it on a phone.
- All tmux invocations go through `tokio::process::Command` with
  `.arg(...)` per argument; no shell interpolation in our process invocation.
- WS authentication accepts both `Authorization: Bearer <token>` and a
  `?token=<token>` query parameter (browsers can't set custom headers on
  WebSocket upgrades).

## License

MIT. See [LICENSE](LICENSE).

## Credits

Concept inspired by [mixpeek/amux](https://github.com/mixpeek/amux).
