# Technology Stack

**Analysis Date:** 2026-05-20

## Languages

**Primary:**
- Rust (edition 2024, MSRV 1.85) — Workspace at `Cargo.toml` with seven member crates under `crates/`. Pinned via `rust-toolchain.toml` (`channel = "stable"`, components `rustfmt`, `clippy`).
- TypeScript 5.7 — Dashboard SPA under `dashboard/src/`. `tsconfig.json` extends SvelteKit's generated config with `strict: true`, `checkJs: true`, `moduleResolution: "bundler"`.

**Secondary:**
- Svelte 5 (`.svelte` components in `dashboard/src/lib/components/` and `dashboard/src/routes/`).
- SQL — Hand-written sqlx migrations in `crates/agentum-store/migrations/` (`0001_initial.sql` … `0014_board_column_rules.sql`).
- HTML/CSS — Static marketing site under `web/` (`web/index.html`, `web/sitemap.xml`, `web/robots.txt`); SvelteKit shell at `dashboard/src/app.html` + `dashboard/src/app.css`.
- Bash — Installer script `scripts/install.sh`; recipes in `justfile`.

## Runtime

**Environment:**
- Tokio 1.x with the `full` feature (workspace dep in `Cargo.toml`) — async runtime driving the daemon, watchdog, and TUI.
- Node.js 22 — Pinned by `.github/workflows/ci.yml` and `.github/workflows/release.yml` for the dashboard build. Required only to produce `dashboard/build/` which is then embedded into the Rust binary.
- Browser runtime — SvelteKit SPA executes in the dashboard tab; a service worker (`dashboard/src/service-worker.ts`) precaches the build chunks for offline-friendly reloads.

**Package Manager:**
- Cargo (workspace mode, `resolver = "2"`) — see `Cargo.toml` lines 1–11.
- pnpm 9 — Dashboard dependencies. Lockfile at `dashboard/pnpm-lock.yaml`. CI installs via `pnpm/action-setup@v4`.
- Lockfile: present (`Cargo.lock` committed at repo root; `dashboard/pnpm-lock.yaml` committed).

## Frameworks

**Core (server / daemon):**
- axum 0.8 with `ws` + `macros` features — HTTP router and WebSocket upgrades. Mounted from `crates/agentum-server/src/lib.rs::router`.
- axum-server 0.7 with `tls-rustls` — Binds the TLS listener in `crates/agentum-server/src/lib.rs::serve`.
- tower 0.5 + tower-http 0.6 (`cors`, `trace`, `compression-gzip`) — Middleware stack layered in `router()` and `crates/agentum-server/src/logging.rs`.
- rustls 0.23 with the `ring` crypto provider — Installed via `rustls::crypto::ring::default_provider().install_default()` in `crates/agentum-server/src/lib.rs`. Self-signed cert generation lives in `crates/agentum-server/src/tls.rs` (rcgen 0.13).
- sqlx 0.8 with `runtime-tokio`, `sqlite`, `macros`, `migrate` features — Sole persistence layer in `crates/agentum-store/src/lib.rs`. Bundled `sqlite` feature deliberately omitted; the workspace consumes sqlx's `sqlite` driver against the system libsqlite (see comment in `Cargo.toml` lines 39–41).

**Core (TUI / CLI):**
- clap 4 with `derive` + `env` — CLI parsing in `crates/agentum/src/cli.rs`.
- ratatui 0.29 with the `crossterm` backend — Terminal UI in `crates/agentum/src/commands/terminal/`.
- crossterm 0.28 with `event-stream` — Raw mode, key/mouse events, alt-screen lifecycle.
- tui-term 0.2 + vt100 0.15 + portable-pty 0.9 — Embedded PTY rendering for the in-TUI side pane (lazygit-style).
- reqwest 0.12 (`json`, `rustls-tls`, `stream`, no default features) — TUI's HTTP client against the daemon's REST surface.
- tokio-tungstenite 0.24 (`connect`, `rustls-tls-native-roots`) — TUI's WebSocket client for `/api/sessions/{id}/stream` and `/api/events`.

**Core (dashboard):**
- SvelteKit 2.17 + Svelte 5.19 — Application framework. Config at `dashboard/svelte.config.js`, Vite plugin in `dashboard/vite.config.ts`.
- `@sveltejs/adapter-static` 3 — Renders the SPA to `dashboard/build/` with `fallback: 'index.html'` so client-side routing works after embedding.
- Vite 6 — Bundler. Dev server proxies `/api` → `http://127.0.0.1:8822` (`dashboard/vite.config.ts` lines 7–21).
- xterm.js 5.5 + `@xterm/addon-fit` 0.10 — Embedded terminal widget in `dashboard/src/lib/components/Terminal.svelte` (paired with `TerminalPanel.svelte`).

**Testing:**
- Built-in Rust unit tests via `cargo test --workspace --lib` — Adapter behaviour in `crates/agentum-executor/src/adapters.rs::tests`; service-level tests in each crate.
- Vitest 4 — Pure-data tests for the dashboard. Configured in `dashboard/vite.config.ts` (`include: ['src/**/*.{test,spec}.ts']`). No DOM environment configured.
- svelte-check 4 — Type-checks Svelte + TS via `pnpm --dir dashboard check` (script in `dashboard/package.json`).
- tempfile 3 — Dev-dep across `agentum-store`, `agentum-core`, `agentum-server`, `agentum` for sandboxed XDG paths.

**Build / Dev:**
- `cargo` (release profile in `Cargo.toml`: `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`).
- `cross` — Used by `.github/workflows/release.yml` for `aarch64-unknown-linux-gnu`.
- `just` — Task runner. `justfile` exposes `dev`, `build`, `check`, `test`, `fmt` recipes.
- rust-embed 8 with `mime-guess` — Compile-time embedding of `dashboard/build/` into the daemon binary; wired in `crates/agentum-server/src/embed.rs`.

## Key Dependencies

**Critical (runtime persistence and process control):**
- sqlx 0.8 (`crates/agentum-store/Cargo.toml`) — Async SQLite driver. WAL journal mode, synchronous=NORMAL, `max_connections = 8` (`crates/agentum-store/src/lib.rs::Store::open`).
- tokio 1 — Async runtime everywhere.
- tokio::process — Wrapping the `tmux` binary in `crates/agentum-tmux/src/lib.rs`. No bindings — every command shells out with one `.arg()` per argument; `shlex` 1 quotes the inner shell-string passed to `tmux new-session`.
- rust-embed 8 — Bakes the dashboard bundle into the release binary. Folder pinned to `../../dashboard/build` in `crates/agentum-server/src/embed.rs`.
- notify 8 — Filesystem watcher used by `crates/agentum-server/src/transcript_store.rs` to tail Claude Code JSONL transcripts.
- sysinfo 0.32 (`system` feature, default features off) — CPU/RAM sampling in `crates/agentum-server/src/routes/host.rs`.
- regex 1 — Crash and context-low signature matching in `crates/agentum-watchdog/src/lib.rs`.

**Security:**
- rustls 0.23 (`ring`, `std`, `tls12`, default features off) — TLS in both daemon (`crates/agentum-server`) and TUI client (`crates/agentum`).
- rcgen 0.13 — Self-signed certificate generation (`crates/agentum-server/src/tls.rs`).
- argon2 0.5 — Password hashing in `crates/agentum-server/src/auth.rs` (run on the blocking pool via `tokio::task::spawn_blocking`).
- password-hash 0.5 with `getrandom` — Salt + PHC string handling alongside argon2.
- sha2 0.10 — Cert fingerprint hash (`crates/agentum-server/src/tls.rs::cert_fingerprint`) and shared crypto in the TUI.
- rand 0.9 — Bearer-token randomness (`crates/agentum-server/src/auth.rs::new_token`).
- base64 0.22 — URL-safe encoding of tokens and PEM decoding.
- tokio-rustls 0.26 — Used by the TUI for TLS-pinned WS connects.

**Infrastructure:**
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

**Dashboard runtime:**
- `@xterm/xterm` 5.5 + `@xterm/addon-fit` 0.10 — Embedded terminal in the dashboard (`dashboard/src/lib/components/Terminal.svelte`).

## Configuration

**Environment:**
- `AGENTUM_BACKEND` — Vite dev-server proxy target for `/api` (`dashboard/vite.config.ts` line 7). Defaults to `http://127.0.0.1:8822`.
- `AGENTUM_TUI_NO_SOUND` — Mutes TUI chimes (`crates/agentum/src/commands/terminal/mod.rs:124`).
- `AGENTUM_THEME` — Overrides the TUI theme name (`crates/agentum/src/commands/terminal/theme.rs:297`).
- `SHELL` — Honored by `TerminalAdapter` to launch the user's shell (`crates/agentum-executor/src/adapters.rs:305`); falls back to `bash`.
- `EDITOR` / `VISUAL` — Used by `agentum config edit` (`crates/agentum/src/commands/config.rs:92-93`).
- `HOME`, `PATH`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `TMUX` — Read for path resolution, binary probing, daemon logs, profile storage, and tmux-detection.
- No `.env` files in repo; configuration is OS-environment + on-disk TOML, not dotenv.

**Build:**
- `Cargo.toml` (root workspace manifest, lines 21–48) pins shared dependency versions.
- Per-crate `Cargo.toml` files (e.g. `crates/agentum-server/Cargo.toml`, `crates/agentum/Cargo.toml`) add binary-specific deps.
- `rust-toolchain.toml` pins channel `stable` with `rustfmt` + `clippy`.
- `dashboard/svelte.config.js` configures the static adapter (`pages: 'build'`, `fallback: 'index.html'`).
- `dashboard/vite.config.ts` configures the dev server, proxy, and Vitest include glob.
- `dashboard/tsconfig.json` extends the SvelteKit-generated TS config.

**Runtime config files (created on first boot, not in repo):**
- `$XDG_DATA_HOME/agentum/db.sqlite` (`crates/agentum-store/src/paths.rs::db_path`).
- `$XDG_DATA_HOME/agentum/tls/{cert,key}.pem` — Self-signed TLS material (`crates/agentum-server/src/tls.rs::ensure_artifacts`; mode 0600).
- `$XDG_CACHE_HOME/agentum/sessions/<id>.log` — Pane logs (`crates/agentum-store/src/paths.rs::pane_log`).
- `$XDG_CONFIG_HOME/agentum/profiles.toml` — Endpoint profiles (CLAUDE.md notes).
- `$XDG_CONFIG_HOME/agentum/known_hosts.toml` — TOFU-pinned cert fingerprints (`crates/agentum/src/commands/terminal/mod.rs`).

## Platform Requirements

**Development:**
- Unix-like host (Linux or macOS). Windows is not targeted: tmux is required at runtime and unix-only build flags pull in `libc` (`crates/agentum/Cargo.toml:65-67`).
- tmux installed on `PATH` — Hard requirement; every session runs as a tmux pane. Probed by `agentum doctor` (`crates/agentum/src/commands/doctor.rs:69`).
- `lf` on `PATH` for `agentum new --pick` (workdir picker).
- Node.js 22 + pnpm 9 — Only when rebuilding the dashboard bundle.
- A C compiler is not required: sqlx uses the unbundled SQLite system driver (see workspace `Cargo.toml` lines 39–41). The user-level note in CLAUDE.md cautions against breaking `cc` shims for cc-rs builds.

**Production:**
- Linux x86_64 (glibc 2.35 — Ubuntu 22.04 baseline so binaries run on Debian 12) and aarch64 (cross-built via `cross`).
- macOS x86_64 and arm64 (`x86_64-apple-darwin`, `aarch64-apple-darwin`) — built on `macos-14` runners.
- Distribution: tarballs + `install.sh` attached to GitHub Releases by `.github/workflows/release.yml`. README's one-liner installer pulls `releases/latest/download/install.sh`.
- The compiled `agentum` binary is self-contained — it embeds the dashboard bundle via `rust-embed`, generates its own TLS material, and migrates SQLite on first boot.

---

*Stack analysis: 2026-05-20*
