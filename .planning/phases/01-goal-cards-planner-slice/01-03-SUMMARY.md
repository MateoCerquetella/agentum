---
phase: 01-goal-cards-planner-slice
plan: "03"
subsystem: agentum-server/routes
tags: [http, axum, routes, board, planner-spawn, security, board-links, board-goals]

dependency_graph:
  requires: ["01-01", "01-02"]
  provides: ["POST /api/board/goals", "POST /api/board/links", "GET /api/board/links", "DELETE /api/board/links/{from}/{to}/{kind}"]
  affects: ["dashboard (plan 01-06)", "TUI overlay (plan 01-07)", "CLI shim (plan 01-05)", "watchdog goal-status (plan 01-04)"]

tech_stack:
  added:
    - "routes/board_goals.rs — POST /api/board/goals handler"
    - "routes/board_links.rs — POST/GET/DELETE /api/board/links handlers"
  patterns:
    - "axum Router<AppState> with pub fn router() per route file"
    - "enforce_transition gate called BEFORE create_board_item (gate-first pattern)"
    - "board.created emitted by store path; goal.created emitted by handler for event-consumer filtering"
    - "spawn_planner_session helper mirrors sessions::start lines 256-274 exactly"
    - "Symbolic key resolution via body prefix `key: <key>` convention (no schema change)"
    - "T-03-02: validate_symbolic_key rejects chars outside [a-zA-Z0-9_-] before any SQL"

key_files:
  created:
    - crates/agentum-server/src/routes/board_goals.rs
    - crates/agentum-server/src/routes/board_links.rs
  modified:
    - crates/agentum-server/src/routes/board.rs
    - crates/agentum-server/src/routes/mod.rs
    - crates/agentum-server/src/lib.rs

decisions:
  - "enforce_transition made pub(crate) in board.rs so board_goals can call it without duplicating the gate logic"
  - "Symbolic key resolution uses body prefix `key: <key>\n\n<body>` (Option A from plan) — no schema column needed in v1"
  - "On planner spawn failure: goal card retained (D-07), response still 201 with empty planner_session_id, goal.planner.spawn_failed event fires"
  - "board.created event emitted from store path (inside create handler); goal.created emitted separately by board_goals handler for consumers that filter on goal-specific events"
  - "tmux-spawn test marked #[ignore] — requires live tmux server, deferred to plan 01-08 end-to-end tests"

metrics:
  duration: "~60 minutes"
  completed: "2026-05-21"
  tasks_completed: 3
  tasks_total: 3
  files_created: 2
  files_modified: 3
---

# Phase 01 Plan 03: Route Layer — board.rs + board_links + board_goals Summary

Three HTTP endpoints wired, locking down the wire contract for downstream plans 01-04 through 01-07.

## Wire Contracts

### POST /api/board/goals

**Request body:**
```json
{ "title": "build OAuth", "body": null, "workdir": "/home/u/proj" }
```
`workdir` defaults to daemon cwd when absent.

**Response (201):**
```json
{
  "goal": { "id": 42, "key": "AG-42", "lbl": "goal", "status": "todo", "title": "build OAuth", "parent_goal_id": null, ... },
  "planner_session_id": "uuid-here"
}
```
`planner_session_id` is an empty string when spawn failed (goal still created — D-07).

**Column-rule gate:** If the `todo` column has raised its required fields above what the request provides, returns `400` with `{"missing": [...], "status": "todo"}`.

### POST /api/board/links

Two body shapes:

**Direct (caller has ids):**
```json
{ "from_card_id": 7, "to_card_id": 9, "kind": "blocks" }
```

**Symbolic (caller has keys):**
```json
{ "parent_goal_id": 42, "from_key": "types", "to_key": "schema", "kind": "blocks" }
```

**Response (201):** `BoardLink` JSON `{ from_card_id, to_card_id, kind, created_at }`.

**Errors:** `400` on unknown kind, unknown symbolic key, or invalid key chars; `409` on duplicate triple.

### GET /api/board/links?goal=\<id\>

Returns `Vec<BoardLink>` for edges where `from_card_id = goal_id`.

### DELETE /api/board/links/{from}/{to}/{kind}

`204` on hit, `404` on miss.

### Extended POST /api/board + PATCH /api/board/{id}

`parent_goal_id` now flows through wire + bus payloads end-to-end:
- `POST /api/board` with `{parent_goal_id: 7}` creates child with that goal link
- `PATCH /api/board/{id}` with `{"parent_goal_id": null}` detaches; with a value reparents; absent field is no-op
- `board.created`, `board.updated`, `board.deleted` events all include `parent_goal_id` in payload

## Bus Events Introduced

| Event kind | Emitter | Payload |
|---|---|---|
| `board.created` | board.rs create | `{id, key, title, parent_goal_id}` |
| `board.updated` | board.rs patch | `{id, key, status, parent_goal_id}` |
| `board.deleted` | board.rs delete | `{id, parent_goal_id}` |
| `goal.created` | board_goals.rs | `{id, key, title}` |
| `goal.planner.spawned` | board_goals.rs | `{goal_id, session_id, tool}` |
| `goal.planner.spawn_failed` | board_goals.rs | `{goal_id, error}` |
| `board.link.created` | board_links.rs | `{from, to, kind}` |
| `board.link.deleted` | board_links.rs | `{from, to, kind}` |

## Spawn Lifecycle (POST /api/board/goals)

1. `enforce_transition` gate (same as board.rs create — column-rule aware)
2. `create_board_item(lbl=goal, status=todo)` → `board.created` event
3. `goal.created` event emitted
4. `load_planner_config()` from disk (D-12: no cache)
5. `create_session(card_id=goal.id, tool=cfg.tool)` → session row
6. `new_session()` → tmux pane; `pipe_pane()` → log file
7. `update_status_and_target(Running, target)` → session DB row
8. `send_keys(prompt.replace("<AG-KEY>", goal.key), enter=true)`
9. `goal.planner.spawned` event (or `goal.planner.spawn_failed` on step 6 failure; goal NOT deleted)

## Security (T-03-02)

`validate_symbolic_key` rejects any key with chars outside `[a-zA-Z0-9_-]` or length > 64 before the body prefix scan. This prevents wildcard injection into LIKE-equivalent patterns.

## Test Coverage

| Module | Tests | Status |
|---|---|---|
| routes::board::tests | 18 tests | all pass |
| routes::board_links::tests | 9 tests | all pass |
| routes::board_goals::tests | 5 tests (1 ignored) | 4 pass, 1 ignored (requires tmux) |

The ignored test (`create_goal_with_missing_planner_binary_returns_201_and_emits_spawn_failed`) is deferred to plan 01-08 end-to-end integration tests which will run inside a real tmux session.

## Deviations from Plan

### Auto-fixed Issues

None — plan executed exactly as written.

### Implementation Notes (not deviations)

1. The `create` handler in board.rs does NOT duplicate the `board.created` event; that event fires from the existing path. The handler adds a separate `goal.created` event so downstream consumers (plan 01-04 watchdog) can subscribe specifically to goal creation without filtering on `lbl` in every `board.created` consumer.

2. `TransitionCtx` has no `body` field — the plan snippet showed a `body: None` init that doesn't apply. Removed from the struct literal; no behavior change.

3. Column-rule gate test uses `RequiredField::Workdir` (not a hypothetical `Body` variant which doesn't exist in the schema) — a more realistic test case that proves the gate fires before `create_board_item`.

## Commits

| Task | Commit | Files |
|---|---|---|
| Task 1: parent_goal_id in board.rs wire + bus | 9da8003 | routes/board.rs |
| Task 2: board_links.rs + route registration | 45e43f9 | routes/board_links.rs, routes/mod.rs, lib.rs |
| Task 3: board_goals.rs | abbc148 | routes/board_goals.rs |

## Known Stubs

None — all handlers call real store methods and emit real bus events. The planner spawn path requires a live tmux binary but degrades gracefully (goal card retained, spawn_failed event fired).

## Self-Check: PASSED

- [x] `crates/agentum-server/src/routes/board_goals.rs` exists
- [x] `crates/agentum-server/src/routes/board_links.rs` exists
- [x] `routes/mod.rs` contains `pub mod board_goals;` and `pub mod board_links;`
- [x] `lib.rs::router()` contains `.merge(routes::board_goals::router())` and `.merge(routes::board_links::router())`
- [x] `cargo test -p agentum-server --lib -- routes::board::tests routes::board_goals::tests routes::board_links::tests` → 31 passed, 1 ignored
- [x] `cargo clippy -p agentum-server --all-targets -- -D warnings` → clean
- [x] `cargo fmt --check -p agentum-server` → clean
- [x] All commits exist: 9da8003, 45e43f9, abbc148
- [x] `auth.rs::is_public()` does not match `/api/board/goals` or `/api/board/links`
