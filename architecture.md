# Spec 446 — Architecture

- **Spec:** `446-remove-internal-workspace`
- **Phase:** Architect
- **Date:** 2026-07-27
- **Base:** `ce6213f5`
- **Verdict:** ready for decomposition

Every production seam cited below was read at the base above. Test names in
backticks are planned additions to the cited existing or new test module.

## Components

### 1. Confirmed workspace tracker transition and reconciliation API

Change `crates/agentum-server/src/routes/worktrees.rs`, whose `router` already
owns the worktree metadata endpoints and the existing GitHub reconciliation
route (`:29-48`), and whose registry row already stores `tracker_provider`,
`tracker_url`, and `tracker_phase` (`:50-81`). Add one narrow manual-write route:

- `POST /api/worktrees/transition-tracker` accepts a registered `worktreeId`
  and one canonical `targetPhase`.
- The handler reads the registered row; rejects a missing/unsupported provider,
  missing URL, unknown worktree, or invalid phase before any write; derives the
  existing provider-specific tracker id; and calls
  `task_sink.rs::apply_tracker_transition` with `AppState.bus` and the worktree
  id.
- Before dispatch, the manual route proves the target is mapped for this board.
  GitHub must have a real Project binding, and the target option id must map
  uniquely back to the requested phase through
  `StatusMapping::tracker_phase_for_option_id`; this prevents the shared
  task-sink's intentional unbound-label path or legacy InReview fallback from
  masquerading as a Project move. Linear's configured target name must likewise
  map uniquely back to the requested phase; team-level absence is then reported
  by the existing transition seam as `Skipped`.
- Only `TransitionResult::Applied` may call the existing
  `worktrees.rs::persist_tracker_progress` (`:293-314`). `Skipped` becomes a
  non-2xx response carrying its reason; a transport error remains a non-2xx
  response. Neither branch passes through `update_meta`, and no branch reads or
  writes `workspaceStatus`.
- A success response is returned only after the canonical phase has been
  persisted. Backward manual moves are allowed: the monotonic guard in
  `tracker_sync.rs::next_phase_write` is automation policy, not manual board
  policy.

Reuse, do not edit, `crates/agentum-server/src/task_sink.rs::TrackerPhase`,
`parse_tracker_phase`, `TrackerEmit`, `TransitionResult`, and
`apply_tracker_transition` (`:175-230`, `:836-926`). That seam already maps
GitHub to its bound Project option and Linear to its workflow state and emits
`tracker.phase_changed` only for `Applied`; `Skipped`/errors emit
`tracker.sync_pending`. Reuse `tracker_sync.rs::resolve_binding` and promote
the already-tested private `tracker_id_for` (`:135-190`) to `pub(crate)` rather
than duplicating its Linear-identifier/GitHub-URL rule. No automated reactor or
poller behavior changes.

Provider reads also need to translate live evidence back to a canonical phase:

- Keep `POST /api/worktrees/reconcile-github-status` and its existing option-id
  validation through
  `github_projects.rs::StatusMapping::tracker_phase_for_option_id`
  (`worktrees.rs:334-384`; `github_projects.rs:75-120`). Type its client result,
  but do not change its wire shape.
- Add sibling `POST /api/worktrees/reconcile-linear-status`, accepting
  `worktreeId` and the live Linear `stateName`. Add
  `linear.rs::LinearStateMap::tracker_phase_for_name` beside `name_for`
  (`linear.rs:185-274`), using the same trimmed, case-insensitive matching rule
  as `match_state_by_name`. Zero or multiple configured phase-name matches are
  unmapped and return `{ "reconciled": false, "phase": null }` without a
  registry write. A unique match reuses
  `apply_confirmed_tracker_phase`, including legitimate backward refreshes.

Focused server coverage lives in the existing `worktrees.rs::tests` and
`linear.rs::tests`: `workspace_board_tracker_rejects_invalid_or_unlinked_rows`,
`workspace_board_tracker_rejects_missing_or_ambiguous_mappings`,
`workspace_board_tracker_persists_only_applied_results`,
`workspace_board_tracker_keeps_failed_results_non_mutating`, and
`workspace_board_tracker_linear_state_name_maps_uniquely`. Retain the existing
`task_sink.rs::tests::pinned_provider_dispatches_to_matching_tracker_arm`,
`github_transition_with_board_bound_strips_label_no_add`, and
`github_transition_with_board_board_failure_falls_back_to_label` as the
provider-dispatch and GitHub Project-write proofs.

### 2. Reusable external-status reads for board refresh

Change
`crates/agentum-desktop/ui/src/components/sidebar/IssueProjectStatusChip.tsx`.
Its `fetchBinding`, `fetchStatus`, app-session caches, and
`resolveIssueProjectStatus` call are the actual GitHub Project Status read
(`:35-86`, `:92-181`). Export one imperative resolver over those same caches
and fetchers, and have `useIssueProjectStatus` delegate to it. The new board
consumer requests a forced refresh on board open/focus; the existing chip keeps
its stale-while-revalidate behavior and event subscription unchanged.

For Linear, reuse
`crates/agentum-desktop/ui/src/runtime/runtime-linear-client.ts::linearGetIssue`
(`:190-204`), the uncached primitive beneath the existing
`WorktreeCard.tsx` refresh effect (`WorktreeCard.tsx:373-391`). A board refresh
must not be suppressed by the ordinary card cache, because reopening the board
is the acceptance-level refresh action.

Add
`crates/agentum-desktop/ui/src/components/sidebar/workspace-kanban-tracker-board.ts`
as a board-specific, IO-injected orchestration module. It has only the two
operations this spec needs:

1. Refresh each visible, supported linked worktree through the existing GitHub
   or Linear read, then send the returned option id/state name to the matching
   worktree reconciliation endpoint. It returns canonical phases and per-card
   warnings; a generation token prevents an older open/focus request from
   overwriting a newer result.
2. Commit one requested canonical move through the new transition endpoint.
   It returns a new phase only for a confirmed success. Unlinked, unsupported,
   multi-card, unmapped-target, skipped, and rejected outcomes retain the input
   phase and return an actionable error descriptor.

The typed client functions and `{ reconciled, phase }` /
`{ applied, phase }` response types belong beside the existing worktree calls
in `runtime/server-worktree-client.ts` (`:19-34`). No new store slice, event
socket, polling loop, provider registry, or generic tracker client is added.

### 3. Fixed tracker-authoritative Workspace board

Change
`crates/agentum-desktop/ui/src/components/sidebar/workspace-kanban-worktree-groups.ts`.
Replace its call to `getWorkspaceStatus` (`:16-41`) with a board-only lane
resolver. Export a fixed ordered lane definition for the five existing
`TrackerPhaseWire` values—`Todo`, `In Progress`, `In Review`, `Ready to Test`,
and `Done`—plus one non-lifecycle `Unlinked` lane. A worktree is unlinked when
its provider is not `github`/`linear` or its tracker target is empty. A linked
worktree uses the freshly reconciled phase when present, otherwise its last
confirmed `trackerPhase`; a newly created linked row with no written phase uses
the server's existing Todo baseline (`worktrees.rs::create` records `None` as
Todo until the first transition). `workspaceStatus` is not an input. Existing
pinned/recent/manual ordering within the resolved lanes stays intact.

Change `WorkspaceKanbanDrawer.tsx`, the current owner of grouping and all drag
commit callbacks (`:47-258`, `:464-473`, `:592-647`):

- use the fixed tracker lanes and refresh orchestration while the drawer is
  open and when the window becomes visible/focused;
- seed with confirmed `trackerPhase`, then replace it only with current
  provider-read/reconciliation results;
- make every cross-lane drop pessimistic: do not change the local lane or
  manual rank until `transition-tracker` confirms success;
- after success, update the board's confirmed-phase map and then apply any
  existing manual-order update, never a `workspaceStatus` update;
- for a same-lane reorder, preserve the existing `manualOrder` behavior without
  making an external transition;
- reject selected batches with “Move one workspace at a time,” preserving
  pinning and selection but performing no bulk tracker writes;
- for an unlinked source, show an error toast with a **Link tracker** action
  that reuses the existing repo-scoped Project Integrations navigation pattern
  from `ProjectTasksPage.tsx:167-175`; and
- for skipped, unavailable, or unmapped moves, show the returned error and keep
  the prior phase/rank. Do not call the optimistic
  `store/slices/worktrees.ts::updateWorktreeMeta` lifecycle path
  (`:1484-1623`).

The current arbitrary status editor (`WorkspaceKanbanDrawer.tsx:381-461` and
`WorkspaceKanbanSettingsMenu.tsx:108-182`) cannot describe external mappings.
Reduce `WorkspaceKanbanSettingsMenu.tsx` and
`WorkspaceKanbanDrawerHeader.tsx` to the existing column-layout setting plus
filter/close controls. Preserve the legacy configured statuses elsewhere.
Likewise, remove the lane-local “new workspace in status” controls from
`WorkspaceKanbanLaneGrid.tsx` and `WorkspaceKanbanStatusLane.tsx`; that flow
only stamps `initialWorkspaceStatus` and would falsely imply an external phase.
The ordinary New Workspace surfaces remain available outside the lanes.

Keep the existing selection, area selection, resize, pointer/native document
drop, drag preview, pin target, outside-dismiss, card, and manual-rank helpers.
Their callbacks remain the mechanics; only the lifecycle commit supplied by
the drawer changes.

### 4. Explicit boundaries

The following stay untouched:

- `crates/agentum-desktop/ui/src/shared/workspace-statuses.ts`, the persisted
  `workspaceStatuses` UI preference, `WorktreeList` grouping,
  `WorktreeContextMenu`, `workspace-kanban-sidebar-drop.ts`, and the legacy
  `workspaceStatus` field. They remain private organization metadata outside
  this Workspace board and are not deleted or migrated.
- `crates/agentum-server/src/task_sink.rs` transition/mapping behavior,
  `github_projects.rs` bindings, Linear/GitHub setup, credentials, and status
  mappings.
- Harness/session-driven transitions, `tracker_sync` rank/worker behavior,
  tracker attention, and event contracts. The only `tracker_sync.rs` change is
  visibility of the existing id helper.
- The retired `/api/board*` routes and `board_items`; no route, table, mirror,
  provider, bulk move, or background poller is restored.
- Pin/unpin, selection, card activation, filtering, column sizing, and sorting.

## APIs

### `POST /api/worktrees/transition-tracker` (new)

Request:

```json
{"worktreeId":"repo::/path","targetPhase":"in_review"}
```

Success, only after external `Applied` and local persistence:

```json
{"applied":true,"phase":"in_review"}
```

The target accepts exactly `todo | in_progress | in_review | ready_to_test |
done`. Invalid/unlinked input is a bad request; `Skipped` is a conflict with
its existing reason; transport or persistence failures are server errors. All
non-success responses use the existing `{ "error": "..." }` envelope. There
is no optimistic or pending success response.

### `POST /api/worktrees/reconcile-linear-status` (new)

Request:

```json
{"worktreeId":"repo::/path","stateName":"In Review"}
```

Response mirrors the existing GitHub reconciliation contract:

```json
{"reconciled":true,"phase":"in_review"}
```

An unknown or ambiguous configured state name returns
`{"reconciled":false,"phase":null}` and performs no write. Wrong-provider,
unregistered, or missing-link input is rejected.

### `POST /api/worktrees/reconcile-github-status` (preserved)

The existing `{ worktreeId, statusOptionId }` request and
`{ reconciled, phase }` response stay unchanged. The desktop client gains a
type for the response; mapping still uses stored option ids, never option-name
guessing.

## Data Flow

### Board open / external refresh

1. The drawer renders the five fixed canonical lanes and `Unlinked`; no saved
   workspace-status definition controls this list.
2. Unsupported/missing bind coordinates resolve directly to `Unlinked` and do
   no provider read.
3. Linked cards render from the last confirmed `trackerPhase` cold cache while
   one generation-scoped refresh runs for visible cards. A missing phase is the
   existing linked-workspace Todo baseline, not a `workspaceStatus` fallback.
4. GitHub reuses the Project binding/status resolver and returns a Status option
   id; Linear reuses `linearGetIssue` and returns a workflow-state name.
5. The server validates that provider evidence against the existing configured
   mapping, persists a uniquely confirmed canonical phase, and returns it.
6. Only the current refresh generation replaces the card's lane. A warning
   keeps the last confirmed lane and exposes the read/reconciliation error.

### Single-card lane move

1. The drawer resolves the source from its confirmed phase map and validates a
   single linked worktree plus a canonical target.
2. The server reloads the registered binding, parses the target, derives the
   existing tracker id, proves that the requested GitHub Project option or
   Linear configured state is uniquely mapped, and invokes
   `apply_tracker_transition` with the shared event bus.
3. GitHub Projects or Linear acknowledges the provider write. Only `Applied`
   persists `TrackerPhase::wire_str()` on the registry row.
4. The success response advances the board-local phase and then applies any
   manual rank change. The emitted event continues to update existing tracker
   chips.
5. A validation failure, `Skipped`, transport error, or unmapped target returns
   an error. The drawer never changed its source phase, `trackerPhase`,
   `workspaceStatus`, or cross-lane manual rank, so the card stays put.

## Important Decisions

### D1 — Fixed canonical lanes over configurable workspace-status columns

Choose the existing five `TrackerPhase` values plus `Unlinked` over adapting
user-authored workspace statuses. External mappings are defined only for those
five phases; allowing rename/add/remove/reorder as lifecycle semantics would
reintroduce unmapped or ambiguous local truth. Legacy status configuration is
retained for the other sidebar organization surfaces.

### D2 — Reuse client provider reads; validate mappings on the server

Choose the existing GitHub Project Status resolver and Linear issue read over a
new aggregate server read API. Those paths already handle the desktop/runtime
credential locations and remote-runtime selection. Send only provider evidence
(option id/state name) to the server, where the authoritative configured
mapping and registry already live; do not duplicate mapping configuration in
TypeScript.

### D3 — Pessimistic moves over optimistic rollback

Keep the card in its source lane until the server confirms `Applied` and local
persistence. This costs one visible transition round trip, but it directly
enforces the invariant that a failed external write never invents progress and
avoids brittle rollback across pointer drag, native drag, and event refreshes.

### D4 — One thin worktree route over a new tracker service abstraction

The server already has a closed provider dispatch in
`apply_tracker_transition`. A worktree-owned handler plus one inverse Linear
mapping method is sufficient. A provider trait, registry, queue, bulk endpoint,
or `/api/board` replacement would be speculative and is deliberately absent.

### D5 — Reuse existing settings navigation for the unlinked action

Choose the repo-scoped Project Integrations target already used by
`ProjectTasksPage` instead of adding an existing-workspace binding wizard in
this slice. The action is truthful and actionable while respecting the
non-goal of redesigning tracker setup; the card remains `Unlinked` until valid
bind coordinates are present.

## Acceptance Criteria → Plan and Tests

| Criterion | Named plan part | Named verification |
| --- | --- | --- |
| AC1 — five external-phase lanes; no `workspaceStatus` lifecycle read | Component 3 fixed tracker-authoritative board | `workspace-kanban-worktree-groups.test.ts::renders_the_five_canonical_tracker_lanes_in_order`; `::tracker_phase_overrides_contradictory_workspace_status` |
| AC2 — GitHub card move updates Project and persists confirmed phase | Components 1 and 3 single-card move | `worktrees.rs::tests::workspace_board_tracker_persists_only_applied_results`; existing `task_sink.rs::tests::github_transition_with_board_bound_strips_label_no_add`; `workspace-kanban-tracker-board.test.ts::confirmed_github_move_commits_phase_without_workspace_status` |
| AC3 — Linear card move updates workflow and persists confirmed phase | Components 1 and 3 single-card move | `linear.rs::tests::workspace_board_tracker_linear_state_name_maps_uniquely`; existing `task_sink.rs::tests::pinned_provider_dispatches_to_matching_tracker_arm`; `workspace-kanban-tracker-board.test.ts::confirmed_linear_move_commits_phase_without_workspace_status` |
| AC4 — refreshed external changes replace the lane | Component 2 provider refresh and Component 3 generation-scoped phase map | `workspace-kanban-tracker-board.test.ts::refresh_reconciles_github_and_linear_external_phases`; `::stale_refresh_cannot_replace_a_newer_phase` |
| AC5 — explicit Unlinked state and blocked move with action | Component 3 `Unlinked` lane and D5 settings action | `workspace-kanban-worktree-groups.test.ts::unlinked_worktree_ignores_legacy_workspace_status`; `workspace-kanban-tracker-board.test.ts::unlinked_drop_opens_tracker_settings_without_any_write` |
| AC6 — rejected/unavailable/unmapped leaves all lifecycle fields unchanged | Components 1 and 3 pessimistic failure flow | `worktrees.rs::tests::workspace_board_tracker_keeps_failed_results_non_mutating`; `workspace-kanban-tracker-board.test.ts::failed_or_unmapped_move_reports_error_and_keeps_prior_phase_and_rank` |

## Risks

- **Provider refresh fan-out can consume rate limit or delay a large board.**
  Mitigation: refresh only visible linked worktrees, reuse the GitHub binding
  and status caches, use one uncached Linear read per visible Linear card only
  on open/focus, and add no interval poller.
- **An older asynchronous refresh can move a card after a newer refresh or
  manual transition.** Mitigation: generation-token every refresh and discard
  stale completions; a confirmed manual move increments/invalidates that
  generation before committing its phase.
- **A configured mapping can be ambiguous or missing.** Mitigation: GitHub
  retains its existing unique option-id check and the manual route requires a
  Project binding plus an exact round trip for the requested phase; Linear uses
  the same unique inverse rule before a move. Ambiguous/unmapped evidence
  returns null/error and never writes a phase.
- **The current drag system can carry multiple selected worktrees.**
  Mitigation: validate `worktreeIds.length === 1` before any external call,
  show the single-card message, and retain bulk pinning only.
- **External success and local registry persistence are not transactional.**
  Mitigation: persist immediately after `Applied`, return success only after
  persistence, keep the emitted `tracker.phase_changed` evidence, and force the
  next provider refresh to reconcile the external truth. This residual split
  is accepted because GitHub/Linear plus a local JSON registry cannot share an
  atomic transaction; writing the local phase first would violate the stronger
  external-truth invariant.
- **Removing arbitrary lane editing could be mistaken for deleting the legacy
  organization feature.** Mitigation: limit the removal to this drawer's
  settings/create controls; preserve the preference, metadata, sidebar groups,
  context menu, migrations, and all non-board consumers unchanged.
- **The Link tracker action does not create a second binding workflow.**
  Accepted because tracker setup/linking changes are an explicit non-goal; the
  action reuses the existing repo-specific Project Integrations surface and the
  board remains honestly `Unlinked` until that existing workflow supplies bind
  coordinates.

## Targeted Verification

```sh
cargo test -p agentum-server --lib workspace_board_tracker
cd crates/agentum-desktop/ui && npm exec vitest run \
  src/components/sidebar/workspace-kanban-worktree-groups.test.ts \
  src/components/sidebar/workspace-kanban-tracker-board.test.ts \
  src/components/sidebar/WorkspaceKanbanDrawerHeader.test.tsx
cargo test -p agentum-server --lib
npm run build --prefix crates/agentum-desktop/ui
```
