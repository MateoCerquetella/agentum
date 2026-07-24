# Spec 028 — Architecture

- **Spec:** `028-bound-transcript-observers`
- **Phase:** Architect
- **Date:** 2026-07-23
- **Verdict:** ready for decomposition

## Current-state findings

1. `routes::sessions::list` calls `TranscriptStore::ensure_started` for every returned row.
   `ensure_started` creates the Claude project directory, one `RecommendedWatcher`, an unbounded
   `std::sync::mpsc` channel, and one permanent `tokio::task::spawn_blocking` receiver for each
   Claude session. The desktop polls the list, so history—not active work—determines watcher/thread
   count.
2. A `Slot` cannot exist without a watcher because it stores `_watcher: RecommendedWatcher`
   directly. Parser state, cursor, pending task dispatches, pinned path, and legacy fallback are
   therefore coupled to live observation.
3. `get_agent_tasks` always calls `ensure_started`, regardless of durable `Session::status`, then
   separately calls `snapshot`. `reset_agent_tasks` calls `reset(id)` without enough session data to
   create a passive slot, so reset-before-first-read is currently a no-op.
4. Stop, kill, delete, and tool-patch routes never retire transcript state. The watchdog already
   queries all `Status::Running` sessions every `RECONCILE_TICK` (five seconds), but the lower
   `agentum-watchdog` crate cannot depend on `agentum-server`.

## Decisions

### 1. One atomic read API owns entry creation, refresh, and observation mode

Replace the two-step `ensure_started`/`snapshot` contract with:

```rust
pub(crate) enum ObservationMode { Live, SnapshotOnly }

pub(crate) fn read(
    &self,
    id: Uuid,
    workdir: PathBuf,
    tool: &str,
    mode: ObservationMode,
) -> AgentTaskState
```

`read` takes the store mutex once for the existence/attach decision. For `tool != "claude"`, it
returns `AgentTaskState::default()` without resolving a Claude path, creating an entry/directory,
or attaching an observer. For Claude it creates or reuses a passive `Slot`, promotes the pinned
file when present, consumes newly appended complete lines synchronously, and clones the state.

The lock makes concurrent first live reads exactly-once. Filesystem reads remain synchronous as
they are today and are bounded to the newly appended suffix after initial load; this spec does not
change parser or response contracts.

### 2. A passive slot owns transcript state; an optional observer owns liveness

Keep the existing slot fields (`workdir`, state, pinned/selected paths, cursor, and pending tasks)
and replace `_watcher` with `observer: Option<Observer>`. Snapshot-only reads never create the
project directory. A live read may create the missing directory because notify must watch the
directory before Claude writes its first transcript.

`Observer` owns both:

- a boxed watcher guard returned by a private `ObserverFactory`; and
- the Tokio consumer `JoinHandle<()>`.

Its `Drop` implementation aborts the consumer before dropping the watcher guard. The production
factory wraps `notify::RecommendedWatcher`; a test factory returns a counting guard so creation and
drop assertions do not depend on OS notify timing.

### 3. Notify delivery is bounded and coalescing

For each live observer use `tokio::sync::mpsc::channel::<()>(1)`. The synchronous notify callback
filters relevant event kinds and calls `try_send(())`; a full channel means a refresh is already
queued, so the callback coalesces the duplicate. A Tokio task awaits `recv()` and invokes the same
incremental refresh used by synchronous reads. No `std::sync::mpsc` receiver or `spawn_blocking`
thread remains.

Watcher setup occurs while the store's first-read decision is serialized. It watches before the
initial synchronous parse and starts the consumer only after the slot exists, so an append in the
setup window is either included by the parse or retained as a queued refresh.

### 4. Reset is a session-aware store operation

Change reset to accept `id`, `workdir`, and `tool`. For Claude, it creates a passive slot when
needed, resolves/promotes the same pinned-or-legacy path, clears state and pending dispatches, and
advances the cursor to the selected file's current byte length. It never attaches an observer.
For non-Claude it remains an empty no-op. Emit the existing `agent_tasks.updated` event only after
a Claude entry was cleared, with the unchanged `{ "session_id": ... }` payload.

This avoids implementing reset as “read then clear,” which could expose a transient pre-reset
state event.

### 5. Routes choose and enforce lifecycle

- Remove the transcript loop from `routes::sessions::list`; the handler becomes only
  `store.list_sessions(status)` plus JSON serialization.
- `routes::agent_tasks::get_agent_tasks` selects `Live` only when both `session.tool == "claude"`
  and `session.status == Status::Running`; every other status selects `SnapshotOnly`. The store
  itself still rejects non-Claude entry creation.
- `reset_agent_tasks` passes the loaded session's workdir/tool into the session-aware reset.
- After a valid stop/kill target is loaded and before tmux shutdown waits, call `stop_observing`.
  A failed teardown can be repaired by a later running-session read.
- After the running/force delete guard passes and before best-effort tmux teardown, call `forget`.
  A failed database deletion leaves no stale observer; a later read recreates passive state.
- A manual tool patch away from Claude calls `stop_observing`; no route automatically starts an
  observer after a patch.

### 6. Watchdog reconciliation receives a one-way retirement callback

Add an optional callback field to `agentum_watchdog::Watchdog`, configured with a builder such as
`with_running_sessions_hook`. Its type accepts the already-loaded running `Session` slice and is
`Send + Sync + 'static`. Invoke it once per successful reconcile immediately after the database
query and before per-session watch-task reconciliation.

The server boot closure captures a clone of `TranscriptStore` and calls
`retain_observers(running)`. That method keeps an observer only when the session is still running,
still Claude, and still refers to the slot's workdir; it drops all other observer handles but keeps
passive cached state. This direction preserves crate boundaries: watchdog exposes a generic hook;
the server supplies transcript policy. Query failure does not run the hook because absence is not
authoritative.

Crash transitions become bounded by the existing five-second reconcile. Stop/kill/delete remain
immediate through their route calls. The hook never calls `read`, so it cannot start observation.

## Data and control flow

```text
GET /api/sessions
  -> Store::list_sessions -> JSON                        (no TranscriptStore call)

GET /api/sessions/:id/agent-tasks
  -> load Session
  -> mode = Running Claude ? Live : SnapshotOnly
  -> TranscriptStore::read
       -> non-Claude: empty, no entry
       -> passive slot create/reuse + synchronous incremental refresh
       -> Live only: exactly-once Observer attach
  -> existing AgentTaskState JSON

notify callback -> try_send(capacity 1) -> Tokio consumer -> refresh(id)

stop/kill -> stop_observing(id) -> Observer::drop -> abort consumer
delete    -> forget(id)         -> Slot::drop -> Observer::drop

watchdog reconcile -> running Session rows -> server hook -> retain_observers(rows)
```

## Race and error handling

- The store mutex serializes concurrent entry creation and live attachment. A stale callback can
  only queue onto its observer-owned channel; abort/drop prevents it from recreating a forgotten
  slot.
- Notify creation/watch failure leaves the passive slot readable and logs the existing warning;
  a later live read may retry attachment because `observer` remains `None`.
- Missing project directories are ordinary for snapshot reads and are never materialized. A live
  read preserves current best-effort directory creation behavior.
- A truncated or missing transcript does not erase cached state unless the file length
  authoritatively shrank, preserving current refresh semantics.
- `retain_observers` only drops; it does not create entries, refresh files, or emit events.

## Exact files and seams

- `crates/agentum-server/src/transcript_store.rs` — passive slot, observation mode/factory,
  atomic read/reset, bounded consumer, lifecycle methods, and deterministic tests.
- `crates/agentum-server/src/routes/agent_tasks.rs` — status-based mode selection and
  session-aware reset tests.
- `crates/agentum-server/src/routes/sessions.rs` — side-effect-free list and immediate
  stop/kill/delete/tool-change retirement; 500-row route regression.
- `crates/agentum-watchdog/src/lib.rs` — optional running-session reconcile hook and hook tests.
- `crates/agentum-server/src/lib.rs` — inject the transcript-retirement hook at watchdog boot.
- `.harness/feature_list.json`, `.harness/verify.sh`, `.harness/qa.sh` — three Spec 028 feature
  routes and isolated runtime QA contract without disturbing existing entries.

## Build order

1. Refactor `TranscriptStore` and land pure/fake-factory tests.
2. Switch agent-task/list/lifecycle routes and add route-level regressions.
3. Add the generic watchdog hook, wire it from the server, and cover reconciliation retirement.
4. Add harness routing, run focused tests, then the workspace/build/diff gates.

## Test strategy and acceptance traceability

| AC | Implementation seam | Verification |
|---|---|---|
| 1 | sessions `list` | 500-row fixture; injected factory create count and cache count stay zero |
| 2 | `read(Live)` | repeated + concurrent Tokio reads create one observer; event payload remains exact |
| 3 | `read(SnapshotOnly)` | append a complete task line between reads; state advances, create count stays zero |
| 4 | non-Claude early return | no entry, directory, observer, or consumer after read/reset |
| 5 | session-aware reset | reset as first operation, then append/read; only post-reset task survives |
| 6 | lifecycle methods/routes + watchdog hook | create/drop counters for stop, crash reconcile, delete, and tool change |
| 7 | shared refresh path | pinned isolation, legacy fallback, promotion, and schema regressions |
| 8 | observer `Drop` + channel(1) | fake guard drop and aborted consumer completion; source/test assertion removes `spawn_blocking` path |

No product choice remains open and no architecture invariant is weakened.
