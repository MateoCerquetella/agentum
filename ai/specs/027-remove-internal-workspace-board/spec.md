---
tracker: https://github.com/MateoCerquetella/agentum/issues/431
---

# Spec 027 — Remove the internal Workspace board

- **Number:** 027
- **Status:** Done
- **Surface:** `crates/agentum-desktop/ui`, `crates/agentum-server`
- **Author:** Codex
- **Date:** 2026-07-22

## Problem

A workspace operator choosing the next tracked item sees an Agentum-owned board alongside the
project's configured tracker, so it is unclear which status is authoritative and mirrored cards
can become stale. The pain occurs in the project Tasks view and when an autonomous run chooses or
updates its work item.

## Goal

A workspace operator selects workspace work from the project's configured external tracker.

## User value

One tracker remains authoritative, eliminating duplicate cards and status reconciliation.

## Acceptance criteria

- [x] A project's Tasks surface renders items from its configured GitHub or Linear source, or renders an explicit no-tracker empty state, and renders no internal-board cards or “Sync to Board” action.
- [x] Requests to the retired internal-board route families `/api/board`, `/api/board/goals`, `/api/board/links`, `/api/board/rules`, and `/api/board/bindings` return `404 Not Found`.
- [x] Creating, selecting, or transitioning a GitHub- or Linear-backed harness item persists or emits the existing external-tracker result without inserting or updating an internal `board_items` row.
- [x] Starting agentum with a database that already contains internal-board rows succeeds, while normal workspace and harness flows neither return nor mutate those rows.
- [x] `cargo test --workspace --lib` and `npm run build --prefix crates/agentum-desktop/ui` complete successfully with focused route, tracker-flow, and Tasks-surface checks passing.

## Scope and non-goals

- **In scope:** remove the internal board from desktop Tasks UX and server routing; remove the
  internal-board fallback from normal work-item creation and transitions; tolerate legacy rows as
  inert data.
- **Out of scope:** deleting historical board migrations or user data; removing or renaming
  external GitHub Projects/Linear board views; redesigning tracker setup; changing GitHub or
  Linear APIs; changing session launch, watchdog streaming, or the verification gate.

## Existing code to reuse

- Project Tasks already resolves project-scoped GitHub and Linear work items through
  `crates/agentum-desktop/ui/src/components/project-hub/ProjectTasksPage.tsx`; retain those
  sources and remove `TaskPage`'s `syncTasksToBoard` path through `runtime/board-client.ts`.
- External tracker selection and transitions already live in
  `crates/agentum-server/src/task_sink.rs`; retain `TrackerChoice::Github` and
  `TrackerChoice::Linear` while removing `TaskSink::Board` as a runtime fallback.
- Internal routes are isolated in `routes/board.rs`, `board_goals.rs`, `board_links.rs`,
  `board_rules.rs`, and `board_sync.rs` and are registered in
  `crates/agentum-server/src/lib.rs`; external GitHub Projects routes remain separate.
- Legacy storage is isolated in `crates/agentum-store/src/board.rs` and existing board migrations;
  leave persisted rows readable by SQLite migration startup but unreachable from normal flows.

## Invariants and verification

- External GitHub Projects are not the internal Workspace board and remain available.
- The one launch path, push-based streaming, tracker best-effort behavior, and green harness gates
  remain unchanged.
- Each acceptance checkbox becomes one `.agentum-harness/feature_list.json` entry;
  `.agentum-harness/verify.sh` runs focused Rust/UI tests plus both build commands above, and
  `.agentum-harness/qa.sh` verifies the Tasks tracker/empty states with no internal-board action.

Specs 016 and 025 scope external boards per project, and spec 002 starts an external ticket
directly; none retires the remaining internal-board API and fallback described here.
