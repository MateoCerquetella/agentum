---
schema: 1
id: SPC-139ZDPPE1FMKBK3WFXHEM1KKBC
revision: 1
title: Local Watchdog Fleet Scheduler
source: legacy-import:ai/specs/029-local-watchdog-fleet-scheduler/spec.md@sha256:c9768dfbbf95d76bbbbfc0cfeef597f7b79ebad7f08b12f36e3d2992a9968671
---

# Local Watchdog Fleet Scheduler

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec 029 — Local Watchdog Fleet Scheduler
>
> - **Number:** 029
> - **Status:** PM               <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `crates/agentum-watchdog`, `crates/agentum-tmux`
> - **Author:** Codex
> - **Date:** 2026-07-23
>
> ## Problem
>
> An operator supervising many local agents pays one independent watchdog timer and one tmux client
> spawn per session on every due sample. At 100 running sessions, routine health monitoring creates a
> continuous process/socket storm even when panes are quiet, making the control plane itself compete
> with the agents it is meant to observe.
>
> ## Goal
>
> An operator can supervise a 100-session local fleet through one shared watchdog schedule whose
> healthy cycles use a constant number of local tmux invocations without changing lifecycle events,
> sampling cadence, compaction behavior, or remote SSH semantics.
>
> ## Users / personas
>
> An engineer running dozens to hundreds of simultaneous local coding-agent sessions feels this when
> CPU usage and tmux client churn scale with historical activity instead of with one fleet-level
> health cycle; the same engineer expects remote sessions to retain their deliberately slower SSH
> sampling behavior.
>
> ## Acceptance criteria
>
> 1. Reconciling and sampling 100 due local running sessions uses **at most two local tmux process
>    invocations in a healthy scheduler cycle**, independent of session count; an authoritative fake
>    runner records the exact invocation count and command requests.
> 2. The local batch interface samples each unique tmux target at most once per cycle and fans the
>    result out to every still-current session registration for that target; 100 sessions containing
>    duplicate targets produce one request per unique target without losing per-session state or
>    events.
> 3. Each requested target returns exactly one typed outcome: `Sample` with scrollback, viewport,
>    foreground command, and pane identity; `Gone` only when the authoritative target no longer
>    exists; or `Retry` for transport failure, malformed/incomplete output, nonce mismatch, or an
>    indeterminate pane race. `Retry` never marks a session crashed or applies partial sample data.
> 4. Batch output is framed by a per-invocation nonce and per-target pane identity. Delimiter text in
>    captured pane contents, malformed frames, reordered/truncated output, and a target whose pane is
>    replaced during sampling cannot attribute one pane's data to another target; the parser returns
>    `Retry` for every ambiguous target.
> 5. The fleet scheduler preserves the existing effective cadence: local active/recently-changing
>    panes are due after 1 second and settled quiet panes after 2 seconds; remote active panes remain
>    3 seconds and settled quiet panes 6 seconds; no session is sampled before its initial base delay.
>    Deterministic paused-time tests assert due-set selection and next-deadline updates.
> 6. Local context-low compaction is placed on a pending-action queue and executed by the next local
>    batch rather than spawning an immediate per-session tmux client. Shared-target actions are
>    deduplicated, the five-minute per-session cooldown remains enforced, and harness-managed
>    sessions continue to emit `harness.context_rotation_requested` instead of receiving `/compact`.
> 7. For every accepted sample, processing order and public effects remain compatible: pane-gone or
>    crash handling wins before compaction, tool drift, and activity; context-low handling precedes
>    tool/activity transitions; a two-sample recognized tool change precedes any activity event from
>    that sample; initial/working/finished/awaiting-input/input-resolved payload semantics remain
>    unchanged.
> 8. A session removed from reconciliation, changed away from Running, deleted, or re-registered with
>    a different target invalidates its prior generation and all queued sample/action results. After
>    removal completes, an old local batch result cannot mutate status/tool state, emit an event, or
>    deliver a queued compaction for that registration.
> 9. Local `Gone` preserves the intentional-stop guard: an already-Stopped session exits silently;
>    otherwise its durable status becomes Crashed, its target is cleared, and exactly one
>    `session.crashed` event with `pane_exited` is persisted/broadcast. A pane race classified as
>    `Retry` leaves durable state and events unchanged for the next due cycle.
> 10. Remote SSH sessions preserve the current host-aware sampling path, streaming ControlMaster,
>     `Sample`/gone/error behavior, and 3/6-second cadence; the local fleet change does not combine
>     different SSH hosts, add remote invocations, or move sampling onto the interactive SSH master.
> 11. Fake-runner and parser tests cover 100-session invocation bounds, shared-target deduplication,
>     active/quiet cadence, malformed/colliding frames, pane disappearance/replacement races, queued
>     and cooldown-limited compaction, crash precedence, two-sample tool drift, all activity
>     transitions, remote-path preservation, and stale-result rejection after removal.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** one fleet scheduler for local watchdog registrations; a local multi-target tmux batch
>   runner/parser; typed `Sample`/`Gone`/`Retry` results; target deduplication; per-registration
>   deadlines, state, and generations; next-batch local compaction; deterministic fake-clock and
>   fake-runner evidence.
> - **Out:** changing SSH batching or cadence; changing `pipe-pane` WebSocket streaming; changing
>   adapter signatures, public event schemas, database schemas, session launch/stop routes, tmux
>   ownership, transcript observation, or desktop/TUI code; introducing a second watchdog loop.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `Watchdog::reconcile_once` and `Watchdog::run`
>   (`crates/agentum-watchdog/src/lib.rs`) already own the authoritative five-second Running-session
>   query and registration lifecycle; retain that one orchestrator and its server-owned hook.
> - `watch_session`, `next_sample_delay`, `classify_activity`, tool-drift debounce, compaction
>   cooldown, and ordered event emission (`crates/agentum-watchdog/src/lib.rs`) already define the
>   compatibility state machine; extract/reuse it rather than creating parallel classifiers.
> - `agentum_tmux::ssh::sample_pane` and `PaneSample`
>   (`crates/agentum-tmux/src/ssh/tmux_ops.rs`) already provide the one-target local/SSH contract and
>   preserve remote execution on the streaming SSH mux; the SSH branch remains the compatibility
>   path.
> - `capture_pane_sample_combined`, exact tmux target forms, and target-gone stderr classification
>   (`crates/agentum-tmux/src/lib.rs`) already establish single-target capture semantics and the
>   rare-error authoritative existence check.
> - `ToolAdapter::{compact_trigger, crash_signatures, busy_signature,
>   awaiting_input_signatures}` (`crates/agentum-executor`) remains the only tool-specific policy.
>
> ### Build new
>
> - An injectable local tmux batch-runner interface that accepts unique target requests plus queued
>   actions, performs the fleet work in no more than two healthy-cycle tmux invocations, and returns
>   typed per-target outcomes.
> - A nonce- and pane-identity-framed parser that fails closed to `Retry` for ambiguous, malformed,
>   incomplete, or raced output and never silently cross-attributes captures.
> - A single local scheduler owning registration generation, next deadline, prior activity/tool/hash
>   state, compaction cooldown, and pending actions, while retaining the existing remote execution
>   path and public watchdog constructor behavior.
> - Deterministic fake clock/runner fixtures and high-cardinality regressions for the complete
>   transport, cadence, state-machine, removal, and invocation-bound contract.
>
> ## Risks & invariants
>
> - **No stale authority:** reconcile removal or replacement must invalidate in-flight samples and
>   pending actions before either can touch the store or bus.
> - **No cross-pane attribution:** target names and pane contents are untrusted framing inputs; batch
>   parsing must bind nonce, requested target, and observed pane identity and fail closed.
> - **No command injection:** preserve argv-safe/exact-target tmux construction; do not interpolate
>   user-controlled targets or captured text into an unquoted shell program.
> - **Event compatibility:** retain crash/compaction/tool/activity precedence, initial payloads,
>   persistence-before-broadcast behavior, and intentional-stop suppression.
> - **One scheduler:** replace local per-session polling rather than leaving it alive beside the new
>   fleet loop; remote tasks may remain per-session only where required to preserve SSH behavior.
> - **Push streaming stays sacred:** this optimization applies only to watchdog classification
>   samples and must not replace or poll the `pipe-pane` byte stream used by clients.
> - **Bounded work:** target deduplication and pending-action deduplication must prevent duplicate
>   commands; one slow or malformed target must not corrupt other framed results.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:** `local-tmux-batch-protocol` (typed framed transport/parser),
>   `local-watchdog-fleet-scheduler` (deadlines, dedupe, generations, queued actions), and
>   `watchdog-fleet-behavior-compatibility` (state-machine/event/remote preservation and 100-session
>   invocation proof).
> - **`verify.sh` asserts:** focused `agentum-tmux` batch parser/runner tests; focused
>   `agentum-watchdog` scheduler/state-machine tests with paused time and an authoritative invocation
>   counter; non-desktop backend workspace tests; formatting, source guard against a local
>   per-session sample loop, and diff hygiene.
> - **`qa.sh` asserts:** isolated backend-only 100-session fake-runner scenario plus an optional real
>   local tmux smoke fixture; no browser surface or desktop dependency is required.
>
> ## Open questions
>
> - None. The local-only batching boundary, typed outcomes, two-invocation ceiling, next-batch
>   compaction, cadence/event compatibility, stale-result rule, and required test matrix are fixed by
>   the intake.
