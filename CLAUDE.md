# CLAUDE.md — agentum codebase guide

> Living guide for Claude (and humans) working in this repo. Update it
> when you change architecture, add a crate, move a primitive, or
> introduce a non-obvious gotcha.

agentum is a self-hosted control plane for AI coding agents (Claude
Code, Codex, Gemini, Cursor, …). It boots a local daemon (`agentum
serve`) that owns:

- a SQLite database of session metadata
- a tmux server where each session is one pane running one agent CLI
- an HTTP/WS API the dashboard (Svelte) and TUI (`agentum terminal`)
  drive

A "session" is a `(name, workdir, tool, model, flags)` tuple. The
daemon spawns the right binary into a tmux pane and streams its
output to clients.

---

## Crate map

```
crates/
  agentum-core/        # Shared types: Session, Status, Event, transcript types.
  agentum-store/       # SQLite repository (sqlx). Persists sessions, board, notes, channels, users, auth.
  agentum-tmux/        # Thin wrapper over tmux: new-session, send-keys, capture-pane, kill.
  agentum-watchdog/    # Background loop. Tails panes, emits Event::AgentFinished/AwaitingInput/Crashed.
  agentum-executor/    # ToolAdapter trait + per-agent argv builders. Owns YOLO marker translation.
  agentum-server/      # axum HTTP+WS API + TLS + auth + routes/. Embeds the dashboard SPA.
  agentum/             # CLI binary. Houses the TUI under commands/terminal/.

dashboard/             # SvelteKit SPA. Builds to dashboard/build/, embedded into the daemon.
```

Each `crates/<x>/Cargo.toml` declares its deps; the workspace root
`Cargo.toml` pins shared versions.

---

## Critical: rebuild rhythm

The dashboard SPA is **embedded into the daemon at compile time** via
`rust-embed` (`crates/agentum-server/src/embed.rs`). After any change
under `dashboard/src/`, you must:

```sh
npm run build --prefix dashboard   # writes dashboard/build/
cargo build --release              # bakes dashboard/build/ into the binary
pkill agentum && agentum serve     # restart whatever was running
```

If you skip step 2, your running daemon serves the OLD bundle.

The TUI binary is the same: `cargo build` again after touching
`crates/agentum/src/commands/terminal/*.rs`. There's no hot reload.

---

## Adding a new agent (tool adapter)

The pattern: each tool implements `ToolAdapter` in
`crates/agentum-executor/src/adapters.rs`. Five files to touch (more
if it has a UI/dashboard surface):

1. **`crates/agentum-executor/src/adapters.rs`** — define an adapter
   struct + impl. Set `name()`, `launch()`, optional `yolo_flag()`,
   `compact_trigger()`, `crash_signatures()`, `busy_signature()`,
   `awaiting_input_signatures()`.
2. **`crates/agentum-executor/src/lib.rs`** — register in
   `adapter_for(tool)` match, add to `FIRST_CLASS` (or
   `PASSTHROUGH_PROBED` if you only want availability gating without
   a bespoke launch). Add a `binary_for(tool)` arm if the binary
   name disagrees with the tool id (e.g. cursor → cursor-agent).
3. **`crates/agentum/src/commands/terminal/app.rs`** — append to
   `TOOL_SUGGESTIONS` so the TUI Tab-cycle picks it up. If the
   adapter has a YOLO flag, also extend `YOLO_TOOLS`. Extend
   `is_probed_tool()` so the picker gates it.
4. **`crates/agentum/src/cli.rs`** — touch the `--tool` help text
   example string.
5. **`dashboard/src/lib/components/NewSessionDialog.svelte`** — add
   to the `TOOLS` array (`firstClass: true` if the binary should be
   gated; `yoloable: true` if `yolo_flag()` returns `Some`).

Tests live in `adapters.rs`'s `#[cfg(test)] mod tests`. Add at minimum
a "registry routes" assertion + a YOLO-translation test.

---

## YOLO marker translation

The TUI and dashboard both push the canonical Claude marker
`--dangerously-skip-permissions` into `Session::flags` whenever the
user toggles YOLO mode, **regardless of which tool the session
targets**. Each adapter's `launch()` calls
`agentum_executor::translate_yolo_marker(&flags, self.yolo_flag())`,
which substitutes the per-tool flag (or drops the marker entirely
when the adapter doesn't expose one).

Per-tool spellings (canonical):

| Tool     | Flag                                        |
| -------- | ------------------------------------------- |
| claude   | `--dangerously-skip-permissions` (identity) |
| codex    | `--dangerously-bypass-approvals-and-sandbox`|
| cursor   | `--force`                                   |
| gemini   | `--yolo`                                    |
| hermes   | `--yolo`                                    |
| opencode | (unverified — currently `None`)             |
| aider    | (unverified — currently `None`)             |

**Don't push different spellings into `Session::flags` from any
client.** That defeats the translation layer and was the root cause
of the v0.6.23 codex crash.

---

## Agent installation gating

`/api/agents` returns `[{name, binary, available, yolo_flag, path}]`
for every tool in `agentum_executor::probed_tools()`. The dashboard
fetches it on `NewSessionDialog` open and dims unavailable tiles;
the TUI fetches it once at startup of the run-loop (see
`app::run_loop`'s `client.list_agents()` call).

To probe a tool that's NOT first-class but should be gated, add it
to `PASSTHROUGH_PROBED` in `crates/agentum-executor/src/lib.rs` —
no adapter needed.

`terminal` and `bash` deliberately stay un-probed: shells are
universally available and don't need the round trip.

---

## Connection profiles (multi-endpoint)

Users can target multiple agentum servers from one TUI/dashboard
without retyping the URL. Two layers:

### CLI / TUI

- **Storage**: `$XDG_CONFIG_HOME/agentum/profiles.toml`. One
  `default = "name"` pointer plus `[profiles.<name>]` tables with
  `url`, optional `fingerprint`, optional `insecure`.
- **Module**: `crates/agentum/src/commands/terminal/profiles.rs`
  (`Profiles::load/upsert/remove/set_default`).
- **CLI**: `agentum profiles list/add/rm/use` lives in
  `crates/agentum/src/commands/profiles.rs`.
- **TUI flag**: `agentum terminal --profile NAME` resolves to the
  profile's URL+fingerprint before the loopback probe runs.
- **TUI overlay**: `Ctrl-S` opens `Overlay::Profiles`. Pick + Enter
  triggers a *soft restart* of the run-loop:
  `app::RunOutcome::SwitchProfile(name)` bubbles up to
  `commands::terminal::run`, which tears down the alt-screen,
  reconnects via `connect_once`, and re-enters `run_tui_session`.
  See `crates/agentum/src/commands/terminal/mod.rs::run` for the
  loop.
- **Active-profile indicator**: rendered in the title bar
  (`ui::draw_title`) as `· @vps`.

### Dashboard

- **Storage**: `localStorage` keys `agentum_profiles` (JSON array)
  + `agentum_active` (string id). The legacy single-token slot
  `agentum_token` is mirror-written by `setActiveToken` for
  backwards-compat with code paths that still read it directly.
- **Module**: `dashboard/src/lib/profiles.ts`
  (`profiles` writable, `getActiveProfile()`, `apiUrl(path)`,
  `wsUrl(path)`).
- **All HTTP** flows through `apiUrl(path)` in `api.ts`'s
  `request()`. **All WS** flows through `wsUrl(path)`
  (`api.streamUrl`, `api.eventsUrl`, the events bus's
  `eventsUrlForActiveProfile`).
- **UI**: `dashboard/src/lib/components/EndpointSwitcher.svelte` in
  the topbar. Switching reloads the page so every store + WS
  re-initialises against the new origin (cheaper and more reliable
  than per-store invalidation).
- **First-run gate**: when `probeAuth() === 'unreachable'`,
  `TokenGate.svelte` shows an inline "Add endpoint" form instead of
  the login prompt.

### TUI/dashboard parity table

| Feature                        | TUI                       | Dashboard               |
| ------------------------------ | ------------------------- | ----------------------- |
| Profile add/list/remove        | `agentum profiles …` + Ctrl-S overlay | EndpointSwitcher in topbar |
| Active profile indicator       | title bar `· @name`       | chip in topbar          |
| Empty-daemon onboarding        | numbered prompt before alt-screen | inline form on TokenGate's unreachable card |
| Agent installation gating      | "(not installed)" hint on Tool field | tile dimmed + tooltip |
| Switch profile mid-session     | Ctrl-S → soft restart of run-loop | switch chip → page reload |

---

## API routes layer

All HTTP/WS routes live in `crates/agentum-server/src/routes/`:

| File              | Path                       | Notes                          |
| ----------------- | -------------------------- | ------------------------------ |
| `health.rs`       | `/api/health`              | Public; no auth.               |
| `auth.rs`         | `/api/auth/*`              | login/register/me/logout.      |
| `cert.rs`         | `/api/cert/fingerprint`    | Public; for TOFU bootstrap.    |
| `sessions.rs`     | `/api/sessions/*` + `/stream` WS | The fat one. CRUD + start/stop/kill + per-session WS. |
| `events.rs`       | `/api/events` WS           | Global broadcast bus.          |
| `agents.rs`       | `/api/agents`              | Probes which tool binaries are on PATH. |
| `agent_tasks.rs`  | `/api/sessions/{id}/agent-tasks` | Plan/todos/tasks tail. |
| `host.rs`         | `/api/host/metrics`        | CPU+RAM samples; also broadcasts. |
| `fs.rs`           | `/api/fs/list`             | Workdir picker. |
| `board.rs`, `notes.rs`, `channels.rs`, `watchdog.rs`, `doctor.rs` | various | Self-explanatory. |

Auth middleware (`crate::auth::require_token`) is applied at the
top-level router merge — see `lib.rs::router`. Public paths are
listed in `auth.rs::is_public`. WS clients pass the bearer token as
`?token=` because browsers can't set headers on upgrade.

---

## Common gotchas

- **rust-embed compile-time**: see "Critical: rebuild rhythm" above.
- **YOLO marker**: never push tool-specific YOLO flag spellings from
  the TUI/dashboard. Always push the Claude marker; let the adapter
  translate.
- **Claude session UUID**: `ClaudeAdapter::launch()` pins
  `--session-id <agentum_uuid>` so transcripts land in a unique file
  per session. Without this, two sessions in the same workdir share
  one transcript and the agent-tasks panel cross-pollinates todos.
- **Capabilities probe**: pre-v0.6.7 daemons don't return
  `capabilities` from `/api/health`. The TUI client treats absence
  as "no optional features supported".
- **Profile token migration**: the legacy `agentum_token`
  localStorage key is read on first load and migrated into a `local`
  profile. Newly added profiles get fresh tokens earned at login.
- **Cargo.lock drift**: `Cargo.lock` gets updated whenever a dep
  changes. Commit it; we ship binaries from CI and reproducibility
  matters.

---

## Conventions

- **Comments**: write *why*, not *what*. Add a short comment when
  the line encodes a decision, an invariant, or a workaround. Don't
  paraphrase the code.
- **Tests**: `cargo test --workspace --lib` covers everything.
  Pre-existing breakage in `agentum-store`'s lib tests
  (NewBoardItem field churn) is a known issue, unrelated to most
  changes.
- **Frontend tests**: `npm run check --prefix dashboard`
  (`svelte-check` + tsc).
- **Clippy / fmt**: workspace runs cargo fmt; please match
  surrounding style.

---

## Quick reference

```sh
# Build everything
cargo build --release
npm run build --prefix dashboard
cargo build --release   # rebake the embedded SPA

# Run the daemon
agentum serve

# Run the TUI against the local daemon
agentum terminal

# Run the TUI against a remote profile
agentum profiles add vps https://my-vps.example.com:8822 --set-default
agentum terminal --profile vps

# Run with mute
AGENTUM_TUI_NO_SOUND=1 agentum terminal

# Tests
cargo test -p agentum-executor -p agentum-server -p agentum --lib
npm run check --prefix dashboard
```

<!-- GSD:project-start source:PROJECT.md -->
## Project

**Agentum**

Agentum is a self-hosted control plane for orchestrating multiple AI coding agents (Claude Code, Codex, OpenCode, Cursor, Gemini, Hermes, …) from a single terminal-or-browser interface. A local daemon owns a tmux server where each session is one agent CLI; a SvelteKit dashboard and a Rust TUI drive it via an HTTP/WS API. It's for solo developers and small teams who want AI agents that keep running on their own hardware while they're away from their desk — including from a phone.

**Core Value:** > One terminal — and one orchestrator — to manage all your AI coding agents across all your projects, even when your laptop is closed. The kanban *is* the orchestrator: a goal in, executing cards out.

### Constraints

- **Tech stack:** Rust 1.85 / edition 2024 workspace + SvelteKit dashboard embedded via `rust-embed`. Adding the orchestrator must not introduce a non-Rust daemon dependency or break the single-binary distribution.
- **Reuse over rebuild:** every new endpoint extends `crates/agentum-server/src/routes/` with the same auth middleware + `AppState` shape. Every new schema column lives behind a numbered migration in `crates/agentum-store/migrations/` (next: `0015_*.sql`).
- **No new SaaS dependency:** the orchestrator must use whatever agent CLI the user already has installed (probed via `/api/agents`). No daemon-side Anthropic/OpenAI API key.
- **Backwards compatibility:** existing board cards (no `parent_goal_id`, no dependency edges) must keep working unchanged. Migrations add columns nullable; default `enforce_transition` behavior is preserved.
- **Embedded SPA rebuild rhythm:** dashboard changes require `npm run build --prefix dashboard && cargo build --release` to bake into the binary. Plans must account for this; CI must catch a missed rebuild.
- **Performance:** the dependency-aware column gate runs on every PATCH. Must stay sub-10ms even with hundreds of cards (in-memory graph walk; no per-edge SQL query).
<!-- GSD:project-end -->

<!-- GSD:stack-start source:codebase/STACK.md -->
## Technology Stack

## Languages
- Rust (edition 2024, MSRV 1.85) — Workspace at `Cargo.toml` with seven member crates under `crates/`. Pinned via `rust-toolchain.toml` (`channel = "stable"`, components `rustfmt`, `clippy`).
- TypeScript 5.7 — Dashboard SPA under `dashboard/src/`. `tsconfig.json` extends SvelteKit's generated config with `strict: true`, `checkJs: true`, `moduleResolution: "bundler"`.
- Svelte 5 (`.svelte` components in `dashboard/src/lib/components/` and `dashboard/src/routes/`).
- SQL — Hand-written sqlx migrations in `crates/agentum-store/migrations/` (`0001_initial.sql` … `0014_board_column_rules.sql`).
- HTML/CSS — Static marketing site under `web/` (`web/index.html`, `web/sitemap.xml`, `web/robots.txt`); SvelteKit shell at `dashboard/src/app.html` + `dashboard/src/app.css`.
- Bash — Installer script `scripts/install.sh`; recipes in `justfile`.
## Runtime
- Tokio 1.x with the `full` feature (workspace dep in `Cargo.toml`) — async runtime driving the daemon, watchdog, and TUI.
- Node.js 22 — Pinned by `.github/workflows/ci.yml` and `.github/workflows/release.yml` for the dashboard build. Required only to produce `dashboard/build/` which is then embedded into the Rust binary.
- Browser runtime — SvelteKit SPA executes in the dashboard tab; a service worker (`dashboard/src/service-worker.ts`) precaches the build chunks for offline-friendly reloads.
- Cargo (workspace mode, `resolver = "2"`) — see `Cargo.toml` lines 1–11.
- pnpm 9 — Dashboard dependencies. Lockfile at `dashboard/pnpm-lock.yaml`. CI installs via `pnpm/action-setup@v4`.
- Lockfile: present (`Cargo.lock` committed at repo root; `dashboard/pnpm-lock.yaml` committed).
## Frameworks
- axum 0.8 with `ws` + `macros` features — HTTP router and WebSocket upgrades. Mounted from `crates/agentum-server/src/lib.rs::router`.
- axum-server 0.7 with `tls-rustls` — Binds the TLS listener in `crates/agentum-server/src/lib.rs::serve`.
- tower 0.5 + tower-http 0.6 (`cors`, `trace`, `compression-gzip`) — Middleware stack layered in `router()` and `crates/agentum-server/src/logging.rs`.
- rustls 0.23 with the `ring` crypto provider — Installed via `rustls::crypto::ring::default_provider().install_default()` in `crates/agentum-server/src/lib.rs`. Self-signed cert generation lives in `crates/agentum-server/src/tls.rs` (rcgen 0.13).
- sqlx 0.8 with `runtime-tokio`, `sqlite`, `macros`, `migrate` features — Sole persistence layer in `crates/agentum-store/src/lib.rs`. Bundled `sqlite` feature deliberately omitted; the workspace consumes sqlx's `sqlite` driver against the system libsqlite (see comment in `Cargo.toml` lines 39–41).
- clap 4 with `derive` + `env` — CLI parsing in `crates/agentum/src/cli.rs`.
- ratatui 0.29 with the `crossterm` backend — Terminal UI in `crates/agentum/src/commands/terminal/`.
- crossterm 0.28 with `event-stream` — Raw mode, key/mouse events, alt-screen lifecycle.
- tui-term 0.2 + vt100 0.15 + portable-pty 0.9 — Embedded PTY rendering for the in-TUI side pane (lazygit-style).
- reqwest 0.12 (`json`, `rustls-tls`, `stream`, no default features) — TUI's HTTP client against the daemon's REST surface.
- tokio-tungstenite 0.24 (`connect`, `rustls-tls-native-roots`) — TUI's WebSocket client for `/api/sessions/{id}/stream` and `/api/events`.
- SvelteKit 2.17 + Svelte 5.19 — Application framework. Config at `dashboard/svelte.config.js`, Vite plugin in `dashboard/vite.config.ts`.
- `@sveltejs/adapter-static` 3 — Renders the SPA to `dashboard/build/` with `fallback: 'index.html'` so client-side routing works after embedding.
- Vite 6 — Bundler. Dev server proxies `/api` → `http://127.0.0.1:8822` (`dashboard/vite.config.ts` lines 7–21).
- xterm.js 5.5 + `@xterm/addon-fit` 0.10 — Embedded terminal widget in `dashboard/src/lib/components/Terminal.svelte` (paired with `TerminalPanel.svelte`).
- Built-in Rust unit tests via `cargo test --workspace --lib` — Adapter behaviour in `crates/agentum-executor/src/adapters.rs::tests`; service-level tests in each crate.
- Vitest 4 — Pure-data tests for the dashboard. Configured in `dashboard/vite.config.ts` (`include: ['src/**/*.{test,spec}.ts']`). No DOM environment configured.
- svelte-check 4 — Type-checks Svelte + TS via `pnpm --dir dashboard check` (script in `dashboard/package.json`).
- tempfile 3 — Dev-dep across `agentum-store`, `agentum-core`, `agentum-server`, `agentum` for sandboxed XDG paths.
- `cargo` (release profile in `Cargo.toml`: `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`).
- `cross` — Used by `.github/workflows/release.yml` for `aarch64-unknown-linux-gnu`.
- `just` — Task runner. `justfile` exposes `dev`, `build`, `check`, `test`, `fmt` recipes.
- rust-embed 8 with `mime-guess` — Compile-time embedding of `dashboard/build/` into the daemon binary; wired in `crates/agentum-server/src/embed.rs`.
## Key Dependencies
- sqlx 0.8 (`crates/agentum-store/Cargo.toml`) — Async SQLite driver. WAL journal mode, synchronous=NORMAL, `max_connections = 8` (`crates/agentum-store/src/lib.rs::Store::open`).
- tokio 1 — Async runtime everywhere.
- tokio::process — Wrapping the `tmux` binary in `crates/agentum-tmux/src/lib.rs`. No bindings — every command shells out with one `.arg()` per argument; `shlex` 1 quotes the inner shell-string passed to `tmux new-session`.
- rust-embed 8 — Bakes the dashboard bundle into the release binary. Folder pinned to `../../dashboard/build` in `crates/agentum-server/src/embed.rs`.
- notify 8 — Filesystem watcher used by `crates/agentum-server/src/transcript_store.rs` to tail Claude Code JSONL transcripts.
- sysinfo 0.32 (`system` feature, default features off) — CPU/RAM sampling in `crates/agentum-server/src/routes/host.rs`.
- regex 1 — Crash and context-low signature matching in `crates/agentum-watchdog/src/lib.rs`.
- rustls 0.23 (`ring`, `std`, `tls12`, default features off) — TLS in both daemon (`crates/agentum-server`) and TUI client (`crates/agentum`).
- rcgen 0.13 — Self-signed certificate generation (`crates/agentum-server/src/tls.rs`).
- argon2 0.5 — Password hashing in `crates/agentum-server/src/auth.rs` (run on the blocking pool via `tokio::task::spawn_blocking`).
- password-hash 0.5 with `getrandom` — Salt + PHC string handling alongside argon2.
- sha2 0.10 — Cert fingerprint hash (`crates/agentum-server/src/tls.rs::cert_fingerprint`) and shared crypto in the TUI.
- rand 0.9 — Bearer-token randomness (`crates/agentum-server/src/auth.rs::new_token`).
- base64 0.22 — URL-safe encoding of tokens and PEM decoding.
- tokio-rustls 0.26 — Used by the TUI for TLS-pinned WS connects.
- directories 5 — XDG path resolution (`crates/agentum-store/src/paths.rs`).
- tracing 0.1 + tracing-subscriber 0.3 (`env-filter`) — Structured logging across all crates.
- anyhow 1 — App-level error bubbling (binaries, route handlers via `crates/agentum-server/src/error.rs::ApiError`).
- thiserror 2 — Domain error enums in every library crate.
- serde 1 + serde_json 1 — Wire serialization for events, sessions, transcripts.
- time 0.3 (`serde`, `formatting`, `parsing`, `macros`) — All timestamps; RFC3339 in/out of SQLite.
- uuid 1 (`v4`, `serde`) — Session IDs pinned to Claude transcripts.
- toml 0.8 + toml_edit 0.22 — Profile and config files (`profiles.toml`, `known_hosts.toml`).
- exec 0.3 — `agentum update` replaces the current process after fetching a new binary.
- which 7 — Probes for installed agent CLIs (`crates/agentum-server/src/routes/agents.rs`).
- bytes 1 — Buffer plumbing for axum bodies and WS frames.
- libc 0.2 (Unix only, `crates/agentum/Cargo.toml`) — Detaches the auto-spawned `agentum serve` sidecar from the TUI's controlling terminal.
- url 2 — URL parsing in the TUI client.
- futures-util 0.3 — Stream combinators for WS / HTTP streams.
- rpassword 7 — Hidden password prompts in TUI auth flows.
- `@xterm/xterm` 5.5 + `@xterm/addon-fit` 0.10 — Embedded terminal in the dashboard (`dashboard/src/lib/components/Terminal.svelte`).
## Configuration
- `AGENTUM_BACKEND` — Vite dev-server proxy target for `/api` (`dashboard/vite.config.ts` line 7). Defaults to `http://127.0.0.1:8822`.
- `AGENTUM_TUI_NO_SOUND` — Mutes TUI chimes (`crates/agentum/src/commands/terminal/mod.rs:124`).
- `AGENTUM_THEME` — Overrides the TUI theme name (`crates/agentum/src/commands/terminal/theme.rs:297`).
- `SHELL` — Honored by `TerminalAdapter` to launch the user's shell (`crates/agentum-executor/src/adapters.rs:305`); falls back to `bash`.
- `EDITOR` / `VISUAL` — Used by `agentum config edit` (`crates/agentum/src/commands/config.rs:92-93`).
- `HOME`, `PATH`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `TMUX` — Read for path resolution, binary probing, daemon logs, profile storage, and tmux-detection.
- No `.env` files in repo; configuration is OS-environment + on-disk TOML, not dotenv.
- `Cargo.toml` (root workspace manifest, lines 21–48) pins shared dependency versions.
- Per-crate `Cargo.toml` files (e.g. `crates/agentum-server/Cargo.toml`, `crates/agentum/Cargo.toml`) add binary-specific deps.
- `rust-toolchain.toml` pins channel `stable` with `rustfmt` + `clippy`.
- `dashboard/svelte.config.js` configures the static adapter (`pages: 'build'`, `fallback: 'index.html'`).
- `dashboard/vite.config.ts` configures the dev server, proxy, and Vitest include glob.
- `dashboard/tsconfig.json` extends the SvelteKit-generated TS config.
- `$XDG_DATA_HOME/agentum/db.sqlite` (`crates/agentum-store/src/paths.rs::db_path`).
- `$XDG_DATA_HOME/agentum/tls/{cert,key}.pem` — Self-signed TLS material (`crates/agentum-server/src/tls.rs::ensure_artifacts`; mode 0600).
- `$XDG_CACHE_HOME/agentum/sessions/<id>.log` — Pane logs (`crates/agentum-store/src/paths.rs::pane_log`).
- `$XDG_CONFIG_HOME/agentum/profiles.toml` — Endpoint profiles (CLAUDE.md notes).
- `$XDG_CONFIG_HOME/agentum/known_hosts.toml` — TOFU-pinned cert fingerprints (`crates/agentum/src/commands/terminal/mod.rs`).
## Platform Requirements
- Unix-like host (Linux or macOS). Windows is not targeted: tmux is required at runtime and unix-only build flags pull in `libc` (`crates/agentum/Cargo.toml:65-67`).
- tmux installed on `PATH` — Hard requirement; every session runs as a tmux pane. Probed by `agentum doctor` (`crates/agentum/src/commands/doctor.rs:69`).
- `lf` on `PATH` for `agentum new --pick` (workdir picker).
- Node.js 22 + pnpm 9 — Only when rebuilding the dashboard bundle.
- A C compiler is not required: sqlx uses the unbundled SQLite system driver (see workspace `Cargo.toml` lines 39–41). The user-level note in CLAUDE.md cautions against breaking `cc` shims for cc-rs builds.
- Linux x86_64 (glibc 2.35 — Ubuntu 22.04 baseline so binaries run on Debian 12) and aarch64 (cross-built via `cross`).
- macOS x86_64 and arm64 (`x86_64-apple-darwin`, `aarch64-apple-darwin`) — built on `macos-14` runners.
- Distribution: tarballs + `install.sh` attached to GitHub Releases by `.github/workflows/release.yml`. README's one-liner installer pulls `releases/latest/download/install.sh`.
- The compiled `agentum` binary is self-contained — it embeds the dashboard bundle via `rust-embed`, generates its own TLS material, and migrates SQLite on first boot.
<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->
## Conventions

## Naming Patterns
### Rust
- snake_case, one module per file: `crates/agentum-server/src/routes/board.rs`, `crates/agentum-server/src/routes/board_rules.rs`, `crates/agentum/src/commands/terminal/profiles.rs`.
- Inline test modules use `tests` (or a topic-specific name like `selection_tests`, `merge_dedup_tests`, `profile_targets_loopback_tests`) — see `crates/agentum/src/commands/terminal/app.rs`.
- snake_case for free functions and methods.
- Route handlers are short verbs: `list`, `create`, `get_one`, `patch`, `delete`, `start`, `stop`, `kill`, `send`, `stream` — see `crates/agentum-server/src/routes/sessions.rs:46-411`.
- Builder/constructor helpers use `new`, `with_*`, `from_*`: `AppState::new`, `AppState::with_fingerprint` (`crates/agentum-server/src/lib.rs:108-133`).
- Test-only `pub(crate)` helpers are prefixed `tests_helpers_*` to make their purpose obvious: `tests_helpers_create`, `tests_helpers_patch` in `crates/agentum-server/src/routes/board.rs:377-392`.
- `PascalCase` for structs, enums, traits, and type aliases: `AppState`, `ApiError`, `Session`, `Status`, `ToolAdapter`, `LaunchCommand`.
- One-letter enum variants are avoided; status/lifecycle variants are full words (`Status::Idle`, `Status::Running`, `Status::Stopped`, `Status::Crashed`) — see `crates/agentum-core/src/lib.rs:30-35`.
- SCREAMING_SNAKE_CASE: `EVENT_BUS_CAPACITY`, `AUTH_RATE_LIMIT_ATTEMPTS`, `AUTH_RATE_LIMIT_WINDOW`, `GRACEFUL_STOP_TIMEOUT`, `IDLE_AFTER_QUIET` (`crates/agentum-server/src/lib.rs:57`, `crates/agentum-watchdog/src/lib.rs:44`).
- All workspace crates are prefixed `agentum-`: `agentum-core`, `agentum-store`, `agentum-tmux`, `agentum-watchdog`, `agentum-executor`, `agentum-server`, plus the binary crate `agentum`.
- Inside `agentum/src/commands/`, one file per subcommand: `new.rs`, `up.rs`, `down.rs`, `kill.rs`, `send.rs`, `tail.rs`, `open.rs`, `ls.rs`, `rm.rs`, `serve.rs`, `doctor.rs`, `auth.rs`, `keys.rs`, `hosts.rs`, `profiles.rs`, `update.rs`, `uninstall.rs`, `config.rs`, plus the `terminal/` subtree.
### TypeScript / Svelte
- Svelte components: PascalCase `.svelte` files in `dashboard/src/lib/components/`: `Sidebar.svelte`, `NewSessionDialog.svelte`, `EndpointSwitcher.svelte`, `TokenGate.svelte`, `ToastStack.svelte`. Dashboard-specific subcomponents live under `dashboard/src/lib/components/dashboard/` (`FleetRow.svelte`, `SessionRail.svelte`, etc.).
- Stores: lowercase, plural noun (`sessions.ts`, `events.ts`, `board.ts`, `attention.ts`) in `dashboard/src/lib/stores/`.
- Shared modules: lowercase: `api.ts`, `profiles.ts`, `dashboard.ts`, `board-schema.ts`.
- Tests: `*.test.ts` co-located with the module under test: `dashboard/src/lib/board-schema.test.ts`.
- camelCase for functions, variables, and store names: `apiUrl`, `wsUrl`, `getActiveProfile`, `loadSessions`, `markFetchOk`.
- PascalCase for interfaces, types, and Svelte components: `Session`, `BoardItem`, `Profile`, `Toast`, `ConnStatus`.
- SCREAMING_SNAKE_CASE for module-level constants: `TOKENS_KEY`, `LABELS_KEY`, `SYNTHETIC_ID`, `HTTP_FAIL_THRESHOLD` (`dashboard/src/lib/profiles.ts:60-70`, `dashboard/src/lib/stores/events.ts:51`).
## Code Style
### Rust
- `cargo fmt --all` (rustfmt defaults, no custom `rustfmt.toml`).
- `rust-toolchain.toml` pins `stable` with `rustfmt` + `clippy` components.
- Edition `2024`, MSRV `1.85` (workspace-level in `Cargo.toml:15-16`).
- `cargo clippy --all-targets --all-features -- -D warnings`. CI fails on any warning.
- Use `#[allow(dead_code)]` with a comment that names the future use — see `crates/agentum/src/commands/terminal/app.rs:53` (`fields populated for future toast-dedup logic`) and `crates/agentum/src/commands/terminal/api.rs:67`.
- Use `#[allow(clippy::field_reassign_with_default)]` only when the test deliberately mutates a `Default::default()` instance between assertions — example at `crates/agentum/src/commands/terminal/prefs.rs:313`.
### TypeScript
- No `.prettierrc` or `biome.json` is checked in. The project relies on `svelte-check` + `tsc --strict` (configured in `dashboard/tsconfig.json`) and per-file consistency.
- Indent style: 2 spaces. Single quotes for strings. Trailing semicolons.
- `dashboard/tsconfig.json` enables `"strict": true`, `"checkJs": true`, `"allowJs": true`, `"forceConsistentCasingInFileNames": true`.
- TypeScript `interface` is preferred for wire shapes that mirror Rust structs; `type` is used for unions: `type Status = 'idle' | 'running' | 'stopped' | 'crashed'` (`dashboard/src/lib/api.ts:9`).
## Import Organization
### Rust
- Internal crates referenced by their `agentum-*` package name, e.g. `use agentum_core::Session;`, `use agentum_store::Store;`. Workspace `Cargo.toml:21-27` defines them with `{ path = "crates/agentum-*" }` so dependents pull in `{ workspace = true }`.
### TypeScript
- `$components` → `src/lib/components`
- `$stores` → `src/lib/stores`
- `$themes` → `src/lib/themes`
- `$lib` (SvelteKit built-in) → `src/lib`
- `$app/*` (SvelteKit built-in) for `$app/state`, `$app/stores`, etc.
## Error Handling
### Rust application layer (`anyhow`)
- Binary entry points (`crates/agentum/src/main.rs`, `crates/agentum/src/commands/*.rs`) return `anyhow::Result<T>`.
- `bail!` for early returns with a formatted message: `bail!("session {name} is not running")` in `crates/agentum/src/commands/send.rs:17`.
- `.context(...)` to attach human-readable context to lower-level errors: imported via `use anyhow::{Context, Result, bail};` (e.g. `crates/agentum/src/commands/config.rs:5`).
- Process-exit on user-input errors uses `eprintln!` + `std::process::exit(N)` with distinct exit codes (e.g. exit 3 for "no such session" in `crates/agentum/src/commands/send.rs:7-9`).
### Rust library layer (`thiserror`)
- Each crate defines a typed error enum with `#[derive(Debug, thiserror::Error)]`:
- Each crate also defines a local `type Result<T> = std::result::Result<T, ThisError>;` alias (e.g. `crates/agentum-store/src/lib.rs:47`).
### HTTP error handling (`ApiError`)
- The default body envelope is `{"error": "<msg>"}`; `Custom` carries its own shape and short-circuits before the envelope path.
- Every handler signature returns `Result<Json<T>, ApiError>`, `Result<(StatusCode, Json<T>), ApiError>`, or `Result<StatusCode, ApiError>`. See `crates/agentum-server/src/routes/notes.rs:19-58` for the full CRUD pattern and `crates/agentum-server/src/routes/sessions.rs:46-411` for the longer surface.
- `From<StoreError> for ApiError` maps store errors to HTTP statuses; unknown variants log via `tracing::error!` before mapping to `Internal`.
### Panics
- `expect("…")` is reserved for invariants that the surrounding code makes infeasible — message describes what must be true: `"ratelimit mutex poisoned"`, `"non-empty after capacity check"` (`crates/agentum-server/src/ratelimit.rs:43,64`). Avoid bare `.unwrap()` in production paths.
- `unreachable!("handled above")` flags code-flow guarantees made earlier in the same function (`crates/agentum-server/src/error.rs:51`).
- Tests freely use `.unwrap()` — failure aborts the test, which is the goal.
### TypeScript
- `dashboard/src/lib/api.ts:198-203` defines `ApiError extends Error` with a `status` field. Wrap fetch errors and surface to UI via the `events.ts` toast stack.
- Network-level failures (DNS, TCP refuse) call `markFetchFailed()`; HTTP 5xx counts as a reachability failure; 401 clears the active token and re-prompts (`api.ts:174-179`).
## Logging
- `agentum::init_tracing()` for daemon / CLI commands — writes to stderr with `EnvFilter` defaulting to `info,sqlx=warn,hyper=warn,h2=warn,tower_http=info` (`crates/agentum/src/lib.rs:7-16`). Override via `AGENTUM_LOG`.
- `agentum::init_tracing_for_tui()` redirects logs to `$XDG_CACHE_HOME/agentum/tui.log` so ratatui's alt-screen isn't scrambled by stray escape sequences from dependencies (`crates/agentum/src/lib.rs:27-52`).
- `tracing::info!`, `tracing::warn!`, `tracing::error!`, `tracing::debug!` — pick by severity.
- Use structured fields with `=` shorthand: `tracing::info!(addr = %opts.addr, "agentum-server listening (https)")`, `tracing::warn!(error = %e, "auth session sweep failed")`, `tracing::warn!(error = ?e, "watchdog reconcile failed")`.
- Span via `tracing::info_span!("request", method = %m, uri = %u, ...)` — wraps HTTP requests in `redacting_trace_layer()` (`crates/agentum-server/src/logging.rs:26-34`).
- The HTTP access log scrubs `token=` query values to `token=REDACTED` before logging — see `crates/agentum-server/src/logging.rs:38-57`.
- Never log raw bearer tokens, password hashes, or `?token=…` values. The redacting span layer covers HTTP, but be deliberate in any custom logs.
- TUI binary must not write to stderr — corrupts the alt-screen. Use `init_tracing_for_tui()`.
## Comments
### Apply when
- Encode a decision (e.g. "Cow keeps the const path alloc-free" — `crates/agentum-server/src/routes/board.rs:101`).
- State an invariant the code depends on (e.g. ratelimit mutex serialises env-mutating tests — `crates/agentum-server/src/routes/profiles.rs:137-141`).
- Capture a workaround or regression context (e.g. v0.6.21..=v0.6.24 ws-url query-string bug — `crates/agentum/src/commands/terminal/api.rs:952-955`).
### Patterns
- Every crate's `lib.rs` opens with a `//!` block describing its role and any non-obvious constraint. Example `crates/agentum-server/src/lib.rs:1-5`:
- Route files open with a `//!` line that names the URL prefix: `crates/agentum-server/src/routes/notes.rs:1` (`//! /api/notes — REST CRUD.`), `crates/agentum-server/src/routes/board.rs:1` (`//! /api/board — kanban CRUD + atomic CAS claim + comments + reorder.`).
- Every `pub` item in libraries gets a doc comment explaining its purpose, not its mechanics. See `agentum_executor::ToolAdapter` (`crates/agentum-executor/src/lib.rs:36-104`) — each trait method has a multi-line `///` block that names the contract, edge cases, and rationale.
- Each `unsafe` block is preceded by a `// SAFETY:` comment explaining why the invariant holds. See `crates/agentum-server/src/routes/profiles.rs:160-167` (env mutation under a process-wide test lock) and `crates/agentum-executor/src/adapters.rs:416-423` (HOME mutation in a serialised test).
- Regression tests include a prose block citing the version where the bug shipped and the failure mode. Examples:
- These comments are load-bearing: future readers use them to decide whether a refactor can drop the test.
### Anti-patterns
- Don't paraphrase code: `// increment counter` for `counter += 1` is noise.
- Don't leave dead-code commented-out blocks; delete and let git history serve.
## Function Design
- Route handlers are small (5-20 lines). Heavy lifting lives on `state.store.*` methods or in helper functions. See `crates/agentum-server/src/routes/notes.rs` (60 lines for a full CRUD surface).
- TUI run loop is the exception: `crates/agentum/src/commands/terminal/app.rs` is the single largest file at ~8000 lines. Within it, `apply_event`, `handle_key`, and the like remain narrowly scoped; new behaviour goes through small helpers + `#[cfg(test)]` modules pinning each piece.
- Axum extractors are listed in canonical order: `State<AppState>`, then `Path<…>`, then `Query<…>`, then `Json<…>` body. See `crates/agentum-server/src/routes/sessions.rs:114-117`.
- Helpers that need multiple optional fields take a `Patch` struct (e.g. `PatchBody` in `sessions.rs:100-112`, `NotePatch` in core) deserialized via `serde(default)` so missing keys mean "don't touch."
- Library functions return `Result<T, CrateError>` using the crate-local `Result` alias.
- HTTP handlers return `Result<Json<T>, ApiError>`, `Result<(StatusCode, Json<T>), ApiError>`, or `Result<StatusCode, ApiError>` for `204 No Content` deletes.
- Builders return owned `Self` (no `&mut self` chains for state types — see `LaunchCommand::argv_only` in `crates/agentum-executor/src/lib.rs:28-34`).
## Module Design
- `pub use` selective re-exports at the crate root keep the surface readable: `pub use error::ApiError`, `pub use transcript_store::TranscriptStore` in `crates/agentum-server/src/lib.rs:34-36`.
- `pub(crate)` is used for test helpers that need to span sibling modules without becoming public API — `tests_helpers_create` / `tests_helpers_patch` in `crates/agentum-server/src/routes/board.rs:377-392`.
- Route registration goes through `routes/mod.rs` which is a flat list of `pub mod <name>;` (`crates/agentum-server/src/routes/mod.rs`). Each route file exposes a `pub fn router() -> Router<AppState>` that the top-level `router()` merges (`crates/agentum-server/src/lib.rs:162-205`).
- Each module is imported directly via its path or alias (`$lib/api`, `$stores/sessions`). No `index.ts` re-export hubs.
## Axum Route Handler Conventions
### Patch over current row
## sqlx Conventions
- WAL mode + `synchronous=NORMAL` + `foreign_keys=true` + `max_connections=8` (`crates/agentum-store/src/lib.rs:61-71`).
- The DB file gets `0600` permissions on open (`restrict_db_perms` referenced at `lib.rs:78`) because it holds password hashes + live bearer tokens.
- **Runtime queries only** — `sqlx::query`, `sqlx::query_as`. **No compile-time `query!` / `query_as!` macros.** This means no `DATABASE_URL` is required at build time, and CI does not need a live DB. Confirmed: `grep -rn "sqlx::query!" crates/` returns zero hits.
- Multi-line SQL uses `r#"…"#` raw strings.
- Always bind via `.bind(…)`; never string-format values into SQL.
- Errors flow through `StoreError::Sqlx(#[from] sqlx::Error)`; UNIQUE-violation detection uses `if let Err(sqlx::Error::Database(db)) = &res { if db.is_unique_violation() { … } }` (`crates/agentum-store/src/lib.rs:115-119`).
- Rows mapped to internal `Row` structs via `#[derive(FromRow)]` then converted to public domain types with `TryFrom` (e.g. `SessionRow → Session`).
- Filename pattern: `NNNN_<topic>.sql` under `crates/agentum-store/migrations/` (0001_initial through 0014_board_column_rules).
- Applied automatically at `Store::open()` via `sqlx::migrate!("./migrations").run(&pool).await?`.
- Every new feature gets a new migration; existing migrations are never edited after a release.
- Migrations include comments explaining the *why* of the schema choice — see `migrations/0014_board_column_rules.sql:1-16` for the rationale on denormalised JSON storage.
## TUI Conventions (`crates/agentum/src/commands/terminal/`)
- `ratatui` 0.x for drawing (alt-screen via `crossterm`).
- `crossterm` for raw mode, mouse capture, bracketed paste, key events.
- `tokio` runtime; the event loop multiplexes:
- `App` struct owns all state; events are dispatched via async `select!`.
- Periodic refresh: 5s for session list polling (`REFRESH_INTERVAL`), 100ms for tick/animations (`TICK_INTERVAL`).
- All side effects happen inside `apply_event` / `handle_key`; rendering is a pure projection in `ui::draw_*`.
- A soft-restart of the loop is modelled via `RunOutcome::SwitchProfile(name)` bubbled up to `commands::terminal::run` (see `mod.rs:84-105`).
- Live in `terminal/ui.rs`. Pure functions taking `&App` + a `Frame<'_>`. No I/O, no mutation. Aliased with module-level doc.
- Never `eprintln!` — use `tracing::info!` and rely on `init_tracing_for_tui()` writing to the cache log file.
## Dashboard (SvelteKit) Conventions
- SvelteKit 2.x with Svelte 5 runes (`$state`, `$derived`, `$props`, `$effect`). All routes are SPA: `+layout.ts:4-6` sets `ssr = false; prerender = false`.
- Static adapter (`@sveltejs/adapter-static`) outputs to `dashboard/build/`, which `rust-embed` bakes into the daemon binary at compile time.
- Svelte writable stores in `dashboard/src/lib/stores/` carry typed state, e.g. `sessions.ts` exports `sessions: Writable<{loading, error, items}>` plus an async `loadSessions()` action.
- Stores own all fetch-then-merge logic; components read via `$store` and call action functions.
- The active profile id is its own writable (`activeProfileId` in `dashboard/src/lib/profiles.ts`); switching reloads the page rather than re-initialising each store individually.
- Each component opens with a Svelte 5 `<script lang="ts">` and a JSDoc-style block comment naming its purpose (see `RemoteAccessInfo.svelte:1-16`).
- Props are typed via a local `interface Props { … }` and destructured: `let { compact = false }: Props = $props();` (`RemoteAccessInfo.svelte:21-25`).
- State uses runes: `let fp = $state<CertFingerprint | null>(null);` and `let url = $derived(...)`.
- Component-scoped CSS via `<style>` blocks; CSS custom properties (`--surface`, `--border`, `--accent`, `--radius`) are themed centrally.
- Single `request<T>(path, init)` and `requestOn<T>(profileId, path, init)` functions wrap fetch.
- Bearer token is read from `getActiveProfile().token` on every call.
- 401 clears the active token (`setToken(null)`) and throws `ApiError(401)`; 5xx marks fetch failed; other 4xx throws but doesn't degrade connection state.
- All URL construction goes through `apiUrl(path)` (HTTP) and `wsUrl(path)` (WS) from `dashboard/src/lib/profiles.ts` so the active profile's `baseUrl` is honoured.
- Source of truth for the profile list is the page-origin daemon at `/api/profiles` (mirrors the TUI's `profiles.toml`).
- Tokens, labels, and the active profile id are browser-local. Storage keys: `agentum_profile_tokens`, `agentum_profile_labels`, `agentum_active`, `agentum_profile_cache`. Legacy `agentum_token` is migrated once on first load (`profiles.ts:65-68`).
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->
## Architecture

## Pattern Overview
- **One daemon, many clients.** `agentum serve` owns the SQLite DB, the tmux server, and the watchdog. The SvelteKit SPA (embedded into the binary via `rust-embed`), the `agentum terminal` TUI (`crates/agentum/src/commands/terminal/`), and `lazyagentum` (`crates/agentum/src/bin/lazyagentum.rs`) all reach it over the same HTTP/WS surface.
- **Trait-driven tool integration.** Every AI CLI (Claude, Codex, Cursor, Gemini, Hermes, …) implements `ToolAdapter` in `crates/agentum-executor/src/adapters.rs`; the rest of the codebase talks to the trait only, so a session row is tool-agnostic until launch time.
- **tmux as the process supervisor.** Each session is one detached tmux pane. The daemon never `exec`s an agent directly; it shells out via `crates/agentum-tmux/src/lib.rs` for spawn, pipe, capture, send-keys, and resize.
- **Event-sourced UI updates.** A single `tokio::sync::broadcast::Sender<Event>` channel (`AppState::bus`, `crates/agentum-server/src/lib.rs:69`) fans watchdog + lifecycle events to every connected WS client. Events are also persisted to the `events` table.
- **Background reconciliation.** A `Watchdog` task (`crates/agentum-watchdog/src/lib.rs`) polls the DB every 5 s and the running-session panes every 1 s, emitting `agent.finished`, `agent.awaiting_input`, `session.crashed`, `watchdog.compact`, etc.
- **TLS by default.** Self-signed cert + cert-server side port for trust-on-first-use; bearer-token middleware sits in front of every `/api/*` route except a small public allow-list.
## Layers
- Purpose: Dependency-light shared types (`Session`, `Status`, `Event`, `BoardItem`, `NewBoardItem`, `BoardPatch`, `User`, `Channel`, `Note`, `WatchdogEvent`, plus the transcript types). No tokio, no sqlx, no axum.
- Location: `crates/agentum-core/src/lib.rs`, `crates/agentum-core/src/board_schema.rs`, `crates/agentum-core/src/profiles.rs`, `crates/agentum-core/src/transcript.rs`.
- Contains: enums, structs with serde derives, validation helpers (`validate_name`, `validate_username`), the per-status board-rules matrix (`required_fields_for`, `validate_transition`).
- Depends on: `serde`, `time`, `uuid`, `thiserror`, `toml`. Nothing application-shaped.
- Used by: every other crate.
- Purpose: All persistence behind a single `Store` handle (`SqlitePool`). WAL mode, `synchronous=NORMAL`, file chmod 0600 because it holds Argon2id hashes + live bearer tokens.
- Location: `crates/agentum-store/src/lib.rs` (2092 lines — sessions, board, board comments, board column rules, notes, channels, events, users, auth_sessions, preferences).
- Contains: `Store::open`, per-table CRUD methods, `update_status_and_target`, `latest_agent_event_per_session`, `sweep_expired_auth_sessions`, XDG path resolution in `crates/agentum-store/src/paths.rs`.
- Migrations: 14 SQL files in `crates/agentum-store/migrations/` (`0001_initial.sql` … `0014_board_column_rules.sql`) baked in via `sqlx::migrate!("./migrations")`.
- Depends on: `agentum-core`, `sqlx`, `directories`.
- Used by: `agentum-server`, `agentum-watchdog`, `agentum` (CLI commands open the store directly for offline ops like `agentum auth setup`).
- Purpose: Thin shell-out wrapper over the `tmux` binary. No state.
- Location: `crates/agentum-tmux/src/lib.rs`.
- Contains: `target_for`, `has_session`, `new_session`, `kill_session`, `capture_pane`, `capture_pane_visible`, `capture_pane_ansi`, `send_keys`, `send_bytes`, `resize_window`, `pipe_pane`, `pane_current_command`, `pane_pid`, `graceful_stop`.
- Depends on: `tokio::process::Command`, `shlex` (only for the single shell-command string handed to `tmux new-session` / `pipe-pane`). No other agentum crate.
- Used by: `agentum-server` (start/stop/send/stream routes), `agentum-watchdog` (capture + send-keys for `/compact`), `agentum` CLI (the `agentum send`, `agentum keys`, `agentum open` commands).
- Purpose: A `ToolAdapter` trait per supported agent. Maps a `Session` to a concrete `LaunchCommand { argv, env }`. Owns YOLO-marker translation across tools.
- Location: `crates/agentum-executor/src/lib.rs` (trait + registry), `crates/agentum-executor/src/adapters.rs` (built-ins).
- Contains: `ToolAdapter` trait, `LaunchCommand`, `YOLO_MARKER` constant, `translate_yolo_marker`, `adapter_for(tool)`, `FIRST_CLASS` + `PASSTHROUGH_PROBED` lists, `probed_tools()`, `binary_for(tool)`.
- Adapters: `ClaudeAdapter`, `CodexAdapter`, `CursorAdapter`, `AgentAdapter`, `GeminiAdapter`, `HermesAdapter`, `TerminalAdapter`, `PassthroughAdapter`.
- Depends on: `agentum-core` only.
- Used by: `agentum-server::routes::sessions` (at `start`), `agentum-watchdog` (reads `compact_trigger`, `crash_signatures`, `busy_signature`, `awaiting_input_signatures`, `is_agent`), `agentum-server::routes::agents` (probes binaries via `which`).
- Purpose: One background task per running session. Captures pane every 1 s; emits events for context-low compaction, crashes, busy↔idle, awaiting-input.
- Location: `crates/agentum-watchdog/src/lib.rs`.
- Contains: `Watchdog::new`, `Watchdog::run` (reconcile loop on `RECONCILE_TICK = 5 s`), `watch_session` (per-session loop on `TICK = 1 s`), `classify_activity`, `ActivityState`, `bottom_lines`, `hash_str`, `canonical_tool_from_command`.
- Reconcile model: diff DB's `Status::Running` set against the in-memory `HashMap<Uuid, JoinHandle>`. Spawn missing tasks; abort orphans.
- Per-tick actions: crash-signature match → mark `crashed`; `Context low.*<\s*50%` regex → `send_keys(compact_trigger, Enter)` with 5-min cooldown; tool-drift detection via `pane_current_command`; activity classification → `agent.finished` / `agent.awaiting_input` / `agent.input_resolved` events.
- Depends on: `agentum-core`, `agentum-store`, `agentum-tmux`, `agentum-executor`, `regex`.
- Used by: `agentum-server::serve` spawns one of these alongside the HTTP server.
- Purpose: axum HTTP+WS API, TLS termination, auth middleware, embedded SvelteKit SPA, cert-server side port for TOFU bootstrap.
- Location: `crates/agentum-server/src/lib.rs` (entry + AppState + serve loop), `crates/agentum-server/src/routes/*.rs` (17 route modules), `crates/agentum-server/src/auth.rs`, `crates/agentum-server/src/tls.rs`, `crates/agentum-server/src/headers.rs`, `crates/agentum-server/src/embed.rs`, `crates/agentum-server/src/transcript_store.rs`, `crates/agentum-server/src/rules.rs`, `crates/agentum-server/src/ratelimit.rs`, `crates/agentum-server/src/error.rs`.
- Contains: `AppState` (Store + broadcast bus + TranscriptStore + cert fingerprint + rate limiter + hostname), `serve(opts, store)`, `router(state)`, `static_handler` (the SPA fallback).
- Depends on: every other crate plus `axum`, `axum-server`, `rust-embed`, `notify`, `rcgen`, `rustls`, `argon2`, `sysinfo`, `which`, `tower-http`.
- Used by: `agentum serve` CLI command (`crates/agentum/src/commands/serve.rs`).
- Purpose: Two binaries (`agentum`, `lazyagentum`) sharing a library shim (`crates/agentum/src/lib.rs`). Houses subcommands + the ratatui TUI.
- Location: `crates/agentum/src/main.rs` (entry), `crates/agentum/src/cli.rs` (clap definitions + `dispatch`), `crates/agentum/src/commands/` (one file per subcommand), `crates/agentum/src/commands/terminal/` (the TUI app).
- TUI: `crates/agentum/src/commands/terminal/mod.rs` boots the alt-screen + connect-or-onboard loop; `app.rs` holds state + event loop; `ui.rs` draws the panes; `api.rs` is the HTTP/WS client; `pty.rs` spawns the local lazygit pane; `prefs.rs` + `profiles.rs` persist per-host UX state; `trust.rs` is the SSH-style cert pinner; `theme.rs` + `palette.rs` + `extensions.rs` are pure UX.
- Depends on: every other crate plus `ratatui`, `crossterm`, `tui-term`, `vt100`, `portable-pty`, `reqwest`, `tokio-tungstenite`, `tokio-rustls`, `clap`, `rpassword`, `url`.
## Data Flow
- Persistent: SQLite (`Store`). Tables: sessions, events, board_items, board_comments, board_column_rules, notes, channels, channel_messages, users, auth_sessions, preferences.
- In-memory only: `AppState::transcripts` (transcript snapshots), `AppState::stream_positions` (per-session WS replay markers), `AppState::auth_limiter` (rate limiter), `Watchdog::tasks` (per-session task handles).
- Client-local: TUI `~/.config/agentum/profiles.toml` + `credentials.toml`; dashboard `localStorage` (`agentum_profile_tokens`, `agentum_profile_labels`, `agentum_profile_cache`, `agentum_active`).
## Key Abstractions
- Purpose: Single source of truth for per-agent launch semantics + watchdog signatures.
- Location: `crates/agentum-executor/src/lib.rs:38`.
- Pattern: trait with default-empty methods so each new adapter is a ~30-line file. Methods: `name()`, `launch(&Session) -> LaunchCommand`, `compact_trigger()`, `crash_signatures()`, `busy_signature()`, `awaiting_input_signatures()`, `yolo_flag()`, `is_agent()`.
- Examples: `crates/agentum-executor/src/adapters.rs` — `ClaudeAdapter` (lines 41–141, full instrumentation), `CodexAdapter` (lines 145–174), `CursorAdapter`, `AgentAdapter`, `GeminiAdapter`, `HermesAdapter`, `TerminalAdapter`, `PassthroughAdapter` (catch-all for unknown tools).
- Purpose: The atomic unit of agentum. A `(name, workdir, tool, model?, flags[])` tuple plus runtime observability fields.
- Location: `crates/agentum-core/src/lib.rs:67`.
- Pattern: serde-derive struct used everywhere — DB row, HTTP payload, WS event payload, TUI state, dashboard state. Optional fields (`tokens`, `cost_usd`, `ctx`, `last_log`, `uptime_seconds`, `state`, `pinned`) are `skip_serializing_if = "Option::is_none"` for compact wire.
- Purpose: The lingua franca for everything happening in the daemon. Dotted `kind` strings (`session.started`, `session.crashed`, `agent.finished`, `agent.awaiting_input`, `agent.input_resolved`, `watchdog.compact`, `agent_tasks.updated`, `host.metrics`, `bus.lagged`).
- Location: `crates/agentum-core/src/lib.rs:165`.
- Pattern: `Event::new(kind).with_session(id, name).with_payload(json!({…}))`. Both persisted (via `Store::insert_event`) and broadcast on `AppState::bus`. Projects to `WatchdogEvent` for the dashboard's compact wire shape.
- Purpose: Shared handler-state for every axum route. Cloneable (cheap — wraps `Arc`s).
- Location: `crates/agentum-server/src/lib.rs:67`.
- Pattern: holds `Arc<Store>`, `broadcast::Sender<Event>`, `Arc<RateLimiter>`, `Arc<String>` (cert fingerprint), `TranscriptStore`, `Arc<Mutex<HashMap<Uuid, StreamCheckpoint>>>`, `hostname: String`, `no_auth: bool`.
- Purpose: The contract between an adapter and tmux: what argv to exec, what env to inject.
- Location: `crates/agentum-executor/src/lib.rs:22`.
- Purpose: TUI's named-endpoint list (URL + optional fingerprint + insecure flag). Persisted to `$XDG_CONFIG_HOME/agentum/profiles.toml`.
- Location: `crates/agentum-core/src/profiles.rs`. Dashboard mirrors the same wire shape; `/api/profiles` (`crates/agentum-server/src/routes/profiles.rs`) is the canonical sync point.
## Entry Points
- Location: `crates/agentum/src/main.rs` → `crates/agentum/src/cli.rs::dispatch`.
- Triggers: User invocation (`agentum new`, `agentum serve`, `agentum terminal`, …).
- Responsibilities: parse clap args, route to the matching `crates/agentum/src/commands/*.rs` module. `Cmd::Terminal` swaps tracing to a file before entering the alt-screen (stderr would scramble ratatui).
- Location: `crates/agentum/src/commands/serve.rs::run` → `agentum_server::serve` (`crates/agentum-server/src/lib.rs:210`).
- Triggers: `agentum serve [--detach]`.
- Responsibilities: open the store, optionally re-spawn previously-stopped sessions, generate or load TLS cert, build `AppState`, spawn the auth-session sweeper, spawn the watchdog, spawn the host-metrics ticker, bind axum on the main port + the cert-server on the side port.
- Location: `crates/agentum/src/bin/lazyagentum.rs`.
- Triggers: User invocation (`lazyagentum`). Equivalent to `agentum terminal` with no other subcommands.
- Responsibilities: re-uses `agentum::commands::terminal::run`.
- Location: `crates/agentum/src/commands/terminal/mod.rs::run` → `crates/agentum/src/commands/terminal/app.rs`.
- Triggers: `agentum terminal [--api … | --profile …]` or the `lazyagentum` binary.
- Responsibilities: connect-or-onboard loop (interactive prompt when daemon unreachable on a TTY), TLS trust pin, login flow if the daemon requires it, then enter the alt-screen ratatui loop that subscribes to `/api/events` and per-session `/stream` WS endpoints.
- Location: `dashboard/src/routes/+layout.svelte` (chrome) + `dashboard/src/routes/*/+page.svelte` (pages: `home`, `sessions`, `sessions/[id]`, `board`, `terminals`, `settings`).
- Triggers: Any non-API HTTP path served by the daemon — `crates/agentum-server/src/embed.rs::static_handler` resolves the request against the embedded `dashboard/build/` tree, falling back to `index.html` for client-routed paths.
- Responsibilities: `+layout.svelte` boots `TokenGate`, connects `/api/events`, starts the host-metrics, attention, event-bridge, and theme-bridge stores. Each route is a self-contained `+page.svelte`.
## Error Handling
- **Crate-level error enums:** `CoreError` (`crates/agentum-core/src/lib.rs:21`), `StoreError` (`crates/agentum-store/src/lib.rs:21`), `TmuxError` (`crates/agentum-tmux/src/lib.rs:14`), `WatchdogError` (`crates/agentum-watchdog/src/lib.rs:50`), `TlsError` (`crates/agentum-server/src/tls.rs:14`), `AuthError` (`crates/agentum-server/src/auth.rs:27`).
- **HTTP error envelope:** `ApiError` (`crates/agentum-server/src/error.rs:8`) — variants `NotFound`, `Conflict`, `BadRequest`, `Unauthorized`, `Forbidden`, `TooManyRequests`, `Internal`, plus `Custom(StatusCode, Value)` for handlers that need a non-default JSON body (board column-rule validators return `{ "missing": [...], "status": "doing" }`). `From<StoreError> for ApiError` maps `NotFound`/`AlreadyExists`/`Core` to user-visible statuses; everything else logs at `error` and surfaces as 500.
- **anyhow at the CLI edge:** `crates/agentum/src/commands/*.rs` use `anyhow::Result<()>` with `Context::context(…)` annotations.
- **Watchdog never panics, only logs:** failed `tmux` calls inside `watch_session` use `tracing::warn!` and `continue` rather than aborting the task. Crashes are reported via `Event::new("session.crashed")` and a status update.
## Cross-Cutting Concerns
- Username/password with Argon2id hashes stored in the `users` table (`crates/agentum-server/src/auth.rs`).
- Each login mints a 32-byte URL-safe random token, stored in `auth_sessions` with sliding 30-day expiry. Bearer presented as `Authorization: Bearer …` (HTTP) or `?token=…` (WS, since browsers can't set headers on upgrade).
- `require_token` middleware (`crates/agentum-server/src/auth.rs:150`) is layered onto every router merge. Public allow-list: `/api/health`, `/api/cert`, `/api/cert/fingerprint`, `/api/auth/status`, `/api/auth/login`, `/api/auth/register`.
- `--no-auth` flag bypasses the middleware entirely (the dashboard auto-skips the login screen).
- Self-signed cert generated on first boot and cached at `$XDG_DATA_HOME/agentum/tls/{cert,key}.pem` (`crates/agentum-server/src/tls.rs`).
- SHA-256 fingerprint printed to the host TTY on boot and served from the plain-HTTP cert-server on the side port (default 8823) for trust-on-first-use bootstrap from a phone.
- HSTS deliberately omitted (self-signed + HSTS = footgun across cert rotations).
- `tracing` everywhere; subscriber configured by `crates/agentum/src/lib.rs::init_tracing` (stderr, EnvFilter via `AGENTUM_LOG`) or `init_tracing_for_tui` (file at `$XDG_CACHE_HOME/agentum/tui.log` — never stderr, which would scramble the alt-screen).
- HTTP requests run through `crates/agentum-server/src/logging.rs::redacting_trace_layer()` (a `tower-http` `TraceLayer` that strips `Authorization` headers and `?token=` query params).
- Every HTTP response gets CSP, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Cross-Origin-Opener-Policy: same-origin` via the middleware in `crates/agentum-server/src/headers.rs`.
- CSP allows `connect-src 'self' http: https: ws: wss:` so the dashboard can fan out to multiple agentum endpoints from one origin (named-profiles feature).
- Login + register attempts: 8 per remote IP per 5-minute window (`crates/agentum-server/src/lib.rs:60`, enforced via `crates/agentum-server/src/ratelimit.rs`).
- All XDG paths funnel through `crates/agentum-store/src/paths.rs`: `data_dir`, `config_dir`, `cache_dir`, `state_dir`, `db_path`, `auth_token_path`, `tls_dir`, `pane_log(session_id)`.
- One bus (`tokio::sync::broadcast::Sender<Event>`, capacity 1024) drives every push channel. Slow consumers see `RecvError::Lagged(n)` and a `bus.lagged` synthetic event before the stream resumes (`crates/agentum-server/src/routes/events.rs:85`).
- `crates/agentum-server/src/embed.rs` uses `rust-embed` to inline `dashboard/build/` into the binary. `crates/agentum-server/build.rs` materialises a placeholder stub if the user runs `cargo build` before `pnpm --dir dashboard build`. Static-asset cache: `_app/immutable/*` gets `public, max-age=31536000, immutable`; everything else gets `no-cache`. SPA fallback always returns `index.html` so the SvelteKit client router takes over.
<!-- GSD:architecture-end -->

<!-- GSD:skills-start source:skills/ -->
## Project Skills

| Skill | Description | Path |
|-------|-------------|------|
| higgsfield-generate | \| Generate images/videos via Higgsfield AI. Default: GPT Image 2 for images/design/text, Seedance 2.0 for video, Nano Banana 2/Pro for character/reference image work, Marketing Studio for ads with avatars/products/hooks, settings, plus Soul V2/Cinema/Cast/Location and Kling 3.0. Use when: "generate an image", "make a video", "animate this photo", "image-to-video", "edit/stylize/remix this image", "produce a clip", "create an ad", "make a UGC video", "product demo", "unboxing", "brand video", "presenter video", "import product from URL", "create avatar for ad", or "analyze video virality". Supports image-to-image, image-to-video, references, job/upload IDs, and Marketing Studio. Chain with higgsfield-soul-id for face/identity consistency. Virality Predictor (`brain_activity`) analyzes video virality: hook strength, attention, retention, distraction risk, and creative score. NOT for: Soul Character training (use higgsfield-soul-id), product photoshoots, marketplace listing cards, text/chat/TTS tasks. | `.agents/skills/higgsfield-generate/SKILL.md` |
| higgsfield-marketplace-cards | \| Generate marketplace product image cards through Higgsfield: compliant main image, secondary product images, and A+ style content modules. Use when the user asks for marketplace listing images, product detail cards, secondary product images, product infographics, lifestyle listing shots, A+ style content, marketplace image sets, or sales-ready product visuals. Backend owns marketplace compliance references and prompt templates; this skill only routes user intent to the CLI. NOT for generic brand product photography without marketplace/listing context (use higgsfield-product-photoshoot), video generation or UGC ads (use higgsfield-generate), or Soul Character training (use higgsfield-soul-id). | `.agents/skills/higgsfield-marketplace-cards/SKILL.md` |
| higgsfield-product-photoshoot | \| Generate brand-quality product images through Higgsfield product-photoshoot prompt enhancement on GPT Image 2 / gpt_image_2. Entry point for professional brand/product visuals. Use when: "product photo", "studio shot", "lifestyle image", "Pinterest pin", "hero/banner", "carousel", "ad creative", "Meta ads", "virtual try-on", "model wearing", "person holding product", "closeup with hands", "levitating/floating/splash product", "CGI/surreal product", "restyle", "seasonal/aesthetic variation", or any product, brand, or paid-social creative. Modes: product_shot, lifestyle_scene, closeup_product_with_person, moodboard_pin, hero_banner, social_carousel, ad_creative_pack, virtual_model_tryout, conceptual_product, restyle. Backend assembles the final prompt; never freehand it. NOT for: no-product text-to-image (use higgsfield-generate), branded avatar video (use higgsfield-generate Marketing Studio), marketplace listing cards (use higgsfield-marketplace-cards), Soul Character training (use higgsfield-soul-id). | `.agents/skills/higgsfield-product-photoshoot/SKILL.md` |
| higgsfield-soul-id | \| Train a Soul Character — a personalized model on a person's face that Higgsfield uses for identity-faithful image and video generation. Use when: "create my Soul", "train my face", "make my digital twin", "build me an avatar", "learn my appearance", "create a character of me", "set up identity for video", "I want my face in generated images". Chain: train Soul (one-time, returns reference_id) → use in higgsfield-generate via `--soul-id <id>` with models like `text2image_soul_v2` or `soul_cinema_studio`. NOT for: one-shot face swaps (use higgsfield-generate with --image), named-character / non-photo avatars (use higgsfield-generate with prompt). | `.agents/skills/higgsfield-soul-id/SKILL.md` |
<!-- GSD:skills-end -->

<!-- GSD:workflow-start source:GSD defaults -->
## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:
- `/gsd-quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd-debug` for investigation and bug fixing
- `/gsd-execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->

<!-- GSD:profile-start -->
## Developer Profile

> Profile not yet configured. Run `/gsd-profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
