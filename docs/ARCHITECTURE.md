# Architecture

agentum is a Rust control plane: a daemon (`agentum-server`) that talks to a
host `tmux` server and exposes an **API-only** HTTP/WS surface. Two clients
drive it — the TUI (`agentum terminal`) and the desktop app (Tauri shell that
embeds the server in-process). State lives in one SQLite file.

```
   TUI (agentum terminal)            Desktop app (Tauri + React)
         │  HTTP/WS                        │  HTTP/WS (embeds the server)
         ▼                                 ▼
┌───────────────────────────────────────────────────────────────┐
│             agentum-server  (axum, API-only)                  │
│   /api/* + /api/events (WS) on :8822 (TLS)                    │
│   ┌──────────────────────────────────────────────────────┐    │
│   │   tokio async runtime                                │    │
│   ├──────────┬──────────┬──────────┬──────────┬──────────┤    │
│   │ session  │ executor │ tmux     │ watchdog │ events   │    │
│   │ service  │ adapter  │ adapter  │ task     │ bus      │    │
│   └──────────┴──────────┴──────────┴──────────┴──────────┘    │
│                              │                                │
└──────────────────────────────┼────────────────────────────────┘
                               ▼
                    ┌────────────────────────────────────┐
                    │   tmux server (host)               │
                    │   $XDG_DATA_HOME/agentum/db.sqlite │
                    └────────────────────────────────────┘
```

## Runtime topology

- Single process, single binary.
- HTTPS on `:8822` (rustls + self-signed cert auto-generated, no Let's
  Encrypt). Cert lives in `$XDG_DATA_HOME/agentum/tls/`.
- Plain HTTP cert-download on `:8823` for trust-on-first-use (out-of-band).
- All state in SQLite at `$XDG_DATA_HOME/agentum/db.sqlite`.
- tmux invoked as a subprocess via `tokio::process::Command`. Long-lived
  panes captured via `tmux pipe-pane` to a tail-able log per session.
- Executor adapters translate a shared `Session` identity into the
  tool-specific command that tmux spawns. Four first-class adapters
  (Claude, Codex, Gemini, Hermes) plus a passthrough fallback.
- WebSocket per session for live terminal stream to clients (TUI / desktop).

## Process model

- **Main** task — axum router.
- **Watchdog** task — per-session loop monitoring tmux pane content for
  `/compact` triggers, stuck prompts, crashes.
- **Event bus** — `tokio::sync::broadcast` channel; UI subscribes via
  WebSocket.
- **Persistence** — sqlx with SQLite. WAL mode for concurrent reads
  during writes.

## Filesystem layout (XDG-compliant)

All paths honor the [XDG Base Directory spec][xdg] with sensible
Linux/macOS fallbacks. Resolved via the `directories` crate.

**`AGENTUM_HOME` override:** when set, config/data/cache/state root under
`$AGENTUM_HOME/{config,data,cache,state}` on every platform. Unset → default
platform behavior. Useful for a self-contained install and for cross-platform
test isolation (`directories` ignores `XDG_*` on macOS).

| Purpose       | Env var                | Default (Linux)              | Default (macOS)                                       |
|---------------|------------------------|------------------------------|-------------------------------------------------------|
| Config        | `XDG_CONFIG_HOME`      | `~/.config/agentum/`         | `~/Library/Application Support/agentum/config/`       |
| Data (DB, TLS, auth_token) | `XDG_DATA_HOME` | `~/.local/share/agentum/` | `~/Library/Application Support/agentum/`              |
| Cache (pane logs) | `XDG_CACHE_HOME`   | `~/.cache/agentum/`          | `~/Library/Caches/agentum/`                           |
| State (lockfiles) | `XDG_STATE_HOME`   | `~/.local/state/agentum/`    | `~/Library/Application Support/agentum/state/`        |

Files inside `$XDG_DATA_HOME/agentum/`:

- `db.sqlite` — primary store (WAL + `db.sqlite-shm`, `db.sqlite-wal`).
- `auth_token` — single bearer token (chmod 0600). Created on first `serve`.
- `tls/cert.pem`, `tls/key.pem` — self-signed pair, regenerated yearly.

Files inside `$XDG_CONFIG_HOME/agentum/`:

- `config.toml` — user config (default port, default theme, configured
  tool aliases). Optional; defaults baked in.

Files inside `$XDG_CACHE_HOME/agentum/`:

- `sessions/<session_id>.log` — `pipe-pane` capture, append-only.
  Rotated when > 10 MB. Safe to delete.

## Watchdog rules (per session, every 5 s while running)

| Condition (regex on last 100 lines of pane)             | Action                                | Cooldown |
|---------------------------------------------------------|---------------------------------------|----------|
| `Context low.*<\s*50%`                                  | send `/compact` + Enter               | 5 min    |
| `redacted_thinking.*cannot be modified`                 | kill pane, restart with last message  | 10 min   |
| `^\s*claude:\s*$` (idle prompt) for >2 min              | log "stuck"; do not auto-send         | n/a      |
| crash signature OR pane exited                          | mark `crashed`, emit `session.crashed`| n/a      |

Implementation: `tokio::spawn` per session; `tokio_util::time::DelayQueue`
for cooldowns; `tmux capture-pane -ep -t <target>` every 5 s.

The watchdog is killable: `agentum-server` exposes `/api/watchdog/pause`
for ops.

## Tech stack

### Backend (Rust)

| Crate                                  | Why                                      |
|----------------------------------------|------------------------------------------|
| `axum`                                 | Routing, middleware, WS, tower-stack     |
| `tokio` (full)                         | Runtime + process + signal + sync        |
| `tower-http`                           | CORS, trace, compression, fs             |
| `sqlx`                                 | Async SQLite, compile-time SQL check     |
| `serde` / `serde_json`                 | (de)serialization                        |
| `rustls` + `rustls-pemfile` + `rcgen`  | TLS + self-signed cert                   |
| `time`                                 | RFC3339 timestamps                       |
| `tracing` + `tracing-subscriber`       | Structured logs                          |
| `clap` (derive)                        | CLI args                                 |
| `anyhow` / `thiserror`                 | Error ergonomics                         |
| `directories`                          | Cross-platform XDG paths                 |
| `uuid`                                 | Session IDs                              |
| `notify`                               | Watch project dirs                       |

MSRV 1.83+, Rust 2024 edition, single workspace.

### Clients

The TUI is pure Rust (in `agentum-tui`, under `commands/terminal/`) and renders
with `ratatui`/`crossterm`. The desktop app is a Tauri 2 shell (`agentum-desktop`)
hosting a React + Vite UI (in `crates/agentum-desktop/ui/`):

| Lib                                    | Why                                      |
|----------------------------------------|------------------------------------------|
| **React 19 + Vite**                    | Desktop UI framework + bundler           |
| **Tauri 2**                            | Native shell; embeds agentum-server      |
| **TypeScript**                         | Type safety                              |
| **Tailwind + CSS custom properties**   | Styling + theme tokens                   |
| `lucide-react`                         | Icon set                                 |
| `xterm.js`                             | Terminal renderer                        |
| `monaco` / `@codemirror/*`             | File + markdown editors                  |

**Why xterm.js**: industry-standard terminal renderer, handles ANSI/escape
sequences correctly, accepts streamed bytes from a WebSocket.

### Build & packaging

| Tool             | Purpose                                       |
|------------------|-----------------------------------------------|
| `cargo`          | Backend + TUI + Tauri shell                   |
| `npm` (Vite)     | `crates/agentum-desktop/ui/` build (`npm --prefix crates/agentum-desktop/ui run build`) |
| `tauri` (cargo)  | Desktop app bundle                            |
| `just`           | Task runner                                   |
| GitHub Actions   | CI: clippy, fmt, test, release builds         |

[xdg]: https://specifications.freedesktop.org/basedir-spec/
