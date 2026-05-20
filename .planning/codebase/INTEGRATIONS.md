# External Integrations

**Analysis Date:** 2026-05-20

## APIs & External Services

agentum is self-hosted by design. It calls out to **local subprocess CLIs** and the **local tmux server**, not to network-hosted SaaS APIs. The only outbound network calls in the daemon happen indirectly through whichever agent CLI it spawns (Claude, Codex, etc., which independently authenticate with their vendor backends).

**Agent CLIs (first-class adapters):**
All implement `ToolAdapter` in `crates/agentum-executor/src/adapters.rs`. The daemon shells out to them via tmux; agentum never wraps a vendor SDK directly.

- **Claude Code** (`claude` binary) — `ClaudeAdapter` in `crates/agentum-executor/src/adapters.rs:41-141`.
  - Launch: `claude [--model=<m>] (--session-id <uuid> | --resume <uuid>) [flags…]`.
  - YOLO flag: `--dangerously-skip-permissions` (identity — also the canonical `YOLO_MARKER` value).
  - Compact trigger: `/compact`.
  - Transcripts: written by Claude to `~/.claude/projects/<enc-cwd>/<uuid>.jsonl`; agentum tails them via `notify` (see `crates/agentum-server/src/transcript_store.rs`) to extract plan/todo state.

- **Codex CLI** (`codex` binary) — `CodexAdapter` in `crates/agentum-executor/src/adapters.rs:147-174`.
  - Launch: `codex [--model=<m>] [flags…]`.
  - YOLO flag: `--dangerously-bypass-approvals-and-sandbox`.
  - Compact trigger: `/compact`.

- **Cursor** (`cursor-agent` binary, headless agent CLI) — `CursorAdapter` in `crates/agentum-executor/src/adapters.rs:181-202`.
  - Launch: `cursor-agent [--model=<m>] [flags…]`.
  - YOLO flag: `--force`.

- **Cursor "agent"** (`agent` binary — renamed Cursor CLI) — `AgentAdapter` in `crates/agentum-executor/src/adapters.rs:212-235`. Same product as Cursor, new entry-point name.
  - YOLO flag: `--force`.

- **Gemini CLI** (`gemini` binary) — `GeminiAdapter` in `crates/agentum-executor/src/adapters.rs:239-261`.
  - Launch: `gemini [--model=<m>] [flags…]`.
  - YOLO flag: `--yolo`.

- **Hermes CLI** (`hermes` binary) — `HermesAdapter` in `crates/agentum-executor/src/adapters.rs:265-290`.
  - Launch: `hermes chat [--model=<m>] [flags…]`.
  - YOLO flag: `--yolo`.

**Passthrough-probed agents:**
Listed in `crates/agentum-executor/src/lib.rs::PASSTHROUGH_PROBED`. Availability is probed via `which` but no first-class adapter exists; argv is the literal tool name plus user flags.

- `opencode` — YOLO flag unverified (currently `None`).
- `aider` — YOLO flag unverified (currently `None`).

**YOLO marker translation:**
All clients push the canonical Claude marker `--dangerously-skip-permissions` into `Session::flags`. Each adapter's `launch()` substitutes via `agentum_executor::translate_yolo_marker` (`crates/agentum-executor/src/lib.rs:120-131`). Adapters that return `None` from `yolo_flag()` drop the marker silently. **Never push tool-specific spellings from clients** — see CLAUDE.md "YOLO marker translation".

**Agent installation gating:**
`GET /api/agents` (`crates/agentum-server/src/routes/agents.rs`) runs `which::which(binary_for(name))` for every entry in `probed_tools()` and returns `[{name, binary, available, yolo_flag, path}]`. TUI and dashboard call this on startup / dialog-open to dim unavailable tiles.

## Data Storage

**Databases:**
- SQLite (single file) via sqlx 0.8.
  - Connection: `$XDG_DATA_HOME/agentum/db.sqlite` (`crates/agentum-store/src/paths.rs::db_path`). On macOS the path resolves under `~/Library/Application Support/agentum/`.
  - Client: `sqlx::SqlitePool` configured with WAL journal mode, `synchronous = NORMAL`, `foreign_keys = ON`, `max_connections = 8` (`crates/agentum-store/src/lib.rs::Store::open` lines 56–80).
  - Migrations: `crates/agentum-store/migrations/0001_initial.sql` through `0014_board_column_rules.sql`. Run automatically at boot via `sqlx::migrate!("./migrations").run(&pool)`.
  - File mode forced to 0600 alongside its `-wal` / `-shm` sidecars (`crates/agentum-store/src/lib.rs::restrict_db_perms`) because the DB stores Argon2id password hashes and live bearer tokens.
  - Tables include: `sessions`, `events`, `board_items`, `board_comments`, `board_column_rules`, `notes`, `channels`, `messages`, `users`, `auth_sessions`, `session_metrics`, `preferences`.

**File Storage:**
- Pane logs: `$XDG_CACHE_HOME/agentum/sessions/<session-id>.log` (`crates/agentum-store/src/paths.rs::pane_log`). Tmux `pipe-pane` writes raw ANSI here for resume / replay.
- TLS material: `$XDG_DATA_HOME/agentum/tls/{cert,key}.pem` (`crates/agentum-server/src/tls.rs::ensure_artifacts`; mode 0600).
- Profile config: `$XDG_CONFIG_HOME/agentum/profiles.toml` (shared by TUI and dashboard).
- Trusted-cert pins: `$XDG_CONFIG_HOME/agentum/known_hosts.toml` (TUI-only TOFU pin store; see `crates/agentum/src/commands/terminal/trust.rs`).
- Daemon logs: `$XDG_STATE_HOME/agentum/` (resolved in `crates/agentum/src/commands/serve.rs:86-89` and `crates/agentum/src/commands/terminal/mod.rs:811`).
- Claude Code transcripts (read-only): `~/.claude/projects/<enc-cwd>/<uuid>.jsonl`. Watched via `notify` 8 in `crates/agentum-server/src/transcript_store.rs`.

**Caching:**
- In-memory only.
  - Event broadcast bus (`tokio::sync::broadcast`, capacity 1024) for `Event` fan-out (`crates/agentum-server/src/lib.rs::EVENT_BUS_CAPACITY`).
  - Per-session stream checkpoints (`crates/agentum-server/src/lib.rs::StreamCheckpoint`) for WS resume after reconnect.
  - Transcript state map (`crates/agentum-server/src/transcript_store.rs::TranscriptStore`) for plan/todo replay.
  - Browser-side: `localStorage` keys `agentum_profile_tokens`, `agentum_profile_labels`, `agentum_profile_cache`, `agentum_active` (see `dashboard/src/lib/profiles.ts:60-68`). Service worker (`dashboard/src/service-worker.ts`) precaches the SvelteKit chunk graph.

## Authentication & Identity

**Auth Provider:** Custom (self-hosted, no external IdP).

**Implementation:**
- Username + password registration / login (`crates/agentum-server/src/routes/auth.rs`, exposed at `/api/auth/login`, `/api/auth/register`, `/api/auth/me`, `/api/auth/logout`, `/api/auth/status`).
- Passwords hashed with **Argon2id** (`argon2` 0.5 + `password-hash` 0.5) on the blocking pool (`crates/agentum-server/src/auth.rs::hash_password`, `verify_password`).
- Bearer tokens: 32 random bytes → URL-safe base64 (`crates/agentum-server/src/auth.rs::new_token`). Persisted to the `auth_sessions` table (`crates/agentum-store/migrations/0005_users.sql`).
- Token TTL: 30 days, sliding (`crates/agentum-server/src/auth.rs::SESSION_TTL`). Refreshed on every authenticated hit; expired rows swept hourly by `serve()` (`crates/agentum-server/src/lib.rs::AUTH_SWEEP_INTERVAL`).
- Middleware: `auth::require_token` applied to all routes via `axum_mw::from_fn_with_state`. Public endpoints are listed in `crates/agentum-server/src/auth.rs::is_public`:
  - `/api/health`
  - `/api/cert`
  - `/api/cert/fingerprint`
  - `/api/auth/status`
  - `/api/auth/login`
  - `/api/auth/register`
- WS clients pass the bearer as `?token=…` because browsers can't set `Authorization` on upgrade (`extract_token` in `crates/agentum-server/src/auth.rs`).
- Auth rate limit: 8 attempts per remote IP per 5-minute window (`crates/agentum-server/src/lib.rs::AUTH_RATE_LIMIT_*`, enforced in `crates/agentum-server/src/ratelimit.rs`).
- `agentum serve --no-auth` disables the middleware entirely (`AppState.no_auth`).

**TLS / cert trust (TOFU):**
- Self-signed cert generated by `rcgen` on first boot, SHA-256 fingerprint printed to the host TTY (`crates/agentum-server/src/lib.rs::serve` lines 268–272).
- Operators verify the fingerprint out-of-band; the TUI pins it in `known_hosts.toml`, browsers display the usual unverified-cert warning.
- Plain-HTTP cert-server runs on a side port (`opts.cert_addr`, typically 8823) serving `/api/cert` → PEM, for phone-style TOFU bootstrap (`crates/agentum-server/src/lib.rs::cert_server_router`).
- `/api/cert/fingerprint` route (`crates/agentum-server/src/routes/cert.rs`) returns the formatted SHA-256 so the dashboard wizard can display it pre-login.

## Monitoring & Observability

**Error Tracking:**
- No external service. Errors propagate through `anyhow` (binaries) and `thiserror`-derived enums (libraries → `crates/agentum-server/src/error.rs::ApiError`).

**Logs:**
- `tracing` + `tracing-subscriber` (`env-filter`). Initialised by each binary. Daemon writes to `$XDG_STATE_HOME/agentum/` (`crates/agentum/src/commands/serve.rs:86-89`).
- HTTP request tracing layered via `crates/agentum-server/src/logging.rs::redacting_trace_layer` — strips bearer tokens from `Authorization` headers and `?token=` query strings before they reach the log line.

**Host metrics:**
- `GET /api/host/metrics` and `host.metrics` events on the event bus, sampled every 2 s (`crates/agentum-server/src/routes/host.rs::HOST_METRICS_INTERVAL`) via `sysinfo` 0.32 (`system` feature). Reports aggregate CPU %, per-core CPU %, memory and swap usage.

**Health probe:**
- `GET /api/health` (`crates/agentum-server/src/routes/health.rs`) returns `{status, version, uptime_seconds, sessions_running, capabilities, hostname}`. `capabilities` advertises feature tags like `resize` and `resume` for client feature detection.

## CI/CD & Deployment

**Hosting:**
- Self-hosted, single static binary. No managed PaaS target. Operators run `agentum serve` on their own machine, VPS, or LAN box.
- Connection profiles let one TUI / dashboard target multiple daemons (`agentum profiles add …` / dashboard `EndpointSwitcher.svelte`).

**CI Pipeline:**
- GitHub Actions.
  - `.github/workflows/ci.yml` — Runs on tag pushes (`v*.*.*`) and manual dispatch. Matrix: `ubuntu-latest`, `macos-latest`. Steps: build dashboard with pnpm, `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all`.
  - `.github/workflows/release.yml` — Triggered by `v*.*.*` tags. Build matrix:
    - `x86_64-unknown-linux-gnu` on `ubuntu-22.04` (pinned to glibc 2.35 for Debian 12 compat).
    - `aarch64-unknown-linux-gnu` on `ubuntu-22.04` via `cross`.
    - `x86_64-apple-darwin` on `macos-14`.
    - `aarch64-apple-darwin` on `macos-14`.
  - Release notes extracted from `CHANGELOG.md` by an `awk` script; release published via `softprops/action-gh-release@v2`. SHA256SUMS generated; `scripts/install.sh` attached for the README's one-liner installer.

**Action versions in use:**
- `actions/checkout@v4`
- `dtolnay/rust-toolchain@stable`
- `Swatinem/rust-cache@v2`
- `pnpm/action-setup@v4` (pnpm 9)
- `actions/setup-node@v4` (Node 22)
- `actions/upload-artifact@v4`, `actions/download-artifact@v4`
- `softprops/action-gh-release@v2`

**Embedded SPA build order:**
1. `pnpm --dir dashboard install --frozen-lockfile && pnpm --dir dashboard build` produces `dashboard/build/`.
2. `cargo build --release` then bakes `dashboard/build/` into the daemon via `rust-embed` (`crates/agentum-server/src/embed.rs`).
3. Skipping step 2 leaves the daemon serving the previously-embedded bundle. See CLAUDE.md "Critical: rebuild rhythm".

## Environment Configuration

**Required env vars:**
- None for normal runtime — agentum self-bootstraps TLS, DB, and config on first launch.
- `HOME` must be set (used to resolve XDG paths and Claude transcripts).

**Optional env vars:**
- `AGENTUM_BACKEND` — Vite dev-server proxy target (`dashboard/vite.config.ts:7`). Default `http://127.0.0.1:8822`.
- `AGENTUM_TUI_NO_SOUND` — Mutes TUI chimes (`crates/agentum/src/commands/terminal/mod.rs:124`).
- `AGENTUM_THEME` — Overrides TUI theme (`crates/agentum/src/commands/terminal/theme.rs:297`).
- `SHELL` — Used by `TerminalAdapter` (`crates/agentum-executor/src/adapters.rs:305`).
- `EDITOR` / `VISUAL` — Used by `agentum config edit` (`crates/agentum/src/commands/config.rs:92`).
- `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` — Standard XDG overrides honored by `directories` 5.
- `TMUX` — Read to detect a nested tmux context (`crates/agentum/src/commands/terminal/app.rs:3548`).
- `PATH` — Probed by `which` for agent CLIs and by the TUI for sound players.

**Secrets location:**
- Daemon-side: SQLite `users.pw_hash` (Argon2id) and `auth_sessions.token` rows. File mode 0600.
- TUI client: per-profile bearer in `$XDG_CONFIG_HOME/agentum/profiles.toml` (and per-host `credentials.toml`).
- Dashboard client: `localStorage` (`agentum_profile_tokens`); never sent to the daemon's `/api/profiles` endpoint by design (see `crates/agentum-server/src/routes/profiles.rs` doc comment lines 1–10).
- No `.env` files; no dotenv loader; no cloud secret manager.

## Subprocess / OS Integrations

These are first-class operational dependencies of the daemon, not networked APIs:

- **tmux** — Hard dependency. Wrapped in `crates/agentum-tmux/src/lib.rs` via `tokio::process::Command`. Every session = one tmux session = one pane. Commands invoked: `has-session`, `new-session`, `send-keys`, `capture-pane`, `pipe-pane`, `kill-session`, `resize-window`. Sessions are addressed as `agentum-<name>` (`target_for` in the same file).
- **hostname** (system command) — Run once at boot to populate `AppState.hostname` (`crates/agentum-server/src/lib.rs::detect_short_hostname`).
- **which** (programmatic, via the `which` crate) — Used by `/api/agents` (`crates/agentum-server/src/routes/agents.rs`) and `agentum doctor` (`crates/agentum/src/commands/doctor.rs`).
- **lf** (terminal file manager) — Optional, used only when `agentum new --pick` is invoked (`crates/agentum/src/cli.rs:42-43`).
- **External "self" binary** — `agentum update` execs into a freshly downloaded `agentum` via the `exec` crate, replacing the current process (`crates/agentum/src/commands/update.rs`).

## Webhooks & Callbacks

**Incoming:** None. agentum exposes a REST + WS API; there is no inbound webhook endpoint.

**Outgoing:** None from the daemon itself. Any outbound HTTP made during a session originates inside the spawned agent CLI (e.g. Claude reaching Anthropic), not from agentum.

## HTTP / WS API Surface

All routes live in `crates/agentum-server/src/routes/`. Public unless noted otherwise (see `auth.rs::is_public`); all others require `Authorization: Bearer <token>` (or `?token=` for WS).

| Path | Method(s) | Notes |
|------|-----------|-------|
| `/api/health` | GET | Public. Returns `{status, version, uptime_seconds, sessions_running, capabilities, hostname}`. |
| `/api/cert` | GET | Public (served by cert-server on side port). Returns PEM. |
| `/api/cert/fingerprint` | GET | Public. SHA-256 fingerprint of the live TLS cert. |
| `/api/auth/status` | GET | Public. Reports whether any users exist. |
| `/api/auth/register` | POST | Public *only* when no users exist; the handler enforces this. |
| `/api/auth/login` | POST | Public. Rate-limited per IP. |
| `/api/auth/logout` | POST | Authed. |
| `/api/auth/me` | GET | Authed. |
| `/api/sessions` | GET, POST | Authed. CRUD entry point. |
| `/api/sessions/{id}` | GET, PATCH, DELETE | Authed. |
| `/api/sessions/{id}/start\|stop\|kill\|send` | POST | Authed. Lifecycle controls. |
| `/api/sessions/{id}/stream` | WS GET | Authed (via `?token=`). Pane bytes + PTY resize. |
| `/api/events` | WS GET | Authed (via `?token=`). Broadcast bus of `Event`s. |
| `/api/sessions/{id}/agent-tasks` | GET | Authed. Tail of plan/todos/tasks. |
| `/api/agents` | GET | Authed. Probes installed agent binaries. |
| `/api/host/metrics` | GET | Authed. CPU/RAM snapshot. |
| `/api/fs/list` | GET | Authed. Workdir picker. |
| `/api/board[/…]` | GET, POST, PATCH, DELETE | Authed. Kanban-style board. |
| `/api/board/rules` | GET, PUT | Authed. Per-column required-field overrides. |
| `/api/notes[/…]` | various | Authed. |
| `/api/channels[/…]` | various | Authed. |
| `/api/watchdog[/…]` | various | Authed. |
| `/api/doctor` | GET | Authed. Diagnostics. |
| `/api/preferences[/…]` | various | Authed. |
| `/api/profiles` | GET, POST | Authed. Connection-profile sync (no tokens). |
| `/api/profiles/default` | PUT | Authed. Default-profile pointer. |
| `/api/profiles/{name}` | PUT, DELETE | Authed. |
| `*` (anything else) | GET | Falls back to `embed::static_handler` → SvelteKit shell. |

CORS is permissive (`tower_http::cors::Any` for origin / methods / headers) without `Allow-Credentials`, so cross-origin bearer-protected access stays safe (`crates/agentum-server/src/lib.rs:189-203`).

---

*Integration audit: 2026-05-20*
