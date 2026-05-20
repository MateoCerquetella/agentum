# Codebase Structure

**Analysis Date:** 2026-05-20

## Directory Layout

```
agentum/
├── Cargo.toml                 # Workspace root: 7 member crates, shared deps
├── Cargo.lock                 # Committed — CI ships release binaries
├── CLAUDE.md                  # Authoritative architecture guide (this doc reflects it)
├── CHANGELOG.md               # Per-release notes
├── PRD.md                     # Product spec
├── README.md                  # User-facing overview
├── rust-toolchain.toml        # Pin: rust 1.85 / edition 2024
├── justfile                   # Convenience task runner
├── crates/
│   ├── agentum-core/          # Shared domain types (no tokio/sqlx/axum)
│   ├── agentum-store/         # SQLite persistence (sqlx), XDG paths, migrations
│   ├── agentum-tmux/          # Thin wrapper over the tmux binary
│   ├── agentum-executor/      # ToolAdapter trait + per-agent argv builders
│   ├── agentum-watchdog/      # Per-session reconcile + activity classification
│   ├── agentum-server/        # axum HTTP/WS server, TLS, auth, embedded SPA
│   └── agentum/               # CLI binary (`agentum`) + standalone TUI (`lazyagentum`)
├── dashboard/                 # SvelteKit SPA — embedded into the daemon binary
│   ├── src/
│   │   ├── routes/            # SvelteKit pages
│   │   ├── lib/
│   │   │   ├── components/    # Sidebar, Topbar, NewSessionDialog, …
│   │   │   ├── stores/        # Reactive state (sessions, events, fleet, …)
│   │   │   ├── themes/        # Per-theme CSS
│   │   │   ├── api.ts         # Fetch wrapper + types
│   │   │   └── profiles.ts    # Named-endpoint store + URL builders
│   │   ├── app.css            # Global styles
│   │   ├── app.html           # SSR shell
│   │   └── service-worker.ts  # SW that notifies controlled tabs on redeploy
│   ├── static/                # Static assets copied verbatim
│   ├── build/                 # SvelteKit output — embedded into the daemon
│   ├── package.json
│   ├── svelte.config.js
│   ├── vite.config.ts
│   └── tsconfig.json
├── docs/                      # Long-form design notes + Superpowers specs
├── scripts/
│   └── install.sh             # Single-shot installer (also driven by `agentum update`)
├── web/                       # Marketing landing page (index.html + sitemap)
├── .github/
│   └── workflows/             # CI + release pipelines
├── .planning/                 # GSD planning artifacts (this file lives here)
└── .agents/skills/            # Project-specific Claude skills (not architectural)
```

## Directory Purposes

**`crates/agentum-core/`:**
- Purpose: dependency-light shared domain types.
- Contains: `Session`, `Status`, `SessionState`, `Event`, `WatchdogEvent`, `BoardItem`, `NewBoardItem`, `BoardPatch`, `BoardComment`, `ReorderEntry`, `Note`, `NotePatch`, `Channel`, `Message`, `User`, transcript types (`TodoItem`, `TaskRecord`, `AgentTaskState`), board-rules schema (`RequiredField`, `TransitionCtx`, `validate_transition`), profiles loader (`Profiles`, `ProfilesFile`, `Profile`, `is_valid_name`).
- Key files: `crates/agentum-core/src/lib.rs`, `crates/agentum-core/src/board_schema.rs`, `crates/agentum-core/src/profiles.rs`, `crates/agentum-core/src/transcript.rs`.

**`crates/agentum-store/`:**
- Purpose: All SQLite I/O behind a single `Store` handle.
- Contains: 14 numbered migrations in `crates/agentum-store/migrations/` (`0001_initial.sql` … `0014_board_column_rules.sql`), `Store::open`, per-table CRUD, XDG path resolution.
- Key files: `crates/agentum-store/src/lib.rs` (2092 lines), `crates/agentum-store/src/paths.rs`.

**`crates/agentum-tmux/`:**
- Purpose: tmux subprocess adapter — no state.
- Contains: `new_session`, `kill_session`, `capture_pane{,_visible,_ansi}`, `send_keys`, `send_bytes`, `resize_window`, `pipe_pane`, `pane_current_command`, `pane_pid`, `graceful_stop`.
- Key file: `crates/agentum-tmux/src/lib.rs`.

**`crates/agentum-executor/`:**
- Purpose: `ToolAdapter` abstraction.
- Contains: trait + `LaunchCommand` + adapter registry in `lib.rs`; one struct per adapter in `adapters.rs`; YOLO-marker translation.
- Key files: `crates/agentum-executor/src/lib.rs`, `crates/agentum-executor/src/adapters.rs`.

**`crates/agentum-watchdog/`:**
- Purpose: Per-session reconciler + activity classifier.
- Contains: `Watchdog`, `watch_session` loop, `classify_activity`, `ActivityState`, `bottom_lines`, `hash_str`, `canonical_tool_from_command`, `intentionally_stopped`, `emit`.
- Key file: `crates/agentum-watchdog/src/lib.rs`.

**`crates/agentum-server/`:**
- Purpose: HTTP/WS surface + embedded SPA + TLS + auth.
- Contains:
  - `lib.rs` — `AppState`, `ServeOptions`, `router`, `serve`, cert-server bootstrap.
  - `routes/` — one module per resource (see "Key File Locations" below).
  - `auth.rs` — Argon2id + bearer middleware.
  - `tls.rs` — self-signed cert lifecycle + fingerprint helper.
  - `headers.rs` — CSP + security-header middleware.
  - `embed.rs` — `rust-embed` static handler for the SPA.
  - `transcript_store.rs` — in-memory per-session plan/todos/tasks cache with `notify` watchers.
  - `rules.rs` — board column-rule overrides (per-server custom required-fields).
  - `ratelimit.rs` — login/register rate limiter.
  - `error.rs` — `ApiError` enum + `IntoResponse`.
  - `logging.rs` — token-redacting tracing layer.
  - `build.rs` — placeholder stub for the embedded `dashboard/build/`.

**`crates/agentum/`:**
- Purpose: CLI binary `agentum` plus standalone `lazyagentum` shim, plus the entire TUI.
- Contains:
  - `main.rs` — entry, picks tracing target based on the command.
  - `cli.rs` — clap definitions + `dispatch`.
  - `lib.rs` — `init_tracing` + `init_tracing_for_tui` shims so both binaries share plumbing.
  - `commands/*.rs` — one file per subcommand (`new`, `up`, `down`, `kill`, `rm`, `ls`, `open`, `tail`, `send`, `keys`, `serve`, `auth`, `config`, `doctor`, `terminal`, `hosts`, `profiles`, `uninstall`, `update`).
  - `commands/terminal/` — the ratatui TUI (see "Special Directories").
  - `bin/lazyagentum.rs` — second binary that drops directly into the TUI.

**`dashboard/`:**
- Purpose: SvelteKit SPA, built statically (`adapter-static`) into `dashboard/build/`, embedded into the daemon binary at compile time.
- Build: `pnpm --dir dashboard install && pnpm --dir dashboard build`. After any change under `dashboard/src/` you MUST re-run `pnpm build` and then `cargo build` (the SPA is baked into the binary).
- Check: `npm run check --prefix dashboard` (`svelte-check` + tsc).

**`docs/`:**
- Purpose: design notes, plans, Superpowers specs. Not consumed at runtime.

**`scripts/`:**
- Purpose: installer (`install.sh`) — also re-fetched by `agentum update`.

**`web/`:**
- Purpose: static marketing site (separate from the dashboard).

**`.planning/`:**
- Purpose: GSD planning artifacts. `codebase/` (this directory), `specs/`, `phases/`, `debug/`, `todos/`.

**`.github/workflows/`:**
- Purpose: CI (`ci.yml`) + release pipeline (`release.yml`).

## Key File Locations

**Workspace entry / config:**
- `Cargo.toml`: workspace members, shared deps, release profile.
- `rust-toolchain.toml`: rust 1.85, edition 2024.

**Binary entries:**
- `crates/agentum/src/main.rs`: `agentum` binary (Tokio runtime + `cli::dispatch`).
- `crates/agentum/src/bin/lazyagentum.rs`: TUI-only binary.

**Daemon entry:**
- `crates/agentum/src/commands/serve.rs`: `agentum serve` command. Handles `--detach`, first-time-setup wizard, optional session auto-resume, then calls into `agentum-server`.
- `crates/agentum-server/src/lib.rs::serve` (line 210): the actual bind/listen loop.

**Core domain types:**
- `crates/agentum-core/src/lib.rs`: `Session`, `Event`, `Status`, `SessionState`, board types, validation helpers.
- `crates/agentum-core/src/transcript.rs`: Claude Code JSONL parser.
- `crates/agentum-core/src/profiles.rs`: TUI's named-endpoint loader.
- `crates/agentum-core/src/board_schema.rs`: per-status required-fields matrix.

**Persistence:**
- `crates/agentum-store/src/lib.rs`: the `Store` handle + every CRUD method.
- `crates/agentum-store/src/paths.rs`: XDG resolution.
- `crates/agentum-store/migrations/`: numbered SQL migrations, baked in at compile time via `sqlx::migrate!`.

**Tool integration:**
- `crates/agentum-executor/src/lib.rs`: `ToolAdapter` trait, `LaunchCommand`, `YOLO_MARKER`, `translate_yolo_marker`, `adapter_for`, `FIRST_CLASS`, `PASSTHROUGH_PROBED`, `probed_tools`, `binary_for`.
- `crates/agentum-executor/src/adapters.rs`: built-in `ClaudeAdapter`, `CodexAdapter`, `CursorAdapter`, `AgentAdapter`, `GeminiAdapter`, `HermesAdapter`, `TerminalAdapter`, `PassthroughAdapter`.

**Process supervision:**
- `crates/agentum-tmux/src/lib.rs`: every tmux shell-out.

**Watchdog:**
- `crates/agentum-watchdog/src/lib.rs`: `Watchdog::run`, `watch_session`, activity classifier.

**HTTP routes (`crates/agentum-server/src/routes/`):**
- `health.rs` — `GET /api/health` (public).
- `auth.rs` — `POST /api/auth/{login,register,logout}`, `GET /api/auth/{status,me}`.
- `cert.rs` — `GET /api/cert/fingerprint` (public).
- `sessions.rs` — full session CRUD + `/start`, `/stop`, `/kill`, `/send`, WS `/stream`. The fat one (915 lines).
- `events.rs` — WS `/api/events` global broadcast.
- `agents.rs` — `GET /api/agents` — probes installed CLIs.
- `agent_tasks.rs` — `GET /api/sessions/{id}/agent-tasks`.
- `board.rs` — kanban CRUD + claim + comments + reorder (1065 lines).
- `board_rules.rs` — per-server column-rule overrides.
- `notes.rs`, `channels.rs`, `host.rs`, `fs.rs`, `doctor.rs`, `watchdog.rs`, `preferences.rs`, `profiles.rs`.

**Server cross-cutting:**
- `crates/agentum-server/src/auth.rs`: bearer middleware (`require_token`), Argon2id hashing.
- `crates/agentum-server/src/tls.rs`: self-signed cert lifecycle.
- `crates/agentum-server/src/headers.rs`: CSP + security headers.
- `crates/agentum-server/src/embed.rs`: SPA static handler.
- `crates/agentum-server/src/transcript_store.rs`: per-session plan/todos cache.
- `crates/agentum-server/src/error.rs`: `ApiError`.
- `crates/agentum-server/src/ratelimit.rs`: per-IP login limiter.
- `crates/agentum-server/src/rules.rs`: board column-rule overrides store.

**CLI commands (`crates/agentum/src/commands/`):**
- `mod.rs`: re-exports + `open_store()` helper.
- `serve.rs`: daemon launcher.
- `new.rs`, `up.rs`, `down.rs`, `kill.rs`, `rm.rs`, `ls.rs`, `open.rs`, `tail.rs`, `send.rs`, `keys.rs` — session-level commands.
- `auth.rs`, `config.rs`, `hosts.rs`, `profiles.rs`, `doctor.rs`, `uninstall.rs`, `update.rs` — admin commands.
- `terminal/` — see "Special Directories".

**TUI (`crates/agentum/src/commands/terminal/`):**
- `mod.rs`: connect-or-onboard loop, profile resolution, alt-screen lifecycle.
- `app.rs` (8290 lines): state, key dispatch, event loop, notification stack.
- `ui.rs` (3059 lines): ratatui draw functions.
- `api.rs`: HTTP/WS client (`Client`, `TermOut`, `TerminalMsg`, `EventMsg`).
- `pty.rs`: local PTY for the lazygit side-pane.
- `term.rs`: pane-state model (`TerminalPane`).
- `theme.rs`, `palette.rs`, `iometer.rs`, `extensions.rs`, `prefs.rs`, `profiles.rs`, `trust.rs`, `sound.rs`.

**Dashboard (`dashboard/src/`):**
- `routes/+layout.svelte` — chrome (Topbar + Sidebar + ToastStack + CommandPalette + ShortcutSheet + NewSessionDialog).
- `routes/+page.svelte` — landing redirect.
- `routes/home/+page.svelte`, `routes/sessions/+page.svelte`, `routes/sessions/[id]/+page.svelte`, `routes/board/+page.svelte`, `routes/terminals/+page.svelte`, `routes/settings/+page.svelte` — top-level pages.
- `lib/api.ts` — typed fetch wrapper; the `api` object holds every endpoint.
- `lib/profiles.ts` — named-endpoint store + `apiUrl`, `wsUrl`, `apiUrlForProfile`, `wsUrlForProfile`, `fetchProfile`.
- `lib/components/` — Svelte components.
- `lib/stores/` — Svelte writable stores (`sessions`, `events`, `fleet`, `host`, `attention`, `notify`, `palette`, `theme-bridge`, `event-bridge`, `profile-bridge`, …).
- `lib/themes/` — per-theme CSS.
- `service-worker.ts` — sends `sw:updated` to controlled tabs after a daemon redeploy so they auto-reload.

## Naming Conventions

**Files (Rust):**
- snake_case `.rs` files. Pattern matches the module path 1:1 (`crates/agentum-server/src/routes/board.rs` is `crate::routes::board`).

**Files (Svelte / TypeScript):**
- Components: PascalCase `.svelte` (`Sidebar.svelte`, `NewSessionDialog.svelte`).
- Stores + libs: kebab-case or single-word camelCase `.ts` (`event-bridge.ts`, `api.ts`, `newSession.ts`).
- Routes: SvelteKit conventions (`+page.svelte`, `+layout.svelte`, `+layout.ts`, `[id]/`).

**Crates:**
- Workspace prefix `agentum-` for libraries (`agentum-core`, `agentum-store`, …). The `agentum` CLI crate has no suffix.

**HTTP routes:**
- All under `/api/`. Plural-noun-as-collection (`/api/sessions`, `/api/board`, `/api/notes`). Per-item: `/api/sessions/{id}`. Sub-actions: `/api/sessions/{id}/start`, `/api/board/{id}/claim`. WS upgrades on `/stream`, `/events`.

**Events:**
- Dotted `kind` strings, namespace-first: `session.started`, `session.crashed`, `session.tool_changed`, `session.stopped`, `agent.finished`, `agent.awaiting_input`, `agent.input_resolved`, `watchdog.compact`, `agent_tasks.updated`, `host.metrics`, `bus.lagged`.

**SQLite tables:**
- Plural snake_case (`sessions`, `events`, `board_items`, `board_comments`, `board_column_rules`, `notes`, `channels`, `channel_messages`, `users`, `auth_sessions`, `preferences`).

**XDG storage:**
- Data: `$XDG_DATA_HOME/agentum/db.sqlite`, `$XDG_DATA_HOME/agentum/tls/{cert,key}.pem`.
- Config: `$XDG_CONFIG_HOME/agentum/{profiles.toml,known_hosts.toml,credentials.toml}`.
- Cache: `$XDG_CACHE_HOME/agentum/sessions/<id>.log`, `$XDG_CACHE_HOME/agentum/tui.log`.
- State (Linux only): `$XDG_STATE_HOME/agentum/daemon.log`.

## Where to Add New Code

**New tool adapter (e.g. supporting a new AI CLI):**
- Primary code: `crates/agentum-executor/src/adapters.rs` — append a `pub struct FooAdapter; impl ToolAdapter for FooAdapter { … }`.
- Registry: extend the `adapter_for(tool)` match in `crates/agentum-executor/src/lib.rs`; add to `FIRST_CLASS` (or `PASSTHROUGH_PROBED`); add a `binary_for(tool)` arm if the binary name disagrees with the tool id.
- TUI picker: append to `TOOL_SUGGESTIONS` in `crates/agentum/src/commands/terminal/app.rs`. Extend `is_probed_tool()`; if it has a YOLO flag, add to `YOLO_TOOLS`.
- CLI help text: touch `--tool` example in `crates/agentum/src/cli.rs`.
- Dashboard picker: add to the `TOOLS` array in `dashboard/src/lib/components/NewSessionDialog.svelte`.
- Tests: `#[cfg(test)] mod tests` in `adapters.rs` — at minimum a "registry routes" assertion + a YOLO-translation test.

**New HTTP route (e.g. a new resource):**
- Implementation: new file in `crates/agentum-server/src/routes/`, exposing `pub fn router() -> Router<AppState>`.
- Wire-up: declare in `crates/agentum-server/src/routes/mod.rs`, merge in `crates/agentum-server/src/lib.rs::router`.
- Public access (no auth): add the path to `is_public` in `crates/agentum-server/src/auth.rs:74`.
- Domain types (if shared on the wire): add to `crates/agentum-core/src/lib.rs` so the TUI client + dashboard `api.ts` share the shape.

**New DB column / table:**
- New numbered SQL file in `crates/agentum-store/migrations/` (`NNNN_description.sql`). Numbers are sequential and embedded in the binary at compile time.
- Extend the matching domain type in `crates/agentum-core/src/lib.rs` (use `#[serde(default, skip_serializing_if = "Option::is_none")]` for additive fields to keep the wire format compact).
- Extend `Store` methods + the `SessionRow`/`BoardItemRow`/etc. `FromRow` impl in `crates/agentum-store/src/lib.rs`.

**New dashboard page:**
- Route: `dashboard/src/routes/<name>/+page.svelte`.
- If it needs the chrome (sidebar / topbar / toasts), it inherits automatically from `dashboard/src/routes/+layout.svelte`.
- If it needs the canvas (full-viewport, no chrome), add the path to `isWideRoute` in `+layout.svelte`.
- New store: `dashboard/src/lib/stores/<name>.ts` (Svelte `writable`).
- Reusable component: `dashboard/src/lib/components/<Name>.svelte`.

**New TUI screen / panel:**
- New mode in `crates/agentum/src/commands/terminal/app.rs` (extend `Focus` enum + `Overlay` if it's modal).
- Draw function in `crates/agentum/src/commands/terminal/ui.rs`.
- Wire keystroke into `app.rs::run_loop`.

**New shared type:**
- `crates/agentum-core/src/lib.rs`. Keep dep-light — `agentum-core` must not pull in tokio, sqlx, or axum.

**New CLI subcommand:**
- File in `crates/agentum/src/commands/<name>.rs` with `pub async fn run(...) -> anyhow::Result<()>`.
- Module declaration in `crates/agentum/src/commands/mod.rs`.
- `Cmd` variant + clap struct in `crates/agentum/src/cli.rs::Cmd`.
- Dispatch arm in `crates/agentum/src/cli.rs::dispatch`.

**New utility / shared helper:**
- Cross-crate utility belongs in `agentum-core` (if dep-light) or as a `pub fn` in the crate that owns the abstraction (e.g. tmux helpers in `agentum-tmux`).
- TUI-only helper: `crates/agentum/src/commands/terminal/<helper>.rs`.

## Special Directories

**`crates/agentum/src/commands/terminal/`:**
- Purpose: the entire ratatui TUI lives here. 12 source files, ~17 600 LOC.
- Generated: no.
- Committed: yes.
- Key files: `mod.rs` (entry + connect-or-onboard), `app.rs` (state + event loop), `ui.rs` (draw), `api.rs` (HTTP/WS client), `pty.rs` (local lazygit pane), `term.rs` (xterm/vt100 pane model).

**`crates/agentum-store/migrations/`:**
- Purpose: sqlx migration files baked into the binary at build time.
- Generated: no.
- Committed: yes.
- Naming: `NNNN_description.sql`, monotonically increasing.

**`dashboard/build/`:**
- Purpose: SvelteKit static output — the thing `rust-embed` inlines.
- Generated: yes (`pnpm --dir dashboard build`).
- Committed: gitignored; CI rebuilds it. `crates/agentum-server/build.rs` materialises a placeholder stub if missing so `cargo build` doesn't fail standalone.

**`dashboard/node_modules/`, `dashboard/.svelte-kit/`:**
- Generated; not committed.

**`target/`:**
- Cargo build artifacts; not committed.

**`.planning/`:**
- Purpose: GSD workflow artifacts.
- Subdirs: `codebase/` (architecture maps — this file), `specs/` (feature specs), `phases/` (implementation plans), `debug/`, `todos/`.

**`.agents/skills/`, `.claude/skills/`:**
- Purpose: project-specific Claude skills. The skills under `.agents/skills/` (higgsfield-*) are unrelated to the agentum codebase — they're shared across the user's projects.

**`.claude/worktrees/`:**
- Purpose: parallel git worktrees for in-flight branches (multi-host-federation, onboarding, security-fixes, tui-sessions). Not part of the build.

**`docs/superpowers/`:**
- Purpose: long-form design specs that pre-date `.planning/specs/`.

---

*Structure analysis: 2026-05-20*
