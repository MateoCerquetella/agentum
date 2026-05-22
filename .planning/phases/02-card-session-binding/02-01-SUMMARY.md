---
phase: 02-card-session-binding
plan: 01
subsystem: database
tags: [sqlx, sqlite, transactions, binding, card-session, store]

# Dependency graph
requires:
  - phase: 01-goal-cards-planner-slice
    provides: "Session.card_id, NewSession.card_id, BoardItem.session_id columns + migration 0015, add_board_link transactional template, BoardPatch double-Option pattern"
provides:
  - "Store::claim_card — atomic INSERT session + bind card in one sqlx transaction"
  - "Store::transfer_card_binding — atomic rebind/unbind of card-session link in one sqlx transaction"
  - "BoardPatch.session_id triple-state deserialization contract locked by regression test"
  - "BoardPatch: Clone contract locked by const-eval static-assert"
affects:
  - "02-02 (route handler will call claim_card from PATCH→doing auto-spawn path)"
  - "02-03 (PATCH handler wiring calls transfer_card_binding + depends on BoardPatch: Clone)"
  - "02-04 (session lifecycle events may query card binding state)"

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "sqlx transaction shape: pool.begin() → operations on &mut *tx → tx.commit() → reload from pool"
    - "AlreadyExists returned for card-already-bound conflicts (maps to HTTP 409 via existing From impl)"
    - "Idempotent unbind: transfer_card_binding(card, None) on unbound card returns Ok(item)"

key-files:
  created: []
  modified:
    - crates/agentum-store/src/lib.rs
    - crates/agentum-core/src/lib.rs

key-decisions:
  - "claim_card overrides new.card_id = Some(card_id) unconditionally — the binding is what makes this 'claim' and both rows must agree"
  - "Reload both rows via get_board_item + get_session_by_id after tx.commit — returns persisted state, not in-memory snapshot"
  - "transfer_card_binding: old session's card_id is cleared even on unbind (None target) for clean state"
  - "const-eval static-assert placed at module level inside mod tests (not inside a fn) so it fires every cargo build, not just test runs"
  - "Tasks 1+2 committed together because claim_card and transfer_card_binding share test helper functions and form one coherent store layer addition"

patterns-established:
  - "Transactional binding pattern: begin → check-existence → check-conflict → mutate-both-sides → commit → reload"
  - "Conflict-on-already-bound: AlreadyExists(format!) mirrors HTTP 409 pattern established in claim_board_item"

requirements-completed: [BIND-01, BIND-06]

# Metrics
duration: 12min
completed: 2026-05-22
---

# Phase 2 Plan 01: Store Transactional Binding Foundation Summary

**Two atomic store primitives — `claim_card` (INSERT+bind) and `transfer_card_binding` (rebind/unbind) — plus regression tests locking the BoardPatch.session_id triple-state contract and BoardPatch: Clone trait for downstream plan 02-03**

## Performance

- **Duration:** 12 min
- **Started:** 2026-05-22T18:03:04Z
- **Completed:** 2026-05-22T18:15:00Z
- **Tasks:** 3 (Tasks 1+2 combined into one commit; Task 3 separate)
- **Files modified:** 2

## Accomplishments

- `Store::claim_card`: atomically creates a session and binds it to a board card in a single sqlx transaction; returns `AlreadyExists` (HTTP 409) if card already bound or session name collides; rolls back card update if session INSERT fails
- `Store::transfer_card_binding`: atomically rebinds or unbinds a card's session link (3-step: clear old session's card_id, set new session's card_id, update card's session_id); returns `AlreadyExists` if target session is bound to a different card
- 10 new unit tests: 4 for `claim_card` (happy path, already-bound, no-card, rollback) + 6 for `transfer_card_binding` (unbind, rebind, conflict, no-card, no-session, idempotent)
- 1 regression test pinning `BoardPatch.session_id` triple-state serde contract (omitted=None, null=Some(None), value=Some(Some(_)))
- 1 const-eval static-assert pinning `BoardPatch: Clone` at compile time

## Task Commits

1. **Tasks 1+2: claim_card and transfer_card_binding** - `98927b1` (feat)
2. **Task 3: session_id contract test + Clone static-assert** - `1ff08f8` (test)

## Files Created/Modified

- `crates/agentum-store/src/lib.rs` — Added `Store::claim_card` and `Store::transfer_card_binding` methods with 10 unit tests and shared test helpers
- `crates/agentum-core/src/lib.rs` — Added `board_patch_session_id_distinguishes_omitted_explicit_null_and_value` test + const-eval static-assert for `BoardPatch: Clone`

## Decisions Made

- Tasks 1 and 2 committed together: both methods share test helper functions (`make_card`, `make_card_new_session`) and form one coherent store layer feature; separating would have required duplicating or later adding helpers
- Worktree required a `git reset --hard 81fee16` at startup because the agent branch was initialized from the pre-Phase-1 codebase (v0.8.2 release tag); this was handled automatically by the worktree_branch_check protocol

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Worktree base commit mismatch required reset**
- **Found during:** Initialization
- **Issue:** The worktree HEAD (71550d9) was not a descendant of 81fee16 (the required base with Phase 1 code). The worktree's `agentum-core` and `agentum-store` lacked `card_id`, `parent_goal_id`, migration 0015, and all Phase 1 store methods.
- **Fix:** `git reset --hard 81fee16` per the `worktree_branch_check` protocol (ACTUAL_BASE != 81fee16 trigger)
- **Files modified:** All Phase 1 files restored to worktree
- **Verification:** Migration 0015 present; `Session.card_id`, `NewSession.card_id`, `BoardItem.parent_goal_id`, `Store::add_board_link` all confirmed in worktree
- **Committed in:** N/A (reset restores history; no additional commit needed)

---

**Total deviations:** 1 auto-fixed (1 blocking — worktree base mismatch)
**Impact on plan:** The reset is required setup; no scope creep. All plan work proceeded as specified after reset.

## Issues Encountered

- Worktree base mismatch: resolved via `git reset --hard 81fee16` (worktree_branch_check protocol)

## Known Stubs

None — this plan adds store-layer primitives only; no UI or data flows to wire.

## Threat Surface Scan

No new network endpoints or auth paths introduced. The two new store methods are pure SQLite operations called only from auth-gated route handlers (plan 02-03). No new trust boundary surface.

## Self-Check

**Verification commands run post-implementation:**

```
cargo test -p agentum-store --lib claim_card          → 4 passed
cargo test -p agentum-store --lib transfer_card_binding → 6 passed
cargo test -p agentum-store --lib                      → 29 passed
cargo test -p agentum-core --lib board_patch_session_id_distinguishes → 1 passed
cargo test -p agentum-core --lib                       → 38 passed
cargo build -p agentum-core                            → OK (static-assert compiles)
cargo clippy --workspace --all-targets -- -D warnings  → clean
cargo fmt --all -- --check                             → clean
cargo check --workspace --all-targets                  → clean
```

## Self-Check: PASSED

- `crates/agentum-store/src/lib.rs` contains `pub async fn claim_card(`: YES
- `crates/agentum-store/src/lib.rs` contains `pub async fn transfer_card_binding(`: YES
- Both methods contain `self.pool.begin()`: YES (2 occurrences)
- Zero `sqlx::query!` macros: YES (0 matches)
- `crates/agentum-core/src/lib.rs` contains `fn board_patch_session_id_distinguishes_omitted_explicit_null_and_value`: YES
- `crates/agentum-core/src/lib.rs` contains `_assert_clone::<`: YES
- `BoardPatch` derives `Clone`: YES (`#[derive(Debug, Clone, Default, Serialize, Deserialize)]`)
- Commits 98927b1 and 1ff08f8 exist in git log: YES

## Next Phase Readiness

- Plan 02-02 (route PATCH handler) can call `state.store.claim_card(card_id, new_session)` with the documented contract
- Plan 02-03 (PATCH session_id wiring) can call `state.store.transfer_card_binding(card_id, session_uuid)` and use `patch.clone()` without conditional fallback
- All 10 new store tests + 1 core regression test are green; workspace clean

---
*Phase: 02-card-session-binding*
*Completed: 2026-05-22*
