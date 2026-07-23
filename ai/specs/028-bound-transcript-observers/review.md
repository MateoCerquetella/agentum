# Spec 028 — Final review

- **Date:** 2026-07-23
- **Role:** Fresh Reviewer, iteration 2 of 2
- **Reviewed commit:** `62236383`
- **Reviewed implementation/evidence range:** `4f3c030c^..62236383`
- **Production implementation through:** `105ca8ad`
- **Verdict:** **SIGN-OFF**
- **Blockers:** 0
- **Should-fixes:** 1 documentation/release note

`passed: true`

## Summary

Spec 028 is ready to close. The final implementation bounds live transcript work to explicitly
read Running Claude sessions, leaves the 500-session list path database-only, and separates
passive parser state from optional observation. The two send-back rounds are closed by two
independent authority boundaries:

1. observer generations make an already-received notify wake harmless once its attachment is
   retired; mutation and synchronous event emission remain inside the generation check's store
   mutex boundary; and
2. a weak-keyed per-session Tokio mutex linearizes the durable session load performed by
   agent-task GET/reset with stop/kill, delete, and tool-patch mutation through their final
   transcript retirement.

No correctness, security, resource-bound, or deadlock blocker remains. All eight acceptance
criteria have implementation and executable evidence. The documented full-workspace Sherpa dylib
and desktop Vite dependency failures are environment limitations outside this backend-only slice;
the authoritative non-desktop backend gate is green.

## Blockers

None.

## Should-fixes

### S1 — Call out the intentional Rust helper API replacement in release notes

**Severity:** should-fix (documentation only)

**Owner:** Release/documentation

**Ship impact:** none for the approved Spec 028 contract

`TranscriptStore` is publicly re-exported from `agentum-server`, while its previous public helper
methods (`ensure_started`, `snapshot`, and the one-argument `reset`) were replaced by the approved
internal mode-aware/session-aware contract. No workspace caller uses those old methods, the server
and desktop consumers compile, and the spec explicitly approves the replacement, so this is not a
Spec 028 blocker. It is nevertheless a Rust source-compatibility change for any untracked external
consumer of those incidental helpers and should be mentioned in release notes. HTTP response
types, event kind/payload, database schema, watchdog construction without a hook, and all normal
server entry points remain compatible.

## Acceptance-criteria disposition

| AC | Reviewer disposition | Evidence |
|---|---|---|
| 1 | **PASS** | `routes::sessions::list` performs only `Store::list_sessions` plus JSON serialization. The production-shaped 500-row fixture (250 Claude) records zero cache entries, observer creates/drops, and transcript-directory creation. |
| 2 | **PASS** | `read(Live)` creates/reuses the passive slot and assigns its observer while holding the store mutex. Repeated and 16-way concurrent reads create exactly one observer, synchronously return parsed state, and retain the exact `agent_tasks.updated` kind and `{session_id}` payload. |
| 3 | **PASS** | Every non-Running durable status selects `SnapshotOnly`; the store itself retires a prior live observer before synchronously consuming complete appended lines. Complete-line/partial-line behavior and the Running-to-Stopped route transition are executable. |
| 4 | **PASS** | A non-Claude read calls `forget` before returning the default state, including after prior live Claude state. Fresh non-Claude read/reset creates no slot, directory, observer, callback consumer, or event work. |
| 5 | **PASS** | Session-aware reset creates only passive metadata, selects/promotes the same pinned-or-legacy path, clears state and pending tasks, advances to current EOF, and exposes only later complete lines. |
| 6 | **PASS** | Stop/kill retire early and again after durable Stopped; forced delete forgets early and again after durable deletion; tool patch retires after the durable non-Claude mutation. Actual-handler barriers prove a preloaded Running/Claude GET finishes before all four transitions and cannot leave a live observer or deleted cache. The server-wired watchdog callback retires stopped, crashed, deleted, tool-changed, or workdir-mismatched observation without starting work. |
| 7 | **PASS** | UUID pinning, legacy fallback, pinned-file promotion, cursor/truncation/complete-line parsing, `AgentTaskState` JSON, and `agent_tasks.updated` payload remain unchanged and covered. No HTTP or database schema changed. |
| 8 | **PASS** | Notify uses `tokio::sync::mpsc::channel(1)` plus non-blocking `try_send`, coalescing bursts. `Observer::drop` aborts the Tokio consumer before the watcher guard is released. Deterministic post-`recv` gates prove SnapshotOnly, stop, retain, and forget reject already-awake work without mutation, event, observer, or cache resurrection. No blocking receiver path remains. |

## Concurrency, ordering, and deadlock review

### Per-session lifecycle boundary

The lock acquisition graph is one-way:

```text
agent-task GET/reset, PATCH, delete, stop/kill core
  -> keyed Tokio lifecycle mutex
  -> transcript store std::sync::Mutex

notify consumer, watchdog retain hook
  -> transcript store std::sync::Mutex only
```

No code holding the transcript-store mutex attempts to acquire a lifecycle mutex, and no HTTP or
MCP wrapper acquires a second lifecycle guard around `stop_session_core`. The harness stop surface
also calls that same core without wrapping it. Consequently there is no nested same-key acquisition
or inverse lock order. Slow local/SSH teardown can hold one session's async guard, but it does not
block unrelated session UUIDs or a Tokio worker thread on mutex acquisition.

GET/reset hold the guard from before the authoritative durable load through the transcript
operation. Stop/kill hold it from load through early retirement, teardown, durable Stopped write,
final retirement, and response load. Delete holds it from load through the force guard, teardown,
durable row deletion, and final forget. PATCH holds it across the durable identity load, tool
mutation, and Claude retirement. Thus a request is ordered wholly before a transition and its work
is retired, or wholly after it and observes Stopped, non-Claude, or missing durable state.

Start and watchdog paths do not acquire the lifecycle mutex, which is correct for this design.
Start only creates/marks Running authority and never retires transcript state; observation remains
demand-driven and a subsequent Running read attaches it. The watchdog callback is a synchronous,
drop-only reconciliation after a successful authoritative Running query and takes only the store
mutex. It cannot start observation or form an inverse lock edge. A concurrent read and watchdog
pass serialize at the store mutex; if an already-loaded read attaches just after a retirement pass,
the next existing five-second pass retires it, preserving the bounded reconcile guarantee.

### Weak registry lifetime and pruning

The registry's standard mutex serializes lookup/installation. An acquisition upgrades the existing
`Weak<AsyncMutex<()>>` or installs one lock, then `lock_owned()` transfers a strong `Arc` into the
waiter/guard. A holder's `OwnedMutexGuard` and a queued waiter's owned lock future each keep the
strong count nonzero. Opportunistic `retain(strong_count > 0)` therefore cannot remove and replace
a lock while any holder or waiter exists. Cancellation drops that strong reference safely. After
the last holder/waiter exits, the dead weak key may remain only until the next registry access,
which prunes it; historical UUIDs do not accumulate permanent lock tombstones. The dedicated
same-key exclusion/dead-key regression confirms both properties.

### Observer generation and callback quiescence

Each successful attachment receives a globally monotonic nonzero generation and stores it in the
slot before the store mutex is released. A consumer captures only that generation. Retirement
`take`s the observer from the slot before aborting it, so an already-awake consumer can no longer
pass `refresh_if_current`. That method verifies the current generation under the store mutex and
keeps file mutation and `broadcast::Sender::send` inside the same boundary. Retirement therefore
cannot return between a successful identity check and event emission. Workdir replacement and
forget also remove the old authority; a newly attached observer has a distinct generation.

The callback owns only a capacity-one sender and never acquires either lifecycle/store mutex.
Watcher destruction therefore cannot deadlock with retirement. A late callback observes a closed
channel; a callback already through `recv` either completes before retirement obtains the store
mutex or fails the generation check afterward. The deterministic synchronous post-receive gate,
not the timing-only real-watcher smoke test, is authoritative for this schedule.

## Resource-bound and 500-session review

- Session list code never calls `TranscriptStore`, so 500 historical rows create neither passive
  slots nor live resources.
- Passive snapshots contain parser state only. A slot owns at most one `Observer`.
- Concurrent first reads cannot duplicate attachment because create/assign is serialized by the
  store mutex.
- Each observer owns one bounded capacity-one channel, one Tokio task, and one watcher guard; the
  former permanent `spawn_blocking` plus `std::sync::mpsc` receiver is absent.
- Reconcile iterates existing slots and can only drop observers. It neither creates slots nor
  touches transcript files.
- Lifecycle UUID keys are weak and opportunistically pruned, so the linearization fix does not
  replace the original observer leak with a historical lock-registry leak.

## Compatibility and maintainability review

- `AgentTaskState`, all HTTP status/body shapes, the `agent_tasks.updated` event name and payload,
  session JSON, and database schemas are unchanged.
- `Watchdog::new` remains valid; `with_running_sessions_hook` and `reconcile_once` are additive.
- The watchdog/server dependency direction remains one-way: the lower watchdog crate accepts a
  generic optional callback; server code captures `TranscriptStore` and supplies policy.
- The old observation implementation is removed rather than left as a parallel path. There is one
  parser refresh function, one live-attachment decision, one stop/kill core shared by HTTP/MCP,
  and one existing watchdog cadence.
- Test-only barriers and counters are fully `cfg(test)` and do not expand production state or
  runtime synchronization.
- S1 records the only compatibility caveat: public Rust helper methods were intentionally replaced
  even though network/event contracts remain stable.

## Security and safety review

- No authorization, credential, listener, SSH command construction, tmux argument construction,
  or externally writable path boundary changed.
- Transcript path resolution continues through the established UUID-pinned Claude helpers.
  Snapshot-only and non-Claude reads do not create directories; only a live Claude attachment may
  materialize the established project directory.
- Per-session lifecycle locks do not broaden authorization or expose state; they only order already
  authorized operations.
- Capacity-one delivery bounds notify burst memory, weak lifecycle entries bound historical lock
  memory, and generation fencing prevents stale state/event publication after retirement.

## Independent Reviewer verification

| Command / inspection | Result |
|---|---|
| `cargo test -p agentum-server transcript_store::tests --lib -- --nocapture` | **PASS — 11/11** |
| `cargo test -p agentum-server routes::sessions::tests::transcript_lifecycle_tests --lib -- --nocapture` | **PASS — 7/7** |
| `cargo test -p agentum-server routes::agent_tasks::tests --lib -- --nocapture` | **PASS — 2/2** |
| server-wired watchdog focused test | **PASS — 1/1** |
| generic watchdog reconcile-hook focused test | **PASS — 1/1** |
| full source/lock-order inspection of `4f3c030c^..62236383` | **PASS** |
| blocking-receiver source inspection | **PASS — no production `spawn_blocking` or `std::sync::mpsc` receiver** |

Tester independently records isolated QA **21/21**, non-desktop backend workspace **839 passed,
0 failed, 2 ignored**, server/watchdog check, formatting, JSON/shell validation, source guard, and
implementation diff checks green. Reviewer reproduced the focused concurrency and watchdog gates.

The raw full-range `git diff --check` previously reports only Markdown hard-break spaces in the
superseded iteration-1 review document. This final review replaces that file and is whitespace
clean. The implementation range itself is clean.

## Environment limitations

- `cargo test --workspace --lib` remains blocked only in `agentum-desktop` by the absent
  `target/release/libsherpa-onnx-c-api.dylib`; `--exclude agentum-desktop` is green.
- `npm run build --prefix crates/agentum-desktop/ui` remains blocked because dependencies are not
  installed (`vite: command not found`). Spec 028 changes no UI/browser code.

Neither limitation weakens the executable backend evidence for this spec.

## Final verdict

**SIGN-OFF.** No blocker remains. The prior consumer-authority, teardown-window, and stale
preloaded-request races are closed with deterministic tests for the exact schedules. Spec 028 may
advance to `done`; release/merge remains human-gated.
