# Architecture

**Analysis Date:** 2026-05-20

## Pattern Overview

**Overall:** Layered, single-process daemon (axum HTTP/WS server) fronting a multi-crate Rust workspace, with a thick browser SPA and a thick TUI as two equal clients of one HTTP/WS API. Domain logic stays inside the daemon; both clients are dumb.

**Key Characteristics:**
- **One daemon, many clients.** `agentum serve` owns the SQLite DB, the tmux server, and the watchdog. The SvelteKit SPA (embedded into the binary via `rust-embed`), the `agentum terminal` TUI (`crates/agentum/src/commands/terminal/`), and `lazyagentum` (`crates/agentum/src/bin/lazyagentum.rs`) all reach it over the same HTTP/WS surface.
- **Trait-driven tool integration.** Every AI CLI (Claude, Codex, Cursor, Gemini, Hermes, …) implements `ToolAdapter` in `crates/agentum-executor/src/adapters.rs`; the rest of the codebase talks to the trait only, so a session row is tool-agnostic until launch time.
- **tmux as the process supervisor.** Each session is one detached tmux pane. The daemon never `exec`s an agent directly; it shells out via `crates/agentum-tmux/src/lib.rs` for spawn, pipe, capture, send-keys, and resize.
- **Event-sourced UI updates.** A single `tokio::sync::broadcast::Sender<Event>` channel (`AppState::bus`, `crates/agentum-server/src/lib.rs:69`) fans watchdog + lifecycle events to every connected WS client. Events are also persisted to the `events` table.
- **Background reconciliation.** A `Watchdog` task (`crates/agentum-watchdog/src/lib.rs`) polls the DB every 5 s and the running-session panes every 1 s, emitting `agent.finished`, `agent.awaiting_input`, `session.crashed`, `watchdog.compact`, etc.
- **TLS by default.** Self-signed cert + cert-server side port for trust-on-first-use; bearer-token middleware sits in front of every `/api/*` route except a small public allow-list.

## Layers

**`agentum-core` — domain types:**
- Purpose: Dependency-light shared types (`Session`, `Status`, `Event`, `BoardItem`, `NewBoardItem`, `BoardPatch`, `User`, `Channel`, `Note`, `WatchdogEvent`, plus the transcript types). No tokio, no sqlx, no axum.
- Location: `crates/agentum-core/src/lib.rs`, `crates/agentum-core/src/board_schema.rs`, `crates/agentum-core/src/profiles.rs`, `crates/agentum-core/src/transcript.rs`.
- Contains: enums, structs with serde derives, validation helpers (`validate_name`, `validate_username`), the per-status board-rules matrix (`required_fields_for`, `validate_transition`).
- Depends on: `serde`, `time`, `uuid`, `thiserror`, `toml`. Nothing application-shaped.
- Used by: every other crate.

**`agentum-store` — SQLite persistence:**
- Purpose: All persistence behind a single `Store` handle (`SqlitePool`). WAL mode, `synchronous=NORMAL`, file chmod 0600 because it holds Argon2id hashes + live bearer tokens.
- Location: `crates/agentum-store/src/lib.rs` (2092 lines — sessions, board, board comments, board column rules, notes, channels, events, users, auth_sessions, preferences).
- Contains: `Store::open`, per-table CRUD methods, `update_status_and_target`, `latest_agent_event_per_session`, `sweep_expired_auth_sessions`, XDG path resolution in `crates/agentum-store/src/paths.rs`.
- Migrations: 14 SQL files in `crates/agentum-store/migrations/` (`0001_initial.sql` … `0014_board_column_rules.sql`) baked in via `sqlx::migrate!("./migrations")`.
- Depends on: `agentum-core`, `sqlx`, `directories`.
- Used by: `agentum-server`, `agentum-watchdog`, `agentum` (CLI commands open the store directly for offline ops like `agentum auth setup`).

**`agentum-tmux` — process supervisor adapter:**
- Purpose: Thin shell-out wrapper over the `tmux` binary. No state.
- Location: `crates/agentum-tmux/src/lib.rs`.
- Contains: `target_for`, `has_session`, `new_session`, `kill_session`, `capture_pane`, `capture_pane_visible`, `capture_pane_ansi`, `send_keys`, `send_bytes`, `resize_window`, `pipe_pane`, `pane_current_command`, `pane_pid`, `graceful_stop`.
- Depends on: `tokio::process::Command`, `shlex` (only for the single shell-command string handed to `tmux new-session` / `pipe-pane`). No other agentum crate.
- Used by: `agentum-server` (start/stop/send/stream routes), `agentum-watchdog` (capture + send-keys for `/compact`), `agentum` CLI (the `agentum send`, `agentum keys`, `agentum open` commands).

**`agentum-executor` — tool-adapter abstraction:**
- Purpose: A `ToolAdapter` trait per supported agent. Maps a `Session` to a concrete `LaunchCommand { argv, env }`. Owns YOLO-marker translation across tools.
- Location: `crates/agentum-executor/src/lib.rs` (trait + registry), `crates/agentum-executor/src/adapters.rs` (built-ins).
- Contains: `ToolAdapter` trait, `LaunchCommand`, `YOLO_MARKER` constant, `translate_yolo_marker`, `adapter_for(tool)`, `FIRST_CLASS` + `PASSTHROUGH_PROBED` lists, `probed_tools()`, `binary_for(tool)`.
- Adapters: `ClaudeAdapter`, `CodexAdapter`, `CursorAdapter`, `AgentAdapter`, `GeminiAdapter`, `HermesAdapter`, `TerminalAdapter`, `PassthroughAdapter`.
- Depends on: `agentum-core` only.
- Used by: `agentum-server::routes::sessions` (at `start`), `agentum-watchdog` (reads `compact_trigger`, `crash_signatures`, `busy_signature`, `awaiting_input_signatures`, `is_agent`), `agentum-server::routes::agents` (probes binaries via `which`).

**`agentum-watchdog` — per-session reconciler:**
- Purpose: One background task per running session. Captures pane every 1 s; emits events for context-low compaction, crashes, busy↔idle, awaiting-input.
- Location: `crates/agentum-watchdog/src/lib.rs`.
- Contains: `Watchdog::new`, `Watchdog::run` (reconcile loop on `RECONCILE_TICK = 5 s`), `watch_session` (per-session loop on `TICK = 1 s`), `classify_activity`, `ActivityState`, `bottom_lines`, `hash_str`, `canonical_tool_from_command`.
- Reconcile model: diff DB's `Status::Running` set against the in-memory `HashMap<Uuid, JoinHandle>`. Spawn missing tasks; abort orphans.
- Per-tick actions: crash-signature match → mark `crashed`; `Context low.*<\s*50%` regex → `send_keys(compact_trigger, Enter)` with 5-min cooldown; tool-drift detection via `pane_current_command`; activity classification → `agent.finished` / `agent.awaiting_input` / `agent.input_resolved` events.
- Depends on: `agentum-core`, `agentum-store`, `agentum-tmux`, `agentum-executor`, `regex`.
- Used by: `agentum-server::serve` spawns one of these alongside the HTTP server.

**`agentum-server` — HTTP/WS API + SPA:**
- Purpose: axum HTTP+WS API, TLS termination, auth middleware, embedded SvelteKit SPA, cert-server side port for TOFU bootstrap.
- Location: `crates/agentum-server/src/lib.rs` (entry + AppState + serve loop), `crates/agentum-server/src/routes/*.rs` (17 route modules), `crates/agentum-server/src/auth.rs`, `crates/agentum-server/src/tls.rs`, `crates/agentum-server/src/headers.rs`, `crates/agentum-server/src/embed.rs`, `crates/agentum-server/src/transcript_store.rs`, `crates/agentum-server/src/rules.rs`, `crates/agentum-server/src/ratelimit.rs`, `crates/agentum-server/src/error.rs`.
- Contains: `AppState` (Store + broadcast bus + TranscriptStore + cert fingerprint + rate limiter + hostname), `serve(opts, store)`, `router(state)`, `static_handler` (the SPA fallback).
- Depends on: every other crate plus `axum`, `axum-server`, `rust-embed`, `notify`, `rcgen`, `rustls`, `argon2`, `sysinfo`, `which`, `tower-http`.
- Used by: `agentum serve` CLI command (`crates/agentum/src/commands/serve.rs`).

**`agentum` — CLI binary + TUI:**
- Purpose: Two binaries (`agentum`, `lazyagentum`) sharing a library shim (`crates/agentum/src/lib.rs`). Houses subcommands + the ratatui TUI.
- Location: `crates/agentum/src/main.rs` (entry), `crates/agentum/src/cli.rs` (clap definitions + `dispatch`), `crates/agentum/src/commands/` (one file per subcommand), `crates/agentum/src/commands/terminal/` (the TUI app).
- TUI: `crates/agentum/src/commands/terminal/mod.rs` boots the alt-screen + connect-or-onboard loop; `app.rs` holds state + event loop; `ui.rs` draws the panes; `api.rs` is the HTTP/WS client; `pty.rs` spawns the local lazygit pane; `prefs.rs` + `profiles.rs` persist per-host UX state; `trust.rs` is the SSH-style cert pinner; `theme.rs` + `palette.rs` + `extensions.rs` are pure UX.
- Depends on: every other crate plus `ratatui`, `crossterm`, `tui-term`, `vt100`, `portable-pty`, `reqwest`, `tokio-tungstenite`, `tokio-rustls`, `clap`, `rpassword`, `url`.

## Data Flow

**Session creation + start (POST /api/sessions + POST /api/sessions/{id}/start):**

1. Client (dashboard or TUI) posts `NewSession { name, workdir, tool, model?, flags[] }` to `/api/sessions`. Handler: `create` in `crates/agentum-server/src/routes/sessions.rs:66`.
2. `create` validates workdir existence and calls `Store::create_session` (`crates/agentum-store/src/lib.rs:87`) — INSERT with status `idle`. Returns 201.
3. Client posts to `/api/sessions/{id}/start`. Handler: `start` in `crates/agentum-server/src/routes/sessions.rs:220`.
4. `start` calls `agentum_executor::adapter_for(session.tool)` and `.launch(&session)` → `LaunchCommand { argv, env }`. YOLO marker translation happens inside `launch()` via `translate_yolo_marker`.
5. `agentum_tmux::new_session(target, workdir, argv, env)` shells out to `tmux new-session -d -s agentum-<name> …`.
6. `agentum_tmux::pipe_pane(target, pane_log_path)` starts streaming the pane to `$XDG_CACHE_HOME/agentum/sessions/<id>.log`.
7. `Store::update_status_and_target(id, Running, Some(&target))` flips the row.
8. The watchdog's next 5-second reconcile pass spots the new `running` session and spawns its per-session `watch_session` task.

**Watchdog → event stream → client:**

1. `watch_session` (`crates/agentum-watchdog/src/lib.rs:123`) ticks every 1 s. Captures `pane` (100 lines incl. scrollback) and `viewport` (visible only).
2. Crash signature match → updates status to `crashed`, calls `emit(&bus, &store, Event::new("session.crashed"))`.
3. `emit` writes the event to the `events` table via `Store::insert_event` AND broadcasts it on `AppState::bus` (a `tokio::sync::broadcast::Sender<Event>` with capacity 1024).
4. Every connected WS to `/api/events` (`crates/agentum-server/src/routes/events.rs`) holds a `broadcast::Receiver<Event>` and forwards each JSON-serialised event to the client.
5. On WS open, the events route first replays one "current state" `agent.*` event per session (marked `replay: true`) so a freshly-connected client gets a non-flickering snapshot before live events resume.

**Terminal stream (WS /api/sessions/{id}/stream):**

1. Client opens WS with optional `?resume=true`. Handler: `stream` → `stream_session` in `crates/agentum-server/src/routes/sessions.rs:411`.
2. Server waits up to 250 ms for the client's first `{"resize":{"cols":C,"rows":R}}` text frame, then calls `agentum_tmux::resize_window` so embedded TUIs render at the right grid before the snapshot.
3. Server either replays from `AppState::stream_positions` (resume path) or snapshots via `capture_pane_ansi` + tails the pane log file growing from `pipe_pane`.
4. Client keystrokes / bytes flow back via `tmux send-keys -H` (`agentum_tmux::send_bytes`) for raw bytes, or `send_keys` for named keys.

**Transcript watching (Claude Code plan/todos/tasks panel):**

1. `TranscriptStore::ensure_started(session_id, workdir, tool)` is called lazily from `/api/sessions` list/get handlers (`crates/agentum-server/src/routes/sessions.rs:58`). Short-circuits for non-Claude tools.
2. The store spawns a `notify` filesystem watcher on `~/.claude/projects/<encoded-workdir>/` and tails `<session_id>.jsonl`.
3. New transcript lines are parsed by `agentum_core::transcript::apply_line` (handles `TaskCreate`, `TaskUpdate`, `TodoWrite`, `ExitPlanMode`, `Agent`/`Task` subagent dispatch, `<command-name>/clear</command-name>` slash-command resets).
4. Updated `AgentTaskState` is cached per-session and broadcast as an `agent_tasks.updated` event on `AppState::bus`.

**State Management:**
- Persistent: SQLite (`Store`). Tables: sessions, events, board_items, board_comments, board_column_rules, notes, channels, channel_messages, users, auth_sessions, preferences.
- In-memory only: `AppState::transcripts` (transcript snapshots), `AppState::stream_positions` (per-session WS replay markers), `AppState::auth_limiter` (rate limiter), `Watchdog::tasks` (per-session task handles).
- Client-local: TUI `~/.config/agentum/profiles.toml` + `credentials.toml`; dashboard `localStorage` (`agentum_profile_tokens`, `agentum_profile_labels`, `agentum_profile_cache`, `agentum_active`).

## Key Abstractions

**`ToolAdapter` (trait):**
- Purpose: Single source of truth for per-agent launch semantics + watchdog signatures.
- Location: `crates/agentum-executor/src/lib.rs:38`.
- Pattern: trait with default-empty methods so each new adapter is a ~30-line file. Methods: `name()`, `launch(&Session) -> LaunchCommand`, `compact_trigger()`, `crash_signatures()`, `busy_signature()`, `awaiting_input_signatures()`, `yolo_flag()`, `is_agent()`.
- Examples: `crates/agentum-executor/src/adapters.rs` — `ClaudeAdapter` (lines 41–141, full instrumentation), `CodexAdapter` (lines 145–174), `CursorAdapter`, `AgentAdapter`, `GeminiAdapter`, `HermesAdapter`, `TerminalAdapter`, `PassthroughAdapter` (catch-all for unknown tools).

**`Session` (struct):**
- Purpose: The atomic unit of agentum. A `(name, workdir, tool, model?, flags[])` tuple plus runtime observability fields.
- Location: `crates/agentum-core/src/lib.rs:67`.
- Pattern: serde-derive struct used everywhere — DB row, HTTP payload, WS event payload, TUI state, dashboard state. Optional fields (`tokens`, `cost_usd`, `ctx`, `last_log`, `uptime_seconds`, `state`, `pinned`) are `skip_serializing_if = "Option::is_none"` for compact wire.

**`Event` (struct):**
- Purpose: The lingua franca for everything happening in the daemon. Dotted `kind` strings (`session.started`, `session.crashed`, `agent.finished`, `agent.awaiting_input`, `agent.input_resolved`, `watchdog.compact`, `agent_tasks.updated`, `host.metrics`, `bus.lagged`).
- Location: `crates/agentum-core/src/lib.rs:165`.
- Pattern: `Event::new(kind).with_session(id, name).with_payload(json!({…}))`. Both persisted (via `Store::insert_event`) and broadcast on `AppState::bus`. Projects to `WatchdogEvent` for the dashboard's compact wire shape.

**`AppState` (struct):**
- Purpose: Shared handler-state for every axum route. Cloneable (cheap — wraps `Arc`s).
- Location: `crates/agentum-server/src/lib.rs:67`.
- Pattern: holds `Arc<Store>`, `broadcast::Sender<Event>`, `Arc<RateLimiter>`, `Arc<String>` (cert fingerprint), `TranscriptStore`, `Arc<Mutex<HashMap<Uuid, StreamCheckpoint>>>`, `hostname: String`, `no_auth: bool`.

**`LaunchCommand` (struct):**
- Purpose: The contract between an adapter and tmux: what argv to exec, what env to inject.
- Location: `crates/agentum-executor/src/lib.rs:22`.

**`Profiles` (struct, shared):**
- Purpose: TUI's named-endpoint list (URL + optional fingerprint + insecure flag). Persisted to `$XDG_CONFIG_HOME/agentum/profiles.toml`.
- Location: `crates/agentum-core/src/profiles.rs`. Dashboard mirrors the same wire shape; `/api/profiles` (`crates/agentum-server/src/routes/profiles.rs`) is the canonical sync point.

## Entry Points

**`agentum` binary (CLI + TUI dispatcher):**
- Location: `crates/agentum/src/main.rs` → `crates/agentum/src/cli.rs::dispatch`.
- Triggers: User invocation (`agentum new`, `agentum serve`, `agentum terminal`, …).
- Responsibilities: parse clap args, route to the matching `crates/agentum/src/commands/*.rs` module. `Cmd::Terminal` swaps tracing to a file before entering the alt-screen (stderr would scramble ratatui).

**`agentum serve` (daemon):**
- Location: `crates/agentum/src/commands/serve.rs::run` → `agentum_server::serve` (`crates/agentum-server/src/lib.rs:210`).
- Triggers: `agentum serve [--detach]`.
- Responsibilities: open the store, optionally re-spawn previously-stopped sessions, generate or load TLS cert, build `AppState`, spawn the auth-session sweeper, spawn the watchdog, spawn the host-metrics ticker, bind axum on the main port + the cert-server on the side port.

**`lazyagentum` binary:**
- Location: `crates/agentum/src/bin/lazyagentum.rs`.
- Triggers: User invocation (`lazyagentum`). Equivalent to `agentum terminal` with no other subcommands.
- Responsibilities: re-uses `agentum::commands::terminal::run`.

**`agentum terminal` TUI:**
- Location: `crates/agentum/src/commands/terminal/mod.rs::run` → `crates/agentum/src/commands/terminal/app.rs`.
- Triggers: `agentum terminal [--api … | --profile …]` or the `lazyagentum` binary.
- Responsibilities: connect-or-onboard loop (interactive prompt when daemon unreachable on a TTY), TLS trust pin, login flow if the daemon requires it, then enter the alt-screen ratatui loop that subscribes to `/api/events` and per-session `/stream` WS endpoints.

**SvelteKit SPA:**
- Location: `dashboard/src/routes/+layout.svelte` (chrome) + `dashboard/src/routes/*/+page.svelte` (pages: `home`, `sessions`, `sessions/[id]`, `board`, `terminals`, `settings`).
- Triggers: Any non-API HTTP path served by the daemon — `crates/agentum-server/src/embed.rs::static_handler` resolves the request against the embedded `dashboard/build/` tree, falling back to `index.html` for client-routed paths.
- Responsibilities: `+layout.svelte` boots `TokenGate`, connects `/api/events`, starts the host-metrics, attention, event-bridge, and theme-bridge stores. Each route is a self-contained `+page.svelte`.

## Error Handling

**Strategy:** Crate-local `thiserror` enums, converted to `ApiError` at the HTTP boundary, surfaced as `IntoResponse` with a `{ "error": msg }` envelope.

**Patterns:**
- **Crate-level error enums:** `CoreError` (`crates/agentum-core/src/lib.rs:21`), `StoreError` (`crates/agentum-store/src/lib.rs:21`), `TmuxError` (`crates/agentum-tmux/src/lib.rs:14`), `WatchdogError` (`crates/agentum-watchdog/src/lib.rs:50`), `TlsError` (`crates/agentum-server/src/tls.rs:14`), `AuthError` (`crates/agentum-server/src/auth.rs:27`).
- **HTTP error envelope:** `ApiError` (`crates/agentum-server/src/error.rs:8`) — variants `NotFound`, `Conflict`, `BadRequest`, `Unauthorized`, `Forbidden`, `TooManyRequests`, `Internal`, plus `Custom(StatusCode, Value)` for handlers that need a non-default JSON body (board column-rule validators return `{ "missing": [...], "status": "doing" }`). `From<StoreError> for ApiError` maps `NotFound`/`AlreadyExists`/`Core` to user-visible statuses; everything else logs at `error` and surfaces as 500.
- **anyhow at the CLI edge:** `crates/agentum/src/commands/*.rs` use `anyhow::Result<()>` with `Context::context(…)` annotations.
- **Watchdog never panics, only logs:** failed `tmux` calls inside `watch_session` use `tracing::warn!` and `continue` rather than aborting the task. Crashes are reported via `Event::new("session.crashed")` and a status update.

## Cross-Cutting Concerns

**Authentication:**
- Username/password with Argon2id hashes stored in the `users` table (`crates/agentum-server/src/auth.rs`).
- Each login mints a 32-byte URL-safe random token, stored in `auth_sessions` with sliding 30-day expiry. Bearer presented as `Authorization: Bearer …` (HTTP) or `?token=…` (WS, since browsers can't set headers on upgrade).
- `require_token` middleware (`crates/agentum-server/src/auth.rs:150`) is layered onto every router merge. Public allow-list: `/api/health`, `/api/cert`, `/api/cert/fingerprint`, `/api/auth/status`, `/api/auth/login`, `/api/auth/register`.
- `--no-auth` flag bypasses the middleware entirely (the dashboard auto-skips the login screen).

**TLS:**
- Self-signed cert generated on first boot and cached at `$XDG_DATA_HOME/agentum/tls/{cert,key}.pem` (`crates/agentum-server/src/tls.rs`).
- SHA-256 fingerprint printed to the host TTY on boot and served from the plain-HTTP cert-server on the side port (default 8823) for trust-on-first-use bootstrap from a phone.
- HSTS deliberately omitted (self-signed + HSTS = footgun across cert rotations).

**Logging:**
- `tracing` everywhere; subscriber configured by `crates/agentum/src/lib.rs::init_tracing` (stderr, EnvFilter via `AGENTUM_LOG`) or `init_tracing_for_tui` (file at `$XDG_CACHE_HOME/agentum/tui.log` — never stderr, which would scramble the alt-screen).
- HTTP requests run through `crates/agentum-server/src/logging.rs::redacting_trace_layer()` (a `tower-http` `TraceLayer` that strips `Authorization` headers and `?token=` query params).

**Security headers:**
- Every HTTP response gets CSP, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Cross-Origin-Opener-Policy: same-origin` via the middleware in `crates/agentum-server/src/headers.rs`.
- CSP allows `connect-src 'self' http: https: ws: wss:` so the dashboard can fan out to multiple agentum endpoints from one origin (named-profiles feature).

**Rate limiting:**
- Login + register attempts: 8 per remote IP per 5-minute window (`crates/agentum-server/src/lib.rs:60`, enforced via `crates/agentum-server/src/ratelimit.rs`).

**Filesystem layout:**
- All XDG paths funnel through `crates/agentum-store/src/paths.rs`: `data_dir`, `config_dir`, `cache_dir`, `state_dir`, `db_path`, `auth_token_path`, `tls_dir`, `pane_log(session_id)`.

**Event broadcast:**
- One bus (`tokio::sync::broadcast::Sender<Event>`, capacity 1024) drives every push channel. Slow consumers see `RecvError::Lagged(n)` and a `bus.lagged` synthetic event before the stream resumes (`crates/agentum-server/src/routes/events.rs:85`).

**Build-time SPA embedding:**
- `crates/agentum-server/src/embed.rs` uses `rust-embed` to inline `dashboard/build/` into the binary. `crates/agentum-server/build.rs` materialises a placeholder stub if the user runs `cargo build` before `pnpm --dir dashboard build`. Static-asset cache: `_app/immutable/*` gets `public, max-age=31536000, immutable`; everything else gets `no-cache`. SPA fallback always returns `index.html` so the SvelteKit client router takes over.

---

*Architecture analysis: 2026-05-20*
