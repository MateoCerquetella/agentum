# Spec 027 — Architecture

- **Spec:** `027-remove-internal-workspace-board`
- **Phase:** Architect
- **Date:** 2026-07-22
- **Verdict:** ready for decomposition

All existing seams cited below were read in this worktree. A file described as
"delete" or "change" exists today; test names in backticks are planned test
cases added to the cited existing test module.

## Components

### 1. Desktop Tasks surfaces

- Change `crates/agentum-desktop/ui/src/components/TaskPage.tsx`, whose existing
  `syncTasksToBoard` callback imports `syncExternalIssues`, maps loaded GitHub or
  Linear issues to cards, and renders both “Send ... to the Board” buttons.
  Remove that callback, its state/imports, and both buttons. Preserve the
  existing GitHub/Linear issue lists, external GitHub Projects and Linear
  kanban layouts, work-item dialogs, refresh actions, and direct launch paths.
- Delete `crates/agentum-desktop/ui/src/runtime/board-client.ts`. Its only
  production export is the internal `POST /api/board/sync` client and repository
  search found `TaskPage.tsx` as its only importer.
- Keep and reuse
  `crates/agentum-desktop/ui/src/components/project-hub/ProjectTasksPage.tsx`.
  Its `ProjectTasksPage` effect already dispatches only
  `repo.trackerProvider === 'github'` or `'linear'`; its unbound/unavailable
  branch already gives an explicit message and `openSettings` action. Adjust
  the text from ambiguous “project board” to “configured tracker” so the empty
  state cannot be mistaken for the retired internal board.
- Extend the existing
  `crates/agentum-desktop/ui/src/components/project-hub/ProjectTasksPage.test.tsx`
  with `renders_only_external_tracker_sources_or_the_settings_empty_state` and
  `global_tasks_has_no_internal_board_sync_affordance`. These structural tests
  pin the provider branches, empty-state/settings copy, absence of the client
  import and sync labels, and continued presence of the external source views.

Boundary: `crates/agentum-desktop/ui/src/lib/board-route.ts`,
`board-project-resolution.ts`, `components/github-project/`, Linear kanban
components, and `components/sidebar/WorkspaceKanbanDrawer.tsx` remain. Those
existing “board” names refer to routing into project Tasks, GitHub Projects,
tracker presentation, or workspace-container status—not Agentum-owned work-item
cards. There is no UI rename or tracker redesign in this spec.

### 2. Retired server routes and extracted shared helpers

- Change `crates/agentum-server/src/lib.rs::router` to remove the existing
  merges for `routes::board`, `board_goals`, `board_links`, `board_rules`, and
  `board_sync`. With no replacement route or fallback, Axum's existing router
  behavior returns `404 Not Found` for the whole `/api/board*` subtree.
- Remove those five declarations from
  `crates/agentum-server/src/routes/mod.rs`, then delete the existing files
  `routes/board.rs`, `routes/board_goals.rs`, `routes/board_links.rs`,
  `routes/board_rules.rs`, and `routes/board_sync.rs`.
- Before deleting `board_goals.rs`, move its existing `SlugReason`,
  `resolve_github_slug`, and private slug validation into
  `crates/agentum-server/src/routes/util.rs`, beside the existing
  `resolve_tracker_slug`. Update the real consumers in `routes/chat.rs`,
  `routes/github.rs`, and `routes/repos.rs`. Move the GitHub-only sink error
  classification needed by `routes/github.rs::create_issue` into that module;
  the Linear and board-goal classifications disappear with their only caller.
  Move the existing slug tests from `board_goals.rs` into `util.rs` rather than
  losing coverage.
- Delete `crates/agentum-server/src/rules.rs` and its `mod rules` declaration in
  `lib.rs`; repository search found only the deleted board and board-rules
  handlers calling it.
- Remove the retired-contract integration tests
  `crates/agentum-server/tests/board_server_sync_016a.rs`,
  `goal_cards_end_to_end.rs`, and `card_session_binding_e2e.rs`. They assert
  success for endpoints that must now be absent.
- Add `internal_board_route_families_are_unregistered` to the existing
  `crates/agentum-server/src/lib.rs::tests` module. Build the real `router` and
  assert 404 for every family root plus a representative nested path, including
  `/api/board/{id}`, `/api/board/goals/{id}/harness-plan`,
  `/api/board/links/...`, `/api/board/rules/{column}`, and
  `/api/board/bindings/{id}/sync`.

Boundary: route registration for `/api/github-projects`,
`/api/project-trackers`, `/api/github`, `/api/harness`, `/api/sdd`, sessions,
and events is unchanged. In particular, GitHub Projects is an external tracker
API and is not part of the `/api/board*` deletion.

### 3. External-only creation and transition seam

- Change `crates/agentum-server/src/task_sink.rs` while retaining its existing
  closed-enum pattern:
  - remove `TaskSink::Board`, the `NewBoardItem` import, and the board branch of
    `TaskSink::create_feature`;
  - remove `TaskSink::pick_provider`, `TaskSink::select`,
    `TrackerChoice::forced_sink`, and `TrackerChoice::resolve_sink`. Repository
    search found the only production `resolve_sink` call in the deleted
    `routes/board_goals.rs`; explicit GitHub and Linear creation callers already
    choose their provider, while `routes/harness.rs::resolve_tracker_pin` only
    needs `parse_tracker_choice`;
  - remove `store` and `parent_goal_id` from the existing `SinkCtx`; neither
    external creation branch uses them. Update its existing callers in
    `routes/chat.rs` and `routes/github.rs`. This makes a board write impossible
    through feature creation by type signature rather than convention;
  - remove the `&Store` argument from `apply_tracker_transition`,
    `transition_inner`, `apply_blocked_transition`, and `blocked_inner`, then
    delete the `"board"` match arm and `board_status_for`. Update all existing
    call sites in `harness/drive.rs`, `tracker_attention.rs`, `tracker_sync.rs`,
    `routes/harness.rs`, and `routes/mcp.rs`. GitHub/Linear behavior and event
    emission remain byte-for-byte at the same seam;
  - after the dedicated board arm is gone, legacy `provider: "board"` follows
    the existing unknown-provider best-effort path: no database access, a
    `tracker.sync_pending` result/event where applicable, and no harness halt.
- Change `crates/agentum-server/src/routes/mcp.rs`: advertise only `github` and
  `linear` in `agentum_tasks_report_status`; remove the board-card test fixture
  and cover external delegation plus a bounded, non-writing legacy-provider
  result.
- Keep the calls made by `harness/drive.rs`, `routes/harness.rs`,
  `tracker_sync.rs`, and `tracker_attention.rs`; only their now-unneeded store
  argument and stale board wording change. Keep the provider string wire fields
  in `harness/types.rs` and
  `crates/agentum-desktop/ui/src/runtime/harness-client.ts` so old harness files
  deserialize, but remove comments that advertise `board` as supported.
- In the existing `task_sink.rs::tests`, remove board-only cases and add/rename
  focused cases `only_github_and_linear_are_creation_sinks`,
  `github_and_linear_transitions_keep_existing_results_and_events`, and
  `legacy_board_provider_is_non_mutating_and_best_effort`. Retain the existing
  fake-`gh` and isolated Linear credential seams used by
  `pinned_provider_dispatches_to_matching_tracker_arm`.

This component deliberately does not introduce a provider trait, registry, or
new “no tracker” sink. The only automatic sink selection caller is being
deleted; project UI already owns the no-tracker state and remaining creation
routes choose GitHub or Linear explicitly.

### 4. Remove runtime persistence and reconciliation; retain schema tolerance

- In `crates/agentum-server/src/lib.rs::spawn_background_workers`, stop spawning
  the existing `agentum_watchdog::run_goal_reconciler` and
  `run_session_comment_bridge`. Preserve the ordinary watchdog plus external
  `tracker_sync` and `tracker_attention` workers.
- Delete `crates/agentum-watchdog/src/reconciler.rs` and `comment_bridge.rs`,
  their module declarations/re-exports in `agentum-watchdog/src/lib.rs`, and the
  board-only tests in that file. Session activity, compaction, crash detection,
  and their tests remain untouched.
- Delete `crates/agentum-store/src/board.rs` and `binding.rs`, remove their module
  declarations and board-only tests from `agentum-store/src/lib.rs`, and remove
  the now-unused `Store::get_session_by_card_id` from `sessions.rs`. After the
  route, MCP, sink, and watchdog removals above, repository search found no
  non-board consumer of these methods.
- Remove `crates/agentum-core/src/board_schema.rs`, its re-exports, and the
  internal `BoardItem`, board patch/comment/link/rule/binding types from
  `agentum-core/src/lib.rs` after their consumers are removed. Retain
  `Session.card_id` and the corresponding session row field as deprecated
  read/write compatibility: historical serialized sessions and the additive
  sessions column still contain it, while all surviving session constructors
  already pass `None`.
- Do not edit any file under `crates/agentum-store/migrations/`. The existing
  `Store::open` runs the embedded migration set, including the historical board
  tables and indexes. Old rows remain on disk, but no surviving runtime module
  exposes their CRUD or reconciliation.
- Add `legacy_board_rows_survive_reopen_and_normal_store_work_is_inert` to the
  existing `agentum-store/src/lib.rs::tests`: seed a representative legacy row
  through `Store::pool`, snapshot it, close/reopen through `Store::open`, perform
  a normal session/store operation, and assert the row is byte-for-byte
  unchanged. The server route test proves it is not returned, and the
  store-free tracker signatures prove external transitions cannot mutate it.

Boundary: historical migrations and user data are retained; no DROP, copy, or
data rewrite is introduced. External tracker binding/config persistence is
unrelated and remains. `Session.card_id` is compatibility-only, not a supported
board lookup or binding API.

## APIs

### Removed HTTP API

Every method at or below these route families becomes unmatched and returns
`404 Not Found`:

- `/api/board`
- `/api/board/goals`
- `/api/board/links`
- `/api/board/rules`
- `/api/board/bindings`

There is no tombstone, redirect, or `410` handler. The client call is removed,
and an unregistered route is the acceptance contract.

### Preserved API and internal signatures

- GitHub Issues/Projects, Linear, project-tracker, harness, SDD, session, and
  event wire contracts stay unchanged.
- `TaskSink` remains the existing closed enum, now with only `Github` and
  `Linear`. `SinkCtx` keeps only the existing `workdir` and optional GitHub
  `slug` inputs.
- `apply_tracker_transition` and `apply_blocked_transition` retain provider,
  tracker identity, phase/comment inputs, result types, and event semantics;
  only the unused/internal-board `Store` argument is removed.
- `agentum_tasks_report_status` retains its tool/result shape and advertises
  external providers only. Legacy `board` input is non-writing and best-effort.
- SQLite board tables remain compatible storage artifacts with no application
  API.

## Data Flow

### Project Tasks

1. `ProjectTasksPage` reads the project's persisted tracker provider.
2. GitHub resolves its existing project binding and renders
   `ProjectViewWrapper`; Linear resolves its existing workspace/project and
   renders `LockedLinearProjectTasks`.
3. No provider or an unavailable binding renders the existing settings-linked
   empty state with clarified tracker wording.
4. No desktop path imports `board-client.ts`, calls `/api/board*`, or mirrors an
   issue into `board_items`.

### Harness/SDD tracker lifecycle

1. Existing harness metadata carries `tracker_provider`, stable id, and URL.
2. Existing harness, SDD-driven harness, MCP, sync, and attention callers invoke
   the same transition seam without a store parameter.
3. GitHub and Linear execute their existing provider branches and emit the
   existing success or pending events. A missing/legacy provider follows the
   existing best-effort unsupported-provider behavior.
4. No creation or transition signature has access to board persistence; harness
   gates, launch, retry, settle, verification, and QA behavior are unchanged.

### Legacy database startup

1. `Store::open` runs unchanged migrations.
2. Legacy board tables and rows remain present.
3. No router, background worker, task sink, MCP provider, core board model, or
   public store method reads or writes them.
4. Normal session and external tracker work proceeds without data conversion or
   deletion.

## Important Decisions

### D1 — Remove the board capability at compile-time

Choose removal of store parameters from external creation/transition seams over
merely deleting the board match arms. The former makes an accidental future
`board_items` write impossible through the normal tracker pipeline and directly
supports the non-mutation acceptance criterion.

### D2 — Unregister routes instead of adding tombstones

Choose deletion/unregistration over compatibility handlers or `410 Gone`.
Axum's existing unmatched-route behavior produces the required 404, and keeping
handlers would preserve an operational internal-board boundary.

### D3 — Retain legacy tables instead of dropping data

Choose unchanged migrations and inert rows over a drop migration. Existing
databases continue to open and no user data is destroyed. Compatibility ends at
schema/session-field tolerance; callable CRUD and reconcilers are removed.

### D4 — Reuse external tracker primitives

Keep `TaskSink::{Github, Linear}`, `apply_tracker_transition`, the existing
project task scope, GitHub/Linear clients, provider-specific views, and empty
state. A trait/plugin layer, replacement cache, new tracker router, or new
empty-state component would be speculative for this removal.

### D5 — Preserve unrelated meanings of “board”

GitHub Projects, Linear kanban, filesystem-derived `HarnessBoard`, project Task
routing helpers, and the workspace-container Kanban drawer remain. They are
external tracker presentations or workspace layout, not internal board cards.

## Acceptance Criteria → Plan and Tests

| Criterion | Named plan part | Named test/verification |
| --- | --- | --- |
| Project Tasks shows configured GitHub/Linear or an explicit no-tracker state, with no cards/sync action | Component 1: Desktop Tasks surfaces | `ProjectTasksPage.test.tsx::renders_only_external_tracker_sources_or_the_settings_empty_state`; `::global_tasks_has_no_internal_board_sync_affordance`; browser QA for bound GitHub, bound Linear, and unbound project |
| All retired `/api/board*` route families, including nested paths, return 404 | Component 2: Retired server routes | `agentum_server::tests::internal_board_route_families_are_unregistered` |
| GitHub/Linear harness creation, selection, and transition preserve external results and never write `board_items` | Component 3: External-only creation and transition seam | `task_sink::tests::only_github_and_linear_are_creation_sinks`; `::github_and_linear_transitions_keep_existing_results_and_events`; retained `pinned_provider_dispatches_to_matching_tracker_arm`; route harness `resolve_tracker_pin_maps_d4`; compile-time absence of `Store` from both seams |
| Legacy rows survive startup and normal workspace/harness flows neither return nor mutate them | Components 2 and 4 | `agentum_store::tests::legacy_board_rows_survive_reopen_and_normal_store_work_is_inert`; server 404 matrix; `task_sink::tests::legacy_board_provider_is_non_mutating_and_best_effort` |
| Required workspace Rust tests and desktop build pass with focused checks | All components | focused tests above; `cargo test --workspace --lib`; `npm run build --prefix crates/agentum-desktop/ui` |

## Risks

- **External “board” features are removed accidentally.** Mitigation: deletions
  are limited to `/api/board*`, its client/card models/store methods/workers,
  and explicit board sink arms. D5 names external Projects, kanban, routing, and
  workspace layout that must remain; existing external-view tests remain green.
- **Deleting `board_goals.rs` breaks shared GitHub slug resolution.** Mitigation:
  move the already-existing resolver and enum into `routes/util.rs`, update all
  three real consumer modules, and move its host-aware tests before deletion.
- **A tracker caller is missed when the store argument is removed.** Mitigation:
  update the enumerated repository-search results in `harness/drive.rs`,
  `tracker_attention.rs`, `tracker_sync.rs`, `routes/harness.rs`, and
  `routes/mcp.rs`; the workspace compiler rejects any missed call site.
- **Legacy databases fail startup or lose data.** Mitigation: do not alter
  migrations; test migrated reopen plus byte-for-byte row preservation around a
  normal store operation.
- **A hidden background/runtime writer continues touching board rows.**
  Mitigation: remove both worker spawns/modules, remove callable Store board
  methods, and remove Store access from tracker seams. The remaining migration
  SQL is startup schema creation only.
- **Old harness metadata contains `provider: "board"`.** Mitigation: preserve
  provider strings in serialized harness types; let the existing unsupported
  best-effort path emit/report pending without a DB handle or halting the run.
- **The removal broadens into a tracker or navigation redesign.** Mitigation:
  preserve the existing project tracker scope, clients, views, direct launch,
  and session/harness contracts; only copy is clarified in the current empty
  state.

## Developer Order

1. Remove desktop sync UI/client and extend the existing Tasks structural test.
2. Move shared slug/error helpers, unregister/delete internal routes/rules, and
   add the real-router 404 matrix.
3. Make `TaskSink` external-only, remove store parameters, update every named
   caller, and update focused sink/MCP/harness tests.
4. Remove board workers, persistence APIs, and core types while retaining all
   migrations and `Session.card_id`; add the legacy reopen/inertness test.
5. Run focused tests, `cargo test --workspace --lib`, and
   `npm run build --prefix crates/agentum-desktop/ui`.
