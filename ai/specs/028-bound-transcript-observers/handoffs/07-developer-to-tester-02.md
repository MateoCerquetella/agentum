# Handoff — Developer to Tester (evidence retry)

- **Spec:** 028-bound-transcript-observers
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- Production stop/kill/delete route lifecycle regression with observer create/drop/cache/no-start
  assertions.
- Server-built watchdog reconciliation regression covering running, stopped, crashed, deleted, and
  tool-changed sessions through the actual retirement callback.
- Controllable injected notify callbacks and consumer lifecycle accounting, with capacity-one burst
  coalescing, queued refresh, prompt stop/forget completion, and stale-callback silence coverage.
- Isolated real `RecommendedWatcher` append → `agent_tasks.updated` event-bus update → retirement
  silence coverage, plus truthful harness/spec wording and a blocking-receiver source guard.

## Acceptance-criteria evidence

- **AC 1–5, 7:** Prior green evidence remains unchanged.
- **AC 6:** Actual session lifecycle routes and server-wired watchdog reconciliation now execute
  with deterministic observer/cache accounting and prove retirement never starts observation.
- **AC 8:** The injected callback/consumer test proves capacity-one coalescing, queued-wake
  consumption, consumer completion after both stop and forget, and no stale update/cache recreation.
  The real watcher regression proves the production notify path emits and becomes silent on retire.

## Verification

- `cargo test -p agentum-server transcript_store::tests --lib` — PASS (9 tests).
- `cargo test -p agentum-server routes::agent_tasks::tests --lib` — PASS (2 tests).
- `cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib` — PASS
  (3 tests).
- `cargo test -p agentum-server tests::server_wired_watchdog_callback_retires_only_non_running_claude_observers --lib` — PASS (1 test).
- `cargo test -p agentum-watchdog reconcile_passes_authoritative_running_slice_to_optional_hook_once --lib` — PASS (1 test).
- `HARNESS_FEATURE_ID=mode-aware-transcript-read bash .harness/qa.sh` — PASS (15 isolated tests,
  including the production `RecommendedWatcher` runtime leg).
- `cargo check -p agentum-server -p agentum-watchdog` — PASS.
- `cargo test --workspace --lib --exclude agentum-desktop` — PASS (833 passed, 2 ignored).
- `cargo fmt --all -- --check`, blocking-receiver source guard, and `git diff --check` — PASS.
- Full workspace and UI builds retain the documented environment blockers: missing release Sherpa
  dylib and uninstalled Vite dependencies.

## Decisions and invariants

- Tests use the real route/server wiring while keeping external tmux and five-second timing out of
  the deterministic gate.
- Test-only accounting does not change production observer semantics; production still uses one
  capacity-one Tokio channel and abort-on-drop ownership.
- The isolated backend QA gate claims a real filesystem watcher and event-bus update, not portable
  OS thread counts or WebSocket transport; `verify.sh` guards the removed blocking receiver source.

## Remaining risks / next action

- Fresh Tester should rerun and independently inspect AC 6/8 evidence before Reviewer starts.
- The original `.claude/worktrees/question-orq` directory was externally deleted twice. The branch
  is safely checked out at `/Users/mateocerquetella/Developer/projects/agentum-question-orq-recovery`;
  continue there to avoid repeated uncommitted-data loss.
