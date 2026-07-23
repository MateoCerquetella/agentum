# Spec 028 — Tester Verification

- **Spec:** `028-bound-transcript-observers`
- **Date:** 2026-07-23
- **Base under test:** `ff53faac` (`9baca2a1..ff53faac` evidence-retry diff)
- **Role:** Fresh Tester, iteration 2 of 2
- **Verdict:** **PASS — AC 1–8 have executable evidence and the required backend gates are green.**

The iteration-1 send-back is closed. The added tests exercise the production
stop/kill/delete route functions and the server's real watchdog-construction
seam, while the observer seam now drives callbacks and accounts for bounded
delivery and consumer completion. The isolated QA route also executes a real
`notify::RecommendedWatcher` append/update/retirement cycle and reports only
the behavior it measures.

## Independent gates

| Gate | Result |
|---|---|
| `cargo test -p agentum-server transcript_store::tests --lib -- --nocapture` | **PASS — 9/9** |
| `cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture` | **PASS — 2/2** |
| `cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture` | **PASS — 3/3** |
| `cargo test -p agentum-server tests::server_wired_watchdog_callback_retires_only_non_running_claude_observers --lib -- --nocapture` | **PASS — 1/1** |
| `cargo test -p agentum-watchdog reconcile_passes_authoritative_running_slice_to_optional_hook_once --lib -- --nocapture` | **PASS — 1/1** |
| `HARNESS_FEATURE_ID=mode-aware-transcript-read bash .harness/qa.sh` | **PASS — 15/15 isolated tests**, including the real production watcher leg |
| blocking-receiver source guard (`rg 'spawn_blocking\|std::sync::mpsc' ...`) | **PASS — no match** |
| `cargo check -p agentum-server -p agentum-watchdog` | **PASS** |
| `cargo test --workspace --lib --exclude agentum-desktop` | **PASS — 833 passed, 0 failed, 2 ignored** |
| `cargo fmt --all -- --check` | **PASS** |
| `git diff --check` before this report | **PASS** |

The first raw Cargo invocation found `cargo` absent from `PATH`; all reruns
prepended the already-installed `/Users/mateocerquetella/.cargo/bin`. Nothing
was installed. Per the task constraint, desktop/UI dependencies were not
installed and UI gates were not run. The backend-equivalent legs of
`.harness/verify.sh` were run directly because that script also requires the
known unavailable desktop Sherpa dylib and UI toolchain.

## Acceptance-criterion map

| AC | Verdict | Independent evidence |
|---|---|---|
| 1 | **PASS** | The production `list` handler contains no transcript call. Its 500-row fixture returns all rows, including 250 Claude rows, while cache count, observer create/drop counts, and transcript-directory existence remain zero. |
| 2 | **PASS** | A 16-way barrier test proves repeated/concurrent running-Claude reads attach exactly one observer. Route mode selection is executable, and the exact `agent_tasks.updated` kind plus `{ "session_id": ... }` payload remain asserted. |
| 3 | **PASS** | Snapshot-only reads synchronously consume newly appended complete lines, retain a partial line until newline, and attach no observer. The route selects snapshot mode for every non-running status; a live-to-stopped read also retires the prior observer. |
| 4 | **PASS** | Non-Claude read/reset returns empty with no entry, directory, observer, or consumer. Transitioning a previously observed Claude session to a non-Claude read forgets its cached state and drops its observer. |
| 5 | **PASS** | Reset as the first interaction creates only passive metadata, moves the selected cursor to EOF, clears state, and exposes only a later post-reset todo. No observer is created. |
| 6 | **PASS** | The route regression invokes production stop, kill, and delete functions against a counting store: stop/kill drop observation and retain cache; delete drops observation and cache; none attaches. The server-built watchdog runs one authoritative production reconcile and keeps only the running Claude observer while retiring stopped, crashed, deleted, and running-but-tool-changed sessions; passive caches remain and create count does not increase. The generic watchdog hook test separately proves one callback per successful authoritative query. |
| 7 | **PASS** | Focused tests preserve UUID-pinned promotion, legacy fallback, and complete-line parsing. Agent-task route serialization and the 833-test backend workspace suite remain green; the implementation diff introduces no HTTP schema change. |
| 8 | **PASS** | Production source uses `tokio::sync::mpsc::channel(1)`, `try_send`, and observer-owned `JoinHandle::abort`, with no blocking receiver path. The controllable callback regression proves a three-notify burst records one queued wake and two coalesced sends, consumes one update with no duplicate, observes consumer completion within one second after both stop and forget, then records closed sends with no stale bus event, state mutation, or forgotten-cache recreation. The real watcher test independently proves append-to-event delivery and post-retirement silence. |

## Iteration-1 blocker closure

### B1 — AC 6 production lifecycle and reconcile wiring: CLOSED

The new route test reaches `stop`, `kill`, and `delete`, not only lower-level
store helpers. The new server test constructs the watchdog through
`watchdog_with_transcript_retirement`—the same helper used by
`spawn_background_workers`—and calls the production `reconcile_once` path.
Create/drop/cache assertions cover all required retirement classes and the
no-start invariant.

### B2 — AC 8 bounded delivery and consumer termination: CLOSED

The counting factory retains its actual notify callbacks and exposes queue,
coalesced, closed, consumer-started, and consumer-finished counters. The test
fills the capacity-one channel before yielding, observes one refresh, aborts
both stop and forget consumers, and invokes stale callbacks afterward. This is
executable evidence for the asynchronous behaviors that were source-only in
iteration 1.

### B3 — QA truthfulness: CLOSED

The isolated QA branch runs the nine transcript-store tests, including
`real_notify_observer_emits_for_append_then_retirement_stays_silent`, plus the
three route, two agent-task, and one server-wiring tests. Its output explicitly
disclaims portable OS-thread counts and WebSocket transport. The printed claims
match the tests that ran.

## Negative, race, and error audit

- Concurrent first live reads remain serialized and exactly-once.
- Snapshot/live and Claude/non-Claude transitions retire prior live work.
- Partial JSONL, reset-first, pinned promotion, fallback, and empty non-Claude
  behavior remain covered.
- Stop/kill retire before host lookup or tmux teardown, so teardown errors cannot
  leak the observer; delete forgets state before best-effort teardown.
- A callback queued before retirement is either consumed before the retirement
  lock completes or its consumer is aborted. After retirement returns, retained
  test callbacks see a closed channel and cannot emit, mutate, or recreate state.
- Observer construction failure remains best-effort and retryable by inspection:
  the passive slot remains with `observer: None`, so a later live read retries.
- Watchdog query failure does not invoke retirement because the hook follows the
  successful `list_sessions(Running)` query.

No implementation file, `ai/STATE.md`, handoff, commit, dependency, or external
state was changed by Tester. This report is the only intended worktree change.
