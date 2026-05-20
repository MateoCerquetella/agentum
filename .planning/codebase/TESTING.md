# Testing Patterns

**Analysis Date:** 2026-05-20

agentum tests three surfaces — Rust workspace (177 `#[test]` / `#[tokio::test]` cases across the six crates), a single TypeScript data-model parity suite under the dashboard, and a Svelte type-check pass. There is no end-to-end test harness; the TUI's interactive behaviour and the dashboard's UI flows are exercised by hand. CI runs everything tagged for release on `ubuntu-latest` and `macos-latest`.

## Test Framework

### Rust

**Runner:**
- Built-in `cargo test` driving `#[test]` (sync) and `#[tokio::test]` (async).
- No external test framework — no `rstest`, no `cucumber`. The pattern is "inline `#[cfg(test)] mod tests` per source file."
- Config: none — relies on the default `[lib]` / `[[bin]]` test discovery in each crate's `Cargo.toml`.

**Async runtime:**
- `#[tokio::test]` for async tests. Single-threaded by default; multi-thread only when explicitly annotated. No tests in this codebase override that.

**Run commands:**
```bash
cargo test --all                         # full workspace, integration + unit
cargo test --workspace --lib             # unit tests only (skips bin/integration)
cargo test -p agentum-server             # one crate
cargo test -p agentum-executor adapters  # filter by module
cargo test --all --no-run                # build tests without running
```

The `justfile` provides `just test` (= `cargo test --all`).

### TypeScript

**Runner:**
- `vitest` 4.x — configured in `dashboard/vite.config.ts:22-26`. **Pure data-model tests only**, intentionally **no DOM** (no `jsdom` / `happy-dom`).
- Type-checking through `svelte-check` + `tsc` is the primary signal of dashboard health; runtime tests cover the data shapes that need cross-language parity.

**Discovery:**
- `include: ['src/**/*.{test,spec}.ts']` — co-located with the module under test.
- Only one test file exists today: `dashboard/src/lib/board-schema.test.ts`.

**Run commands:**
```bash
pnpm --dir dashboard test           # vitest run (one-shot, used by CI)
pnpm --dir dashboard test:watch     # vitest watch mode
pnpm --dir dashboard check          # svelte-kit sync + svelte-check + tsc
pnpm --dir dashboard build          # production build (also exercises tsc)
```

The CI workflow runs `pnpm --dir dashboard build` (validates types via `svelte-check`) before `cargo test --all`. `vitest` itself is not invoked in CI yet — running it locally is the developer responsibility for board-schema parity edits.

## Test File Organization

**Location (Rust):**
- Inline `#[cfg(test)] mod tests` (or a topic-specific name) at the bottom of every source file containing testable logic.
- No `tests/` integration-test directory anywhere in the workspace. Cross-crate coverage is achieved by putting the test where the *handler* lives and pulling in the store via `Store::open` against a tempdir.

**Location (TypeScript):**
- `*.test.ts` files alongside the module: `dashboard/src/lib/board-schema.test.ts` lives next to `dashboard/src/lib/board-schema.ts`.

**Naming:**
- Rust test functions: snake_case, descriptive. Examples from `crates/agentum-executor/src/adapters.rs`:
  - `claude_argv`
  - `claude_restart_uses_resume_when_transcript_exists`
  - `codex_argv_translates_yolo_marker`
  - `passthrough_routes_unknown_tool`
- Multiple test modules per file when the surface is large — the file picks a topic-specific module name instead of stuffing everything into `tests`:
  - `crates/agentum/src/commands/terminal/app.rs` has `profile_targets_loopback_tests`, `merge_dedup_tests`, `selection_tests`, `paste_tests`, and more.

**Structure (representative crate count):**
```
crates/agentum-core/src/
  board_schema.rs        # #[cfg(test)] mod tests — required_fields_for + validate_transition
  lib.rs                 # status/name validators
  profiles.rs            # profiles.toml load/save round-trip
  transcript.rs          # transcript path resolution
crates/agentum-store/src/
  lib.rs                 # tmp_store() helper + ~30 #[tokio::test] cases
crates/agentum-executor/src/
  adapters.rs            # fixture() helper + per-adapter argv tests
crates/agentum-server/src/
  auth.rs                # token format + argon2 hash round-trip
  logging.rs             # token redaction
  ratelimit.rs           # bucket + window behaviour
  routes/board.rs        # transition gate via in-process create/patch helpers
  routes/board_rules.rs  # column-rules CRUD + gate override interactions
  routes/profiles.rs     # XDG-isolated CRUD
  routes/sessions.rs     # parse_resize envelope
crates/agentum-watchdog/src/
  lib.rs                 # context-low regex, classify_activity matrix
crates/agentum-tmux/src/
  lib.rs                 # target_format + tmux lifecycle smoke (skipped without tmux)
crates/agentum/src/
  cli.rs                 # clap parser + arg_to_flag
  commands/terminal/api.rs              # ws_url builder
  commands/terminal/app.rs              # 7+ topic-specific test modules
  commands/terminal/iometer.rs          # rate window math
  commands/terminal/prefs.rs            # default + ttl clamp
  commands/terminal/trust.rs            # fingerprint pin/mismatch
```

## Test Structure

### Rust suite layout

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(/* … */) -> SomeType { /* build a known-good instance */ }

    #[test]
    fn descriptive_what_is_under_test() {
        let s = fixture(/* … */);
        let actual = under_test(&s);
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn async_happy_path() {
        let s = tmp_store().await;
        let out = s.do_thing().await.unwrap();
        assert_eq!(out.field, expected);
    }
}
```

See `crates/agentum-executor/src/adapters.rs:339-460` for the canonical adapter pattern: a `fixture(tool, model, flags)` builder that returns a fully populated `Session`, followed by one test per branch.

### Patterns

**Setup:**
- Each crate that needs a DB defines a local `async fn tmp_store() -> Store` helper that calls `tempfile::tempdir()`, leaks the handle via `std::mem::forget` to keep it alive for the test's lifetime, and opens a fresh `Store`. See `crates/agentum-store/src/lib.rs:1461-1468`.
- Route-level tests in `agentum-server` mirror this with a `fresh_state()` helper that builds a full `AppState` (store + bus + ratelimiter + transcript store) over a tempdir-rooted SQLite. See `crates/agentum-server/src/routes/board.rs:408-429` and `crates/agentum-server/src/routes/board_rules.rs:121-142`.
- Tests that need a controlled `$HOME` or `$XDG_CONFIG_HOME` use a module-scoped `static TEST_LOCK: Mutex<()> = Mutex::new(());` plus `unsafe { std::env::set_var(…) }` — see `crates/agentum-server/src/routes/profiles.rs:147-172`. Lock prevents parallel-test env-var races.

**Teardown:**
- `tempdir()` handles are deliberately leaked (`std::mem::forget`) for the test's lifetime — Rust runs each test in the same process, so an early drop would yank the SQLite file out from under an open pool. The OS cleans them up on process exit.
- HOME mutations restore the original value in a finally-style block before the assert (`crates/agentum-executor/src/adapters.rs:439-444`).

**Assertions:**
- `assert_eq!`, `assert!`, `assert!(matches!(…, Pattern))`, `assert!(err.is_some())`, `#[should_panic(expected = "…")]` for panic tests.
- Failure messages include the offending value: `assert!(argv.iter().any(|s| s == "--resume"), "expected --resume in argv when transcript exists: {argv:?}")` — `crates/agentum-executor/src/adapters.rs:447-450`.

### TypeScript suite layout

`dashboard/src/lib/board-schema.test.ts` uses vitest's `describe` / `it` / `expect`:

```ts
import { describe, expect, it } from 'vitest';
import { requiredFieldsFor, validateTransition } from './board-schema';

describe('board-schema parity', () => {
  it('todo requires title + lbl', () => {
    expect(requiredFieldsFor('todo')).toEqual(['title', 'lbl']);
  });
  // …
});
```

The file opens with a **parity contract comment** pinning the Rust counterpart it must match (`crates/agentum-core/src/board_schema.rs::required_fields_for`). When the contract changes, both sides must move together — that comment is load-bearing for cross-language drift detection.

## Mocking

**Framework:** None. Tests use **real, in-process collaborators with throwaway state**, not test doubles.

**Patterns observed:**

- **SQLite:** real `Store::open` against a tempdir, never a mock. `tmp_store()` is the universal recipe.
- **HTTP handler unit tests:** call the handler function directly with `State(state)`, `Path(...)`, `Query(...)`, `Json(...)` wrappers built by hand. Example `crates/agentum-server/src/routes/board.rs:456-480`:
  ```rust
  let state = fresh_state().await;
  let (code, json) = create(
      State(state.clone()),
      Json(doing_pass_payload()),
  )
  .await
  .unwrap();
  assert_eq!(code, StatusCode::CREATED);
  ```
- **Error response inspection:** call `ApiError::into_response()` and drain the body via `axum::body::to_bytes` to assert on status + JSON shape:
  ```rust
  async fn err_status_and_body(err: ApiError) -> (StatusCode, serde_json::Value) {
      let resp = err.into_response();
      let status = resp.status();
      let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
      (status, serde_json::from_slice(&bytes).unwrap())
  }
  ```
  See `crates/agentum-server/src/routes/board.rs:434-438` and `crates/agentum-server/src/routes/board_rules.rs:144-149`.
- **Broadcast bus:** real `tokio::sync::broadcast::channel(16)`, no fake. Receivers are bound to `_rx` to keep the channel alive without consuming.
- **tmux:** the `crates/agentum-tmux/src/lib.rs:351-370` lifecycle smoke test calls *real* `tmux` and skips the test if the binary is missing (`if Command::new("tmux").arg("-V").status().await.is_err() { return; }`). Don't stub external binaries; degrade gracefully.

**What to mock:**
- Nothing, in this codebase. If you find yourself reaching for a mock, the convention says use the real thing under a tempdir or a feature flag.

**What NOT to mock:**
- The store. Spin up a fresh SQLite.
- The broadcast bus. Use a small-capacity real channel.
- The TLS / network layer. The TUI / dashboard parts are exercised manually.
- The filesystem. Use `tempfile::tempdir()` + `std::mem::forget` to extend its lifetime.

## Fixtures and Factories

**Rust factories:**
- Per-file `fn fixture(...)` helpers build domain types with safe defaults. `crates/agentum-executor/src/adapters.rs:347-369` is the model:
  ```rust
  fn fixture(tool: &str, model: Option<&str>, flags: &[&str]) -> Session {
      let now = OffsetDateTime::now_utc();
      Session {
          id: Uuid::new_v4(),
          name: "alpha".into(),
          workdir: "/tmp/work".into(),
          tool: tool.into(),
          model: model.map(String::from),
          flags: flags.iter().map(|s| s.to_string()).collect(),
          status: Status::Idle,
          // … remaining fields default to None / false
      }
  }
  ```
- Where multiple tests need the same payload, the file declares a top-level `fn doing_pass_payload() -> NewBoardItem` (`crates/agentum-server/src/routes/board.rs:441-453`) and tests mutate it as needed.

**TypeScript fixtures:**
- None yet. The single parity test inlines its inputs.

**Location:**
- All test fixtures live inside the same `#[cfg(test)] mod tests` block as the test functions — never in a shared `tests/common/` directory (there isn't one).

## Coverage

**Requirements:** None enforced. CI does not measure coverage and does not block on a threshold.

**Tooling:** Not configured. To measure locally:
```bash
cargo install cargo-llvm-cov   # one-time
cargo llvm-cov --workspace --html
```
No `.cargo/config.toml` coverage profile is checked in.

**Observed scope:**
- `agentum-core` validators (board schema, status parsing, profile load/save, transcript path) — high coverage.
- `agentum-store` — every `Store::*` method has at least one happy-path test.
- `agentum-executor` adapters — argv shape is covered for every first-class tool plus YOLO translation per tool.
- `agentum-server` routes — gate logic on `board` + `board_rules` has full AC coverage; sessions has minimal coverage (just the resize parser).
- `agentum-watchdog` — pure functions (regex, `classify_activity`) tested; the per-session task loop is not.
- TUI (`crates/agentum/src/commands/terminal/app.rs`) — small pure helpers (`merge_sessions_dedup`, `profile_targets_loopback`, `extract_selection_from_screen`, paste classifier) are well-covered; the event loop itself is not.

## Test Types

**Unit Tests:**
- The default. Scope: a single function or method, with collaborators replaced by real-but-throwaway instances (tempdir SQLite, in-memory broadcast).

**Integration tests:**
- "Integration" here means handler-level: call an axum route function directly with real `AppState` and a fresh SQLite. The store, the bus, and the handler all run for real; only HTTP transport is skipped.
- No `tests/` directory exists in any crate — there is no separate integration-test binary.

**Smoke Tests:**
- `crates/agentum-tmux/src/lib.rs:351-370` `lifecycle_smoke` is a true integration test that drives a real `tmux` server. It self-skips when `tmux` is not on `PATH` so it stays green in environments where the binary is absent.

**E2E Tests:**
- Not used. The dashboard and TUI are exercised manually, with regression coverage flowing back into unit tests when bugs are caught (e.g. v0.6.21-v0.6.24 ws-url regression added to `crates/agentum/src/commands/terminal/api.rs:927-985`).

**Cross-language parity tests:**
- `dashboard/src/lib/board-schema.test.ts` mirrors `crates/agentum-core/src/board_schema.rs::required_fields_for` and `validate_transition`. The file's opening comment names the parity contract and warns that both sides must move together.

## Common Patterns

### Async testing

```rust
#[tokio::test]
async fn descriptive_name() {
    let s = tmp_store().await;
    let item = s.create_board_item(NewBoardItem { /* … */ }).await.unwrap();
    assert!(item.key.starts_with("AG-"));
}
```

See `crates/agentum-store/src/lib.rs:1506-1540` for the full kanban CAS-claim test as an example of async happy-path + negative-path coverage in one function.

### Error / panic testing

- Happy-path uses `.unwrap()` — failure is the failure mode.
- Error variant matching uses `assert!(matches!(err, StoreError::AlreadyExists(_)))` — `crates/agentum-store/src/lib.rs:1503`.
- Panic-expecting tests use `#[should_panic(expected = "must not contain a query string")]` — `crates/agentum/src/commands/terminal/api.rs:973`.

### Regression tests

Every bug fix gains a test that:
1. Names the failure (e.g. "Repro for the v0.7.45 crash where every restart of a Claude session died with `Error: Session ID <X> is already in use`").
2. Cites the version where the bug shipped.
3. Pins the desired behaviour with `assert!` calls that include the actual value in the failure message.

Examples:
- `crates/agentum-executor/src/adapters.rs:392-456` — v0.7.45 Claude restart crash.
- `crates/agentum/src/commands/terminal/api.rs:950-984` — v0.6.21-v0.6.24 ws-url query-string bug, with a `#[should_panic]` companion that catches the original mistake in debug builds.
- `crates/agentum-watchdog/src/lib.rs:653-697` — v0.7.68 sidebar-dot-stuck-green fix (codex/cursor/gemini activity classification).
- `crates/agentum/src/commands/terminal/app.rs:7691-7732` — sidebar loopback-classifier safety net.

### Skip-when-missing pattern

Tests that need an external binary self-skip rather than fail when the binary is unavailable. The tmux smoke test (`crates/agentum-tmux/src/lib.rs:353-355`) is the canonical example.

### Env-mutating tests

Two cases require process-wide env mutation:
1. `crates/agentum-server/src/routes/profiles.rs:130-380` (XDG_CONFIG_HOME).
2. `crates/agentum-executor/src/adapters.rs:392-456` (HOME).

Both serialise with a module-local `static TEST_LOCK: Mutex<()> = Mutex::new(());` and document the unsafe block with a `// SAFETY:` comment naming the lock. Lock acquisition uses `lock().unwrap_or_else(|e| e.into_inner())` so a poisoned lock from an earlier panic doesn't take the rest of the suite down.

## CI

**File:** `.github/workflows/ci.yml`.

**Trigger:** tag pushes matching `v*.*.*`, plus manual `workflow_dispatch`. Untagged commits to `main` do NOT run CI — the project keeps the Actions tab quiet by gating on release tags.

**Matrix:** `ubuntu-latest`, `macos-latest`. `fail-fast: false` so one OS failure doesn't mask the other.

**Steps:**
1. Checkout, install stable Rust with rustfmt + clippy.
2. `Swatinem/rust-cache@v2` for incremental builds.
3. Install pnpm 9, Node 22, cache via `dashboard/pnpm-lock.yaml`.
4. `pnpm --dir dashboard install --frozen-lockfile && pnpm --dir dashboard build` — this is critical because `rust-embed` bakes `dashboard/build/` into the daemon binary. Skipping the build would leave embedded asset references dangling and break the `cargo build`/`cargo test` step that follows.
5. `cargo fmt --all -- --check`.
6. `cargo clippy --all-targets --all-features -- -D warnings`.
7. `cargo test --all`.

**Local convenience:** `justfile` exposes `just check` (fmt + clippy) and `just test`. There is no equivalent for the dashboard's `pnpm check` — run it directly when touching `dashboard/`.

## Pre-commit hooks

None. No `.husky/` directory, no `lefthook.yml`, no `pre-commit` config. The contract is "run `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`, and `pnpm --dir dashboard check` locally before pushing." The `agentum` project's auto-memory captures this as a hard rule (CI must be green before push).

---

*Testing analysis: 2026-05-20*
