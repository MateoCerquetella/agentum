---
phase: 02-card-session-binding
plan: 04
subsystem: watchdog
tags: [watchdog, bus-subscriber, comment-bridge, system-comments, tdd]
requires:
  - phase: 02-card-session-binding
    plan: 03
    provides: "PATCH handler sets card.session_id; session.card_id written via claim_card"
provides:
  - "pub async fn run_session_comment_bridge in agentum-watchdog"
  - "tokio::spawn(run_session_comment_bridge) in agentum-server::serve"
  - "[system] comment thread on bound non-goal cards for agent.awaiting_input, agent.finished, session.crashed"
affects: 02-05, 02-06

tech-stack:
  added: []
  patterns:
    - "Bus-subscriber task: subscribe → loop { recv().await → filter → dispatch } mirrors run_goal_reconciler shape"
    - "In-memory HashMap<Uuid, &'static str> dedupe: skip identical back-to-back (session_id, kind) pairs"
    - "80-char signature cap via chars().take(80) prevents comment-body amplification"
    - "RecvError::Lagged → tracing::warn! + continue; no resync (D-09)"

key-files:
  created: []
  modified:
    - crates/agentum-watchdog/src/lib.rs
    - crates/agentum-server/src/lib.rs

key-decisions:
  - "Used get_session_by_id (not get_session) — the correct Store method name"
  - "Goal-card filter via card.lbl.as_deref() == Some(\"goal\") on the board_items row"
  - "Dedupe key is &'static str (awaiting_input / finished / crashed) not the full event kind string, to avoid String allocation per event"
  - "bus-lag test uses yield_now() before flood so bridge is subscribed before overflow — ensures Lagged fires on bridge receiver"

metrics:
  duration: 20min
  completed: 2026-05-22
  tasks: 2
  files_modified: 2
---

# Phase 02 Plan 04: Session Comment Bridge Summary

**`run_session_comment_bridge` — watchdog bus-subscriber that converts agent lifecycle events into `[system]` comments on bound cards.**

## What Landed

### Task 1: `pub async fn run_session_comment_bridge` in `crates/agentum-watchdog/src/lib.rs`

Added two functions after the existing `run_goal_reconciler` block:

**`run_session_comment_bridge(store, bus)`** — the public entry point:
- Subscribes to the broadcast bus
- Filters to `agent.awaiting_input`, `agent.finished`, `session.crashed` only (all other kinds hit `continue`)
- On `RecvError::Lagged(n)` logs `tracing::warn!(lagged = n, "session_comment_bridge: bus lagged; will resume on next event")` and continues — no resync (D-09)
- On `RecvError::Closed` exits cleanly

**`handle_session_event(store, ev, kind, last_kind)`** — private dispatch helper:
1. Guards on `ev.session_id` being `Some`
2. Resolves session via `store.get_session_by_id(session_id)` — returns early on missing (concurrent delete, benign)
3. Guards on `session.card_id` being `Some`
4. Fetches the card via `store.get_board_item(card_id)` — returns early on missing (card deleted, benign)
5. Goal-card filter: `if card.lbl.as_deref() == Some("goal") { return Ok(()); }` — skips planner sessions (D-08)
6. Dedupe: `if last_kind.get(&session_id) == Some(&kind) { return Ok(()); }` then inserts — skips identical back-to-back (D-07)
7. Composes body:
   - `"awaiting_input"` → `"[system] agent awaiting input"`
   - `"finished"` → `"[system] agent finished"`
   - `"crashed"` → `"[system] session crashed: {sig}"` where `sig` is `ev.payload["signature"]` trimmed to 80 chars (chars-based); missing/empty signature substitutes literal `"unknown"` (D-06, T-02-15)
8. Calls `store.create_board_comment(card_id, NewBoardComment { author: "system", body })` directly (no HTTP hop, per codebase pattern)

**11 TDD tests added** covering all spec behaviors:

| Test | Behavior |
|------|----------|
| `bridge_inserts_awaiting_input_comment_on_bound_non_goal_card` | Test 1: awaiting_input → `[system] agent awaiting input` |
| `bridge_inserts_finished_comment_on_bound_non_goal_card` | Test 2: finished → `[system] agent finished` |
| `bridge_inserts_crashed_comment_with_signature` | Test 3: crashed + signature → `[system] session crashed: SIGSEGV` |
| `bridge_inserts_crashed_comment_without_signature_uses_unknown` | Test 4: crashed, no signature → `[system] session crashed: unknown` |
| `bridge_trims_signature_to_80_chars` | Test 5: 200-char signature trimmed to 80 |
| `bridge_skips_goal_card_events` | Test 6: lbl="goal" card skipped |
| `bridge_skips_unbound_sessions` | Test 7: session.card_id=None skipped |
| `bridge_dedupes_identical_back_to_back_events` | Test 8: two agent.finished → one comment |
| `bridge_dedupe_resets_on_different_kind` | Test 9: finished then awaiting_input → two comments |
| `bridge_recovers_from_bus_lag_and_continues` | Test 10: flood → Lagged → next real event still inserts comment |
| `bridge_ignores_irrelevant_event_kinds` | Test 11: board.created, host.metrics dropped silently |

TDD gate: RED commit `c2864c4` (tests fail — function missing), GREEN commit `c4a5e6c` (all 11 pass).

### Task 2: Spawn from `crates/agentum-server/src/lib.rs`

Added the symmetric spawn block immediately after the reconciler spawn at line ~268:

```rust
// Watchdog → comment bridge (plan 02-04). Subscribes to the same bus
// as the reconciler and converts agent.*/session.crashed events into
// [system] comments on the bound card's thread (CONTEXT D-04..D-09).
{
    let store = state.store.clone();
    let bus = bus.clone();
    tokio::spawn(async move {
        agentum_watchdog::run_session_comment_bridge(store, bus).await;
    });
}
```

Structure is identical to the reconciler spawn block (clone store + clone bus + tokio::spawn(async move { … })), per CONTEXT D-05 intent.

## Deviations from Plan

None. Plan executed exactly as written.

One implementation detail resolved: the plan's pseudo-code uses `store.get_session(session_id)` but the Store API is `store.get_session_by_id(session_id)`. Used the correct method name (confirmed by reading `crates/agentum-store/src/lib.rs:198`).

## Verification Results

```
cargo test -p agentum-watchdog --lib        → 28 passed, 0 failed, 0 ignored
cargo test -p agentum-server --lib          → 78 passed, 0 failed, 4 ignored
cargo clippy -p agentum-watchdog --all-targets -- -D warnings  → clean
cargo clippy -p agentum-server --all-targets -- -D warnings    → clean
cargo fmt --all -- --check                  → clean
grep -nE "sqlx::query!" crates/agentum-watchdog/src/lib.rs     → no matches
```

## TDD Gate Compliance

- RED gate commit: `c2864c4` — `test(02-04): add failing tests for run_session_comment_bridge` (11 compile errors; function not found)
- GREEN gate commit: `c4a5e6c` — `feat(02-04): add run_session_comment_bridge to agentum-watchdog` (all 11 pass)
- Style commit: `236a76b` — `style(02-04): apply cargo fmt` (no logic changes)

## Known Stubs

None. The bridge is fully wired: events → filter → DB write. No placeholder bodies or mock data.

## Threat Flags

No new network endpoints, auth paths, or schema changes. The bridge is an in-process bus consumer that calls existing Store methods. T-02-15 (signature amplification) mitigated by the 80-char cap, as verified by Test 5.

## Self-Check

### Files exist
- [x] `crates/agentum-watchdog/src/lib.rs` contains `pub async fn run_session_comment_bridge`
- [x] `crates/agentum-server/src/lib.rs` contains `run_session_comment_bridge`

### Commits exist
- [x] `c2864c4` — RED tests
- [x] `c4a5e6c` — GREEN implementation
- [x] `269bb94` — Server spawn
- [x] `236a76b` — Format fix

## Self-Check: PASSED
