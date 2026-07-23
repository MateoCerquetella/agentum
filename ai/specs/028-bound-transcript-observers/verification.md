# Spec 028 — Tester Verification

- **Spec:** `028-bound-transcript-observers`
- **Date:** 2026-07-23
- **Base under test:** `87321fd2` (`e878dd6e..87321fd2` Reviewer-race closure diff)
- **Role:** Fresh Tester after Reviewer send-back
- **Verdict:** **SEND-BACK — AC 6 still has a reproducible stale-request race.**

The generation-aware consumer fix is correct and closes the already-received-wake
race: generation identity, transcript mutation, and `agent_tasks.updated`
broadcast are checked/performed under the same transcript-store mutex boundary.
The new deterministic wake test passes through SnapshotOnly,
`stop_observing`, `retain_observers`, and `forget`.

The final stop/kill and delete cleanup calls also close recreation that happens
*before* those final calls, exactly as their new route tests assert. They do not,
however, fence an agent-task request that loaded the durable `Running` row before
the final boundary and reaches `TranscriptStore::read(..., Live)` afterward.
Such a request can still attach after successful stop/kill, or recreate cache and
observation after successful deletion.

## Independent gates

| Gate | Result |
|---|---|
| `cargo test -p agentum-server transcript_store::tests --lib -- --nocapture` | **PASS — 10/10** |
| `cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture` | **PASS — 2/2 committed tests** |
| `cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture` | **PASS — 4/4** |
| server-wired watchdog focused test | **PASS — 1/1** |
| generic watchdog reconcile-hook focused test | **PASS — 1/1** |
| `HARNESS_FEATURE_ID=transcript-observer-lifecycle bash .harness/qa.sh` | **PASS — 17/17 isolated tests**, including the real `RecommendedWatcher` smoke leg |
| temporary stale-request regressions described below | **FAIL — 0/2; AC 6 defect reproduced** |
| `cargo check -p agentum-server -p agentum-watchdog` | **PASS** |
| `cargo test --workspace --lib --exclude agentum-desktop` | **PASS — 835 passed, 0 failed, 2 ignored** |
| `cargo fmt --all -- --check` | **PASS** |
| `git diff --check e878dd6e..87321fd2` | **PASS** |
| blocking-receiver source guard | **PASS — no `spawn_blocking` or `std::sync::mpsc` match in `transcript_store.rs`** |
| `.harness/feature_list.json` parse and `bash -n .harness/{qa,verify}.sh` | **PASS** |

The temporary regressions were added only to force the missing schedules, run,
and then removed. No implementation source change remains from Tester.

## Acceptance-criterion map

| AC | Verdict | Independent evidence |
|---|---|---|
| 1 | **PASS** | The production list handler remains database-only. The 500-row fixture, including 250 Claude sessions, returns every row with zero cache entries, observer creations/drops, or transcript-directory creation. |
| 2 | **PASS** | The 16-way concurrent/repeated running-Claude test creates exactly one observer. The route selects Live for Running Claude, and the existing update kind/payload remain asserted. |
| 3 | **PASS** | SnapshotOnly reads synchronously incorporate appended complete lines, retain a partial line until newline, attach no observer, and now reject an already-received wake after live-to-snapshot retirement. |
| 4 | **PASS** | Non-Claude read/reset creates no entry, directory, observer, or consumer; transitioning from Claude forgets the prior observer/cache. |
| 5 | **PASS** | Reset-first advances the selected transcript cursor to EOF and exposes only later complete lines without observation. |
| 6 | **BLOCKED** | Mid-teardown reattachment/recreation is cleaned by the new final calls, but a GET that already loaded `Running` can execute its cached Live decision after those calls. The two forced schedules leave an observer after stop and cache+observer after delete. |
| 7 | **PASS** | Pinned isolation/promotion, legacy fallback, complete-line parsing, response/event payloads, and the non-desktop workspace regressions remain green. |
| 8 | **PASS** | Notify transport remains capacity-one/coalescing with no blocking receiver. The post-`recv()` barrier proves stale generations cannot mutate or emit after all four transcript retirement operations, and consumers complete after release. |

## Reproduced blocker — stale Running request crosses the final route boundary

`get_agent_tasks` loads a `Session` at `routes/agent_tasks.rs:35-39`, derives
Live from that snapshot at `:41-45`, and later calls `TranscriptStore::read` at
`:46-48`. There is no shared lifecycle authority spanning those operations and
the final stop/delete boundary.

Two temporary tests copied that production ordering and asserted the required
post-boundary state:

1. **Stop/kill schedule**
   - Load a Claude session snapshot while its durable status is `Running`.
   - Perform early retirement, commit `Stopped`, and perform the production
     final `stop_observing` boundary (`sessions.rs:735`).
   - Resume the already-loaded request and call `read(..., Live)` from its stale
     snapshot.
   - Expected observer count: `0`; actual: `1`.

2. **Forced-delete schedule**
   - Load a Claude session snapshot while its durable status is `Running`.
   - Perform early forget, durable row deletion, and the production final
     `forget` boundary (`sessions.rs:501-505`).
   - Resume the already-loaded request and call `read(..., Live)` from its stale
     snapshot.
   - Expected cache count: `0`; actual: `1` (with a live observer also created).

The focused command reported both failures:

```text
running 2 tests
stale_running_request_cannot_attach_after_final_stop_boundary ... FAILED
  assertion failed: left 1, right 0
stale_running_request_cannot_recreate_after_final_delete_boundary ... FAILED
  assertion failed: left 1, right 0
test result: FAILED. 0 passed; 2 failed
```

This is not contradicted by the committed route regressions. Those deliberately
reattach/recreate while teardown is parked and then release teardown, so the
replacement exists before the final cleanup call. They do not park a request
after its durable session load and resume it after final cleanup.

## Required correction

Introduce one per-session asynchronous lifecycle linearization boundary shared
by transcript reads/resets and session lifecycle mutations:

- hold the session's boundary across durable session load plus
  `TranscriptStore::read`/`reset` in agent-task routes;
- hold the same boundary across load, teardown/status mutation, and final
  transcript retirement in stop/kill/delete; and
- use it for tool patching so a request carrying the old Claude identity cannot
  attach after the tool-change retirement.

With that ordering, an agent-task operation either completes before lifecycle
mutation, in which case final cleanup removes its work, or enters afterward and
observes `Stopped`, non-Claude, or a missing row. Prefer a scoped keyed-lock
registry with weak/ref-counted entries and opportunistic cleanup; a permanent
per-UUID generation/tombstone map would grow with historical/deleted sessions
and would recreate the resource-retention problem this spec is meant to solve.

Add deterministic route tests that park after the agent-task durable load,
complete successful stop/kill or forced delete through the final boundary, then
release the request and prove zero live observers; delete must also prove zero
cached entries. Retain the existing already-received-wake and mid-teardown tests.

No `ai/STATE.md`, handoff, implementation file, dependency, commit, or external
state was changed by Tester. This report is the only intended worktree change.
