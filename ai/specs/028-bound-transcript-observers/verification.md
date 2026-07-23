# Spec 028 — Tester Verification

- **Spec:** `028-bound-transcript-observers`
- **Date:** 2026-07-23
- **Base under test:** `3fdf597b` (`4f3c030c..3fdf597b` implementation diff)
- **Role:** Fresh Tester
- **Verdict:** **SEND-BACK — executable evidence does not close AC 6 and AC 8.**

The implementation is internally consistent on inspection and every command that
can run without desktop prerequisites is green. No reproducible implementation
failure was found. The send-back is for missing required verification: the
committed tests and `qa.sh` do not execute several lifecycle and asynchronous
resource behaviors that the spec, architecture, task evidence, and harness
descriptions claim are pinned.

## Independent gates

| Gate | Result |
|---|---|
| `cargo test -p agentum-server transcript_store::tests --lib -- --nocapture` | **PASS — 7/7** |
| `cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture` | **PASS — 2/2** |
| `cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture` | **PASS — 2/2** |
| `cargo test -p agentum-watchdog reconcile_passes_authoritative_running_slice_to_optional_hook_once --lib -- --nocapture` | **PASS — 1/1** |
| `HARNESS_FEATURE_ID=mode-aware-transcript-read bash .harness/qa.sh` | **PASS as implemented — 2/2 unit tests in isolated `HOME` / `AGENTUM_HOME` / `TMUX_TMPDIR`** |
| `cargo test --workspace --lib --exclude agentum-desktop` | **PASS — 829 passed, 0 failed, 2 ignored** |
| `cargo fmt --all -- --check` | **PASS** |
| `git diff --check` before this report | **PASS** |
| `cargo test --workspace --lib` | **ENVIRONMENT BLOCKED — desktop build script reports `Library not found: ../../target/release/libsherpa-onnx-c-api.dylib`** |
| `npm run build --prefix crates/agentum-desktop/ui` | **ENVIRONMENT BLOCKED — `vite: command not found`; dependencies were not installed** |

The first raw Cargo invocation found `cargo` absent from `PATH`; reruns used the
existing `/Users/mateocerquetella/.cargo/bin` and required no installation or
repository change.

## Acceptance-criterion map

| AC | Verdict | Independent evidence |
|---|---|---|
| 1 | **PASS** | The 500-row direct route fixture returns all rows, including 250 Claude rows, with cache count, observer creations/drops, and transcript-directory existence all unchanged at zero. `sessions::list` contains no transcript-store call. |
| 2 | **PASS** | The 16-way barrier test proves one observer under concurrent/repeated live reads. It also asserts the exact `agent_tasks.updated` kind and `{ "session_id": ... }` payload. The running route test selects `Live`. |
| 3 | **PASS** | Snapshot reads consume a later complete line synchronously, retain a partial trailing line until newline, and create no observer. A live-to-stopped route read drops the prior observer. Idle and crashed statuses share the same non-running mode branch by inspection. |
| 4 | **PASS** | Store and route tests prove non-Claude read/reset returns empty and leaves cache and observer creation at zero; the live-Claude-to-non-Claude regression also proves prior state is forgotten and its observer dropped. |
| 5 | **PASS** | Reset-first test creates passive metadata, advances to EOF, returns empty, and later exposes only the post-reset todo with zero observer creation. |
| 6 | **SEND-BACK** | Lower-level `stop_observing`, `forget`, and `retain_observers` counters pass, and source inspection finds calls in stop/kill, delete, tool patch, and the server watchdog hook. However the promised executable lifecycle matrix is absent: the session route module has only list and tool-change tests; it never invokes stop, kill, or delete with the counting store. The watchdog test only records the running slice and never proves the server hook retires a crashed/stopped/deleted/tool-changed observer without creating one. |
| 7 | **PASS** | Focused store tests pin UUID promotion, legacy fallback, and complete-line parsing; the full non-desktop workspace suite keeps core transcript parsing and server response serialization green. No HTTP schema type changed. |
| 8 | **SEND-BACK** | Source uses `tokio::sync::mpsc::channel(1)`, `try_send`, and `JoinHandle::abort`, and contains no `spawn_blocking` or `std::sync::mpsc`. But the counting factory discards the callback, so no test fills/coalesces the channel, drives a queued wake, or observes the consumer task terminate after observer drop. Existing drop assertions prove only that the fake watcher guard was released, not that the async consumer ended promptly or stayed silent after retirement. |

## Blocking findings

### B1 — AC 6 lifecycle routing and reconcile retirement lack executable proof

The spec's `verify.sh` contract requires focused stop/crash/delete retirement
coverage, and the architecture requires create/drop counters for stop, crash
reconcile, delete, and tool change. The committed
`transcript_lifecycle_tests` suite contains only:

- `listing_500_sessions_creates_zero_transcript_entries_or_observers`
- `patching_away_from_claude_retires_live_observation`

The store-level lifecycle test calls retirement methods directly. The watchdog
test invokes a generic hook that only captures IDs. Neither proves that the
actual stop/kill/delete routes or server-wired watchdog callback perform the
claimed retirement. Add deterministic tests at those production seams and
assert observer creation/drop/cache counts, including the no-start invariant.

### B2 — AC 8 bounded delivery and consumer termination are source-only

`Observer::drop` calls `abort`, but no test retains/invokes the injected notify
callback or observes the consumer handle's completion. This leaves queued-wake,
coalescing, prompt abort, and stale-event behavior untested. Extend the injected
factory/test seam so tests can trigger notification bursts and await/observe
consumer shutdown after `stop_observing` and `forget`; assert one queued refresh,
no post-retirement event, and no surviving task.

### B3 — Spec 028 `qa.sh` overstates runtime coverage

The QA branch runs two direct unit tests and prints that live attach/retire
passed. It does not start a production `RecommendedWatcher`, count operating
system observers or threads, append through a live watched transcript, observe
an `agent_tasks.updated` stream, or call the stop route. This does not implement
the spec's stated QA assertion that a production-shaped fleet leaves observer
and thread counts unchanged while a running Claude session streams updates and
stopping it retires observation. Replace or relabel this leg so it executes and
truthfully reports those runtime assertions.

## Negative, race, and error audit

- Concurrent first live reads are serialized and tested exactly once.
- Snapshot/live and Claude/non-Claude mode transitions are tested.
- Partial trailing JSONL handling, reset-first replay prevention, pinned
  promotion, and empty non-Claude behavior are tested.
- Observer-creation/watch failure and retry remain untested, though inspection
  shows a failed creation leaves the passive slot retryable.
- A callback already queued or a consumer already entering synchronous refresh
  during retirement is not exercised. This is the race most in need of AC 8
  evidence because `JoinHandle::abort` is not itself an awaited completion
  assertion.
- Stop/kill host teardown failures retire observation before returning the
  teardown error by inspection; no route-level regression pins that error path.
- Delete retires cache before best-effort host teardown by inspection; no route
  regression proves it.

## Environment-only desktop probes

- UI build is unavailable because the worktree has no installed Vite
  dependencies (`vite: command not found`). No dependencies were installed.
- Full-workspace tests reach the known desktop build prerequisite and fail with
  `Library not found: ../../target/release/libsherpa-onnx-c-api.dylib`. The
  complete non-desktop workspace library suite is green and is the required
  executable backend baseline.

No implementation file, `ai/STATE.md`, handoff, commit, dependency, or external
state was changed by Tester. This report is the only intended worktree change.
