# Architecture

agentum is a single Rust binary that ships with an embedded Svelte UI and
talks to a host `tmux` server. State lives in one SQLite file.

```
┌───────────────────────────────────────────────────────────────┐
│                   agentum (single binary)                     │
│                                                               │
│  ┌────────────────────┐       ┌────────────────────────────┐  │
│  │   axum HTTP/HTTPS  │◄──────┤  embedded Svelte build     │  │
│  │   on :8822 (TLS)   │       │  (rust-embed, gzip'd)      │  │
│  └─────────┬──────────┘       └────────────────────────────┘  │
│            │                                                  │
│   ┌────────┴─────────────────────────────────────────────┐    │
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
- Plain HTTP cert-download on `:8823` for trust-on-first-use from a phone.
- All state in SQLite at `$XDG_DATA_HOME/agentum/db.sqlite`.
- tmux invoked as a subprocess via `tokio::process::Command`. Long-lived
  panes captured via `tmux pipe-pane` to a tail-able log per session.
- Executor adapters translate a shared `Session` identity into the
  tool-specific command that tmux spawns. Four first-class adapters
  (Claude, Codex, Gemini, Hermes) plus a passthrough fallback.
- WebSocket per session for live terminal stream to the browser.

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
| `rust-embed`                           | Embed Svelte build into binary           |
| `time`                                 | RFC3339 timestamps                       |
| `tracing` + `tracing-subscriber`       | Structured logs                          |
| `clap` (derive)                        | CLI args                                 |
| `anyhow` / `thiserror`                 | Error ergonomics                         |
| `directories`                          | Cross-platform XDG paths                 |
| `uuid`                                 | Session IDs                              |
| `notify`                               | Watch project dirs                       |

MSRV 1.83+, Rust 2024 edition, single workspace.

### Frontend (Svelte)

| Lib                                    | Why                                      |
|----------------------------------------|------------------------------------------|
| **SvelteKit 2 (latest)**               | App framework. Build target = static     |
| `@sveltejs/adapter-static`             | Pre-render to static, embedded by Rust   |
| **TypeScript**                         | Type safety                              |
| **Vanilla CSS + custom properties**    | Theme engine — no Tailwind               |
| `lucide-svelte`                        | Icon set (terminal, kanban, etc.)        |
| `xterm.js`                             | Terminal renderer                        |
| `@codemirror/*`                        | Notes editor (markdown)                  |
| `dayjs`                                | Time formatting                          |

**Why no Tailwind**: theming via CSS custom props is more flexible and
theme files become drop-in. Tailwind's `dark:` variant doesn't help when
we want 4+ themes that share component shapes.

**Why xterm.js**: industry-standard browser terminal, handles
ANSI/escape sequences correctly, accepts streamed bytes from a
WebSocket.

### Build & packaging

| Tool             | Purpose                                |
|------------------|----------------------------------------|
| `pnpm`           | Frontend deps                          |
| `cargo`          | Backend                                |
| `just`           | Task runner (`just build`, `just dev`) |
| GitHub Actions   | CI: clippy, fmt, test, release builds  |
| `cargo-dist`     | Generate release binaries + installer  |
| `cargo-watch`    | Hot reload during dev                  |

[xdg]: https://specifications.freedesktop.org/basedir-spec/
