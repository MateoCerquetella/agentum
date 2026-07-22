# Spec 025 — Architecture

- **Phase:** Architect
- **Date:** 2026-07-21
- **Verdict:** PASS — proceed in four ordered increments.

## Current-state findings

Tracker ownership is fragmented across four persistence domains:

1. `Repo.trackerProvider` is an untyped flattened key in `repos.json`
   (`shared/types.ts:108-111`, `routes/repos.rs:45-69,964-1004`).
2. GitHub Projects automation lives in `github_projects.json`, keyed by a
   lowercase repository slug (`github_projects.rs:121-243`).
3. GitHub board picks and Linear contexts live in desktop UI settings keyed by
   `Repo.id` (`github-project-types.ts:233-243`, `types.ts:2490-2504`).
4. Exact linked tickets live on worktrees/features as provider + URL
   (`routes/worktrees.rs:48-68`, `harness/types.rs:85-91`).

The project hub already has a good fail-closed request lifecycle, but only for
GitHub bindings (`ProjectHubPage.tsx:83-132`). Transition code receives the
`Store` yet ignores it for GitHub binding lookup and reads the slug-keyed JSON
file directly (`task_sink.rs:956-1051,1123-1164`). The old board sync table is
also global: `board_tracker_bindings` has no `repo_id` and resolves a sole
binding as a fallback (`agentum-store/src/board.rs:332-387`,
`routes/board_sync.rs:587-625`).

## Decisions

### D1 — SQLite is the canonical owner, keyed by `Repo.id`

Add migration `0027_project_tracker_configs.sql`:

```sql
CREATE TABLE project_tracker_configs (
  repo_id TEXT PRIMARY KEY,
  revision INTEGER NOT NULL,
  config_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
```

No foreign key is possible because the project registry is still `repos.json`.
Deletion is therefore explicit and idempotent. `agentum-store` owns CAS
read/upsert/delete methods in a new `project_trackers.rs`; no React component or
provider module writes the table directly.

Rejected alternatives:

- `repos.json` flattened state: its read-modify-write path is not locked or
  revisioned, and provider code would depend on a route-private record.
- desktop UI settings: unavailable to headless/TUI/MCP/harness consumers.
- extending `github_projects.json`: provider-specific and keyed by mutable slug,
  so it cannot represent Linear or distinguish two Agentum project records.

### D2 — One versioned, provider-aware wire contract

Define the Rust domain types in `agentum-core` and matching TypeScript types in
`shared/types.ts`:

```text
ProjectTrackerConfig {
  schemaVersion: 1
  repoId: string
  revision: number
  provider: "github" | "linear" | null
  github?: {
    repositorySlug: string
    projectBinding?: BoardBinding
  }
  linear?: {
    workspaceId: string
    teamId?: string
    scope?: { kind: "project" | "view"; id: string }
  }
  taskPreferences: {
    github?: { mode, preset, query, hiddenFieldIdsByView }
    linear?: { mode, preset, query }
  }
  provenance: "configured" | "migrated"
}
```

Provider targets are validated as complete before persistence. `provider:null`
is the only unconfigured state; `auto` remains a legacy/default input, not a
runtime resolution mode. Global settings may seed the editor for a newly added
project, but only an explicit save materializes a project config.

`BoardBinding` moves to `agentum-core` (or is re-exported there without changing
its serialized shape) so the canonical config reuses the exact status mapping,
option IDs, and `done_closes_issue` behavior. Provider credentials stay in their
existing global stores and are never copied into this table.

### D3 — One repo-scoped HTTP seam with optimistic concurrency

Add `routes/project_trackers.rs`:

- `GET /api/repos/{repo_id}/tracker-config`
- `PUT /api/repos/{repo_id}/tracker-config`
- `PATCH /api/repos/{repo_id}/tracker-config/preferences`
- `DELETE /api/repos/{repo_id}/tracker-config?expectedRevision=N`

`PUT`, `PATCH`, and `DELETE` accept `expectedRevision`; stale writers return
`409` plus the current config. The store performs comparison and mutation in one
SQLite transaction. The response is always the new canonical record. Both
Project Settings and Tasks call the same runtime client/actions; neither writes
`trackerProvider`, `activeProjectByRepo`, `linearContextByRepo`, or
`github_projects.json` after migration.

The existing GitHub binding routes remain compatibility adapters for one release:
when `repoId` resolves, GET/PUT/DELETE delegate to the canonical service; a
slug-only write resolves exactly one registered project or returns `409`
ambiguity. They never create a new legacy entry. Read-only exact-slug fallback
to `github_projects.json` remains solely for migration and old installations.

### D4 — Deterministic migration, with no global assignment

On a missing canonical row, `GET` builds and transactionally persists a server
migration candidate from:

1. the requested repo's explicit `trackerProvider` (`github`/`linear`; `auto`
   contributes no provider), and
2. an exact-slug `github_projects.json` binding resolved through the repo's
   existing host-aware slug path.

A GitHub binding is authoritative for GitHub target/automation fields and implies
`provider:github` unless an explicit legacy `linear` pin exists; that conflict is
returned as `migrationConflict` and remains unconfigured until explicit save.

The server cannot read desktop UI settings. After GET, the UI may submit one
`legacyHints` conditional PUT through the same canonical endpoint:

- `activeProjectByRepo[repoId]` fills a missing GitHub browse identity only when
  it matches the authoritative binding (or when no binding exists and the legacy
  provider is explicitly GitHub).
- `linearContextByRepo[repoId]` fills Linear scope only when provider is
  explicitly Linear.
- global `activeProject`, `linearContext`, modes, queries, and presets are never
  assigned to a project.

The migration transaction is idempotent. Legacy files/maps remain readable for
rollback but canonical presence always wins. Unknown flattened repo keys are
untouched.

### D5 — One UI store keyed by repo and revision

Add a `projectTrackers` Zustand slice:

```text
trackerConfigByRepo[repoId] =
  idle | loading(requestGeneration) | loaded(config|null) | error(message)
```

Every request captures `(repoId, hostId, requestGeneration)`; reducers accept a
response only when all three still match. `openProjectHub` invalidates only the
target repo entry before navigation, preserving current no-flash behavior.

`RepositoryPane`, `ProjectBindingEditor`, `TrackerIntakePanel`, `TaskPage`, and
Create Workspace consume selectors/actions from this slice. The GitHub-only
`projectBindingByRepo` becomes a compatibility projection during the migration
increment and is then removed. Embedded Tasks never reads global legacy slots.

Task preference writes use the preferences PATCH with the loaded revision.
Search text is debounced; structural choices (provider mode, project/view,
preset, hidden fields) write immediately. A `409` refetches and reapplies only
the local preference patch once; a second conflict surfaces an inline retry.

### D6 — Exact ticket coordinates and project configuration have different jobs

Worktrees/features keep `trackerProvider` + `trackerUrl` unchanged. Add the
parent `repo_id` to the internal `TrackerWorktree` projection (it already exists
on the registry record), and pass an optional `project_repo_id` through
`TrackerEmit`/transition context. Harness transitions resolve the run workdir to
its registered repo/worktree once and pass the same context for all phases.

For GitHub transitions:

1. parse the exact ticket URL (existing behavior),
2. apply labels to that exact slug/number,
3. resolve the canonical config by `project_repo_id`, and
4. apply the Projects status only when that config's GitHub repository slug
   matches the ticket slug.

When a caller (for example MCP status reporting) has no project ID, an exact slug
match may be used only if exactly one canonical config matches, or all matches
carry byte-identical GitHub bindings. Otherwise the Projects arm returns an
actionable `Skipped("ambiguous project tracker config …")`; it never chooses the
sole/global `board_tracker_bindings` row. Linear continues to use the exact issue
identifier/URL and its existing global credential client; target mismatch is
validated before create/list operations.

### D7 — Deletion and legacy board sync are isolated

`DELETE /api/repos/{id}` first deletes that repo's canonical config, then removes
the registry row; both operations are idempotent so retry heals a partial
failure. The desktop remove action clears only the matching config cache and
project preference map. It does not alter existing worktree ticket coordinates.

The legacy internal-board sync routes stop using "sole binding" inference.
Requests must carry a project `repoId` or explicit binding ID. Existing
`board_tracker_bindings` rows remain for old board cards but are not consulted by
project Tasks/configuration.

## Data and control flow

```text
Project Settings / Project Tasks
            │ GET/PUT/PATCH by Repo.id + revision
            ▼
  project_trackers route/service
            │ validate + CAS
            ▼
 project_tracker_configs (SQLite)
            │
            ├── GitHub/Linear list + create target resolution
            ├── Project Hub repo-scoped cache
            └── transition resolver (Repo.id + exact ticket URL)

worktree / harness feature
  keeps exact provider + URL ────────────────┘
```

## Error and race handling

- Unknown `Repo.id`: `404`; missing recorded SSH host: existing actionable host
  error; never local fallback.
- Invalid/incomplete provider target: `422` with field paths; no partial row.
- Revision mismatch: `409` with current record; one bounded client retry for
  disjoint preference patches only.
- Provider unavailable/credential missing: configuration remains saved, while
  list/create renders the existing typed actionable error.
- Repo switch/unmount: generation guard discards late responses; cached rows are
  keyed by repo + provider target identity and never reused across keys.
- Migration conflict: no guessed provider; return conflict metadata and require
  explicit save. Global legacy values are ignored.
- Transition mismatch/ambiguity: label/exact-ticket work may proceed, but
  project-specific status mapping fails closed and is logged; harness never halts.

## Exact edit map

### Domain/store/server

- `crates/agentum-store/migrations/0027_project_tracker_configs.sql` — table.
- `crates/agentum-core/src/lib.rs` (or cohesive new module) — tracker config
  domain types and validation-neutral wire shape.
- `crates/agentum-store/src/project_trackers.rs`, `src/lib.rs` — CAS CRUD and
  lookup-by-provider-target methods.
- `crates/agentum-server/src/routes/project_trackers.rs`, `routes/mod.rs` —
  canonical API, validation, host-aware migration, compatibility projection.
- `crates/agentum-server/src/routes/repos.rs` — typed legacy reads, project
  lookup helper, delete cleanup.
- `crates/agentum-server/src/routes/github_projects.rs` and
  `github_projects.rs` — delegate writes/reads and reuse `BoardBinding`.
- `crates/agentum-server/src/task_sink.rs`, `tracker_sync.rs`,
  `harness/drive.rs`, `routes/worktrees.rs` — project-aware transition context.
- `crates/agentum-server/src/routes/board_sync.rs` — remove sole-binding
  inference for project operations.

### Desktop UI

- `shared/types.ts`, `runtime/project-tracker-client.ts` — typed wire/client.
- `store/slices/project-trackers.ts`, `store/index.ts`, `store/types.ts` —
  revisioned per-repo state/actions and generation guard.
- `components/settings/RepositoryPane.tsx`,
  `components/project-hub/ProjectBindingEditor.tsx`,
  `components/project-hub/TrackerIntakePanel.tsx` — one shared configuration
  editor/action seam.
- `components/project-hub/ProjectHubPage.tsx`, `components/TaskPage.tsx`,
  `components/github-project/ProjectViewWrapper.tsx`,
  `components/github-project/ProjectPicker.tsx` — canonical resolution only.
- `store/slices/ui.ts`, `shared/types.ts`, `shared/task-project-scope.ts` —
  migrate per-repo task preferences; retain global fields as read-only legacy.
- Create Workspace/Chat tracker consumers — initialize from canonical config and
  keep explicit one-off overrides local until save.

## Acceptance-criteria traceability

| AC | Implementation seam | Verification |
|---|---|---|
| 1 | D1–D3 store/types/GET | Store + route tests for two repo IDs and no global read |
| 2 | D3/D5 shared actions + CAS | Settings-save then Tasks-read equality and reverse |
| 3 | D5 generation guard | Repo-switch component/store tests + browser capture |
| 4 | D2/D5 target-keyed consumers | GitHub/Linear list/create client tests |
| 5 | D6 existing worktree coords | Create-worktree and config-edit regression test |
| 6 | D6 transition context | Exact/mismatch/ambiguous transition tests |
| 7 | D4 migration | Hermetic legacy files + idempotent CAS tests |
| 8 | D3/D5 host tuple | Local/SSH route and stale-host-response tests |
| 9 | D7 deletion | Two-project byte-preservation tests |
| 10 | D2/D4 provenance/default | Editor state and no-runtime-global-fallback tests |

## Build order

1. Store/domain/API + migration compatibility.
2. Shared UI config owner and Settings/Tasks editors.
3. All task consumers and project-scoped preferences.
4. Workspace/harness transition context, legacy fallback removal, deletion.

Each step must leave old callers functional through adapters and must pass its
focused tests before the next step begins.

## Gate strategy

- Rust: focused `agentum-store` and `agentum-server` library tests after each
  increment; full relevant workspace lib gate at the end.
- UI: focused Vitest suites for store, resolution, Settings, TaskPage, and Create
  Workspace; Vite production build after each UI increment.
- Static: `git diff --check`; grep proves embedded/project consumers no longer
  read `activeProject`, `linearContext`, or sole-binding inference.
- QA: real desktop with GitHub project A, Linear project B, reload/switch/create,
  one SSH project, exact-ticket transitions, then clear/delete isolation.

## Invariant check

- No session launch path, YOLO translation, adapter behavior, or terminal
  streaming code changes.
- Existing provider clients and `BoardBinding` mapping are reused.
- Tracker failures remain best-effort for harness execution, but are explicit in
  logs/UI and never redirect to another project.
- SQLite provides the single durable writer and atomic revision boundary.
