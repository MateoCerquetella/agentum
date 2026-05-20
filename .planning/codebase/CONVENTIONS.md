# Coding Conventions

**Analysis Date:** 2026-05-20

This codebase is a Rust workspace (six member crates) plus a SvelteKit SPA. Conventions are written separately per language and enforced via `cargo fmt`, `cargo clippy`, `svelte-check`, and `tsc --strict`. There are no pre-commit hooks (no `.husky`, no `lefthook`); the contract is "CI runs `fmt --check`, `clippy -D warnings`, `cargo test --all`" — see `.github/workflows/ci.yml`. Local convenience commands live in `justfile`.

## Naming Patterns

### Rust

**Files:**
- snake_case, one module per file: `crates/agentum-server/src/routes/board.rs`, `crates/agentum-server/src/routes/board_rules.rs`, `crates/agentum/src/commands/terminal/profiles.rs`.
- Inline test modules use `tests` (or a topic-specific name like `selection_tests`, `merge_dedup_tests`, `profile_targets_loopback_tests`) — see `crates/agentum/src/commands/terminal/app.rs`.

**Functions:**
- snake_case for free functions and methods.
- Route handlers are short verbs: `list`, `create`, `get_one`, `patch`, `delete`, `start`, `stop`, `kill`, `send`, `stream` — see `crates/agentum-server/src/routes/sessions.rs:46-411`.
- Builder/constructor helpers use `new`, `with_*`, `from_*`: `AppState::new`, `AppState::with_fingerprint` (`crates/agentum-server/src/lib.rs:108-133`).
- Test-only `pub(crate)` helpers are prefixed `tests_helpers_*` to make their purpose obvious: `tests_helpers_create`, `tests_helpers_patch` in `crates/agentum-server/src/routes/board.rs:377-392`.

**Types:**
- `PascalCase` for structs, enums, traits, and type aliases: `AppState`, `ApiError`, `Session`, `Status`, `ToolAdapter`, `LaunchCommand`.
- One-letter enum variants are avoided; status/lifecycle variants are full words (`Status::Idle`, `Status::Running`, `Status::Stopped`, `Status::Crashed`) — see `crates/agentum-core/src/lib.rs:30-35`.

**Constants:**
- SCREAMING_SNAKE_CASE: `EVENT_BUS_CAPACITY`, `AUTH_RATE_LIMIT_ATTEMPTS`, `AUTH_RATE_LIMIT_WINDOW`, `GRACEFUL_STOP_TIMEOUT`, `IDLE_AFTER_QUIET` (`crates/agentum-server/src/lib.rs:57`, `crates/agentum-watchdog/src/lib.rs:44`).

**Modules:**
- All workspace crates are prefixed `agentum-`: `agentum-core`, `agentum-store`, `agentum-tmux`, `agentum-watchdog`, `agentum-executor`, `agentum-server`, plus the binary crate `agentum`.
- Inside `agentum/src/commands/`, one file per subcommand: `new.rs`, `up.rs`, `down.rs`, `kill.rs`, `send.rs`, `tail.rs`, `open.rs`, `ls.rs`, `rm.rs`, `serve.rs`, `doctor.rs`, `auth.rs`, `keys.rs`, `hosts.rs`, `profiles.rs`, `update.rs`, `uninstall.rs`, `config.rs`, plus the `terminal/` subtree.

### TypeScript / Svelte

**Files:**
- Svelte components: PascalCase `.svelte` files in `dashboard/src/lib/components/`: `Sidebar.svelte`, `NewSessionDialog.svelte`, `EndpointSwitcher.svelte`, `TokenGate.svelte`, `ToastStack.svelte`. Dashboard-specific subcomponents live under `dashboard/src/lib/components/dashboard/` (`FleetRow.svelte`, `SessionRail.svelte`, etc.).
- Stores: lowercase, plural noun (`sessions.ts`, `events.ts`, `board.ts`, `attention.ts`) in `dashboard/src/lib/stores/`.
- Shared modules: lowercase: `api.ts`, `profiles.ts`, `dashboard.ts`, `board-schema.ts`.
- Tests: `*.test.ts` co-located with the module under test: `dashboard/src/lib/board-schema.test.ts`.

**Symbols:**
- camelCase for functions, variables, and store names: `apiUrl`, `wsUrl`, `getActiveProfile`, `loadSessions`, `markFetchOk`.
- PascalCase for interfaces, types, and Svelte components: `Session`, `BoardItem`, `Profile`, `Toast`, `ConnStatus`.
- SCREAMING_SNAKE_CASE for module-level constants: `TOKENS_KEY`, `LABELS_KEY`, `SYNTHETIC_ID`, `HTTP_FAIL_THRESHOLD` (`dashboard/src/lib/profiles.ts:60-70`, `dashboard/src/lib/stores/events.ts:51`).

## Code Style

### Rust

**Formatting:**
- `cargo fmt --all` (rustfmt defaults, no custom `rustfmt.toml`).
- `rust-toolchain.toml` pins `stable` with `rustfmt` + `clippy` components.
- Edition `2024`, MSRV `1.85` (workspace-level in `Cargo.toml:15-16`).

**Linting:**
- `cargo clippy --all-targets --all-features -- -D warnings`. CI fails on any warning.
- Use `#[allow(dead_code)]` with a comment that names the future use — see `crates/agentum/src/commands/terminal/app.rs:53` (`fields populated for future toast-dedup logic`) and `crates/agentum/src/commands/terminal/api.rs:67`.
- Use `#[allow(clippy::field_reassign_with_default)]` only when the test deliberately mutates a `Default::default()` instance between assertions — example at `crates/agentum/src/commands/terminal/prefs.rs:313`.

### TypeScript

**Formatting:**
- No `.prettierrc` or `biome.json` is checked in. The project relies on `svelte-check` + `tsc --strict` (configured in `dashboard/tsconfig.json`) and per-file consistency.
- Indent style: 2 spaces. Single quotes for strings. Trailing semicolons.

**Strictness:**
- `dashboard/tsconfig.json` enables `"strict": true`, `"checkJs": true`, `"allowJs": true`, `"forceConsistentCasingInFileNames": true`.
- TypeScript `interface` is preferred for wire shapes that mirror Rust structs; `type` is used for unions: `type Status = 'idle' | 'running' | 'stopped' | 'crashed'` (`dashboard/src/lib/api.ts:9`).

## Import Organization

### Rust

**Order (rustfmt default, observed consistently):**
1. `std` imports
2. External crate imports (`agentum_*` workspace crates, then third-party)
3. `crate::` / `super::` imports

Each group separated by a blank line. Example from `crates/agentum-server/src/routes/sessions.rs:1-23`:

```rust
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use agentum_core::{Event, NewSession, Session, Status};
use agentum_store::paths;
use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use bytes::Bytes;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::time::sleep;
use uuid::Uuid;

use crate::AppState;
use crate::StreamCheckpoint;
use crate::error::ApiError;
```

**Path aliases (workspace):**
- Internal crates referenced by their `agentum-*` package name, e.g. `use agentum_core::Session;`, `use agentum_store::Store;`. Workspace `Cargo.toml:21-27` defines them with `{ path = "crates/agentum-*" }` so dependents pull in `{ workspace = true }`.

### TypeScript

**Path aliases (`dashboard/svelte.config.js`):**
- `$components` → `src/lib/components`
- `$stores` → `src/lib/stores`
- `$themes` → `src/lib/themes`
- `$lib` (SvelteKit built-in) → `src/lib`
- `$app/*` (SvelteKit built-in) for `$app/state`, `$app/stores`, etc.

**Order (observed):**
1. SvelteKit `$app/*` imports
2. Local `$stores/*`, `$lib/*`, `$components/*` aliases
3. Type imports interleaved using `type` modifier: `import { type Session } from '$lib/api'`
4. Relative imports (`./`, `../`) only inside the same directory cluster

Example (`dashboard/src/lib/components/Sidebar.svelte:1-15`):

```ts
import { page } from '$app/state';
import { sessions } from '$stores/sessions';
import { openPalette } from '$stores/palette';
import { openNewSession } from '$stores/newSession';
import { type Session } from '$lib/api';
import { connStatus } from '$stores/events';
import {
  profiles,
  activeProfileId,
  setActiveProfile,
  type Profile
} from '$lib/profiles';
```

## Error Handling

### Rust application layer (`anyhow`)

- Binary entry points (`crates/agentum/src/main.rs`, `crates/agentum/src/commands/*.rs`) return `anyhow::Result<T>`.
- `bail!` for early returns with a formatted message: `bail!("session {name} is not running")` in `crates/agentum/src/commands/send.rs:17`.
- `.context(...)` to attach human-readable context to lower-level errors: imported via `use anyhow::{Context, Result, bail};` (e.g. `crates/agentum/src/commands/config.rs:5`).
- Process-exit on user-input errors uses `eprintln!` + `std::process::exit(N)` with distinct exit codes (e.g. exit 3 for "no such session" in `crates/agentum/src/commands/send.rs:7-9`).

### Rust library layer (`thiserror`)

- Each crate defines a typed error enum with `#[derive(Debug, thiserror::Error)]`:
  - `agentum_core::CoreError` — `crates/agentum-core/src/lib.rs:20-26`
  - `agentum_store::StoreError` — `crates/agentum-store/src/lib.rs:21-45` with `#[from]` conversions for every dependency (`sqlx::Error`, `serde_json::Error`, `time::error::*`, `uuid::Error`, `std::io::Error`, plus higher-level `CoreError`).
  - `agentum_executor` — no error type; adapters return `LaunchCommand` infallibly.
  - `agentum_watchdog::WatchdogError` — `crates/agentum-watchdog/src/lib.rs:50-56`.
  - `agentum_server::ApiError` — `crates/agentum-server/src/error.rs:7-32`.
  - `agentum_server::auth::AuthError` — `crates/agentum-server/src/auth.rs:27-33`.
- Each crate also defines a local `type Result<T> = std::result::Result<T, ThisError>;` alias (e.g. `crates/agentum-store/src/lib.rs:47`).

### HTTP error handling (`ApiError`)

`crates/agentum-server/src/error.rs` is the canonical pattern. `ApiError` is an enum with structured variants for each common HTTP status (`NotFound`, `Conflict`, `BadRequest`, `Unauthorized`, `Forbidden`, `TooManyRequests`, `Internal`) plus an `ApiError::Custom(StatusCode, serde_json::Value)` escape hatch for handlers that need a non-default JSON body shape (board column-rules gate uses this).

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

**Framework:** `tracing` (workspace dep `tracing = "0.1"` in `Cargo.toml:34`).

**Initialisation:**
- `agentum::init_tracing()` for daemon / CLI commands — writes to stderr with `EnvFilter` defaulting to `info,sqlx=warn,hyper=warn,h2=warn,tower_http=info` (`crates/agentum/src/lib.rs:7-16`). Override via `AGENTUM_LOG`.
- `agentum::init_tracing_for_tui()` redirects logs to `$XDG_CACHE_HOME/agentum/tui.log` so ratatui's alt-screen isn't scrambled by stray escape sequences from dependencies (`crates/agentum/src/lib.rs:27-52`).

**Macros & fields:**
- `tracing::info!`, `tracing::warn!`, `tracing::error!`, `tracing::debug!` — pick by severity.
- Use structured fields with `=` shorthand: `tracing::info!(addr = %opts.addr, "agentum-server listening (https)")`, `tracing::warn!(error = %e, "auth session sweep failed")`, `tracing::warn!(error = ?e, "watchdog reconcile failed")`.
  - `%v` formats with `Display`; `?v` formats with `Debug`.
- Span via `tracing::info_span!("request", method = %m, uri = %u, ...)` — wraps HTTP requests in `redacting_trace_layer()` (`crates/agentum-server/src/logging.rs:26-34`).
- The HTTP access log scrubs `token=` query values to `token=REDACTED` before logging — see `crates/agentum-server/src/logging.rs:38-57`.

**Don'ts:**
- Never log raw bearer tokens, password hashes, or `?token=…` values. The redacting span layer covers HTTP, but be deliberate in any custom logs.
- TUI binary must not write to stderr — corrupts the alt-screen. Use `init_tracing_for_tui()`.

## Comments

The project's `CLAUDE.md` is explicit: **write WHY, not WHAT.**

### Apply when

A comment must:
- Encode a decision (e.g. "Cow keeps the const path alloc-free" — `crates/agentum-server/src/routes/board.rs:101`).
- State an invariant the code depends on (e.g. ratelimit mutex serialises env-mutating tests — `crates/agentum-server/src/routes/profiles.rs:137-141`).
- Capture a workaround or regression context (e.g. v0.6.21..=v0.6.24 ws-url query-string bug — `crates/agentum/src/commands/terminal/api.rs:952-955`).

### Patterns

**Module-level doc comments (`//!`):**
- Every crate's `lib.rs` opens with a `//!` block describing its role and any non-obvious constraint. Example `crates/agentum-server/src/lib.rs:1-5`:
  ```rust
  //! axum HTTP(S) server for agentum.
  //!
  //! HTTPS via self-signed rustls cert + bearer-token middleware on `/api/*`
  //! (excluding `/api/health` + `/api/cert`). A plain-HTTP cert-server runs
  //! on a side port for trust-on-first-use bootstrap.
  ```
- Route files open with a `//!` line that names the URL prefix: `crates/agentum-server/src/routes/notes.rs:1` (`//! /api/notes — REST CRUD.`), `crates/agentum-server/src/routes/board.rs:1` (`//! /api/board — kanban CRUD + atomic CAS claim + comments + reorder.`).

**Item-level doc comments (`///`):**
- Every `pub` item in libraries gets a doc comment explaining its purpose, not its mechanics. See `agentum_executor::ToolAdapter` (`crates/agentum-executor/src/lib.rs:36-104`) — each trait method has a multi-line `///` block that names the contract, edge cases, and rationale.

**`SAFETY:` for `unsafe`:**
- Each `unsafe` block is preceded by a `// SAFETY:` comment explaining why the invariant holds. See `crates/agentum-server/src/routes/profiles.rs:160-167` (env mutation under a process-wide test lock) and `crates/agentum-executor/src/adapters.rs:416-423` (HOME mutation in a serialised test).

**Bug-fix context:**
- Regression tests include a prose block citing the version where the bug shipped and the failure mode. Examples:
  - `crates/agentum-executor/src/adapters.rs:392-405` (v0.7.45 claude-restart crash).
  - `crates/agentum/src/commands/terminal/api.rs:952-955` (v0.6.21..=v0.6.24 ws-url query-string).
  - `crates/agentum-watchdog/src/lib.rs:653-660` (v0.7.68 sidebar dot stuck green).
- These comments are load-bearing: future readers use them to decide whether a refactor can drop the test.

### Anti-patterns

- Don't paraphrase code: `// increment counter` for `counter += 1` is noise.
- Don't leave dead-code commented-out blocks; delete and let git history serve.

## Function Design

**Size:**
- Route handlers are small (5-20 lines). Heavy lifting lives on `state.store.*` methods or in helper functions. See `crates/agentum-server/src/routes/notes.rs` (60 lines for a full CRUD surface).
- TUI run loop is the exception: `crates/agentum/src/commands/terminal/app.rs` is the single largest file at ~8000 lines. Within it, `apply_event`, `handle_key`, and the like remain narrowly scoped; new behaviour goes through small helpers + `#[cfg(test)]` modules pinning each piece.

**Parameters:**
- Axum extractors are listed in canonical order: `State<AppState>`, then `Path<…>`, then `Query<…>`, then `Json<…>` body. See `crates/agentum-server/src/routes/sessions.rs:114-117`.
- Helpers that need multiple optional fields take a `Patch` struct (e.g. `PatchBody` in `sessions.rs:100-112`, `NotePatch` in core) deserialized via `serde(default)` so missing keys mean "don't touch."

**Returns:**
- Library functions return `Result<T, CrateError>` using the crate-local `Result` alias.
- HTTP handlers return `Result<Json<T>, ApiError>`, `Result<(StatusCode, Json<T>), ApiError>`, or `Result<StatusCode, ApiError>` for `204 No Content` deletes.
- Builders return owned `Self` (no `&mut self` chains for state types — see `LaunchCommand::argv_only` in `crates/agentum-executor/src/lib.rs:28-34`).

## Module Design

**Exports:**
- `pub use` selective re-exports at the crate root keep the surface readable: `pub use error::ApiError`, `pub use transcript_store::TranscriptStore` in `crates/agentum-server/src/lib.rs:34-36`.
- `pub(crate)` is used for test helpers that need to span sibling modules without becoming public API — `tests_helpers_create` / `tests_helpers_patch` in `crates/agentum-server/src/routes/board.rs:377-392`.

**Internal modules:**
- Route registration goes through `routes/mod.rs` which is a flat list of `pub mod <name>;` (`crates/agentum-server/src/routes/mod.rs`). Each route file exposes a `pub fn router() -> Router<AppState>` that the top-level `router()` merges (`crates/agentum-server/src/lib.rs:162-205`).

**No barrel files in TypeScript:**
- Each module is imported directly via its path or alias (`$lib/api`, `$stores/sessions`). No `index.ts` re-export hubs.

## Axum Route Handler Conventions

Every route file in `crates/agentum-server/src/routes/` follows the same shape:

1. **Doc header** naming the URL prefix.
2. **`pub fn router() -> Router<AppState>`** that builds and returns the sub-router. Order matters: literal path segments like `/api/board/reorder` must be registered **before** dynamic `/{id}` segments to avoid the dynamic extractor swallowing them — see `crates/agentum-server/src/routes/board.rs:23-34`.
3. **Local `#[derive(Deserialize)]` query/body structs** named after the handler (`ListQuery`, `PatchBody`, `DeleteQuery`). Optional fields use `#[serde(default)]`.
4. **Handlers** in the canonical order `list → create → get_one → patch → delete → action verbs (start/stop/kill/send/stream)`.
5. **Helpers** at the bottom of the file (`load`, `parse_uuid`, etc.).
6. **`#[cfg(test)] mod tests`** at the very end.

### Patch over current row

Patch handlers fetch the current row, merge the patch (treating `None` as "no change"), and call the store. When validation gates exist, the merged context is fed to a per-status validator (see `enforce_transition` at `crates/agentum-server/src/routes/board.rs:91-103` and the gate logic in `agentum_core::board_schema`).

## sqlx Conventions

**Configuration:**
- WAL mode + `synchronous=NORMAL` + `foreign_keys=true` + `max_connections=8` (`crates/agentum-store/src/lib.rs:61-71`).
- The DB file gets `0600` permissions on open (`restrict_db_perms` referenced at `lib.rs:78`) because it holds password hashes + live bearer tokens.

**Query style:**
- **Runtime queries only** — `sqlx::query`, `sqlx::query_as`. **No compile-time `query!` / `query_as!` macros.** This means no `DATABASE_URL` is required at build time, and CI does not need a live DB. Confirmed: `grep -rn "sqlx::query!" crates/` returns zero hits.
- Multi-line SQL uses `r#"…"#` raw strings.
- Always bind via `.bind(…)`; never string-format values into SQL.
- Errors flow through `StoreError::Sqlx(#[from] sqlx::Error)`; UNIQUE-violation detection uses `if let Err(sqlx::Error::Database(db)) = &res { if db.is_unique_violation() { … } }` (`crates/agentum-store/src/lib.rs:115-119`).
- Rows mapped to internal `Row` structs via `#[derive(FromRow)]` then converted to public domain types with `TryFrom` (e.g. `SessionRow → Session`).

**Migrations:**
- Filename pattern: `NNNN_<topic>.sql` under `crates/agentum-store/migrations/` (0001_initial through 0014_board_column_rules).
- Applied automatically at `Store::open()` via `sqlx::migrate!("./migrations").run(&pool).await?`.
- Every new feature gets a new migration; existing migrations are never edited after a release.
- Migrations include comments explaining the *why* of the schema choice — see `migrations/0014_board_column_rules.sql:1-16` for the rationale on denormalised JSON storage.

## TUI Conventions (`crates/agentum/src/commands/terminal/`)

**Stack:**
- `ratatui` 0.x for drawing (alt-screen via `crossterm`).
- `crossterm` for raw mode, mouse capture, bracketed paste, key events.
- `tokio` runtime; the event loop multiplexes:
  - `EventStream` from crossterm
  - WS frames from `super::api::Client`
  - host events bus
  - PTY messages from `super::pty::LocalPty`
  - timers via `tokio::time::interval`

**Module layout (`crates/agentum/src/commands/terminal/mod.rs:15-27`):**
```
api         // HTTP + WS client
app         // state + event loop + key dispatch (big file)
extensions  // lazygit side-pane integration
iometer     // per-session byte rate meter
palette     // command palette
prefs       // prefs.toml load/save
profiles    // named connection profiles
pty         // local PTY for the side pane
sound       // notification chimes
term        // single-pane terminal state
theme       // theme registry
trust       // TOFU known-hosts pinning
ui          // pure render helpers
```

**Event loop pattern (`crates/agentum/src/commands/terminal/app.rs:1-32`):**
- `App` struct owns all state; events are dispatched via async `select!`.
- Periodic refresh: 5s for session list polling (`REFRESH_INTERVAL`), 100ms for tick/animations (`TICK_INTERVAL`).
- All side effects happen inside `apply_event` / `handle_key`; rendering is a pure projection in `ui::draw_*`.
- A soft-restart of the loop is modelled via `RunOutcome::SwitchProfile(name)` bubbled up to `commands::terminal::run` (see `mod.rs:84-105`).

**Render helpers:**
- Live in `terminal/ui.rs`. Pure functions taking `&App` + a `Frame<'_>`. No I/O, no mutation. Aliased with module-level doc.

**Logging in the TUI:**
- Never `eprintln!` — use `tracing::info!` and rely on `init_tracing_for_tui()` writing to the cache log file.

## Dashboard (SvelteKit) Conventions

**Framework:**
- SvelteKit 2.x with Svelte 5 runes (`$state`, `$derived`, `$props`, `$effect`). All routes are SPA: `+layout.ts:4-6` sets `ssr = false; prerender = false`.
- Static adapter (`@sveltejs/adapter-static`) outputs to `dashboard/build/`, which `rust-embed` bakes into the daemon binary at compile time.

**Stores:**
- Svelte writable stores in `dashboard/src/lib/stores/` carry typed state, e.g. `sessions.ts` exports `sessions: Writable<{loading, error, items}>` plus an async `loadSessions()` action.
- Stores own all fetch-then-merge logic; components read via `$store` and call action functions.
- The active profile id is its own writable (`activeProfileId` in `dashboard/src/lib/profiles.ts`); switching reloads the page rather than re-initialising each store individually.

**Components (`dashboard/src/lib/components/`):**
- Each component opens with a Svelte 5 `<script lang="ts">` and a JSDoc-style block comment naming its purpose (see `RemoteAccessInfo.svelte:1-16`).
- Props are typed via a local `interface Props { … }` and destructured: `let { compact = false }: Props = $props();` (`RemoteAccessInfo.svelte:21-25`).
- State uses runes: `let fp = $state<CertFingerprint | null>(null);` and `let url = $derived(...)`.
- Component-scoped CSS via `<style>` blocks; CSS custom properties (`--surface`, `--border`, `--accent`, `--radius`) are themed centrally.

**API client (`dashboard/src/lib/api.ts`):**
- Single `request<T>(path, init)` and `requestOn<T>(profileId, path, init)` functions wrap fetch.
- Bearer token is read from `getActiveProfile().token` on every call.
- 401 clears the active token (`setToken(null)`) and throws `ApiError(401)`; 5xx marks fetch failed; other 4xx throws but doesn't degrade connection state.
- All URL construction goes through `apiUrl(path)` (HTTP) and `wsUrl(path)` (WS) from `dashboard/src/lib/profiles.ts` so the active profile's `baseUrl` is honoured.

**Connection profiles (`dashboard/src/lib/profiles.ts`):**
- Source of truth for the profile list is the page-origin daemon at `/api/profiles` (mirrors the TUI's `profiles.toml`).
- Tokens, labels, and the active profile id are browser-local. Storage keys: `agentum_profile_tokens`, `agentum_profile_labels`, `agentum_active`, `agentum_profile_cache`. Legacy `agentum_token` is migrated once on first load (`profiles.ts:65-68`).

---

*Convention analysis: 2026-05-20*
