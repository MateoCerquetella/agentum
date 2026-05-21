---
phase: 01-goal-cards-planner-slice
plan: "01"
subsystem: store
tags: [schema, sqlx, migration, board, sessions, board-links]
one-liner: "SQLite migration 0015 + agentum-core types (BoardLink, LinkKind, parent_goal_id, card_id) + five new Store methods with a full round-trip integration test"

dependency-graph:
  requires: []
  provides:
    - migration-0015
    - BoardItem.parent_goal_id
    - NewBoardItem.parent_goal_id
    - BoardPatch.parent_goal_id
    - Session.card_id
    - NewSession.card_id
    - BoardLink
    - LinkKind
    - Store::add_board_link
    - Store::list_children_of_goal
    - Store::list_board_links_for_goal
    - Store::delete_board_link
    - Store::max_child_status_rank
  affects:
    - agentum-server/routes/board.rs (tests updated)
    - agentum-server/routes/board_rules.rs (tests updated)
    - agentum-executor/adapters.rs (test fixture updated)
    - agentum/commands/new.rs (NewSession updated)
    - agentum/commands/terminal/app.rs (Session fixture updated)

tech-stack:
  added: []
  patterns:
    - "double-Option via deserialize_optional_field for BoardPatch.parent_goal_id (clear vs omit)"
    - "CASE WHEN ? = 1 THEN ? ELSE col END for nullable patch fields in UPDATE"
    - "sqlx #[sqlx(default)] on new nullable columns for backwards-compat FromRow"
    - "MAX() returns one row with NULL when no children; use (Option<i64>,) tuple + fetch_one"
    - "ON DELETE CASCADE FK on board_links for referential integrity without application-layer cleanup"

key-files:
  created:
    - crates/agentum-store/migrations/0015_orchestrator.sql
  modified:
    - crates/agentum-core/src/lib.rs
    - crates/agentum-store/src/lib.rs
    - crates/agentum-executor/src/adapters.rs
    - crates/agentum-server/src/routes/board.rs
    - crates/agentum-server/src/routes/board_rules.rs
    - crates/agentum/src/commands/new.rs
    - crates/agentum/src/commands/terminal/app.rs

decisions:
  - "D-01 honoured: goal IS a BoardItem with lbl=goal — no parallel board_goals table; only parent_goal_id + card_id + board_links added"
  - "board_links as separate table (not JSON column) so Phase 3 dependency gate can do indexed JOIN sub-10ms"
  - "max_child_status_rank uses inline CASE WHEN in SQL (single round-trip) rather than fetching rows and ranking in Rust"
  - "pre-existing failing test inside_tmux_uses_dcs_passthrough is out-of-scope (crossterm OSC52 issue predating this plan)"

metrics:
  duration: "~2h (across two sessions)"
  completed: "2026-05-21"
  tasks-completed: 2
  tasks-total: 2
  files-modified: 7
  files-created: 1
---

# Phase 01 Plan 01: Schema + Core Types + Store Layer Summary

SQLite migration 0015 + agentum-core types (BoardLink, LinkKind, parent_goal_id, card_id) + five new Store methods with a full round-trip integration test.

## What Was Built

### Task 1: Migration 0015 + agentum-core types (commit 85182b9)

Created `crates/agentum-store/migrations/0015_orchestrator.sql`:
- `ALTER TABLE board_items ADD COLUMN parent_goal_id INTEGER` (nullable, backwards-compat)
- `ALTER TABLE sessions ADD COLUMN card_id INTEGER` (nullable, backwards-compat)
- `CREATE TABLE board_links` with composite PK `(from_card_id, to_card_id, kind)` and ON DELETE CASCADE FKs
- Partial index `idx_board_items_parent_goal_id WHERE parent_goal_id IS NOT NULL`
- Index `idx_board_links_to` on `to_card_id` for Phase 3 dependency gate

Extended `crates/agentum-core/src/lib.rs`:
- `CoreError::ParseLinkKind(String)` variant
- `Session.card_id: Option<i64>` + `NewSession.card_id: Option<i64>`
- `BoardItem.parent_goal_id: Option<i64>` + `NewBoardItem.parent_goal_id: Option<i64>`
- `BoardPatch.parent_goal_id: Option<Option<i64>>` (double-Option via `deserialize_optional_field`)
- `LinkKind` enum (`ParentOf`, `Blocks`) with `as_str()`, `FromStr`, serde `rename_all = "snake_case"`
- `BoardLink` struct with `from_card_id`, `to_card_id`, `kind`, `created_at`
- 4 unit tests: serde round-trips for new fields, double-Option distinction test

### Task 2: Store wiring + new methods + integration test (commit 28baeae)

Extended `crates/agentum-store/src/lib.rs`:
- `SessionRow` gains `#[sqlx(default)] card_id: Option<i64>`
- `BoardItemRow` gains `#[sqlx(default)] parent_goal_id: Option<i64>`
- Both `TryFrom` impls updated to pass through new fields
- `create_session` INSERT includes `card_id`; return value includes `card_id`
- `create_board_item` INSERT includes `parent_goal_id`; return value includes `parent_goal_id`
- `patch_board_item` UPDATE includes CASE WHEN handling for `parent_goal_id` double-Option
- New public Store methods:
  - `add_board_link(from, to, kind) -> Result<BoardLink>` — returns AlreadyExists on duplicate
  - `list_children_of_goal(goal_id) -> Result<Vec<BoardItem>>` — uses partial index
  - `list_board_links_for_goal(goal_id) -> Result<Vec<BoardLink>>` — reads from PK b-tree prefix
  - `delete_board_link(from, to, kind) -> Result<bool>` — idempotent, returns false on no-op
  - `max_child_status_rank(goal_id) -> Result<Option<i32>>` — inline CASE WHEN, single round-trip
- `links_and_parent_round_trip` integration test (11-step, all green)
- All `NewBoardItem` and `NewSession` struct literals in workspace tests updated

## Test Results

- `cargo test -p agentum-store --lib`: 17/17 passed (includes new `links_and_parent_round_trip`)
- `cargo test -p agentum-core --lib`: 37/37 passed
- `cargo test -p agentum-executor --lib`: 14/14 passed
- `cargo test -p agentum-server --lib`: 42/42 passed
- `cargo check --workspace --all-targets`: clean
- `cargo clippy --workspace --all-targets -- -D warnings`: clean
- `cargo fmt --all -- --check`: clean

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Missing `card_id` field in Session struct literals across workspace**
- **Found during:** Task 2 (`cargo check --workspace`)
- **Issue:** Seven files outside `agentum-store` had `Session`, `NewSession`, `NewBoardItem` struct literals that needed the new fields
- **Fix:** Added `card_id: None` / `parent_goal_id: None` to all struct literals in `adapters.rs`, `board.rs`, `board_rules.rs`, `new.rs`, `app.rs`
- **Files modified:** 5 files in `agentum-executor`, `agentum-server`, `agentum`
- **Commit:** 28baeae (part of Task 2)

## Known Out-of-Scope Issue

Pre-existing test `commands::terminal::app::osc52_tests::inside_tmux_uses_dcs_passthrough` was failing before this plan due to crossterm 0.28 OSC52 behavior (documented in memory: `[crossterm OSC responses]`). This is unrelated to the schema/store changes in this plan.

## Self-Check: PASSED
