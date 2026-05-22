---
phase: 02-card-session-binding
plan: "02"
subsystem: agentum-server/routes
tags: [http, route, axum, pane-snapshot, sessions, tdd]
dependency_graph:
  requires: []
  provides:
    - "GET /api/sessions/{id}/pane — pane-snapshot HTTP route"
    - "clamp_lines() — server-side lines clamp helper"
    - "PaneSnapshot — response type {lines, captured_at}"
  affects:
    - "crates/agentum-server/src/routes/sessions.rs"
tech_stack:
  added: []
  patterns:
    - "axum Query extractor for optional u32 param"
    - "time::Rfc3339 formatting for captured_at field"
    - "in-process AppState harness for route handler tests"
key_files:
  created: []
  modified:
    - "crates/agentum-server/src/routes/sessions.rs"
decisions:
  - "Handler calls get_session_by_id (not get_session) — matches existing pattern in sessions.rs"
  - "clamp_lines placed near ListQuery (top of file) so all query helpers are co-located"
  - "PaneSnapshot derives Debug to enable Result<Json<PaneSnapshot>, ApiError> in test panics"
  - "pane handler placed after stream handler, before save_checkpoint, to keep WS surface together"
  - "Tests use in-process AppState harness (no HTTP server) matching board_links.rs pattern"
metrics:
  duration: "~15 minutes"
  completed: "2026-05-22T18:11:35Z"
  tasks_completed: 1
  tasks_total: 1
  files_modified: 1
---

# Phase 02 Plan 02: Pane-Snapshot HTTP Route Summary

GET /api/sessions/{id}/pane route with server-side lines clamp and RFC3339 captured_at using the existing capture_pane_visible tmux helper.

## What Was Built

Added `GET /api/sessions/{id}/pane?lines={n}` to `crates/agentum-server/src/routes/sessions.rs`:

**Response shape** (UI-SPEC §Component Inventory contract):
```json
{ "lines": ["line1", "line2", ...], "captured_at": "2026-05-22T18:11:00Z" }
```

**Lines clamp behavior:**
- `?lines=` absent → default 20
- `?lines=0` → clamped to 1
- `?lines=500` → clamped to 200
- Range: 1..=200 enforced server-side

**Idle session behavior:**
- Sessions with `tmux_target = None` return HTTP 200 with `"lines": []` and a valid `captured_at`
- No error, no 404 — UI-SPEC §empty state

**Auth:**
- No entry added to `auth::is_public` — bearer-auth inherited from top-level `require_token` middleware merge in `lib.rs`
- Confirmed: `grep "/api/sessions/.*pane" crates/agentum-server/src/auth.rs` returns zero matches

## Tests Added (6 total, all passing)

| Test | Type | What it proves |
|------|------|----------------|
| `pane_clamp_upper_bound` | unit | `clamp_lines(Some(500)) == 200` |
| `pane_clamp_lower_bound_and_default` | unit | `clamp_lines(Some(0)) == 1`, `clamp_lines(None) == 20` |
| `pane_idle_session_returns_empty_lines` | async handler | idle session → 200 + empty lines + valid captured_at |
| `pane_nonexistent_session_returns_404` | async handler | missing UUID → `ApiError::NotFound` |
| `pane_response_shape_is_correct` | async handler | `lines: Vec<String>`, `captured_at: String` |
| (existing) `parse_resize_*` | — | unaffected |

Full server lib suite: **71 passed, 0 failed** (prior: 65 + 6 new).

## Commits

| Hash | Message |
|------|---------|
| 584f776 | feat(02-02): add GET /api/sessions/{id}/pane pane-snapshot route |

## Scope Note

This plan is Rust-only per plan-checker iter-1 revision. The typed dashboard client (`api.getSessionPane`, `PaneSnapshot` TypeScript interface, `AbortSignal` plumbing) is owned by plan 02-05, which coalesces all `dashboard/src/` edits behind a single embedded-SPA rebuild rhythm. No `dashboard/src/` files were touched in this plan.

## Deviations from Plan

None — plan executed exactly as written.

The `save_checkpoint` doc comment placement required ordering care: the handler was inserted BEFORE `save_checkpoint` so the existing `///` comment attached to `save_checkpoint` rather than leaking across the boundary. Clippy caught this and the fix was applied inline.

## Self-Check: PASSED

- `crates/agentum-server/src/routes/sessions.rs` contains `/api/sessions/{id}/pane` — FOUND
- `crates/agentum-server/src/routes/sessions.rs` contains `async fn pane(` — FOUND
- `crates/agentum-server/src/routes/sessions.rs` contains `fn clamp_lines(` — FOUND
- Commit 584f776 — FOUND
- `cargo test -p agentum-server --lib` → 71 passed — VERIFIED
- `cargo clippy -p agentum-server --all-targets -- -D warnings` → clean — VERIFIED
- `cargo fmt --all -- --check` → clean — VERIFIED
- No matches for `/api/sessions/.*pane` in `auth.rs` — VERIFIED
- No `sqlx::query!` macros in `sessions.rs` — VERIFIED
