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

## F2 — `phase-chip-live` (AC 4–6) — DONE

**What changed** (architecture §4/§5):

- `crates/agentum-server/src/routes/worktrees.rs` — the `scan_git_worktrees`
  row body extracted into a pure `detected_row(repo_id, idx, path, branch,
  &registry)` (behavior-identical) and the row gained the three camelCase
  keys `trackerProvider`/`trackerUrl`/`trackerPhase` from the registry meta.
  Registry `Worktree` struct serde shape UNTOUCHED (alias-free rule holds by
  construction).
- `crates/agentum-desktop/ui/src/shared/types.ts` — `Worktree` gains the
  three OPTIONAL fields (`trackerPhase` as the 5-value wire union).
- NEW `ui/src/lib/tracker-phase.ts` — pure model: `TrackerPhaseWire`,
  `parseTrackerPhaseWire`, `trackerEventFromFrame` (both kinds; malformed →
  null), `matchEventToWorktree` (id first, trackerUrl fallback),
  `deriveTrackerChip` (persisted + live overlay → chip | null).
- NEW `ui/src/lib/tracker-phase.test.ts` — jsdom-free vitest (12 tests).
- NEW `ui/src/store/slices/tracker-phase.ts` —
  `trackerLiveByWorktreeId` + `patchTrackerPhase` (attention:=false) /
  `setTrackerAttention` / `clearTrackerLive`; no-op-on-equal writes.
  Registered in `store/index.ts` + `store/types.ts`.
- NEW `ui/src/hooks/useTrackerPhaseSync.ts` — `subscribeServerEvents` (shared
  socket) → parse → join → patch; clears the slice on unmount.
- NEW `ui/src/components/sidebar/TrackerPhaseChip.tsx` — thin badge
  (MetadataStatusBadge styling), attention = rose/alert variant, null render
  when unbound.
- TOUCH `ui/src/components/sidebar/WorktreeCardMeta.tsx` — hover's issue
  badge row renders the chip beside `IssueStateBadge`; the hover props gain
  optional `worktreeId`/`trackerPhase`.
- TOUCH `ui/src/components/sidebar/WorktreeCard.tsx` — passes
  `worktree.id`/`worktree.trackerPhase` into the details hover.
- TOUCH `ui/src/App.tsx` — `useTrackerPhaseSync()` mounted beside
  `useServerWorktreeActivity()`.

**Gate:** `cargo test -p agentum-server --lib routes::worktrees` → 16/16
(incl. new `detected_row_exposes_tracker_keys_bound_and_null_unbound`);
`bun run build` ✓ (3m11s); `bunx vitest run src/lib/tracker-phase.test.ts` →
12/12. Full-suite vitest baseline recorded below when the background run
finishes (pre-existing failures are a known baseline per project memory).

**Deviations:** `detected_row` extraction (pure fn) was not named by the
architecture — added so the wire shape is hermetically testable without
git/host plumbing; JSON emitted is byte-identical.

**Full-suite vitest baseline** (recorded during F2, includes F2 code):
39 failed files / 138 failed tests / 5900 passed (749 files, 6038 tests) —
the known pre-existing baseline (project memory: ~38 failing files). Spot
check: `WorktreeCardMeta.test.tsx > includes branch identity before metadata
details` fails because the test passes a `review` prop the component does not
have (asserts 'PR #456' renders; it never does) — pre-existing, untouched by
the chip change (the fixture passes no `worktreeId`, so the chip branch never
renders).

## F3 — `board-live-refresh` (AC 7) — DONE

**What changed** (architecture §5 F3):

- NEW `ui/src/components/github-project/project-view-live-refresh.ts` — pure
  trailing-edge coalescer: `PROJECT_VIEW_EVENT_REFETCH_COALESCE_MS = 2_000`
  (named constant), `isTrackerEventKind` (`kind.startsWith('tracker.')`),
  `coalesceEvent(state, nowMs) -> {state, schedule}` /
  `coalesceFire(state)` reducers — no timers in the model.
- NEW `ui/src/components/github-project/project-view-live-refresh.test.ts` —
  jsdom-free vitest (6 tests): burst ⇒ ONE fire, post-window event ⇒ second
  fire, non-tracker kinds ignored, stale-fire no-op, constant pinned.
- NEW `ui/src/components/github-project/use-project-view-live-refresh.ts` —
  hook: shared `subscribeServerEvents` socket, kind filter, `setTimeout`
  drive, latest-callback ref; unmount unsubscribes + clears the pending timer
  (hidden/inactive views fetch nothing).
- TOUCH `ui/src/components/github-project/ProjectViewWrapper.tsx` — ONE
  `useProjectViewLiveRefresh(liveRefetch)` call; `liveRefetch` is a
  `useCallback` built from the same values as the auto-fetch effect
  (`activeProject` + `lastViewByProject` + `appliedQueryByView`) with
  `force: true`.

**Gate:** `bun run build` ✓ (1m28s); coalescer vitest 6/6 (ran green in the
combined targeted run); `grep -rn setInterval src/components/github-project/`
→ 0.

**Deviations:** none.
