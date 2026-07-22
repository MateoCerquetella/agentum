# Spec 431 — Architecture

- **Spec:** `431-lets-remove-the-internal-worskpace-board`
- **Phase:** Architect
- **Date:** 2026-07-22
- **Verdict:** ready for developer verification and completion

Every implementation seam cited below was read in this worktree and exists at
the time of this architecture review. The worktree already contains partial
changes for this issue; the developer must preserve and finish those changes,
not restore the retired internal-board modules.

## Components

### 1. Desktop Tasks surfaces

- Finalize `crates/agentum-desktop/ui/src/components/TaskPage.tsx`. This is the
  existing global Tasks renderer and direct-launch surface. It must retain its
  GitHub Issues, GitHub Projects, and Linear views while containing no internal
  board client import, mirroring callback, or “Send ... to the Board” action.
- Keep and reuse
  `crates/agentum-desktop/ui/src/components/project-hub/ProjectTasksPage.tsx`.
  Its existing `ProjectTasksPage` effect dispatches only
  `repo.trackerProvider === 'github'` or `'linear'`; its existing unbound and
  unavailable branches provide explicit tracker messaging and an
  `openSettings` action. No replacement task-source component is needed.
- Extend the existing structural test module
  `crates/agentum-desktop/ui/src/components/project-hub/ProjectTasksPage.test.tsx`.
  The named cases `renders only external tracker sources or the settings empty
  state` and `global tasks has no internal board sync affordance` pin both
  provider branches, the no-tracker/settings state, the external views, and the
  absence of the retired sync symbols.

Boundary: keep `crates/agentum-desktop/ui/src/lib/board-route.ts`,
`crates/agentum-desktop/ui/src/lib/board-project-resolution.ts`, GitHub Projects
components, Linear kanban components, and
`crates/agentum-desktop/ui/src/components/sidebar/WorkspaceKanbanDrawer.tsx`.
Those existing “board” names represent navigation, external tracker
presentation, or workspace layout rather than Agentum-owned cards. This spec
does not redesign tracker setup or navigation.

### 2. Server route boundary and tracker-only work-item seams

- Finalize `crates/agentum-server/src/lib.rs::router` and
  `crates/agentum-server/src/routes/mod.rs`. The current router composes GitHub,
  GitHub Projects, project-tracker, harness, and SDD routes but no internal
  board route modules. Do not add tombstone handlers: Axum's unmatched-route
  behavior is the required `404 Not Found` contract.
- Retain `crates/agentum-server/src/lib.rs::tests::internal_board_route_families_are_unregistered`.
  It builds the real application router and checks every retired family plus
  representative nested paths under `/api/board`, `/goals`, `/links`,
  `/rules`, and `/bindings`.
- Finalize `crates/agentum-server/src/task_sink.rs` using its existing closed
  enum pattern. `TaskSink` remains limited to `Github` and `Linear`;
  `parse_tracker_choice` accepts only automatic/external selection;
  `apply_tracker_transition` and `apply_blocked_transition` retain their
  existing external results and event semantics without receiving a `Store`.
  A legacy provider string such as `"board"` follows the existing unknown
  provider best-effort path and cannot write persistence.
- Preserve and compile-check every current transition caller:
  `crates/agentum-server/src/harness/drive.rs`,
  `crates/agentum-server/src/tracker_sync.rs`,
  `crates/agentum-server/src/tracker_attention.rs`,
  `crates/agentum-server/src/routes/harness.rs`, and
  `crates/agentum-server/src/routes/mcp.rs`. These callers keep using the one
  tracker transition seam; only GitHub/Linear are advertised as operational
  providers.
- Retain the focused tests
  `task_sink::tests::only_github_and_linear_are_creation_sinks`,
  `task_sink::tests::pinned_provider_dispatches_to_matching_tracker_arm`,
  `task_sink::tests::legacy_board_provider_is_non_mutating_and_best_effort`,
  `routes::harness::tests::resolve_tracker_pin_maps_d4`, and
  `routes::mcp::tests::report_status_legacy_board_provider_is_non_writing`.

Boundary: `/api/github-projects`, `/api/project-trackers`, `/api/github`,
`/api/harness`, `/api/sdd`, sessions, events, launch behavior, harness gates,
and push streaming stay unchanged. GitHub Projects is an external tracker and
must not be removed with the internal `/api/board*` family.

### 3. Inert legacy persistence and runtime cleanup

- Keep `crates/agentum-store/src/lib.rs` as the store module boundary without
  public internal-board CRUD modules. Retain
  `tests::legacy_board_rows_survive_reopen_and_normal_store_work_is_inert`,
  which inserts a historical row through `Store::pool`, reopens via
  `Store::open`, performs ordinary session work, and compares the full row
  snapshot.
- Keep `crates/agentum-store/src/sessions.rs` and
  `crates/agentum-core/src/lib.rs` tolerant of the existing optional
  `Session.card_id`/`NewSession.card_id` field. It is serialization/database
  compatibility only; no surviving lookup or work-selection API may use it.
- Keep `crates/agentum-server/src/lib.rs::spawn_background_workers` limited to
  the ordinary watchdog and external tracker workers. Keep
  `crates/agentum-watchdog/src/lib.rs` focused on session activity,
  compaction, and crash detection, with no internal goal reconciler or session
  comment bridge.
- Do not edit files under `crates/agentum-store/migrations/`. `Store::open`
  continues to apply the historical schema, so old rows remain readable by
  SQLite and survive startup, while no normal runtime module returns or
  mutates them.

Boundary: historical tables and user data are retained. No DROP migration,
copy, rewrite, or cleanup job is introduced. External tracker configuration and
GitHub Projects bindings are unrelated persistence and remain operational.

### 4. Current documentation and live SDD contracts

- Update `docs/API.md` so its current HTTP contract does not advertise any
  `/api/board*` endpoint.
- Update `docs/DATA-MODEL.md` so `board_items` appears only as explicitly
  labeled legacy migration compatibility, with normal runtime non-use stated.
- Update the live embedded playbooks
  `crates/agentum-server/src/sdd_playbooks/sdd-orchestrate.md` and
  `crates/agentum-server/src/sdd_playbooks/sdd-spec.md` so tracker reporting
  names GitHub/Linear-backed items and never presents the internal board as a
  fallback or supported work-item system.
- Keep legacy provider wire fields in
  `crates/agentum-server/src/harness/types.rs` and
  `crates/agentum-desktop/ui/src/runtime/harness-client.ts` so historical
  harness files still deserialize, but label any retained board value as
  compatibility-only.
- Add
  `crates/agentum-server/src/sdd.rs::tests::current_work_item_docs_are_external_only_and_legacy_schema_is_labeled`.
  Reuse the existing embedded-playbook constants in `sdd.rs`, and read the two
  repository docs relative to `CARGO_MANIFEST_DIR`; assert the API and live
  playbooks do not advertise `/api/board`, `TaskSink::Board`, or a board
  fallback, while `DATA-MODEL.md` explicitly marks `board_items` as legacy.

Boundary: archived specs and historical migrations remain historical records;
this criterion changes current product/API documentation and live SDD
playbooks only.

## APIs

### Removed HTTP API

All methods at or below these route families are unmatched and return
`404 Not Found`:

- `/api/board`
- `/api/board/goals`
- `/api/board/links`
- `/api/board/rules`
- `/api/board/bindings`

There is no redirect, `410 Gone`, compatibility handler, or replacement
internal endpoint.

### Preserved APIs and signatures

- GitHub Issues/Projects, Linear, project-tracker, harness, SDD, session, and
  event wire contracts stay unchanged.
- `TaskSink` remains the existing closed enum with only `Github` and `Linear`.
  `SinkCtx` retains only the current work directory and optional GitHub slug.
- `apply_tracker_transition` and `apply_blocked_transition` keep provider,
  tracker identity, phase/comment inputs, results, and event behavior; neither
  receives store access.
- `agentum_tasks_report_status` keeps its tool/result shape and advertises
  external providers only. A historical `board` input returns a bounded,
  non-writing best-effort result.
- SQLite board tables and `Session.card_id` remain compatibility artifacts, not
  application APIs.

## Data Flow

### Project Tasks

1. `ProjectTasksPage` reads the persisted project tracker provider.
2. GitHub uses the existing project binding and `ProjectViewWrapper`; Linear
   uses the existing binding and `LockedLinearProjectTasks`.
3. Missing or unavailable configuration renders the existing settings-linked
   empty state.
4. No desktop path mirrors an issue into an Agentum board card or calls an
   `/api/board*` endpoint.

### Harness and SDD tracker lifecycle

1. Existing harness metadata carries the external provider, stable id, and
   optional URL.
2. Creation/selection resolves only GitHub or Linear through `TaskSink` and
   `resolve_tracker_pin`.
3. Harness, SDD, MCP, sync, and attention paths call the shared transition seam.
4. GitHub/Linear preserve their current results and event emission. A legacy
   provider is skipped best-effort without a store handle and cannot halt the
   harness.

### Legacy database startup

1. `Store::open` runs the unchanged migration set.
2. Historical board tables and rows remain present.
3. No router, background worker, task sink, public store method, or current UI
   reads or writes those rows.
4. Normal session and external-tracker work proceeds without data conversion or
   deletion.

## Important Decisions

### D1 — Remove capability at the route and type boundaries

Choose an unregistered router plus external-only `TaskSink` and store-free
transition signatures over leaving disabled handlers or guarded board match
arms. The chosen shape makes normal board access impossible by construction and
uses the codebase's existing closed-enum and Axum composition patterns.

### D2 — Return 404 instead of adding tombstones

Choose Axum's existing unmatched-route `404 Not Found` behavior over new `410`
or redirect handlers because the acceptance contract requires 404 and retaining
handlers would preserve an operational internal-board boundary.

### D3 — Preserve legacy rows instead of dropping data

Choose unchanged migrations and inert compatibility fields over a destructive
drop migration. Existing installations continue to open and historical user
data remains intact; compatibility stops at schema/serialization tolerance.

### D4 — Reuse current external tracker primitives

Reuse `TaskSink::{Github, Linear}`, the current transition functions,
`ProjectTasksPage`, provider-specific views, and its empty state. A provider
trait, replacement cache, new task router, or new empty-state component would
be speculative for this removal.

### D5 — Preserve unrelated “board” terminology

Keep GitHub Projects bindings, Linear kanban, filesystem-derived harness board
views, task-routing helpers, and workspace layout components. They describe
external presentation or run state, not internal work-item cards; renaming them
would broaden this issue without user value.

## Acceptance Criteria Mapping

| Acceptance criterion | Named plan part | Named test / verification |
| --- | --- | --- |
| Project Tasks uses configured GitHub/Linear or an explicit no-tracker state and has no internal cards/sync action | Component 1 | `ProjectTasksPage.test.tsx` cases `renders only external tracker sources or the settings empty state` and `global tasks has no internal board sync affordance`; desktop build |
| Every retired `/api/board*` family and nested path returns 404 | Component 2 | `agentum_server::tests::internal_board_route_families_are_unregistered` |
| GitHub/Linear harness creation, selection, and transitions preserve external behavior without board writes | Component 2 | `task_sink::tests::only_github_and_linear_are_creation_sinks`; `task_sink::tests::pinned_provider_dispatches_to_matching_tracker_arm`; `routes::harness::tests::resolve_tracker_pin_maps_d4`; compile-time store-free transition signatures |
| Legacy rows survive startup and normal workspace/harness flows neither return nor mutate them | Components 2 and 3 | `agentum_store::tests::legacy_board_rows_survive_reopen_and_normal_store_work_is_inert`; server 404 matrix; `task_sink::tests::legacy_board_provider_is_non_mutating_and_best_effort`; `routes::mcp::tests::report_status_legacy_board_provider_is_non_writing` |
| Current API/data-model docs and live SDD playbooks omit the internal board except explicit legacy compatibility | Component 4 | `sdd::tests::current_work_item_docs_are_external_only_and_legacy_schema_is_labeled` |
| Workspace Rust library tests and desktop build pass with focused checks | Components 1–4 | all focused tests above; `cargo test --workspace --lib`; `npm run build --prefix crates/agentum-desktop/ui` |

## Risks

- **External features called “board” are removed accidentally.** Mitigation:
  deletions and negative assertions are limited to internal `/api/board*`,
  board-card persistence, and the retired sync path; D5 names the external and
  presentation-only features that remain, and their existing tests stay green.
- **A tracker caller retains an internal fallback or is missed.** Mitigation:
  compile all enumerated callers against the store-free transition signatures,
  retain explicit provider-selection tests, and run the complete workspace
  library suite.
- **Legacy databases fail startup or lose data.** Mitigation: leave migrations
  unchanged and run the reopen/full-row-snapshot test around an ordinary
  session write.
- **A background path still mutates legacy rows.** Mitigation: keep board CRUD
  absent from the store boundary, keep the goal reconciler/comment bridge out
  of `spawn_background_workers`, and ensure tracker functions have no store
  parameter.
- **Historical `provider: "board"` metadata halts an autonomous run.**
  Mitigation: retain string deserialization and route it through the bounded
  unknown-provider best-effort result tested in task sink and MCP tests.
- **Current documentation drifts back toward a board fallback.** Mitigation:
  the named SDD/docs structural test covers both embedded live playbooks and
  current API/data-model docs in `cargo test --workspace --lib`.
- **The removal expands into tracker setup or navigation redesign.**
  Mitigation: preserve the existing project scope, clients, provider views,
  direct launch behavior, and settings-linked empty state; no new abstraction
  or route is introduced.

## Developer Order

1. Complete the Tasks UI cleanup and focused structural tests.
2. Verify the real router's 404 matrix and external-only tracker selection and
   transition seams across every named caller.
3. Verify legacy storage is inert while retaining migrations and `card_id`
   compatibility.
4. Complete current docs/live playbooks and add their structural test.
5. Run all focused tests, `cargo test --workspace --lib`, and
   `npm run build --prefix crates/agentum-desktop/ui`.
