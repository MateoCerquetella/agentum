# Handoff — Developer to Tester

- **Spec:** 028-bound-transcript-observers
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- Passive transcript cache entries with optional bounded live observers, atomic mode-aware reads,
  session-aware reset, and explicit observer/cache lifecycle operations.
- Side-effect-free session listing, status-selected transcript reads, immediate route retirement,
  and watchdog-driven reconcile retirement.
- Developer send-back corrections: snapshot-only reads drop existing observers, non-Claude reads
  forget prior Claude state, and regressions exercise both transitions without manual masking.
- Spec 028 harness feature routes and isolated QA fixtures.

## Acceptance-criteria evidence

- **AC 1:** `listing_500_sessions_creates_zero_transcript_entries_or_observers` returns the full
  fleet with zero cached entries and observer creations.
- **AC 2–4:** transcript-store and agent-task route suites prove exactly-once live attachment,
  synchronous snapshot refresh, live-to-snapshot drop, and live-Claude-to-non-Claude cleanup.
- **AC 5:** `reset_before_first_read_never_resurrects_pre_reset_tasks` pins reset-first cursor
  semantics.
- **AC 6:** session lifecycle tests, explicit lifecycle methods, and the watchdog running-session
  hook cover immediate and reconcile retirement.
- **AC 7:** pinned transcript promotion, legacy fallback, complete-line parsing, and existing HTTP
  response types remain covered by focused and workspace suites.
- **AC 8:** observer ownership aborts its Tokio consumer on drop and uses a capacity-one coalescing
  channel; no permanent `spawn_blocking` receiver remains.

## Verification

- `cargo test -p agentum-server transcript_store::tests --lib -- --nocapture` — PASS (7 tests).
- `cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture` — PASS (2 tests).
- `cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture` — PASS (2 tests).
- `cargo test -p agentum-watchdog reconcile_passes_authoritative_running_slice_to_optional_hook_once --lib -- --nocapture` — PASS (1 test).
- `HARNESS_FEATURE_ID=mode-aware-transcript-read bash .harness/qa.sh` — PASS (isolated fleet and
  live-to-stopped route legs).
- `cargo test --workspace --lib --exclude agentum-desktop` — PASS.
- `cargo fmt --all -- --check` — PASS.
- `git diff --check` — PASS.
- `cargo test --workspace --lib` — ENVIRONMENT BLOCKED: missing
  `target/release/libsherpa-onnx-c-api.dylib` required by `agentum-desktop`.
- `npm run build --prefix crates/agentum-desktop/ui` — ENVIRONMENT BLOCKED: dependencies are not
  installed (`vite: command not found`).

## Decisions and invariants

- `ObservationMode` defines the observer state after a read; `SnapshotOnly` therefore detaches a
  prior live observer.
- Non-Claude reads own no transcript cache state, including after a Claude tool transition.
- The watchdog hook remains a retirement-only backstop and never starts observation.

## Remaining risks / next action

- Tester should independently trace all eight acceptance criteria and rerun the focused/harness
  gates. Treat the two documented build failures as environment prerequisites, not source
  regressions, unless fresh evidence changes their signatures.
