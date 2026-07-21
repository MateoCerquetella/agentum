# Spec 025 — Project-scoped tracker contract

- **Number:** 025
- **Status:** Developer (F1 in progress; F2–F4 pending)
- **Surface:** `crates/agentum-server`, `crates/agentum-store`, `crates/agentum-desktop/ui`
- **Author:** Codex (from Mateo's direct ask via Agentum SDD)
- **Date:** 2026-07-21

## Problem

An Agentum project does not have one authoritative tracker configuration. The
provider is stored on the repo, GitHub Project automation is stored separately
by repository slug, Tasks browsing choices live in UI settings keyed by repo ID,
and the selected ticket is copied onto each worktree. As a result, the Tasks
surface, issue creation, status automation, and project settings can disagree or
briefly show another project's tracker after a project switch.

## Goal

Make each Agentum project own one canonical tracker configuration that every
task surface and automation path resolves before reading, creating, or updating
tickets.

## Users / personas

- **Multi-project operator:** moves between local and SSH projects connected to
  different GitHub Projects or Linear workspaces and expects the visible tasks,
  new-ticket destination, and automated status writes to remain inside the
  selected project.
- **Project maintainer:** configures a tracker from Project Settings or the Tasks
  surface and expects both entry points to show and edit the same durable state.

## Acceptance criteria

1. `GET` of a project's tracker configuration returns one typed, provider-aware
   contract keyed by `Repo.id`, including provider (`github`, `linear`, or
   unconfigured), provider target identity, and the automation options needed by
   that provider; it never consults another repo's configuration or a global
   active-project fallback.
2. Saving or clearing tracker configuration from either Project Settings or the
   project's Tasks surface persists through the same server write path, and a
   read from the other surface immediately returns the identical revision.
3. Opening or switching Project Hub → Tasks resolves the selected project's
   configuration before fetching rows. While resolution is pending it renders a
   project-scoped loading state; when unconfigured it renders an honest setup
   state; rows cached or returned for another `Repo.id` never render.
4. Task listing, filtering, issue creation, issue linking, and manual refresh use
   the resolved project's provider target. GitHub and Linear selections cannot
   fall back to a globally last-used Project, workspace, team, view, or query.
5. A workspace created from a project inherits the project's provider and stores
   only the linked work item's immutable execution coordinates
   (`trackerProvider` + `trackerUrl`). Later project-configuration edits affect
   new selections and project-level browsing but do not silently retarget an
   existing workspace to a different ticket.
6. Harness/SDD status transitions resolve the parent project's canonical
   tracker configuration and the feature/worktree ticket coordinates, then
   either update that exact target or fail closed with an actionable error;
   ambiguity never selects the sole/global binding as a convenience fallback.
7. Existing installs migrate deterministically on first project-scoped read or
   explicit save: an existing server GitHub binding remains authoritative for
   GitHub automation; `Repo.trackerProvider` supplies the provider preference;
   matching per-repo UI selections may fill missing display/browse fields; and
   global legacy selections are never assigned to a project automatically.
   Migration is idempotent and preserves unknown repo fields.
8. Local and SSH projects use the same `Repo.id` contract and route provider
   discovery/mutations through the repo's existing runtime host. A response for
   one host or repo cannot populate another project's cache.
9. Removing a project deletes or detaches only that project's tracker
   configuration and project-scoped task preferences. Other projects' configs,
   tickets, cached rows, and worktree ticket links remain byte-unchanged.
10. Global settings may define explicit defaults for newly added projects, but
    runtime task resolution never treats global last-used state as a project's
    configuration; project settings display whether a value is configured,
    inherited as a creation-time default, migrated, or missing.

## Scope & non-goals (YAGNI)

- **In:** a canonical per-project tracker contract and server read/write/clear
  seam; GitHub/Linear provider target data; Project Settings and embedded Tasks
  consumers; per-project task view state; workspace inheritance; status-sync
  resolution; legacy migration; local/SSH routing; project deletion cleanup.
- **Out:** adding tracker providers, replacing GitHub/Linear clients, changing
  ticket schemas on those services, synchronizing the same project to multiple
  trackers simultaneously, moving an existing workspace between tickets,
  redesigning the Kanban visuals, or changing agent launch/streaming behavior.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `Repo` and the generic repo PATCH round-trip
  (`crates/agentum-desktop/ui/src/shared/types.ts:86-111`,
  `crates/agentum-server/src/routes/repos.rs:45-69,964-1004`) — `Repo.id` remains
  project identity and unknown fields must continue to survive migration.
- GitHub `BoardBinding`, status mapping, locked read-modify-write helpers, and
  host-aware binding routes (`crates/agentum-server/src/github_projects.rs:121-243`,
  `crates/agentum-server/src/routes/github_projects.rs`) — adapt behind the
  canonical contract rather than reimplement GitHub Projects behavior.
- Repo-scoped binding cache and fail-closed loading lifecycle
  (`crates/agentum-desktop/ui/src/components/project-hub/ProjectHubPage.tsx:83-132`,
  `crates/agentum-desktop/ui/src/store/slices/github.ts:1347-1370`) — generalize
  it to the provider-aware contract and revision key.
- Existing Tasks provider clients, caches, and picker models
  (`crates/agentum-desktop/ui/src/components/TaskPage.tsx`,
  `crates/agentum-desktop/ui/src/store/slices/github.ts`,
  `crates/agentum-desktop/ui/src/runtime/linear-client.ts`) — keep their network
  and dedupe primitives; replace only target resolution and cache ownership.
- Worktree ticket coordinates
  (`crates/agentum-desktop/ui/src/store/slices/worktrees.ts:1025-1110`) and
  `tracker_sync` / harness transition paths
  (`crates/agentum-server/src/tracker_sync.rs`,
  `crates/agentum-server/src/harness/drive.rs`) — preserve exact-ticket linkage
  while removing ambiguous binding fallback.

### Build new

- One typed `ProjectTrackerConfig` domain contract with an explicit schema
  version/revision, provider-specific targets, and configured/migrated/missing
  provenance.
- Server endpoints to read, atomically replace, and clear tracker configuration
  by `Repo.id`, resolving the repo's local or SSH runtime before provider work.
- A deterministic legacy adapter/migration for `trackerProvider`,
  `github_projects.json`, `activeProjectByRepo`, and `linearContextByRepo`; no
  migration path consumes global `activeProject` or `linearContext`.
- One UI project-tracker store keyed by `Repo.id` + config revision, with
  request-generation guards for repo/host switches; Project Settings and Tasks
  share its actions and selectors.
- Project-scoped task preferences (query, preset, chosen provider view, hidden
  fields) whose lifecycle follows the project instead of one global resume blob.

## Risks & invariants

- **No cross-project leakage:** every async request, cache entry, and mutation is
  keyed by `Repo.id` plus the resolved host/target and rejects stale responses.
- **One durable writer:** configuration writes are atomic server operations;
  components never coordinate multiple persisted blobs themselves.
- **Migration safety:** legacy/global data is retained until successful
  canonical persistence, migration is repeatable, and unknown repo fields are
  preserved. Ambiguous global state remains unassigned.
- **Ticket immutability:** project configuration identifies where to browse and
  create; a worktree/feature's stored ticket URL identifies what automation may
  update. Neither silently rewrites the other.
- **Host isolation:** SSH discovery and mutations use existing repo-aware runtime
  routing; local paths are never executed against a remote target or vice versa.
- **Architecture principles:** use existing provider clients and MCP/API seams;
  do not add polling, bypass `spawn_agent_into_pane`, alter YOLO translation, or
  change push-based terminal streaming.

## Harness wiring (the gate)

- **feature_list.json entries:**
  1. `project-tracker-contract` — typed server model, repo-scoped CRUD, revision,
     host resolution, and deterministic legacy read/migration.
  2. `shared-project-tracker-configuration` — Project Settings and Tasks read and
     mutate the same contract; repo-switch races and empty/loading states are
     fail-closed.
  3. `project-scoped-task-consumers` — listing, creation, refresh, and view
     preferences resolve only the active project's provider target.
  4. `workspace-and-transition-inheritance` — workspaces keep exact ticket
     coordinates while harness/SDD transitions use canonical project config
     without global/sole-binding fallback; deletion cleanup is isolated.
- **`verify.sh` asserts:** Rust migration/CRUD tests cover two projects with
  different providers, local/SSH routing, revision conflicts, idempotence,
  unknown-field preservation, deletion isolation, and no sole-binding fallback;
  focused Vitest covers shared settings/tasks state, immediate repo switching,
  stale response rejection, project-scoped preferences, and immutable existing
  workspace links; Vite build and relevant `agentum-server`/`agentum-store` lib
  tests are green.
- **`qa.sh` asserts:** configure project A for GitHub and project B for Linear
  from different entry points, switch repeatedly and reload, then capture that
  each Tasks surface lists/creates only in its configured target; start linked
  workspaces and observe exact-ticket transitions; repeat one project over SSH;
  clear/delete A and verify B remains unchanged.

## Open questions

- None blocking. The architect may choose the physical persistence layout, but
  `Repo.id` ownership, one server write path, deterministic migration, and the
  no-global-fallback behavior are locked by this spec.
