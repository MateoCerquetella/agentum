# Architecture: Project-Scoped Integrations and Board Isolation

**Status:** Architect PASS

**Date:** 2026-07-22

**Baseline:** `06239f0e`

**Required build order:** F1 -> F2 -> F3 -> F4

## Outcome

The Project Hub becomes a scope-locked task surface. A repository owns its tracker provider and exact external board identity; opening or switching hubs resolves that identity before rendering tasks, and every read or mutation is rejected if its captured scope is no longer current. The standalone Tasks page remains the global explorer.

The implementation should not attempt to make the existing 6,000-line `TaskPage` conditionally safe. Instead, `ProjectHubPage` must mount a new scope resolver and provider-specific locked views. This creates a structural boundary between global exploration state and project-scoped work.

## Grounding Corrections and Contract Interpretations

### 1. Local Linear support is incomplete, not merely unwired

`crates/agentum-desktop/src/commands/linear.rs` explicitly marks several read and mutation commands as stubs. `linear_team_states` and `linear_list_project_issues` return empty collections, while `linear_get_issue`, `linear_get_project`, `linear_create_issue`, and `linear_update_issue` return `None`. F3 and F4 therefore include implementing these existing Tauri commands and their Rust tests. The UI must use the existing runtime client command seam; no second transport is introduced.

### 2. “Global Integrations contains only connections” is ambiguous

The PM handoff locks relocation of provider choice and repository board bindings. The current Integrations pane also owns account-wide GitHub label dictionaries, Linear workflow-state dictionaries, and Harness toggles. These are pipeline/account preferences, not a repository tracker binding. This design removes `GithubProjectsBoardEditor` and all repository provider/binding controls from global Integrations but retains those pipeline controls. If acceptance criterion 1 intends their removal as well, PM must define their destination before F1; silently deleting or relocating them would exceed the approved product decision.

### 3. GitHub binding persistence is slug-keyed today

The server route accepts an Agentum `repoId`, resolves its normalized GitHub slug, and stores the binding under that slug. This design preserves that backend contract and exposes it through a repo-owned UI. Consequently, two registered Agentum repositories resolving to the same GitHub slug share one GitHub Projects binding. F2 must not invent a second per-Agentum-repo GitHub persistence store.

### Existing-code evidence map

All line references are against baseline `06239f0e`.

| Existing seam | Evidence | Architectural consequence |
|---|---|---|
| Repository provider model | `crates/agentum-desktop/ui/src/shared/types.ts:83,111` | Extend `Repo`; do not create a parallel settings record. |
| Repo update whitelist and per-repo serialization | `crates/agentum-desktop/ui/src/store/slices/repos.ts:66-109,644-680` | Add the Linear field to the existing sanitizer and queue. |
| Flattened server persistence and PATCH merge | `crates/agentum-server/src/routes/repos.rs:46-69,238-266` | Linear binding round-trips without a schema migration. Existing tracker round-trip coverage is at `:964-1005`. |
| Current repo provider selector | `crates/agentum-desktop/ui/src/components/settings/RepositoryPane.tsx:330-375` | F1 expands this repository-owned surface. |
| Current global GitHub binding editor | `crates/agentum-desktop/ui/src/components/settings/IntegrationsPane.tsx:227-273,590-592` | F1 relocates this editor. Global Linear state controls and Harness hooks remain at `:1020-1029`. |
| Embedded global Tasks ownership | `crates/agentum-desktop/ui/src/components/project-hub/ProjectHubPage.tsx:251-258` | Replace the embedded `TaskPage` mount structurally. Current tracker configuration is at `:266-342`. |
| GitHub binding error collapse | `crates/agentum-desktop/ui/src/components/project-hub/ProjectHubPage.tsx:83-137` | Preserve full identity and distinguish unbound from unavailable. |
| Global Linear fallback and broad reads | `crates/agentum-desktop/ui/src/components/TaskPage.tsx:338-395,826-870,909-1260,2819-3148` | Do not reuse embedded `TaskPage` for the locked hub. |
| Global Linear actions | `crates/agentum-desktop/ui/src/components/TaskPage.tsx:1660-1742,2650-2726,3192-3212` | Reimplement only bounded actions in `LockedLinearProjectTasks`. |
| Existing GitHub resolver and picker | `crates/agentum-desktop/ui/src/components/github-project/ProjectViewWrapper.tsx:107-119,171-202,740-950` | Locked mode bypasses global resolver/picker while retaining fetch-run optimization. |
| Slug-keyed GitHub persistence | `crates/agentum-server/src/routes/github_projects.rs:281-300`; `crates/agentum-server/src/github_projects.rs:208-221` | Preserve sharing by normalized GitHub slug. |
| Linear cached stale fallback | `crates/agentum-desktop/ui/src/store/slices/linear.ts:1011-1057` | Locked reads must validate response identity; cached global fallback is not authoritative. |
| Stubbed local Linear commands | `crates/agentum-desktop/src/commands/linear.rs:6-11,586-589,673-715` | F3/F4 must complete the existing Rust command surface. |
| Tracker intake fallbacks | `crates/agentum-desktop/ui/src/components/project-hub/use-tracker-intake.ts:84-165,257-320` | Locked intake receives scope explicitly and removes global fallback. |
| Workspace modal model and submit | `crates/agentum-desktop/ui/src/components/new-workspace/create-workspace-wizard-model.ts:120-269`; `crates/agentum-desktop/ui/src/hooks/useComposerState.ts:2376-2497` | Carry and revalidate required scope at the actual creation boundary. |

## D1. Exact Repository Binding and Persistence Contract

### Shared model

Add the following shared type and field in `crates/agentum-desktop/ui/src/shared/types.ts`:

```ts
export type LinearProjectBinding = Readonly<{
  workspaceId: string
  workspaceName: string
  projectId: string
  projectName: string
  projectUrl?: string
}>

export type Repo = {
  // existing fields
  trackerProvider?: "github" | "linear" | "auto"
  linearProjectBinding?: LinearProjectBinding | null
}
```

Persistence key is exactly `linearProjectBinding`. Its meanings are:

| Stored value | Meaning |
|---|---|
| property absent | Legacy or never configured |
| `null` | Explicitly cleared/unbound |
| object | Exact Linear workspace/project binding |

`workspaceId` and `projectId` are authoritative. Names and `projectUrl` are display metadata only and may be refreshed after a successful exact-ID lookup. Persisting a project name without both IDs is forbidden.

### Sanitization and serialization

Extend `RepoUpdate` and `sanitizeRepoUpdates` in `store/slices/repos.ts` to accept the exact field. The sanitizer must:

- accept `null` as an explicit clear;
- require trimmed, non-empty `workspaceId`, `workspaceName`, `projectId`, and `projectName` for an object;
- allow absent `projectUrl`, otherwise require a valid `https:` URL;
- return `undefined` for an invalid object so malformed data is not persisted;
- clone and freeze the normalized object at the model boundary;
- use the existing per-repository serialized update queue so rapid provider/binding edits retain order.

The server requires no schema migration: `routes/repos.rs` already round-trips flattened unknown fields and merges PATCH updates. Add a server round-trip regression test for object, explicit `null`, and unrelated-field preservation.

### GitHub persistence

Continue using the existing host-aware GitHub Projects binding GET/PUT/DELETE routes. A successful GET must retain the full binding DTO and resolved repository slug in the client cache; it must not reduce the result to a display-only identity. Transport/authorization failure is distinct from an unbound `404`.

### Provider resolution

Within Project Hub, only explicit `github` or `linear` resolves a task board. `auto` and an absent provider both resolve to `unbound`. There is no fallback to the global provider, selected workspace, last-used board, or same-name project.

## D2. Immutable Project Task Scope

Create `lib/project-task-scope.ts` with a discriminated immutable model:

```ts
type ScopeBase = Readonly<{
  repoId: string
  repoName: string
  generation: number
}>

export type ProjectTaskScope =
  | (ScopeBase & { status: "loading" })
  | (ScopeBase & {
      status: "unbound"
      provider: "github" | "linear" | "auto" | null
      reason: "provider-unset" | "github-unbound" | "linear-unbound"
    })
  | (ScopeBase & {
      status: "unavailable"
      provider: "github" | "linear"
      reason: "connection" | "authorization" | "not-found" | "invalid-binding" | "transport"
      message: string
    })
  | (ScopeBase & {
      status: "bound"
      provider: "github"
      scopeKey: string
      repoSlug: string
      projectId: string
      owner: string
      ownerType: "user" | "organization"
      projectNumber: number
      projectTitle: string
    })
  | (ScopeBase & {
      status: "bound"
      provider: "linear"
      scopeKey: string
      workspaceId: string
      workspaceName: string
      projectId: string
      projectName: string
      projectUrl?: string
      teamIds: readonly string[]
    })
```

`scopeKey` must be generated with a single helper and no lossy concatenation:

```ts
JSON.stringify(
  provider === "github"
    ? [repoId, "github", repoSlug, projectId]
    : [repoId, "linear", workspaceId, projectId],
)
```

Each hub open or `repoId`, provider, or binding change increments `generation` and synchronously transitions to `loading`. Provider-specific task state, selection, pagination, search results, error state, and modal state are cleared before resolving the new scope.

Resolution rules:

1. Read the current repository by the explicit hub `repoId`; never from global selection.
2. For GitHub, fetch the binding with the explicit `repoId`, preserve full DTO plus resolved slug, and map not-bound separately from fetch failure.
3. For Linear, verify the persisted workspace exists through an explicit `workspaceId` request, then call `getProject(workspaceId, projectId)` and require both returned IDs to match. Derive immutable `teamIds` from the exact project response.
4. Publish `bound` only if the final response still matches the live `{repoId, generation}`.

## D3. Structural UI Boundary

```text
ProjectHubPage(repoId)
  -> ProjectTasksPage(repoId)
       -> resolve immutable ProjectTaskScope
       -> loading / unbound / unavailable state
       -> bound github -> ProjectViewWrapper(lockedScope)
       -> bound linear -> LockedLinearProjectTasks(scope)

Standalone Tasks route
  -> TaskPage (global explorer; unchanged ownership)
```

`ProjectHubPage` must stop mounting `TaskPage embedded`. The new `ProjectTasksPage` owns only scope resolution, empty/error/configure states, and provider dispatch.

### Locked GitHub behavior

Extend `ProjectViewWrapper` with a required-for-embedded `lockedScope` variant. In locked mode:

- do not call `resolveBoardProject`, consult `activeProjectByRepo`, or render `ProjectPicker`;
- fetch and mutate only the project identity carried by `lockedScope`;
- a task row from another repository may be displayed when it genuinely belongs to the bound board;
- repository-backed mutations and workspace start/edit actions are enabled only when the row slug resolves to the active scope `repoId`;
- unresolved or differently registered row repositories are read-only, with a clear reason.

The unlocked standalone behavior remains unchanged.

### Locked Linear behavior

Create `LockedLinearProjectTasks`. It must use explicit runtime-client calls with `scope.workspaceId` and `scope.projectId`; it must not import/read selected workspace, selected project, `linearContextByRepo`, workspace-wide issue search, cached global views, or global resume state.

Every returned issue must carry exact `workspaceId` and `projectId` identity from the Rust command. Missing identity or any mismatch is rejected before render. Creation always includes the bound project ID and a team ID from `scope.teamIds`. Status transitions are limited to states returned for a team belonging to that exact project.

### Configure link

Unbound/unavailable states link to the existing repository settings target with the current `repoId` and a stable Project Integrations section id. They never redirect to global Integrations.

## D4. Stale Response, Action, and Modal Guards

Create `lib/project-task-scope-guard.ts` and use a live scope ref owned by `ProjectTasksPage`. A captured guard is:

```ts
type ProjectTaskScopeGuard = Readonly<{
  scopeKey: string
  generation: number
  repoId: string
}>
```

Before committing any asynchronous result or starting any mutation, require all three values to equal the live bound scope. Cancellation flags and fetch run IDs are optimizations, not authorization.

Apply the guard to:

- GitHub and Linear list/search refreshes;
- pagination/load-more results;
- issue/item selection and detail hydration;
- create, edit, status/move, archive/close, and assignment mutations;
- refreshes triggered after mutations;
- workspace start/edit/create and all gated-modal callbacks.

Each action must additionally validate its target identity:

- GitHub: target project ID and resolved row repository must satisfy the current locked scope rules;
- Linear: target `workspaceId` and `projectId` must equal the scope, and target `teamId` must be in `scope.teamIds`.

The workspace modal contract gains:

```ts
requiredProjectTaskScope?: Readonly<{
  scopeKey: string
  generation: number
  repoId: string
}>
```

When supplied, repository selection is locked. Revalidate immediately before `createWorktree`, again after any connection/environment gate returns, and before invoking tracker mutation callbacks. If stale, close the modal or surface a retryable stale-scope error without performing the action.

## D5. Local, SSH, and Runtime Boundaries

- Repository binding persistence always PATCHes the active runtime's repository registry with explicit `repoId`; this already covers local and SSH-connected repositories through the existing repository store seam.
- GitHub binding reads/writes continue through the host-aware repo route with explicit `repoId` and resolved working directory.
- Linear credentials remain runtime/account scoped. Every hub call supplies persisted workspace and project IDs and ignores global selected-workspace state.
- Starting a workspace for an SSH repository continues through the existing new-workspace wizard and connection gate, with repo selection locked and the scope guard rechecked on return.
- Gated runs remain local-only under the current product constraint; this feature does not broaden gated-run support.
- Generic runtime-environment Linear RPC commands are not part of this feature. The existing Tauri Linear command surface is completed instead.

## File-Level Implementation Plan

In the tables below, `ui/` means `crates/agentum-desktop/ui/`; Rust paths are repository-relative.

### F1 — Relocate provider and GitHub binding controls

| File | Required change |
|---|---|
| `ui/src/components/settings/ProjectIntegrationsSection.tsx` (new) | Repository-scoped provider selector, GitHub binding editor, configure/clear states, stable section id, test ids. |
| `ui/src/components/settings/RepositoryPane.tsx` | Replace the provider-only row with `ProjectIntegrationsSection` for the selected repo. |
| `ui/src/components/settings/repository-search.ts` | Include Project Integrations/provider/binding terms. |
| `ui/src/components/settings/IntegrationsPane.tsx` | Remove `GithubProjectsBoardEditor` and repository binding language; retain credentials and account/pipeline controls described above. |
| settings tests | Prove provider and GitHub binding controls are repository-scoped and absent globally. |

### F2 — Persist explicit Linear binding

| File | Required change |
|---|---|
| `ui/src/shared/types.ts` | Add `LinearProjectBinding` and `Repo.linearProjectBinding`. |
| `ui/src/shared/linear-project-binding.ts` (new) | Normalize, validate, and freeze binding values. |
| `ui/src/store/slices/repos.ts` | Add whitelist/sanitizer support while preserving serialized per-repo writes. |
| `ui/src/components/settings/ProjectIntegrationsSection.tsx` | Explicit workspace then project picker; save exact IDs/names; clear via `null`; never infer by name. |
| `ui/src/components/settings/project-integrations-section-model.ts` (new) | Pure provider/binding transitions and validation. |
| `crates/agentum-server/src/routes/repos.rs` | Add round-trip tests only; flattened storage already implements persistence. |
| binding/store/settings tests | Cover valid object, invalid object, null clear, order, reload, and cross-repo isolation. |

### F3 — Introduce locked read surfaces

| File | Required change |
|---|---|
| `ui/src/lib/project-task-scope.ts` (new) | Immutable variants, exact resolver, scope-key generation, error taxonomy. |
| `ui/src/components/project-hub/ProjectTasksPage.tsx` (new) | Own generation/reset/resolution and dispatch locked provider views. |
| `ui/src/components/project-hub/LockedLinearProjectTasks.tsx` (new) | Exact-project issue list/detail read surface. |
| `ui/src/components/project-hub/ProjectHubPage.tsx` | Mount `ProjectTasksPage`; remove embedded `TaskPage` ownership. |
| `ui/src/store/slices/github.ts` and `ui/src/lib/board-project-resolution.ts` | Preserve full binding DTO, slug, and distinct unavailable/unbound states; no global fallback in locked path. |
| `ui/src/components/github-project/ProjectViewWrapper.tsx` | Add locked mode; suppress picker/global resolution; enforce row repository ownership. |
| `crates/agentum-desktop/src/commands/linear.rs` | Implement exact `linear_get_project` and `linear_list_project_issues`; include workspace/project identity in results. |
| scope/hub/provider/Rust tests | Prove exact board reads, switch reset, stale-result rejection, and read-only cross-repo rows. |

### F4 — Guard all writes and workspace flows

| File | Required change |
|---|---|
| `ui/src/lib/project-task-scope-guard.ts` (new) | Capture and validate scope guard/action identities. |
| `ui/src/components/github-project/ProjectViewWrapper.tsx` | Guard every locked mutation, refresh, and workspace callback. |
| `ui/src/components/project-hub/LockedLinearProjectTasks.tsx` | Add guarded create/edit/status/workspace actions. |
| `crates/agentum-desktop/src/commands/linear.rs` | Implement exact `get_issue`, `team_states`, `create_issue`, and `update_issue` with identity validation. |
| `ui/src/components/project-hub/use-tracker-intake.ts` and `ui/src/components/project-hub/TrackerIntakePanel.tsx` | Accept a locked scope/guard; remove active-project and global-provider fallback in Project Hub. |
| `ui/src/components/new-workspace/create-workspace-wizard-model.ts` and `ui/src/hooks/useComposerState.ts` | Add required scope, lock repo, and revalidate before/after gates and before creation. |
| action/modal/runtime tests | Prove all stale or mismatched writes are rejected and correct local/SSH flows succeed. |

## Acceptance and Test Mapping

| AC | Proof obligation | Primary tests |
|---|---|---|
| 1 | Global Integrations has no repository provider/binding editor. | `integrations-pane-project-controls.test.ts` |
| 2 | Every repo exposes provider and board configuration. | `project-integrations-section.test.tsx` |
| 3 | GitHub and Linear exact bindings survive reload/SSH registry round-trip. | repos route, sanitizer serialization, section model tests |
| 4 | Hub renders only its bound provider/board. | `ProjectTasksPage.test.tsx`, locked provider tests |
| 5 | Switching projects clears state immediately and stale reads never render. | scope and repo-switch tests |
| 6 | All mutations remain in exact board/project and valid team/repository. | guard, provider action, Rust command tests |
| 7 | Standalone Tasks remains global. | existing TaskPage suite plus no TaskPage behavior changes |
| 8 | Workspace start/edit uses the active hub repo and SSH connection gate. | wizard model/composer tests |
| 9 | Unbound/unavailable state is scoped and links to repo settings. | `ProjectTasksPage.test.tsx` |
| 10 | Each fixture independently proves configured local GitHub and SSH Linear behavior. | final browser/harness scenarios plus persistence tests |

Required switch-race tests use deferred promises: open repo A, begin list/detail/mutation preparation, switch to repo B, resolve A, and assert no A state or action reaches B. Repeat both provider directions and same-provider/different-binding switches.

## Exact Verification Commands

### F1

```bash
npx vitest run --root crates/agentum-desktop/ui \
  src/components/settings/RepositoryPane.test.ts \
  src/components/settings/project-integrations-section.test.tsx \
  src/components/settings/integrations-pane-project-controls.test.ts
npm run build --prefix crates/agentum-desktop/ui
git diff --check
```

### F2

```bash
npx vitest run --root crates/agentum-desktop/ui \
  src/shared/linear-project-binding.test.ts \
  src/store/slices/repos-update-serialization.test.ts \
  src/components/settings/project-integrations-section-model.test.ts
cargo test -p agentum-server --lib routes::repos::tests::linear_project_binding
npm run build --prefix crates/agentum-desktop/ui
git diff --check
```

### F3

```bash
npx vitest run --root crates/agentum-desktop/ui \
  src/lib/project-task-scope.test.ts \
  src/components/project-hub/ProjectTasksPage.test.tsx \
  src/components/github-project/ProjectViewWrapper.repo-switch.test.tsx \
  src/components/github-project/ProjectViewWrapper.locked.test.tsx
cargo test -p agentum-desktop --lib commands::linear::tests::project_
npm run build --prefix crates/agentum-desktop/ui
git diff --check
```

### F4 / final gate

```bash
npx vitest run --root crates/agentum-desktop/ui \
  src/lib/project-task-scope.test.ts \
  src/lib/project-task-scope-guard.test.ts \
  src/components/project-hub/ProjectTasksPage.test.tsx \
  src/components/project-hub/LockedLinearProjectTasks.test.tsx \
  src/components/github-project/ProjectViewWrapper.locked.test.tsx \
  src/components/new-workspace/create-workspace-wizard-model.test.ts \
  src/runtime/runtime-linear-client.test.ts \
  src/store/slices/repos-update-serialization.test.ts
cargo test -p agentum-desktop --lib commands::linear::tests
cargo test -p agentum-server --lib routes::repos::tests
npm run build --prefix crates/agentum-desktop/ui
cargo test --workspace --lib
git diff --check
```

## Harness and QA Contract

Add deterministic harness cases, in this exact order and with stable ids:

1. `sdd-project-scope-f1-settings-relocation`
2. `sdd-project-scope-f2-linear-persistence`
3. `sdd-project-scope-f3-locked-reads`
4. `sdd-project-scope-f4-guarded-actions`

Each block must assert its own fixture setup and teardown. The final verification block must contain all four ids in order before `</Harness>`.

Final QA needs two independent fixtures:

- local repository, explicit GitHub provider, exact Projects binding;
- SSH repository, explicit Linear provider, exact workspace/project binding.

For each, verify persistence by reloading, opening its Project Hub, observing only bound tasks, switching to the other hub under throttled responses, attempting one valid mutation and one stale/mismatched mutation, and inspecting API/command traffic to prove exact IDs were used.

## Risks and Escalation Triggers

- If product requires literal removal of pipeline dictionaries/toggles from global Integrations, pause F1 for PM destination ownership.
- If product requires separate GitHub bindings for two Agentum repos sharing one slug, pause F2 for a server storage migration; current routes cannot express that distinction.
- If Linear API payloads cannot return project/workspace identity for issue reads, do not trust client input alone; extend the Rust query to fetch and validate those fields before F3 passes.
- Do not restore global fallbacks to avoid an unbound state. An explicit unbound state is a safety property.

## Architect Verdict

**PASS.** The spec is implementation-ready under the three explicit interpretations above. The required safety property is achieved by a structural Project Hub boundary plus immutable identity guards, not by cosmetic filtering of global task state.
