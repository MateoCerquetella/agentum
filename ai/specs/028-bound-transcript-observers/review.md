# Spec 028 — Final review

- **Date:** 2026-07-23
- **Role:** Fresh Reviewer
- **Reviewed commit:** `ff43ef40`
- **Reviewed implementation/evidence range:** `4f3c030c..ff43ef40`
- **Verdict:** **SEND-BACK**
- **Blockers:** 3
- **Should-fixes:** 1

`passed: false`

## Summary

The principal design is sound: session listing is side-effect free, transcript
parser state is passive, live attachment is serialized, notify delivery is
capacity-one and coalescing, and the watchdog dependency direction remains
correct. AC 1, 2, 4, 5, and 7 have adequate implementation and evidence.

Sign-off is blocked by one consumer-cancellation race and two route-retirement
ordering races. Together they mean observation is not reliably quiescent after
SnapshotOnly/stop/kill/delete retirement, despite the current tests and
verification report claiming that it is. AC 3's live-to-snapshot transition,
AC 6, and AC 8 therefore are not fully satisfied.

## Blockers

### B1 — An already-awake consumer can refresh and emit after observer retirement

**Severity:** blocker  
**Owner:** Developer; Tester must replace the missing evidence  
**AC impact:** AC 3 transition semantics and AC 8; also the no-stale-event risk

`Observer::drop` only calls `JoinHandle::abort`
(`transcript_store.rs:68-71`). The consumer receives a wake and then enters the
synchronous `store.refresh(id)` call (`:299-304`). `refresh` blocks on the
store's `std::sync::Mutex`, mutates any slot still present, releases the mutex,
and emits without checking which observer produced the wake (`:312-321`).

Tokio abort is cooperative: it cancels a task when the future next yields to
the runtime; it cannot interrupt synchronous code that is already executing.
The following schedule is therefore valid:

1. the consumer passes `rx.recv().await` and blocks in `refresh` on the store
   mutex;
2. a SnapshotOnly read, `stop_observing`, or `retain_observers` owns the mutex,
   takes the observer, and calls `abort`;
3. retirement releases the mutex and returns;
4. the already-awake consumer acquires the mutex, refreshes the retained passive
   slot, and emits `agent_tasks.updated` after retirement.

There is a second form of the same race when the consumer refreshes under the
mutex, is descheduled before the out-of-lock emit, and retirement completes in
between. Abort does not retroactively cancel that synchronous send.

The existing
`notify_bursts_coalesce_and_retirement_finishes_consumers_without_stale_updates`
test does not cover either schedule. It waits for the queued refresh event and
state mutation to complete at lines 642-651, and only then retires at line 653.
Its post-retirement callback assertion proves that a newly sent wake sees a
closed channel; it does not prove that a wake already past `recv()` is harmless.
The real watcher smoke test has the same limitation and its 300 ms silence
window is not synchronization evidence.

**Required correction:** give every attachment a monotonically unique
generation/liveness token stored with the slot and captured by its consumer.
The consumer must use a `refresh_if_current(id, generation)` seam that verifies
under the store mutex that the slot still owns that generation before mutation.
Event emission must be ordered by that same retirement boundary (for example,
verify and synchronously broadcast while the mutex still proves the generation
current); a check followed by an unlocked send leaves the second race open.
Retirement clears/advances the generation before aborting the task.

Add a deterministic test-only barrier/hook that pauses the consumer after
`recv()` but before it can refresh. Retire through SnapshotOnly,
`stop_observing`, `retain_observers`, and `forget`, release the barrier, and
assert prompt consumer completion, no state mutation, no bus event, and no
forgotten-slot recreation.

### B2 — Stop/kill can leave an observer reattached during teardown

**Severity:** blocker  
**Owner:** Developer; Tester must exercise a successful route interleaving  
**AC impact:** AC 6

`stop_session_core` retires at `sessions.rs:698`, then awaits host lookup and up
to five seconds of graceful teardown while the durable row still says
`Running`. A concurrent agent-task GET during that interval selects `Live` and
can attach a new observer. After committing `Stopped` at lines 705-722, the
route performs no final retirement, so the replacement can survive until a
later read or watchdog pass.

The route regression does not close this race. Its created sessions are not
marked running, the stop/kill results are discarded, and the nonexistent tmux
targets can take the early error path. It proves that the early production
retirement call is reached, not that a successful route remains retired across
its asynchronous teardown window.

**Required correction:** retain the early retirement, then perform a final
`stop_observing(id)` after the stopped status is durably committed and before a
successful response. Add a controlled teardown barrier test that performs a
Live read during the wait, proves it can reattach, completes stop and kill
successfully, and then proves the final observer count is zero with cache
retained and no later stale event. The store liveness/generation boundary from
B1 must also prevent a request/wake carrying pre-retirement authority from
reviving work after the final boundary.

### B3 — Delete can recreate cache/observation between its early forget and DB deletion

**Severity:** blocker  
**Owner:** Developer; Tester must add the route race regression  
**AC impact:** AC 6

`delete` calls `forget(id)` at `sessions.rs:460`, then performs asynchronous
host/tmux teardown before deleting the durable row at line 499. During that
window the row remains readable. A concurrent GET can recreate passive state,
or for a force-deleted running Claude session attach a new observer. The route
never forgets again. The watchdog eventually drops a recreated observer but
intentionally retains passive cache, leaving deleted-session state stranded
indefinitely.

**Required correction:** retain the early forget, then call `forget(id)` again
after successful durable deletion and before returning 204. Add a controlled
route test that reads during the teardown window and proves the completed
delete leaves both cache and observer counts at zero, including forced deletion
of a running Claude session. As with B2, the B1 generation/liveness boundary
must reject pre-final-boundary consumer work.

## Should-fixes

### S1 — Make the lifecycle evidence and QA wording match the adversarial schedule

After B1-B3 are fixed, update the lifecycle test names/assertions and
`verification.md` rather than retaining the present broad claim that stale
wakes and route retirement are already proved. The real
`RecommendedWatcher` test is a useful runtime smoke test, but deterministic
barriers—not timing-only silence—must remain authoritative for shutdown races.
The isolated QA output should identify both the real-watcher smoke leg and the
separate deterministic interleaving proofs.

## Acceptance-criteria disposition

| AC | Reviewer disposition | Evidence / gap |
|---|---|---|
| 1 | **PASS** | The production list handler is database-only. The 500-row/250-Claude fixture returns all rows with zero cache, observer, drop, and directory side effects. |
| 2 | **PASS** | One store mutex serializes passive-slot creation and first Live attachment; the 16-way regression records exactly one observer and preserves the exact update kind/payload. |
| 3 | **BLOCKED** | Fresh SnapshotOnly reads synchronously consume complete appended lines without attaching. A live-to-snapshot transition is not quiescent because B1 permits its detached consumer to refresh/emit afterward. |
| 4 | **PASS** | Non-Claude reads forget prior Claude state and return empty; fresh read/reset creates no entry, directory, observer, or consumer. A late old consumer cannot recreate an absent map entry, though its stale event risk is covered by B1/AC 8. |
| 5 | **PASS** | Reset-first creates passive metadata, promotes/selects the transcript, clears state, advances to EOF, and exposes only post-reset complete lines. |
| 6 | **BLOCKED** | Direct retirement and watchdog selection logic are correct, but B2 and B3 allow route-window reattachment/recreation; B1 also permits post-retirement consumer work. Current route tests do not exercise those successful interleavings. |
| 7 | **PASS** | UUID pinning, legacy fallback, pinned promotion, complete-line cursoring, event payload, and response types are preserved by focused tests and source review. No HTTP or DB schema changed. |
| 8 | **BLOCKED** | Capacity-one `try_send` and removal of blocking receivers are correct. Abort-on-drop alone does not terminate a consumer already executing synchronous `refresh`, and current tests do not cover that schedule. |

## Architecture and invariant review

- **Passive/live separation:** correct in the steady state. Snapshot-only reads
  do not create directories; only Live attachment materializes a watch path.
- **Exactly-once first attachment:** correct for repeated/concurrent Live reads
  because slot lookup and observer assignment share one mutex boundary.
- **Mode transitions:** non-Claude forget and SnapshotOnly take the prior
  observer as designed, but B1 prevents the latter from being a complete
  asynchronous quiescence boundary.
- **Callback lifetime:** bounded channel ownership is correct; the missing
  generation check means retained test callbacks accurately expose channel
  closure but not in-flight task invalidation.
- **Watchdog direction:** correct. `agentum-watchdog` exposes a generic optional
  successful-query hook; the server captures `TranscriptStore` and supplies
  retirement policy. There is no reverse crate dependency, new timer, or
  observation-start path.
- **Watchdog ordering:** the hook runs after the authoritative running-session
  query and before per-session watcher reconciliation. Query failure skips the
  hook, so absence is not treated as authoritative.
- **Route ordering:** early retirement before slow/best-effort teardown is
  desirable, but it must be paired with the final boundaries in B2-B3.
- **No duplicate/dead production path:** the old `ensure_started`/snapshot and
  blocking receiver implementation are removed rather than retained in
  parallel. No duplicate parser or second reconcile timer was introduced.
- **Existing platform invariants:** Claude UUID pinning, the centralized launch
  path, YOLO translation, push-based pane streaming, boot-revival-before-watchdog
  ordering, and public event/HTTP schemas are untouched.

## Security and safety review

- No authorization, credential, network-listener, command-construction, or
  external-write boundary changed.
- Transcript paths continue through the established workdir-to-Claude path
  helpers; Live mode preserves the existing best-effort directory creation,
  while passive reads avoid it.
- Capacity-one `try_send` bounds notify burst memory and removes the permanent
  blocked receiver thread, which is a material resource-safety improvement.
- The blockers are lifecycle/resource correctness issues rather than data
  exfiltration or privilege-escalation defects. B2/B3 can nevertheless retain
  unwanted observers/cache, so they must be fixed before the performance and
  shutdown guarantees are accepted.

## Verification review

- Reviewer spot check: focused transcript-store tests **PASS, 9/9**.
- Reviewer diff hygiene: `git diff --check 4f3c030c..ff43ef40` **PASS**.
- Reviewer source guard: no `spawn_blocking` or `std::sync::mpsc` receiver path
  remains in `transcript_store.rs`.
- Tester-recorded focused/runtime/backend gates are green as listed in
  `verification.md`; the desktop/UI environment blockers are unrelated to this
  backend-only implementation.
- Those green runs do not override B1-B3 because none forces the missing
  mutex/cancellation or route-teardown schedules.

## Final verdict

**SEND-BACK.** Fix B1-B3 and add deterministic regressions for the exact
interleavings above, then rerun the focused transcript, route, server-watchdog,
isolated QA, backend workspace, formatting, and diff gates. Spec 028 must not be
marked done at `ff43ef40`.
