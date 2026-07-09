# Tasks — Spec 014: Live auto-status + agent-attention signal

Developer log, one section per gated slice. Build worktree:
`feat/014-live-auto-status` off `origin/develop` @ v0.68.0 (`27c7c132`).

## F1 — `tracker-phase-event` (AC 1–3) — DONE

**What changed** (architecture §1/§2, Q1 = emit inside the seam, Q5 = two kinds):

- `crates/agentum-server/src/task_sink.rs`
  - New `TrackerPhase::wire_str()` (the canonical wire table now lives on the
    seam type).
  - New `pub struct TrackerEmit<'a> { bus, worktree_id: Option<&str> }` —
    REQUIRED param on both seam fns, so "transition without emitting" is
    unrepresentable.
  - `apply_tracker_transition` renamed body → private `transition_inner`;
    public wrapper emits `tracker.phase_changed`
    `{worktree_id, provider, phase, tracker_url}` on — and only on —
    `Ok(TransitionResult::Applied)` via `let _ = emit.bus.send(...)`
    (fire-and-forget, no await on the bus).
  - `apply_blocked_transition` same shape → private `blocked_inner`; emits
    `tracker.blocked` `{worktree_id, provider, tracker_url, reason:
    gate_label}` on Applied.
- `crates/agentum-server/src/tracker_sync.rs`
  - `tracker_phase_wire` is now a thin delegate to `TrackerPhase::wire_str`
    (round-trip test untouched, stays green).
  - `react_to_session_start` + reactor loop thread `&bus`; passes
    `worktree_id: Some(&worktree.id)`.
  - `drive_and_persist` / `poll_pr_lifecycle_once` gain a `bus` param;
    `run_pr_merge_poller(store, bus)` signature change.
- `crates/agentum-server/src/lib.rs` — poller spawn passes `bus.clone()`.
- `crates/agentum-server/src/harness/drive.rs` — `transition_tracker` and the
  blocked-path caller pass `TrackerEmit { bus: &state.bus, worktree_id: None }`.
- `crates/agentum-server/src/routes/harness.rs` — `ensure_spec_and_plan` gains
  a `bus` param (threaded from `state.bus` at both route callers); tests use a
  throwaway `test_bus()`.
- `crates/agentum-server/src/routes/board_goals.rs`,
  `crates/agentum-server/src/routes/mcp.rs` — pass
  `TrackerEmit { bus: &state.bus, worktree_id: None }`.

**Tests**

- NEW `task_sink::tests::applied_transition_emits_phase_changed_on_bus`
  (hermetic board arm; asserts kind, full payload, exactly ONE event).
- NEW `task_sink::tests::skipped_transition_emits_nothing` (board unknown-key
  skip + github no-url pipeline skip + github no-url blocked skip ⇒ bus empty).
- All existing seam-calling tests updated mechanically with a throwaway
  `broadcast::channel(8)`.

**Gate:** `cargo test -p agentum-server --lib` → 644 passed / 0 failed /
5 ignored. `cargo fmt --all` clean (fallout in 2 untouched files committed
separately as `2cbd9223`).

**Invariants checked:** `next_phase_write` untouched; no new `TrackerPhase`
variant; Skipped/Err emit nothing; no transition awaits the bus; launch path
untouched.

**Deviations:** none.
