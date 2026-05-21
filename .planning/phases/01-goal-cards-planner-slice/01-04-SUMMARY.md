---
phase: 01-goal-cards-planner-slice
plan: "04"
subsystem: watchdog
tags: [watchdog, broadcast-bus, goal-status, planner-auto-stop, reconciler]
dependency_graph:
  requires: [01-01, 01-03]
  provides: [goal-status-auto-progression, planner-auto-stop]
  affects: []
tech_stack:
  added: []
  patterns: [run_goal_reconciler, handle_board_event, planner_stopped HashSet, depth-1 guard]
key_files:
  created: []
  modified:
    - crates/agentum-watchdog/src/lib.rs
    - crates/agentum-watchdog/Cargo.toml
    - crates/agentum-server/src/lib.rs
    - crates/agentum-store/src/lib.rs
decisions:
  - "Reconciler runs as a separate spawned task (run_goal_reconciler), not folded into the per-session watchdog loop — keeps reconcile concerns out of the pane-classifier path"
  - "planner_stopped: HashSet<i64> tracks per-daemon-lifetime auto-stop firings so the same planner is never stopped twice (D-07 idempotency)"
  - "Depth-1 invariant guard: if a goal row itself has parent_goal_id set, log warn and skip recompute (CONTEXT D-03) — refuses to cascade writes up an unbounded tree"
  - "goal.planner.first_child event emitted BEFORE the tmux stop call so test fixtures without a real tmux still observe the event"
  - "Bus lag (RecvError::Lagged) treated as benign — next event triggers a full recompute via max_child_status_rank so the goal converges (T-04-02)"
metrics:
  completed: "2026-05-21"
  tasks: 2
  files_changed: 4
---

# Phase 01 Plan 04: Goal-Status Auto-Progression + Planner Auto-Stop Summary

A tokio task subscribed to the broadcast bus that watches every `board.created` / `board.updated` / `board.deleted` event touching a row with `parent_goal_id IS NOT NULL`, recomputes `max(child statuses)` for the parent goal, and PATCHes the goal card if the rank changed. Also handles D-07: when the first child arrives for a goal, emit `goal.planner.first_child` and stop the planner session bound via `session.card_id = goal.id`.

## What Was Built

### Store-Layer Helpers (1063a9d)

- `get_session_by_card_id(card_id: i64)` — looks up the planner session bound to a goal card. Used by the auto-stop branch.
- `status_rank(s)` / `rank_to_status(r)` — string ↔ ordinal conversion for ranking child statuses (`todo=0`, `doing=1`, `awaiting=2`, `done=3`). Matches `max_child_status_rank`'s rank space from 01-01.

### Reconciler Task (2f133e1)

`pub async fn run_goal_reconciler(store: Arc<Store>, bus: broadcast::Sender<Event>)` in `agentum-watchdog::lib`:

1. Subscribes to the bus via `bus.subscribe()`.
2. Filters for `board.created` / `board.updated` / `board.deleted` event kinds.
3. Dispatches each to `handle_board_event`.
4. Bus-lag is logged as a warn and the loop continues — convergence is event-driven, not stream-position-driven.

`handle_board_event` flow:

1. Extract `parent_goal_id` from the event payload (or, for deletes, from the cached payload).
2. Read the goal row. If gone (concurrent delete), exit early.
3. Depth-1 guard: if the goal row has `parent_goal_id` itself, log warn + skip.
4. **D-07 first-child detection**: only on `board.created`, only if the `planner_stopped` HashSet didn't already contain `goal_id` — emit `goal.planner.first_child` then call `agentum_tmux::graceful_stop` on the planner session (if running).
5. **Status recompute**: read `max_child_status_rank(goal_id)`, convert to a status string, compare against `goal.status`. If different, `patch_board_item` with the new status and emit `goal.status.changed { goal_id, from, to }`.

### Server Wiring

`crates/agentum-server/src/lib.rs::serve` now spawns `run_goal_reconciler` alongside the existing watchdog task. The reconciler shares the same `Store` + bus as the rest of the server.

## Tests

11 new `#[tokio::test]` tests in `agentum-watchdog::tests`:

- `reconciler_promotes_goal_when_first_child_moves_to_doing` — goal at `todo` + 1 child → child moves to `doing` → goal becomes `doing`, `goal.status.changed` event fires with `from: "todo", to: "doing"`.
- `reconciler_demotes_goal_when_last_doing_child_returns_to_todo` — symmetric demotion path.
- `reconciler_promotes_goal_to_done_when_all_children_done` — goal becomes `done` once max-rank child is `done`.
- `reconciler_no_event_when_status_unchanged` — patch a child with same status, no `goal.status.changed`.
- `reconciler_skips_when_goal_has_parent_goal_id` — depth-1 guard test.
- `reconciler_handles_concurrent_goal_delete_gracefully` — child PATCH arrives after goal already deleted; no panic.
- `reconciler_recovers_from_bus_lag` — fill the channel past capacity, verify reconciler resumes on next event.
- `reconciler_firstchild_event_emitted_before_planner_stop` — order of emit vs tmux call.
- `reconciler_firstchild_idempotent_per_daemon_lifetime` — repeated `board.created` events for children of the same goal only fire `goal.planner.first_child` once.
- `reconciler_handles_board_deleted_event` — child delete triggers recompute (max status of remaining children).
- `reconciler_ignores_unrelated_event_kinds` — `session.started` etc. don't drive the loop.

Plus the existing 8 watchdog activity-classifier tests (untouched).

## Deviations from Plan

**Orchestrator finished the work after the parallel executor agent was halted mid-clippy.** The executor agent had committed `1063a9d` (store helpers) and left the implementation + tests uncommitted in main's working tree while iterating on a clippy `manual_async_fn` lint on the `make_goal_item` test helper. The orchestrator:

1. Verified the implementation compiled + passed all 11 new tests.
2. Fixed the clippy `manual_async_fn` warning by inlining `make_goal_item` as a plain `async fn` (one-line edit).
3. Committed the rest as `feat(01-04): goal-status auto-progression subscriber + planner auto-stop` (2f133e1).

This is the consequence of Claude Code's `isolation="worktree"` failing to actually isolate the agent — see the Wave 3 incident note for the broader context.

**Tempfile added as `[dev-dependencies]`** in `agentum-watchdog/Cargo.toml` for the SQLite tempdir setup in the new test helpers — same pattern used by `agentum-store`'s tests.

## Forward References (v2 Deferred Work)

- **Multi-daemon planner_stopped persistence**: `planner_stopped` is a per-process HashSet. A daemon restart will re-fire `goal.planner.first_child` once per goal that has live children at boot. Persistence is deferred — the current behaviour is idempotent for downstream consumers (tmux session is already stopped, so the second `graceful_stop` is a no-op).
- **Deep-tree status propagation**: depth-2+ goal trees (a goal whose child is itself a goal) are explicitly rejected by the depth-1 guard. v2 will lift this once the UI supports parent navigation across multiple levels.
- **Event replay on subscriber reconnect**: the reconciler reads from a tokio broadcast channel that drops messages on lag. The convergence story (next event triggers a fresh `max_child_status_rank` read) is correct but not optimal — a real WAL-backed subscriber would catch up on missed events.

## Self-Check: PASSED
