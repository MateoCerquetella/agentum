# agentum — Product Requirements Document

> A self-hosted control plane for AI coding agents.
> Rust backend, Svelte frontend, single binary, themeable, fast.

**Status**: Draft v2 — execution-ready, decisions resolved
**Author**: Mateo Cerquetella
**Date**: 2026-05-04
**Repo**: https://github.com/mateocerquetella/agentum
**License**: MIT
**Reference**: forked-in-spirit from [mixpeek/amux](https://github.com/mixpeek/amux) (MIT + Commons Clause). agentum is a clean-room rewrite, not a fork — only the *concept* is inherited. agentum ships under MIT.

---

## 0. How to use this document

This PRD is written so a coding agent (or you, in tmux) can execute it phase by phase without further clarification on architecture. Each phase has:

- **Goal** — what done looks like
- **Scope** — files/crates touched
- **Acceptance** — verifiable check that the phase works
- **Time estimate** — single-developer-with-AI-assist hours

Run phases in order. After each phase, commit with the suggested message. Don't skip acceptance checks.

---

## 1. Why this exists

amux proves the concept works: tmux + a web dashboard + a watchdog gives you parallel AI coding agents you can monitor from anywhere. But:

- **Performance**: Python + 36k-line single file. Hot reload edits the running file. Not a model for v1 of anything.
- **UX**: ships functional but plain. The marketing site shows a designed mockup that the actual app does not match.
- **Theming**: bolt-on, not architectural.
- **Distribution**: requires `python3` + `tmux` + manual `install.sh` + pasting an unbounded script into `/usr/local/bin`.

agentum keeps the good ideas (single artifact, embedded UI, watchdog, channels, kanban, notes) and rebuilds for **speed**, **distribution simplicity**, and **design polish**.

## 2. Goals & non-goals

### Goals (v0.1)
- **Single static binary** — `agentum`. `cargo install` or `curl | sh`. No runtime deps except `tmux` and `git` on PATH.
- **Sub-100ms** dashboard interactions over LAN/WG. Sub-30ms for cached views.
- **Themeable from day 1** — Terminal Dark + Paperlight + System (auto). Theme = a CSS file, swappable at runtime.
- **Mobile PWA** — usable from phone, installable, offline-capable for read views.
- **Open core, MIT** — one license, no Commons Clause, no friction for adoption.

### Non-goals (v0.1)
- Multi-user / RBAC (single-user, token auth only).
- SaaS hosting / cloud sync.
- Native mobile app (PWA only).
- Plugin / extension marketplace.
- Cross-machine cluster orchestration.

---

## 3. Architecture

```
┌───────────────────────────────────────────────────────────────┐
│                   agentum (single binary)                     │
│                                                                │
│  ┌────────────────────┐       ┌────────────────────────────┐  │
│  │   axum HTTP/HTTPS  │◄──────┤  embedded Svelte build     │  │
│  │   on :8822 (TLS)   │       │  (rust-embed, gzip'd)      │  │
│  └─────────┬──────────┘       └────────────────────────────┘  │
│            │                                                   │
│   ┌────────┴─────────────────────────────────────────────┐    │
│   │   tokio async runtime                                │    │
│   ├──────────┬──────────┬──────────┬──────────┬──────────┤    │
│   │ session  │ tmux     │ watchdog │ events   │ store    │    │
│   │ service  │ adapter  │ task     │ bus      │ (sqlx)   │    │
│   └──────────┴──────────┴──────────┴──────────┴──────────┘    │
│                              │                                 │
└──────────────────────────────┼─────────────────────────────────┘
                               ▼
                    ┌────────────────────────────────────┐
                    │   tmux server (host)               │
                    │   $XDG_DATA_HOME/agentum/db.sqlite │
                    └────────────────────────────────────┘
```

### Runtime topology
- Single process, single binary.
- HTTPS on :8822 (rustls + self-signed cert auto-generated, no Let's Encrypt). Cert lives in `$XDG_DATA_HOME/agentum/tls/`.
- Plain HTTP cert-download on :8823 for trust-on-first-use from a phone.
- All state in SQLite at `$XDG_DATA_HOME/agentum/db.sqlite`.
- tmux invoked as a subprocess via `tokio::process::Command`. Long-lived panes captured via `tmux pipe-pane` to a tail-able log per session.
- WebSocket per session for live terminal stream to the browser.

### Process model
- **Main** task — axum router.
- **Watchdog** task — per-session loop monitoring tmux pane content for `/compact` triggers, stuck prompts, crashes.
- **Event bus** — `tokio::sync::broadcast` channel; UI subscribes via WebSocket.
- **Persistence** — sqlx with SQLite. WAL mode for concurrent reads during writes.

### Filesystem layout (XDG-compliant)
All paths honor the [XDG Base Directory spec](https://specifications.freedesktop.org/basedir-spec/) with sensible Linux/macOS fallbacks. Resolved via the `directories` crate.

| Purpose       | Env var                | Default (Linux)              | Default (macOS)                                       |
|---------------|------------------------|------------------------------|-------------------------------------------------------|
| Config        | `XDG_CONFIG_HOME`      | `~/.config/agentum/`         | `~/Library/Application Support/agentum/config/`        |
| Data (DB, TLS, auth_token) | `XDG_DATA_HOME`     | `~/.local/share/agentum/`     | `~/Library/Application Support/agentum/`               |
| Cache (pane logs) | `XDG_CACHE_HOME`   | `~/.cache/agentum/`          | `~/Library/Caches/agentum/`                            |
| State (lockfiles) | `XDG_STATE_HOME`   | `~/.local/state/agentum/`    | `~/Library/Application Support/agentum/state/`         |

Files inside `$XDG_DATA_HOME/agentum/`:
- `db.sqlite` — primary store (WAL + `db.sqlite-shm`, `db.sqlite-wal`).
- `auth_token` — single bearer token (chmod 0600). Created on first `serve`.
- `tls/cert.pem`, `tls/key.pem` — self-signed pair, regenerated yearly.

Files inside `$XDG_CONFIG_HOME/agentum/`:
- `config.toml` — user config (default port, default theme, configured tool aliases). Optional; defaults baked in.

Files inside `$XDG_CACHE_HOME/agentum/`:
- `sessions/<session_id>.log` — `pipe-pane` capture, append-only. Rotated when > 10 MB. Safe to delete.

---

## 4. Tech stack & dependencies

### Backend (Rust)
| Crate            | Why                                   |
|------------------|---------------------------------------|
| `axum`           | Routing, middleware, WS, tower-stack  |
| `tokio` (full)   | Runtime + process + signal + sync     |
| `tower-http`     | CORS, trace, compression, fs          |
| `sqlx`           | Async SQLite, compile-time SQL check  |
| `serde`/`serde_json` | (de)serialization                  |
| `rustls` + `rustls-pemfile` + `rcgen` | TLS + self-signed cert |
| `rust-embed`     | Embed Svelte build into binary        |
| `time`           | RFC3339 timestamps                    |
| `tracing` + `tracing-subscriber` | Structured logs       |
| `clap` (derive)  | CLI args (`agentum serve`, `register`, etc.) |
| `anyhow` / `thiserror` | Error ergonomics                |
| `directories`    | Cross-platform `~/.agentum`           |
| `uuid`           | Session IDs                           |
| `notify`         | Watch project dirs (later)            |

Pin to MSRV 1.83+. Use the 2024 edition. Single workspace.

### Frontend (Svelte)
| Lib              | Why                                   |
|------------------|---------------------------------------|
| **SvelteKit 2 (latest)** | App framework. Build target = static |
| `@sveltejs/adapter-static` | Pre-render to static, embedded by Rust |
| **TypeScript**   | Type safety                           |
| **Vanilla CSS + custom properties** | Theme engine — no Tailwind |
| `lucide-svelte`  | Icon set (terminal, kanban, etc.)     |
| `xterm.js`       | Terminal renderer                     |
| `@codemirror/*`  | Notes editor (markdown)               |
| `dayjs`          | Time formatting                       |

**Why no Tailwind**: theming via CSS custom props is more flexible and theme files become drop-in. Tailwind's `dark:` variant doesn't help when we want 4+ themes that share component shapes.

**Why xterm.js**: industry-standard browser terminal, handles ANSI/escape sequences correctly, accepts streamed bytes from a WebSocket.

### Build & packaging
| Tool        | Purpose                                |
|-------------|----------------------------------------|
| `pnpm`      | Frontend deps                         |
| `cargo`     | Backend                               |
| `just`      | Task runner (`just build`, `just dev`)|
| GitHub Actions | CI: clippy, fmt, test, release builds |
| `cargo-dist` | Generate release binaries + installer |
| `cargo-watch` | Hot reload during dev                |

---

## 5. Repository layout

```
agentum/
├── Cargo.toml                     # workspace manifest
├── rust-toolchain.toml            # pinned MSRV
├── justfile                       # `just dev`, `just build`, `just release`
├── README.md
├── PRD.md                         # this file
├── LICENSE                        # MIT
├── .gitignore
├── .github/
│   └── workflows/
│       ├── ci.yml                 # fmt, clippy, test
│       └── release.yml            # cargo-dist
├── crates/
│   ├── agentum/                   # main binary + CLI
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs            # clap CLI dispatch
│   │       ├── cli.rs             # subcommands
│   │       └── commands/
│   │           ├── serve.rs       # spawn server
│   │           ├── register.rs
│   │           ├── start.rs
│   │           ├── stop.rs
│   │           ├── ls.rs
│   │           └── attach.rs
│   ├── agentum-server/            # axum app, routes, handlers
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── routes/
│   │       │   ├── mod.rs
│   │       │   ├── sessions.rs
│   │       │   ├── board.rs
│   │       │   ├── notes.rs
│   │       │   └── ws.rs          # /api/sessions/:id/stream
│   │       ├── tls.rs
│   │       ├── auth.rs
│   │       └── embed.rs           # rust-embed for web/build
│   ├── agentum-tmux/              # tmux process adapter
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs             # has-session, new-session, capture-pane, send-keys, pipe-pane
│   ├── agentum-watchdog/          # auto-compact, crash-restart
│   │   └── src/lib.rs
│   ├── agentum-store/             # sqlx wrapper, migrations, types
│   │   ├── Cargo.toml
│   │   ├── migrations/
│   │   │   └── 0001_initial.sql
│   │   └── src/lib.rs
│   └── agentum-core/              # shared types: Session, Status, etc.
│       └── src/lib.rs
├── web/                           # SvelteKit
│   ├── package.json
│   ├── pnpm-lock.yaml
│   ├── svelte.config.js
│   ├── vite.config.ts
│   ├── tsconfig.json
│   ├── static/
│   │   ├── icon-192.png
│   │   ├── icon-512.png
│   │   └── manifest.webmanifest
│   ├── src/
│   │   ├── app.html
│   │   ├── app.css                # base reset + theme var application
│   │   ├── lib/
│   │   │   ├── api.ts             # fetch wrappers
│   │   │   ├── ws.ts              # session stream
│   │   │   ├── stores/
│   │   │   │   ├── theme.ts
│   │   │   │   ├── sessions.ts
│   │   │   │   └── command-palette.ts
│   │   │   ├── components/
│   │   │   │   ├── SessionCard.svelte
│   │   │   │   ├── Terminal.svelte
│   │   │   │   ├── ThemeSwitcher.svelte
│   │   │   │   ├── CommandPalette.svelte
│   │   │   │   ├── Sidebar.svelte
│   │   │   │   └── EmptyState.svelte
│   │   │   └── themes/
│   │   │       ├── _vars.css      # @property declarations
│   │   │       ├── terminal-dark.css
│   │   │       ├── paperlight.css
│   │   │       └── system.css
│   │   └── routes/
│   │       ├── +layout.svelte     # nav, theme provider, palette
│   │       ├── +layout.ts
│   │       ├── +page.svelte       # /sessions
│   │       ├── sessions/
│   │       │   ├── +page.svelte   # list
│   │       │   └── [id]/
│   │       │       └── +page.svelte
│   │       ├── board/+page.svelte
│   │       ├── notes/+page.svelte
│   │       ├── settings/+page.svelte
│   │       └── auth/+page.svelte
├── docs/
│   ├── architecture.md
│   ├── theming.md
│   └── api.md
├── reference/                     # vendored amux clone for inheritance reading
│   └── amux/...                   # do NOT import code from here
└── scripts/
    ├── install.sh
    └── dev.sh
```

---

## 6. Data model (SQLite schema)

```sql
-- migrations/0001_initial.sql

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,           -- uuid v4
    name        TEXT NOT NULL UNIQUE,        -- human-friendly slug
    workdir     TEXT NOT NULL,
    tool        TEXT NOT NULL DEFAULT 'claude',  -- claude | codex | opencode | custom
    model       TEXT,                        -- e.g. claude-opus-4-7
    flags       TEXT NOT NULL DEFAULT '[]', -- JSON array of CLI flags
    status      TEXT NOT NULL DEFAULT 'idle',-- idle | running | stopped | crashed
    tmux_target TEXT,                        -- e.g. "agentum-Bandely"
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    last_activity_at TEXT
);

CREATE INDEX sessions_status_idx ON sessions(status);

CREATE TABLE events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    kind       TEXT NOT NULL,                -- session.started, watchdog.compact, etc.
    payload    TEXT,                         -- JSON
    ts         TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE SET NULL
);

CREATE INDEX events_session_ts_idx ON events(session_id, ts);

CREATE TABLE board_items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT NOT NULL UNIQUE,        -- AG-1, AG-2…
    title       TEXT NOT NULL,
    body        TEXT,
    status      TEXT NOT NULL DEFAULT 'todo',-- todo | doing | done | <custom>
    claimed_by  TEXT,                        -- session_id, atomic CAS
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE notes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '[]', -- JSON
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE channels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    a_session   TEXT NOT NULL,
    b_session   TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(a_session, b_session),
    FOREIGN KEY(a_session) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(b_session) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id  INTEGER NOT NULL,
    sender      TEXT NOT NULL,
    body        TEXT NOT NULL,
    ts          TEXT NOT NULL,
    FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE TABLE token_usage (
    session_id   TEXT NOT NULL,
    day          TEXT NOT NULL,              -- YYYY-MM-DD
    input        INTEGER NOT NULL DEFAULT 0,
    output       INTEGER NOT NULL DEFAULT 0,
    cache_read   INTEGER NOT NULL DEFAULT 0,
    cache_write  INTEGER NOT NULL DEFAULT 0,
    cost_usd     REAL NOT NULL DEFAULT 0,
    PRIMARY KEY(session_id, day),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE auth_tokens (
    token       TEXT PRIMARY KEY,
    label       TEXT,
    created_at  TEXT NOT NULL,
    last_used_at TEXT
);
```

WAL mode + `synchronous=NORMAL` for performance. Run `VACUUM` weekly via watchdog.

---

## 7. HTTP API surface

All JSON. Auth via `Authorization: Bearer <token>` (single token in `$XDG_DATA_HOME/agentum/auth_token` on first run, mode 0600). Bearer-token-only — no hash-based scheme, no OAuth, no multi-user. Rotation = `agentum auth rotate` (overwrites the file with a new random 32-byte URL-safe token).

### Sessions
| Method | Path                              | Body / Query                | Returns |
|--------|-----------------------------------|-----------------------------|---------|
| GET    | `/api/sessions`                   | `?status=running`           | `Session[]` |
| POST   | `/api/sessions`                   | `{name, workdir, tool, model?, flags?}` | `Session` |
| GET    | `/api/sessions/:id`               | —                           | `Session` |
| PATCH  | `/api/sessions/:id`               | partial                     | `Session` |
| DELETE | `/api/sessions/:id`               | —                           | 204 |
| POST   | `/api/sessions/:id/start`         | —                           | `Session` |
| POST   | `/api/sessions/:id/stop`          | —                           | `Session` |
| POST   | `/api/sessions/:id/send`          | `{text, keys?, append_enter?}` | 204 |
| GET    | `/api/sessions/:id/peek?lines=30` | —                           | `{lines: string[]}` |
| WS     | `/api/sessions/:id/stream`        | upgrade                     | binary frames of pane bytes |

### Board
- `GET /api/board` → items grouped by status
- `POST /api/board` → create
- `PATCH /api/board/:id` → status / body / title
- `POST /api/board/:id/claim` → atomic CAS by session_id
- `DELETE /api/board/:id`

### Notes / Channels / Messages
Conventional REST per the data model.

### Server
- `GET /api/health` → `{version, uptime, sessions_running, db_size_mb}`
- `GET /api/version`
- `GET /api/cert` → self-signed cert PEM (cert-server on :8823)

### Events stream
- `WS /api/events` → broadcast bus (session.started, watchdog.compact, etc.)

---

## 8. UI / UX specification

### Information architecture
```
Sidebar
├── Sessions    (default landing)
├── Board
├── Notes
├── Channels
├── Files       (later phase)
├── Skills      (later phase)
└── Settings
```

Top bar: search, `⌘K` palette button, theme switcher, new-session button, notification bell.

### Pages

**/sessions** — grid of session cards. Each card shows:
- name, status pill (idle/running/crashed), workdir (path-truncated), tool badge, last activity
- live last-line preview (10 chars) when running
- clicking opens detail

**/sessions/:id** — detail view:
- left: terminal (xterm.js), connected to `/api/sessions/:id/stream` WS
- right: meta panel (model, flags, token usage today, recent events)
- bottom: input bar to `/send` text/keys
- header actions: stop, restart, fork (clone session), delete

**/board** — kanban with columns from distinct `status` values. Drag-drop updates via `PATCH`. Atomic claim via `POST /:id/claim` returns 409 if already claimed.

**/notes** — list + markdown editor (CodeMirror 6). Auto-save on blur + debounce.

**/channels** — peer 1:1 chat, mention agents to inject messages.

**/settings** — auth token mgmt, theme picker, default flags, dangerous: delete all data.

### Theme system

Themes are pure CSS files in `web/src/lib/themes/`. Each defines `:root` custom properties. Switching = `<html data-theme="paperlight">`.

```css
/* terminal-dark.css */
:root[data-theme="terminal-dark"] {
  --bg:        #0a0a0c;
  --surface:   #111114;
  --surface-2: #1a1a1f;
  --border:    #2a2a30;
  --text:      #e8e8ec;
  --text-2:    #a0a0b8;
  --muted:     #707088;
  --accent:    #ff8a4c;     /* orange */
  --success:   #4ade80;
  --warn:      #fbbf24;
  --danger:    #f87171;
  --font-sans: "DM Sans", system-ui, sans-serif;
  --font-mono: "JetBrains Mono", "SF Mono", Consolas, monospace;
  --font-display: var(--font-mono);
  --radius:    10px;
  --shadow:    0 1px 0 0 rgba(255,255,255,0.04) inset;
}

/* paperlight.css */
:root[data-theme="paperlight"] {
  --bg:        #faf6ee;
  --surface:   #fdfaf3;
  --surface-2: #f1ebdc;
  --border:    #d9cdb1;
  --text:      #1a1411;
  --text-2:    #4a3f33;
  --muted:     #7a6c58;
  --accent:    #c2410c;
  --success:   #166534;
  --warn:      #a16207;
  --danger:    #b91c1c;
  --font-sans: "Spectral", Georgia, serif;
  --font-mono: "JetBrains Mono", "SF Mono", monospace;
  --font-display: "Spectral", serif;
  --radius:    6px;
  --shadow:    0 1px 0 rgba(0,0,0,0.04);
}
```

System theme (auto): `<html data-theme="system">` — uses `prefers-color-scheme` to map to terminal-dark or paperlight.

Theme switcher writes to `localStorage.agentum_theme` and applies on layout mount.

### Command palette (`⌘K`)

Modal with fuzzy search over: pages, sessions, board items, notes, recent commands. Powered by a small in-memory list rebuilt on every `+layout.svelte` mount.

Built-in commands:
- `New session…`
- `Switch theme: terminal-dark | paperlight | system`
- `Open settings`
- `Restart watchdog`
- `Show keyboard shortcuts (?)`

### Empty states

Every page must have a designed empty state:
- Sessions: hero card "No sessions yet — `agentum new <name> --tool <cli> --dir <path>`" + button to open the new-session dialog.
- Board: ASCII-art kanban placeholder + "+ Add task".
- Notes: notebook icon + "Capture your first thought."
- Channels: "No channels yet — agents need at least 2 sessions."

### Status pills
| Status   | Color (var) | Glyph |
|----------|-------------|-------|
| idle     | --muted     | ○     |
| running  | --success   | ●     |
| crashed  | --danger    | ✕     |
| stopped  | --text-2    | ◇     |

### Mobile
- Sidebar collapses behind a hamburger below 720px.
- Terminal view stacks above meta panel.
- Touch targets ≥ 44px.
- PWA manifest + service worker for offline read.

---

## 9. CLI surface (`agentum`)

Fresh design — not amux-compatible. Verbs are short and noun-free. The CLI is **BYO-tool**: `--tool` is required on `new` and accepts any executable on PATH (`claude`, `codex`, `opencode`, `aider`, `cursor`, custom). agentum ships with **no default tool** — the user picks per session.

### Synopsis

```
agentum new <name> --tool <cli> --dir <path> [--model <m>] [--arg KEY=VAL]… [--up]
agentum up <name>                       # start a registered session
agentum down <name>                     # stop gracefully (SIGTERM, then SIGKILL after 5s)
agentum kill <name>                     # immediate SIGKILL
agentum rm <name> [--force]             # remove (must be down unless --force)
agentum ls [--running] [--tool <t>]     # list sessions
agentum ps                              # alias for `ls --running`
agentum open <name>                     # tmux attach passthrough (detach: Ctrl-b d)
agentum tail <name> [-n 30] [-f]        # show last N lines (or follow)
agentum send <name> <text>              # send text + Enter
agentum keys <name> <key-spec>          # raw tmux keys, e.g. 'C-c'
agentum serve [--port 8822] [--no-tls]  # start dashboard
agentum auth show                       # print bearer token
agentum auth rotate                     # generate a new bearer token
agentum config get <key>
agentum config set <key> <value>
agentum config edit                     # open $EDITOR on config.toml
agentum doctor                          # check tmux, XDG dirs, db, cert, port
agentum --version
agentum --help
```

### Semantics

- **No `register`/`start` split** — `agentum new <name> --up` is the equivalent shortcut.
- **No `--yolo`** — pass agent-specific flags through `--arg`. Example for Claude:
  `agentum new alpha --tool claude --dir ~/proj --arg dangerously-skip-permissions=true --arg model=opus`.
  agentum forwards these as `--<key>` (or `--<key>=<value>`) to the configured tool.
- **No default tool** — `agentum new` errors if `--tool` is omitted. Suggest setting a per-user default via `agentum config set default_tool claude` (still explicit at the CLI).
- **DB lazy-init** — first `new` or `serve` creates `$XDG_DATA_HOME/agentum/db.sqlite` and the migrations apply.

### Exit codes

| Code | Meaning              |
|------|----------------------|
| 0    | ok                   |
| 1    | generic error        |
| 2    | usage / bad args     |
| 3    | not-found (no session by that name) |
| 4    | already-exists       |
| 5    | backend not reachable (`serve` down) |
| 6    | tmux missing / unhealthy |
| 7    | tool binary not found on PATH |

---

## 10. Watchdog rules (per session, every 5s while running)

| Condition (regex on last 100 lines of pane)             | Action                                | Cooldown |
|---------------------------------------------------------|---------------------------------------|----------|
| `Context low.*<\s*50%`                                  | send `/compact` + Enter               | 5 min    |
| `redacted_thinking.*cannot be modified`                 | kill pane, restart with last message  | 10 min   |
| `^\s*claude:\s*$` (idle prompt) for >2 min              | log "stuck"; do not auto-send         | n/a      |
| crash signature OR pane exited                          | mark `crashed`, emit `session.crashed`| n/a      |

Implementation: `tokio::spawn` per session; `tokio_util::time::DelayQueue` for cooldowns; `tmux capture-pane -ep -t <target>` every 5s.

Watchdog is killable: `agentum-server` exposes `/api/watchdog/pause` for ops.

---

## 11. Build & distribution

- `just dev` → `cargo watch -x 'run -- serve --port 8822 --dev'` + `pnpm --dir web dev` (Svelte hot reload, server proxies dev assets when `--dev`).
- `just build` → `pnpm --dir web build` then `cargo build --release` (rust-embed pulls `web/build/`).
- `just release` → `cargo dist build` → tar/zip per target (linux-x86_64-musl, linux-arm64-musl, darwin-arm64, darwin-x86_64).
- Install one-liner: `curl -fsSL https://github.com/<you>/agentum/releases/latest/download/install.sh | sh`.
- `cargo install agentum` (publish to crates.io once API stabilizes).
- `brew install agentum` (homebrew tap, post-v0.1).

Service files shipped in `scripts/`:
- `agentum.service` — systemd user unit (linger required for boot survival).
- `agentum.plist` — macOS launchd unit.

---

## 12. Phases (execution plan)

Each phase is a single git commit (or feature branch + squash). Estimates assume one developer + AI assist.

### Phase 0 — bootstrap (DONE in this PRD step)
- Repo scaffolded with reference clone, PRD, README, LICENSE, .gitignore.
- **Acceptance**: `git log` shows initial commit. `cat PRD.md` shows this file.

### Phase 1 — Cargo workspace + minimal HTTP server (3–4 h)
- Create workspace + crates per §5.
- XDG-aware path resolution via `directories` crate; `$XDG_DATA_HOME/agentum/` created on first run.
- `agentum serve` boots axum on :8822, returns `{"status":"ok","version":"…"}` on `/api/health`.
- Plain HTTP first (TLS comes phase 5).
- SQLite store crate with `sessions` table only; migration runs on boot.
- `agentum new`, `agentum up`, `agentum down`, `agentum ls` all working against the DB (no tmux yet — just status flag transitions).
- **Acceptance**:
  ```
  cargo run -- new demo --tool claude --dir ~/Developer/projects/CerqueTech/agentum
  cargo run -- ls                         # demo  idle  claude
  curl http://localhost:8822/api/sessions # returns [demo]
  test -f ~/.local/share/agentum/db.sqlite
  ```
- **Commit**: `feat(phase-1): workspace + http server + new/up/down/ls`

### Phase 2 — tmux adapter (3 h)
- `agentum-tmux` crate: `has_session`, `new_session(name, dir, cmd, env)`, `kill_session`, `capture_pane`, `send_keys`, `pipe_pane(out_path)`.
- `agentum up <name>` actually spawns a tmux session `agentum-<name>` running the configured `tool`.
- `agentum down` graceful (SIGTERM → SIGKILL after 5s); `agentum kill` immediate.
- DB `status` + `tmux_target` updates on every transition.
- pane capture via `tmux pipe-pane -o` to `$XDG_CACHE_HOME/agentum/sessions/<id>.log`.
- **Acceptance**:
  ```
  cargo run -- up demo
  tmux ls | grep agentum-demo             # exists
  cargo run -- down demo
  tmux ls | grep agentum-demo             # gone
  test -f ~/.cache/agentum/sessions/<id>.log
  ```
- **Commit**: `feat(phase-2): tmux adapter`

### Phase 3 — SvelteKit static frontend + theme system (4–5 h)
- `pnpm create svelte@latest web` → SvelteKit + adapter-static + TS + ESLint.
- Layout, sidebar, topbar.
- Two pages: `/sessions` (list) + `/sessions/[id]` (placeholder).
- Theme stores + 2 themes (terminal-dark + paperlight) + system.
- Empty state for `/sessions` if backend returns `[]`.
- API client (`fetch` wrapper, base URL configurable for dev).
- `pnpm build` produces `web/build/`.
- Backend embeds via `rust-embed` and serves at `/`.
- **Acceptance**: `cargo run -- serve` then open `http://localhost:8822` → see sessions page rendered with active theme; theme switch persists in localStorage; both themes look intentional.
- **Commit**: `feat: svelte frontend + theme system`

### Phase 4 — live session terminal (4 h)
- `xterm.js` integration in `/sessions/[id]/+page.svelte`.
- WebSocket `/api/sessions/:id/stream` — backend tails the `pipe-pane` output file at `$XDG_CACHE_HOME/agentum/sessions/<id>.log` and forwards bytes.
- Input bar sends to `POST /api/sessions/:id/send` with `append_enter=true`.
- **Acceptance**: `agentum new shell --tool bash --dir ~ --up` then open the detail page, type `echo hi` in the input bar, see `hi` in the terminal pane.
- **Commit**: `feat(phase-4): live terminal stream`

### Phase 5 — TLS + bearer auth (2 h)
- rustls + rcgen self-signed cert generation to `$XDG_DATA_HOME/agentum/tls/cert.pem` + `key.pem`. Regenerate yearly; serve from disk.
- Plain HTTP cert-server on :8823 returning the PEM (for trust-on-first-use from a phone).
- Bearer-token middleware on `/api/*` (excluding `/api/cert` + `/api/health`). Token in `$XDG_DATA_HOME/agentum/auth_token`, chmod 0600, generated by `rand::thread_rng` (32 bytes URL-safe base64). No hashing — bearer token only, single value, rotatable via `agentum auth rotate`.
- Frontend prompts for token on first load, stores in `localStorage` keyed by origin.
- **Acceptance**: `https://localhost:8822/` works with browser warn; requests without `Authorization` return 401; `agentum auth rotate` invalidates old token (next request 401 with prior token).
- **Commit**: `feat(phase-5): tls + bearer auth`

### Phase 6 — watchdog (3 h)
- Per-session task implementing rules in §10.
- Emit events to event bus.
- UI listens to `/api/events` WS and shows toasts for `watchdog.compact` / `session.crashed`.
- **Acceptance**: simulate `Context low: 45%` in a test session pane → `/compact` is sent → event visible in UI.
- **Commit**: `feat: watchdog + event bus`

### Phase 7 — board + atomic claim (3 h)
- Schema already in §6; routes + handlers; UI kanban with @hello-pangea/dnd or native HTML5 DnD.
- Atomic claim via `UPDATE board_items SET claimed_by=? WHERE id=? AND claimed_by IS NULL` returning rows-affected.
- **Acceptance**: two browsers open, both try to claim same item, only one wins (other gets 409 + toast).
- **Commit**: `feat: kanban + atomic claim`

### Phase 8 — notes + channels (2 + 2 h)
- Notes: CRUD + CodeMirror.
- Channels: list, create (pick two sessions), message stream via WS.
- **Acceptance**: edit a note → reload → content persists. Send a message between two registered sessions → appears live.
- **Commit**: `feat: notes + channels`

### Phase 9 — command palette + keyboard shortcuts + polish (3 h)
- `⌘K` opens palette (per §8).
- `?` shows shortcut sheet.
- Polish empty states, status pills, loading skeletons.
- Lighthouse pass: PWA manifest + service worker.
- **Acceptance**: install to home screen on phone via WG IP, open offline → cached shell loads.
- **Commit**: `feat: command palette + pwa`

### Phase 10 — release v0.1.0 (2 h)
- README with screenshots (terminal-dark + paperlight).
- `cargo dist init` → release workflow.
- Tag `v0.1.0`, GitHub Release with binaries.
- Announce: `gh repo edit --homepage … --description …`, README badges.
- **Commit**: `chore: v0.1.0`

**Total estimate**: ~30–35 h of focused work.

---

## 13. Decisions (resolved 2026-05-04)

| #  | Question                | Resolution                                    |
|----|-------------------------|-----------------------------------------------|
| 1  | GitHub repo             | `mateocerquetella/agentum`                    |
| 2  | License                 | MIT                                           |
| 3  | TLS strategy            | rustls + self-signed only (no Let's Encrypt)  |
| 4  | Auth                    | Bearer token only (single value, rotatable)   |
| 5  | CLI design              | Fresh — not amux-compatible (see §9)          |
| 6  | Persistence path        | XDG-aware (see §3 Filesystem layout)          |
| 7  | Default tool            | None — user picks `--tool` per session (BYO)  |
| 8  | v0.1 themes             | terminal-dark + paperlight only               |

Reverse any decision by editing this section, the affected section it touches, and noting the reversal in the commit message.

---

## 14. Non-functional requirements

- **Performance**: 99p response < 100ms for `/api/sessions`. Terminal stream latency < 50ms WS one-way.
- **Memory**: < 50MB RSS idle with 10 sessions registered, < 200MB with 10 running.
- **Binary size**: < 25 MB stripped on linux-x86_64-musl.
- **Cold start**: `agentum serve` to first 200 OK on `/api/health` < 200ms.
- **Reliability**: no panic-on-write paths; all DB writes inside transactions; graceful SIGTERM (drain WS, sync DB, exit ≤ 2s).
- **Security**: no shell-string interpolation in tmux commands (use `Command::arg` per arg); auth token chmod 0600; CSP header set; no eval; rate-limit auth attempts at 5/min.

---

## 15. Acceptance for v0.1.0 (release gate)

- [ ] All phase acceptance checks pass.
- [ ] Two themes render without visible regressions on Chrome / Safari / Firefox.
- [ ] PWA installable on iOS Safari + Chrome Android.
- [ ] Single-binary release for linux-x86_64, linux-arm64, darwin-arm64.
- [ ] `cargo install agentum` works.
- [ ] README has 3 screenshots (sessions list, terminal view, kanban) in both themes.
- [ ] `clippy --all-targets --all-features -- -D warnings` clean.
- [ ] No TODOs in code outside `// TODO(post-v0.1):` markers.

---

## 16. Glossary

- **Session** — a tmux session running an AI agent CLI (claude/codex/opencode/aider/cursor/custom) in a project dir.
- **Tool** — the CLI binary the session runs. Required on `agentum new`; no default.
- **Workdir** — absolute path the agent starts in.
- **Watchdog** — per-session monitor that auto-compacts and restarts on known failure signatures.
- **Pane bytes** — raw bytes captured from `tmux pipe-pane`, including ANSI escapes; xterm.js renders them.
- **Channel** — directed pair of sessions for inter-agent messaging (1:1).
- **Atomic claim** — board item ownership transfer via SQL CAS.
- **XDG** — Base Directory Specification governing where config / data / cache / state live (see §3).

---

*End of PRD. Hand this to a coding agent or pick up phase 1 yourself.*
