# Spec 431 — Remove the internal Workspace board

- **Status:** Done
- **Surface:** `crates/agentum-desktop/ui`, `crates/agentum-server`
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/431

## Problem

When a workspace operator chooses the next tracked item in a project's Tasks view, Agentum's
internal board appears alongside the configured tracker, leaving the operator unsure which status
is authoritative and exposing stale mirrored cards to autonomous runs.

## Goal

A workspace operator selects workspace work from the project's configured external tracker.

## User value

One tracker remains authoritative, eliminating duplicate cards and status reconciliation.

## Acceptance criteria

- [x] A project's Tasks surface renders items from its configured GitHub or Linear source, or renders an explicit no-tracker empty state, and renders no internal-board cards or “Sync to Board” action.
- [x] Requests to `/api/board`, `/api/board/goals`, `/api/board/links`, `/api/board/rules`, and `/api/board/bindings`, including their nested routes, return `404 Not Found`.
- [x] Creating, selecting, or transitioning a GitHub- or Linear-backed harness item persists or emits the existing external-tracker result without inserting or updating an internal `board_items` row.
- [x] Agentum starts successfully with a database containing legacy internal-board rows, and normal workspace and harness flows neither return nor mutate those rows.
- [x] Current API/data-model documentation and live SDD playbooks omit the internal board as an available work-item system and label retained board schema or input values as legacy compatibility only.
- [x] `cargo test --workspace --lib` and `npm run build --prefix crates/agentum-desktop/ui` complete successfully with focused route, tracker-flow, and Tasks-surface checks passing.

## Scope and non-goals

- **In scope:** the desktop Tasks surface, internal-board server routes, internal-board fallbacks in
  normal work-item flows, current product/SDD documentation, and inert compatibility for existing
  board rows or legacy tracker input.
- **Out of scope:** deleting historical migrations or user data; removing external GitHub
  Projects/Linear views; redesigning tracker setup; changing provider APIs, session launch,
  watchdog streaming, or harness gates.

## Existing code to reuse

- `crates/agentum-desktop/ui/src/components/project-hub/ProjectTasksPage.tsx` already resolves
  project-scoped GitHub and Linear items; `TaskPage.tsx` and `runtime/board-client.ts` contain the
  internal `syncTasksToBoard` path.
- `crates/agentum-server/src/task_sink.rs` already owns external tracker selection and transitions
  through `TrackerChoice::Github` and `TrackerChoice::Linear`; `TaskSink::Board` is the internal
  fallback.
- Internal routes live in `crates/agentum-server/src/routes/board.rs`, `board_goals.rs`,
  `board_links.rs`, `board_rules.rs`, and `board_sync.rs`, registered by
  `crates/agentum-server/src/lib.rs`; external GitHub Projects routes are separate.
- Legacy persistence lives in `crates/agentum-store/src/board.rs` and existing board migrations.
- Current board contracts are also described by `docs/API.md`, `docs/DATA-MODEL.md`,
  `crates/agentum-server/src/sdd_playbooks/sdd-orchestrate.md`,
  `crates/agentum-server/src/sdd_playbooks/sdd-spec.md`, and tracker wire comments in
  `crates/agentum-desktop/ui/src/runtime/harness-client.ts` and
  `crates/agentum-server/src/task_sink.rs`; retain only explicitly identified legacy tolerance.

## Invariants and overlap

- External GitHub Projects remain available; the existing launch path, push streaming, tracker
  best-effort behavior, and harness gates remain unchanged.
- Specs 002, 016, and 025 cover direct external-ticket start and project-scoped trackers but do
  not retire the internal API and fallback. `ai/specs/027-remove-internal-workspace-board/spec.md`
  is the canonical archive of this same issue; this root file is its harness execution copy, not a
  separate or competing spec.
