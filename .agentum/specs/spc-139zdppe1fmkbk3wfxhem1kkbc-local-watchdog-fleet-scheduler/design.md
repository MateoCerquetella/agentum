# Spec 029 Architecture — Local Watchdog Fleet Scheduler

- **Spec:** `029-local-watchdog-fleet-scheduler`
- **Status:** Architect PASS
- **Date:** 2026-07-23
- **Surfaces:** `agentum-watchdog`, `agentum-tmux`

## 1. Current-state findings

`Watchdog::reconcile_once` is already the authoritative five-second query for Running sessions and
the server-owned transcript-retirement callback. The scalability defect is below that seam:
`tasks: HashMap<Uuid, JoinHandle<()>>` starts one `watch_session` loop per session, each local loop
sleeps independently and calls `agentum_tmux::ssh::sample_pane`. The local arm ultimately starts one
tmux client for every due session. Duplicate session rows targeting the same pane repeat the same
capture and maintain unrelated timers.

The compatibility state machine is concentrated in `watch_session` and must not be rewritten from
memory. Its observable order is:

1. pane gone / crash signature and intentional-stop suppression;
2. context-low handling and five-minute cooldown;
3. two-consecutive-sample recognized tool drift;
4. activity transition classification and event emission;
5. next 1/2-second local or 3/6-second remote deadline.

`agentum_tmux::capture_pane_sample_combined` proves that one tmux client can execute a sequence of
commands, but its fixed separator and single target cannot safely frame a fleet. The SSH arm of
`ssh::sample_pane` deliberately uses `SshMux::Streaming`, one target at a time, and classifies an
exit-43 existence result. That remote path remains in place.

## 2. Decisions

### 2.1 Ownership and file seams

| Owner | File | Responsibility |
| --- | --- | --- |
| `agentum-tmux` | new `src/local_batch.rs` | Public batch request/result types, nonce-framed command builders, strict parsers, concrete local tmux runner, and the injectable runner trait. It knows targets, pane identities, frames, and compact delivery; it does not know sessions, stores, adapters, events, cadence, or generations. |
| `agentum-tmux` | `src/lib.rs`, `Cargo.toml` | Export `local_batch`; add workspace `uuid` for unpredictable v4 invocation nonces. Existing one-target and SSH APIs remain compatible. |
| `agentum-watchdog` | new `src/fleet.rs` | Local registration table, target deduplication/fanout, deadlines, generations, pending compact actions, cycle orchestration, and the fake runner. |
| `agentum-watchdog` | `src/lib.rs` | Keep `Watchdog::{new,with_running_sessions_hook,run,reconcile_once}` stable; replace local `watch_session` tasks with the fleet; retain remote tasks only; extract the existing ordered per-session machine once for local and remote use. |
| `agentum-watchdog` | `Cargo.toml` | Enable Tokio `test-util` for deterministic paused-time tests only if the existing workspace feature set does not expose it to tests. |
| repository | `CLAUDE.md`, `.harness/{feature_list.json,verify.sh,qa.sh}` | Document the new ownership and make the three Spec 029 slices executable gates. No desktop/UI change. |

No new crate, database schema, server route, event type, or alternate local polling path is needed.

### 2.2 Public batch contract

`agentum_tmux::local_batch` exposes total, request-indexed results. Suggested concrete names may be
adjusted for Rust ergonomics, but the type boundary is fixed:

```rust
pub struct BatchTarget { pub request_id: u32, pub target: String }
pub struct BatchRequest { pub targets: Vec<BatchTarget>, pub lines: usize }
pub struct PaneIdentity(String);                 // exactly tmux `%` + decimal id
pub struct BatchSample { pub pane: String, pub viewport: String,
                         pub current_command: String, pub pane_id: PaneIdentity }
pub enum BatchOutcome { Sample(BatchSample), Gone, Retry(RetryReason) }
pub struct CompactAction { pub request_id: u32, pub expected_pane: PaneIdentity,
                           pub keys: String }
pub enum ActionOutcome { Delivered, Retry(RetryReason) }
pub struct BatchResult { pub outcomes: BTreeMap<u32, BatchOutcome>,
                         pub actions: BTreeMap<u32, ActionOutcome> }
```

The runner is deliberately two-stage because Rust must inspect the captured pane for crash
signatures before `/compact` is allowed to run:

```rust
pub trait LocalBatchRunner: Send + Sync {
    fn probe<'a>(&'a self, request: BatchRequest) -> BatchFuture<'a, BatchProbe>;
    fn finish<'a>(&'a self, probe: BatchProbe, actions: Vec<CompactAction>)
        -> BatchFuture<'a, BatchResult>;
}
```

`BatchFuture` is a boxed `Send` future so `Watchdog` can keep
`Arc<dyn LocalBatchRunner>` without adding `async-trait`. `TmuxLocalBatchRunner` is the production
implementation; a recording fake is injected by watchdog tests. `BatchProbe` exposes only
well-typed candidate samples plus per-request parse evidence needed to decide whether an action is
safe. It keeps raw framing private to `agentum-tmux`.

`probe` always uses exactly one local tmux process when the target set is non-empty. `finish` uses
zero processes when every probe is already a final `Sample` and there are no actions; otherwise it
uses exactly one command-sequence invocation for every necessary confirmation and safe action.
Neither method may call `has_session`, `capture_pane_sample_combined`, or `send_keys` per target.
Therefore any non-empty cycle uses one or two local tmux invocations, never `N`.

Every requested id occurs exactly once in `BatchResult.outcomes`. Spawn, UTF-8, global nonce, or
unrecoverable process-output failures are expanded to `Retry` for all requested ids rather than
returning a partial map.

### 2.3 Probe frame grammar

Each physical invocation gets an unpredictable UUID-v4 nonce rendered as exactly 32 lowercase hex
characters. Request ids are dense scheduler-assigned integers; target text is never printed in a
frame. For each request, the command builder appends standalone argv to the single tmux command
sequence. It uses the exact target-pane form `=<target>:` and never invokes a shell.

Records are ASCII lines with this grammar (`N` is the invocation nonce, `R` the decimal request id,
`P` a `%`-prefixed decimal tmux pane id):

```text
AGENTUM-BATCH/1|N|R|BEGIN
AGENTUM-BATCH/1|N|R|META|P|<pane_current_command>
AGENTUM-BATCH/1|N|R|SCROLLBACK
<raw capture-pane -p -S -100 bytes>
AGENTUM-BATCH/1|N|R|VIEWPORT
<raw capture-pane -p -S 0 bytes>
AGENTUM-BATCH/1|N|R|END|P
AGENTUM-BATCH/1|N|R|CLOSE
```

`BEGIN`, section markers, and `CLOSE` are unconditional `display-message -p` commands so a failed
targeted command remains scoped to its request. `META` and `END` are target-bound
`display-message` calls. Captures retain their existing scrollback/viewport semantics. The parser
requires the exact state order, nonce, known request id, one `META`, one of each section, matching
begin/end pane identities, one close, and no trailing or duplicate record. A marker-shaped line in
captured content is not trusted as data: it makes the containing frame ambiguous and therefore
`Retry`. The parser resynchronizes only at the next expected request's `BEGIN`; if a collision can
plausibly claim another request, both implicated requests retry, while later independently closed
frames remain usable. Unknown ids, nonce mismatch, reordered/truncated records, duplicate sections,
invalid UTF-8, or unmatched frames never produce a `Sample`.

The two target-bound identity reads bracket both captures. A different pane id at `END` is a pane
replacement race and yields `Retry`. A vanished target with no valid identity is provisional until
`finish`; it is never inferred as `Gone` merely from stderr or missing bytes.

### 2.4 Confirmation and action invocation

The optional second invocation uses a fresh nonce and the same unconditional begin/close scoping.
For every provisional absence/malformed probe it reads the exact target's current pane identity:

```text
AGENTUM-BATCH/1|N2|R|CONFIRM-BEGIN
AGENTUM-BATCH/1|N2|R|CONFIRM|P
AGENTUM-BATCH/1|N2|R|CONFIRM-END
```

Resolution is fail closed:

- valid, identity-stable probe -> `Sample`;
- no pane identity in the probe and no authoritative identity in confirmation -> `Gone`;
- a pane existed during any partial probe, a confirmed target now exists, identities differ, or
  either frame is incomplete/ambiguous -> `Retry`;
- process/transport failure or nonce mismatch -> `Retry`, never `Gone`.

Thus a pane that disappears or is replaced during sampling retries; only a target absent at the
authoritative confirmation boundary becomes `Gone`.

An action is admitted to `finish` only when the same cycle produced a complete sample, the shared
session machine found no pane/crash terminal condition, and at least one still-current registration
has a generation-matching pending compact. One action is emitted per target regardless of the
number of contributing sessions. The action command targets the sampled concrete `%pane_id`, not
the user-controlled target, and is guarded inside tmux by both `#{window_active}` and
`#{pane_active}`. Both literal `/compact` and Enter must succeed before an `ACTION|DELIVERED` record
is printed; a missing record, identity change, inactive pane, or command error is `ActionOutcome::Retry`.
The branch is a tmux command-language string (not an OS shell) constructed from the static
adapter-owned compact trigger plus exact pane id; one escaping helper handles tmux command grammar
and it never accepts user text.

The scheduler emits `watchdog.compact` and starts the per-session cooldown only for `Delivered`.
Retry keeps the pending action and does not emit. If several registrations contributed, one command
is delivered but each still-current contributor receives its existing per-session event and
cooldown update. Conflicting commands for one target are not executed; all contributors retry.

### 2.5 Fleet scheduler and registrations

`Watchdog` holds:

```text
remote_tasks: session id -> {registration signature, JoinHandle}
fleet: Arc<tokio::sync::Mutex<FleetState>>
runner: Arc<dyn LocalBatchRunner>
```

`FleetState` contains `next_generation: u64` and
`registrations: HashMap<Uuid, LocalRegistration>`. A registration contains the session id/name,
resolved target, generation, `next_due: tokio::time::Instant`, prior delay, and the extracted
session-machine state: activity, persisted tool, tool candidate, footer hash/change instant,
last successful compact instant, and optional pending compact. Generation allocation is monotonic
and checked; a generation is never reused during the process lifetime.

Reconciliation keeps the current authoritative Running-session query and callback. Host resolution
still defaults a missing row to Local. It performs these changes atomically under the fleet mutex:

- new local registration -> allocate generation, initialize state, set `next_due = now + 1s`, and
  emit the existing `session.started` event;
- unchanged `(session id, local host, resolved target)` -> retain generation, state, and deadline;
- removed, non-Running, deleted, host-kind-changed, or target-changed registration -> remove it and
  its pending action before returning; a replacement gets a new generation and full initial delay;
- remote Running registration -> retain/spawn the existing per-session SSH task using its 3/6s
  cadence and streaming ControlMaster; local registrations never receive a task handle.

`Watchdog::run` uses a reconcile interval plus the earliest local deadline (a `tokio::select!` or
equivalent deadline loop). The first reconcile is immediate. An empty fleet waits for reconciliation,
not a spin timer. No local registration is sampled before its initial one-second delay.

At a local deadline:

1. under the fleet mutex, snapshot due `(session id, generation, target)` registrations and pending
   actions, group them by target, assign dense request ids, then release the mutex;
2. call `runner.probe` once for the unique target set;
3. reacquire the fleet mutex and discard every snapshot whose id/generation/target no longer
   matches; do not call `finish` for a target with no current recipient;
4. inspect accepted samples with the shared crash-first machine without committing effects, remove
   actions from any terminal/crashed target, then call `finish` at most once while retaining the
   fleet commit guard;
5. generation-check again and commit results/effects in the order below; update the next deadline
   from the cycle completion time.

Releasing the lock during the first tmux invocation lets reconciliation retire a registration while
capture is in flight. Reacquiring and checking generation before the optional action invocation is
the authority boundary. The guard remains held through `finish` and all store/event commits, so a
concurrent reconciliation cannot complete removal and then observe an old action or result mutate
state. This establishes the required rule: after reconciliation removal/replacement completes, the
old generation can neither deliver `/compact`, update the store, nor emit an event.

Only registrations that were due and remain current consume a `Sample`; duplicate due registrations
for one target share the bytes but retain independent state/events/deadlines. `Gone` is fanned out
to every current registration for the target because its authoritative target is absent. `Retry`
does not touch tool/activity/cooldown state or durable status; due registrations advance by their
previous effective delay so the next cycle retries without a busy loop.

### 2.6 One shared ordered session machine

Extract the mutable per-session fields and the body of `watch_session` into a private
`SessionMachine`/helper in `agentum-watchdog`; do not duplicate classifiers in `fleet.rs`. Both the
local fleet and the retained remote loop call it. Its commit contract is:

1. `Gone` or matching crash signature: re-read durable status for the intentional-stop guard; an
   already-Stopped row exits silently, otherwise atomically set `Crashed` and clear target, then
   persist/broadcast exactly one `session.crashed` (`pane_exited` for Gone, signature for content);
2. recognize any previously queued local compact delivered in this cycle; emit the existing
   per-session `watchdog.compact` and set cooldown before considering the current context-low text;
3. context low: managed sessions immediately emit the unchanged
   `harness.context_rotation_requested`; remote unmanaged sessions call existing SSH `send_keys`
   and emit `watchdog.compact`; local unmanaged sessions set a generation-bound pending action for
   the next target sample, with no command/event yet;
4. apply the existing two-sample known-tool drift and `session.tool_changed` payload;
5. apply the existing activity transition table, `initial` payloads, input-resolved state, and
   persistence-before-broadcast behavior;
6. compute the next deadline with the existing `next_sample_delay` policy.

The machine receives monotonic `tokio::time::Instant` values from the scheduler so paused-time tests
control footer quieting, cooldown, and due selection. Remote code continues to pass the remote host
kind and immediate compaction sink. `bottom_lines`, `classify_activity`, command canonicalization,
adapter signatures, event construction, and `emit` remain single-source helpers.

### 2.7 Error, resource, and security behavior

- Local runner calls have a bounded timeout; timeout/IO/UTF-8/global framing failure expands to
  `Retry`, logs bounded metadata (nonce/request id/target, never pane contents), and schedules the
  next normal deadline.
- A malformed target cannot donate bytes to a neighbor. The parser only accepts a fully closed
  known frame; ambiguous implicated ids retry, while separately closed later frames survive.
- `Gone` never comes from tmux stderr wording alone. It requires the second exact-target
  confirmation boundary and absence of prior pane identity evidence.
- Targets remain standalone argv after `-t`; user text is never interpolated into a shell. Frame
  records contain only generated nonce/request id, tmux numeric pane id, and the constrained
  foreground command. Captured text is opaque.
- Pending actions are bounded by current registrations (at most one per registration, one command
  per target). Removal and retargeting drop them. No unbounded result/history queue is introduced.
- The optimization does not touch `pipe-pane`, WebSocket streaming, launch/stop routes, or remote
  SSH masters.

## 3. Control and data flow

```text
Store: list Running (5s) ──> Watchdog reconcile ──┬─ SSH registration ─> existing remote task
                                                  └─ LocalRegistration{gen,deadline,state}
                                                                  │ due
                                                                  v
                                              group due sessions by exact target
                                                                  │
                         tmux invocation #1: nonce-framed probe of all unique targets
                                                                  │
                                     strict candidates / provisional Retry
                                                                  │
                   generation filter + pure crash-first inspection + safe action set
                                                                  │
                       tmux invocation #2 (only if needed): confirm Gone / deliver actions
                                                                  │
                                      total Sample | Gone | Retry per target
                                                                  │
                       fanout to current generations through one ordered SessionMachine
                                                                  │
                           Store event/status/tool first ──> broadcast bus; update deadline
```

## 4. Incremental build order

1. **F1 — protocol and runner:** add typed batch transport, two non-shell command builders, strict
   parsers, pane identity/action confirmation, fake subprocess runner, and raw-frame matrix. Keep
   watchdog behavior unchanged while this lands.
2. **F2 — fleet scheduling:** first extract the current ordered body into one session machine, then
   add injectable fleet state/runner, reconcile local registrations, target dedupe/fanout,
   monotonic generations, paused-time deadlines, and queued local compaction. Switch local sessions
   off per-session tasks while the retained remote task calls the same machine.
3. **F3 — compatibility closure:** add crash/tool/activity/harness/cooldown/stale-removal/
   high-cardinality tests, source guards, docs, and isolated QA; remove any helpers made obsolete by
   the extraction. There must be one local polling owner at every committed endpoint.

F1 compiles/tests independently. F2 must not be declared green until the old local polling route is
unreachable and local/remote paths share the extracted machine. F3 is the full compatibility and
workspace gate.

## 5. Verification matrix

| AC | Authoritative evidence |
| --- | --- |
| AC1 | Tmux fake subprocess runner receives 100 unique targets and records one probe command; a healthy action cycle records exactly probe+finish (2), never more. Watchdog fake records the same bound end to end. |
| AC2 | 100 due registrations with repeated target strings produce one request per unique target and independent state/event fanout for every due current registration. |
| AC3 | Table tests cover total `Sample/Gone/Retry`: transport error, malformed/partial output, nonce error, absent confirmation, and pane race. Retry fixtures assert no store/event/machine mutation. |
| AC4 | Parser fixtures inject exact marker-looking text, wrong nonce/id, reordered/duplicate/truncated sections, invalid pane ids, and begin/end identity changes; no bytes cross targets. |
| AC5 | `#[tokio::test(start_paused = true)]` checks initial +1s, local Working/recent +1s, local settled +2s, remote +3/+6s, no early due registration, and completion-based next deadlines. |
| AC6 | Context-low local sample queues without an action invocation/event; the next target batch delivers once, duplicate registrations send once, per-session cooldown/events update, retry remains pending, five-minute expiry re-enables, and managed sessions emit rotation only. |
| AC7 | One ordered scenario asserts crash suppresses compact/tool/activity; context precedes tool/activity; recognized tool changes only on sample two and precedes that sample's activity event; every Unknown/Working/Idle/Awaiting transition and payload is pinned. |
| AC8 | Barrier fake pauses probe; reconcile removes, stops, deletes, or retargets/re-registers before release. Old generation produces no store mutation, event, state update, or action. A removal blocked behind `finish` completes only after the old commit boundary, proving no post-removal effect. |
| AC9 | Gone for an already-Stopped row is silent; otherwise one Crashed+cleared-target mutation and one persisted/broadcast `session.crashed` with `pane_exited`; confirm-present race is Retry and unchanged. |
| AC10 | Recording fake distinguishes local runner from SSH sampler. Remote Running sessions still call `ssh::sample_pane`, preserve `SshMux::Streaming`, never enter a local request, and retain 3/6s tests. |
| AC11 | The focused tmux/watchdog suites contain all fixtures above plus 100-session load. Harness `verify.sh` runs both crates, non-desktop backend workspace tests, fmt, parser/source guards, and diff hygiene; isolated `qa.sh` repeats the 100-session authoritative scenario and optionally runs a real local tmux smoke when available. |

The source guard must fail if the local branch calls `ssh::sample_pane`/`capture_pane_sample_combined`
from a per-session loop or if reconciliation spawns a local `watch_session`. The known desktop
Sherpa dynamic-library and missing-Vite environment blockers are not part of this backend slice;
the backend workspace command excludes `agentum-desktop` when that dylib is unavailable and records
the limitation explicitly.

## 6. Acceptance-criterion coverage

All AC1–AC11 have a single implementation owner and executable test seam. There are no open
architecture or product decisions. The design preserves the server callback, remote SSH execution,
adapter policy, event schemas/order, intentional-stop behavior, and push streaming while replacing
the only N-scaling local polling owner.
