---
phase: 02-card-session-binding
plan: 03
subsystem: api
tags: [http, axum, patch, auto-spawn, unbind, rebind, yolo-marker, board, binding]
requires:
  - phase: 02-card-session-binding
    plan: 01
    provides: Store::claim_card, Store::transfer_card_binding, BoardPatch.session_id double-Option contract
provides:
  - auto-spawn branch in PATCH /api/board/{id} — `{status: "doing"}` on unbound card spawns a session
  - unbind branch in PATCH /api/board/{id} — `{session_id: null}` atomically clears card↔session link
  - rebind branch in PATCH /api/board/{id} — `{session_id: "<uuid>"}` atomically transfers binding (HTTP 409 on collision)
  - spawn_card_session helper in board_goals.rs (mirrors spawn_planner_session pattern from plan 01-05)
  - board.updated event payload now includes session_id
affects: 02-04, 02-05, 02-06

tech-stack:
  added: []
  patterns:
    - "gate-first invariant: enforce_transition runs before any side-effect (claim, spawn, transfer)"
    - "atomic transfer: session_id PATCH stripped before patch_board_item so transfer_card_binding owns the column write"

key-files:
  created: []
  modified:
    - crates/agentum-server/src/routes/board.rs
    - crates/agentum-server/src/routes/board_goals.rs

key-decisions:
  - "Auto-spawn skip-when-bound: item.session_id.is_some() short-circuits the spawn branch — already-bound PATCH is a normal status update (CONTEXT D-04)"
  - "patch.session_id is consumed (set to None) before patch_board_item so the column isn't double-written; transfer_card_binding holds the atomic write"
  - "board.updated event now carries session_id so dashboard/TUI clients can update the card chip without a second GET"
  - "Auto-spawn happy path deferred to plan 02-06 e2e (requires live tmux); unit tests cover skip-when-bound and missing-workdir 400"
  - "Pre-existing test patch_done_to_doing_skips_gate_when_done_lacks_anchor updated to pre-bind session_id, isolating its assertion to the gate-on-target behavior it was designed to pin"

patterns-established:
  - "Three new patch() branches share the same gate-first → mutate → emit board.updated shape used by the existing patch() handler"
  - "spawn_card_session is pub(crate) so future routes (CLI manual-spawn, e.g.) can reuse the same NewSession assembly without duplicating YOLO-marker injection"

requirements-completed: [BIND-01, BIND-05, BIND-06]

duration: 18min
completed: 2026-05-22
---

# Phase 02-03 Summary

**Wired PATCH `/api/board/{id}` to atomically bind, unbind, rebind, and auto-spawn sessions on status transitions to `doing`.**

## What landed

### `crates/agentum-server/src/routes/board_goals.rs` (Task 1)

Added `pub(crate) async fn spawn_card_session(state, item)`:

- Resolves `tool` via CONTEXT D-02: `card.tool → parent_goal.tool → "claude"`.
- Resolves `workdir` via the same chain, returning HTTP 400 with `{"missing": ["workdir"], "status": "doing"}` if absent (matches Phase 1 envelope).
- Validates `workdir` exists on disk.
- Injects the canonical YOLO marker `--dangerously-skip-permissions` into `flags`. `translate_yolo_marker` in the adapter layer substitutes per-tool spellings (codex → `--dangerously-bypass-approvals-and-sandbox`, cursor → `--force`, etc.) per CONTEXT D-08.
- Calls `Store::claim_card` (atomic insert+bind in one transaction — plan 02-01).
- Spawns the tmux pane via `tmux_target_for` + the adapter's `launch()`.
- Sends **no** opening prompt — pane is blank; first input is whatever the user types (CONTEXT D-03; UX-01/UX-02 prompt assembly is Phase 3).

### `crates/agentum-server/src/routes/board.rs` (Task 2)

Added three branches inside `patch()`:

1. **Auto-spawn (CONTEXT D-01, BIND-05).** PATCH `{status: "doing"}` on an unbound card calls `spawn_card_session` after the gate passes and after `patch_board_item` lands the status change. Re-fetches the item so the response includes the bound `session_id` (D-04). Emits `board.updated` with `session_id` in the payload.
2. **Unbind (CONTEXT D-10, BIND-06).** PATCH `{session_id: null}` (explicit null, distinguished from omitted via `BoardPatch.session_id: Option<Option<Uuid>>` from plan 02-01) calls `Store::transfer_card_binding(id, None)`. The `session_id` field is then stripped from `patch` so `patch_board_item` doesn't attempt a second write.
3. **Rebind.** PATCH `{session_id: "<uuid>"}` calls `transfer_card_binding(id, Some(uuid))`. Returns HTTP 409 if the target session is already bound to a different card (`Store::AlreadyExists → ApiError::Conflict`).

`board.updated` event payload now includes `session_id` so the dashboard `Bound-session` panel (plan 02-05) can refresh chips without a second GET.

## Tests

5 new tests in `board.rs::tests`:

- `patch_doing_autospawn_happy_path_requires_live_tmux` — `#[ignore]`, deferred to plan 02-06 e2e.
- `patch_doing_missing_workdir_returns_400` — gate fires before spawn; returns `{"missing": ["workdir"], "status": "doing"}`.
- `patch_doing_skips_autospawn_when_already_bound` — seed `session_id` in store, PATCH `{status: "doing"}` → 200, binding preserved, no new session row.
- `patch_unbind_clears_card_session_id` — PATCH `{session_id: null}` → 200, card row's `session_id` is NULL after.
- `patch_rebind_to_different_session_409` — pre-bind session A to card 1; PATCH card 2 with `{session_id: A}` → 409.

Existing test `patch_done_to_doing_skips_gate_when_done_lacks_anchor` updated to pre-bind `session_id` so the new auto-spawn branch is skipped, preserving the test's gate-on-target verification intent. Comment explains the rationale.

**Results:** server lib suite — 78 passed, 0 failed, 4 ignored (1 new + 3 pre-existing). Workspace clippy clean, fmt clean.

## Deferred

- **Auto-spawn happy path e2e:** plan 02-06 will exercise the full PATCH → tmux pane → session row → `board.updated` event flow with a live in-process daemon.
- **Idempotent unbind:** the unbind branch is already idempotent on already-unbound cards via `transfer_card_binding`'s 3-step pattern (plan 02-01 covers); no extra handler logic needed.

## Reuses

- `Store::claim_card` and `Store::transfer_card_binding` (plan 02-01).
- `BoardPatch.session_id` double-Option deserialization (plan 02-01 — pinned by const-eval static-assert `_assert_clone::<BoardPatch>()`).
- `enforce_transition` gate (Phase 1 — invariant: runs before any side-effect; `patch.clone()` lets the gate-merge see the incoming session_id without consuming it).
- `Event::new("board.updated").with_payload(...)` emit pattern (existing in `patch()`).

## Recovery note

This plan was salvaged after the executor agent stalled mid-`cargo test` (fresh worktree compile took ~10min, tripping the 600s stream watchdog). The agent's code edits and new tests were intact and substantive; the orchestrator validated them (cargo check, clippy, full test suite), fixed one pre-existing test that needed updating for the new auto-spawn branch, and committed/wrote SUMMARY.md.
