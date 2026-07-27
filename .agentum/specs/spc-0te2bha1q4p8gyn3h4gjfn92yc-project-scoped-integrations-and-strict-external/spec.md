---
schema: 1
id: SPC-0TE2BHA1Q4P8GYN3H4GJFN92YC
revision: 1
title: Project-scoped integrations and strict external-board isolation
source: legacy-import:ai/specs/025-project-scoped-integrations-and-board-isolation/spec.md@sha256:212788e969d8f02cf221440905a185b51bca21b1bcb1d7fed758e11e34c07eb5
---

# Project-scoped integrations and strict external-board isolation

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec 025 — Project-scoped integrations and strict external-board isolation
>
> - **Number:** 025
> - **Status:** Architect
> - **Surface:** `crates/agentum-desktop/ui` (Project Settings, Project Hub, Tasks) + existing repo/binding persistence seams
> - **Author:** Mateo Cerquetella (drafted via `sdd-spec`)
> - **Date:** 2026-07-22
>
> ## Problem
>
> Integration configuration is split between global Settings, a tracker strip inside
> the Project Hub, and project settings even though every Agentum project owns its
> own external board. The split is misleading and unsafe: inside one project's
> Tasks page, a Linear-backed project can still browse workspace-wide Linear
> projects, views, and issues belonging to other Agentum projects. That is a
> critical project-boundary violation and can lead the operator to view, file, or
> start work from the wrong external board.
>
> ## Goal
>
> For each Agentum project, make Project Settings define the sole external-board
> binding that its Project Hub Tasks surface can display or operate.
>
> ## Users / personas
>
> - **Mateo, a multi-project operator** — while configuring or opening a client
>   project, he expects its GitHub or Linear board to be the only external board
>   visible and actionable in that project's Tasks surface, even when the same
>   account has access to many unrelated boards.
> - **An engineer using both local and SSH projects** — while switching quickly
>   between project hubs, they need stale selections and late network responses to
>   be unable to replace the new project's board.
>
> ## Acceptance criteria
>
> 1. Global **Settings → Integrations** renders only account/tool connection
>    management (for example GitHub CLI auth and Linear workspace credentials); it
>    does not render a repo selector, a Projects v2 board picker, a tracker-provider
>    choice, a project/status mapping, or any other project-owned binding control.
> 2. Every non-folder **Project Settings** page renders one **Integrations** section
>    for that exact `repo.id`. It renders the persisted tracker provider and only
>    that provider's project-owned configuration: GitHub Projects v2 board plus
>    status mapping, or Linear workspace plus one Linear project/board. Saving,
>    changing, clearing, and reopening the page persists and re-renders only that
>    repo's binding; a sibling repo's configuration remains byte-for-byte
>    unchanged.
> 3. The existing Project Hub tracker configuration UI is removed as a second
>    editing location. Its Tasks surface may render a read-only binding summary and
>    an **Open Project Settings** action, but it cannot pick, change, or clear a
>    provider, board, workspace, project, or status mapping.
> 4. Opening a Project Hub's Tasks tab resolves an immutable project-task scope from
>    the active `repo.id` before rendering provider content. A GitHub-bound project
>    renders only its resolved Projects v2 board; a Linear-bound project renders
>    only issues from its configured Linear project ID in its configured workspace.
>    Provider tabs, Linear `Projects`, `Views`, workspace-wide issue lists, repo
>    multi-selectors, and board pickers that could escape that scope do not render
>    in the embedded Project Hub surface.
> 5. A project with no complete binding, a deleted/inaccessible Linear project, a
>    failed binding fetch, or a provider/account mismatch renders an honest empty
>    state naming the current project and linking to its Project Settings. It does
>    not fall back to a global default provider, `activeProject`, selected Linear
>    workspace, cached collection, resume state, first available board, or another
>    repo's binding.
> 6. Switching Project Hub from repo A to repo B clears repo A's selected external
>    item, Linear project/view context, dialogs, resume context, and loading result
>    before repo B renders. A late request started for repo A is ignored after the
>    switch, and no frame, badge, breadcrumb, row, drawer, or action from repo A is
>    observable under repo B.
> 7. Every embedded Tasks read and action carries and validates the immutable scope:
>    fetch/refresh/search, open issue, create issue, start workspace, sync status,
>    drag/move, and gated-run intake use the active repo plus its bound external
>    project. A mismatched item or response is blocked and cannot mutate either
>    external tracker or Agentum state.
> 8. The standalone, global Tasks page remains the explicit cross-project explorer
>    and can keep provider tabs, repo multi-selection, Linear Projects/Views, and
>    global resume behavior. Its state cannot become an implicit fallback for an
>    embedded Project Hub Tasks page.
> 9. Project isolation holds for local and SSH-backed repos. GitHub binding
>    resolution continues threading `repoId` through the existing host-aware seam,
>    and Linear bindings remain keyed by Agentum `repo.id` rather than by whichever
>    Linear workspace is globally selected.
> 10. Focused unit/component tests, the production UI build, and the relevant Rust
>     library tests pass; browser QA demonstrates two Agentum projects bound to
>     different GitHub boards, then two bound to different Linear projects on the
>     same connected account, cannot see or operate each other's external boards.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** relocate all project-owned tracker controls into Project Settings;
>   introduce/persist a repo-owned Linear project binding; reuse the existing
>   repo-owned GitHub binding; make embedded Project Hub Tasks a locked,
>   fail-closed projection of one binding; reset and guard asynchronous state on
>   project changes; protect reads and mutations at the UI/runtime boundary.
> - **Out:** per-project API credentials or separate OAuth sessions (credentials
>   remain global account connections); redesigning provider authentication;
>   changing the standalone global Tasks explorer; supporting multiple external
>   boards per Agentum project; adding new tracker providers; changing Harness
>   lifecycle states or agent launch behavior.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - `RepositoryPane` tracker section
>   (`crates/agentum-desktop/ui/src/components/settings/RepositoryPane.tsx:330`) —
>   already persists `repo.trackerProvider`; expand this into the single
>   project-owned Integrations editor rather than creating another settings tree.
> - `Repo.trackerProvider` and the repo update whitelist
>   (`crates/agentum-desktop/ui/src/shared/types.ts:111`,
>   `crates/agentum-desktop/ui/src/store/slices/repos.ts:66`) — existing
>   forward-compatible per-repo persistence precedent for the new Linear binding.
> - `GithubProjectsBoardEditor`
>   (`crates/agentum-desktop/ui/src/components/settings/IntegrationsPane.tsx:235`)
>   — this is the current global Settings repo selector and board editor to remove;
>   reuse its `ProjectBindingEditor`, not its global placement.
> - `ProjectBindingEditor`
>   (`crates/agentum-desktop/ui/src/components/project-hub/ProjectHubPage.tsx:320`)
>   and the host-aware `getProjectBinding({ workdir, repoId })` path
>   (`ProjectHubPage.tsx:83`) — move/reuse the shared GitHub board and status-map
>   editor under the current repo's Project Settings.
> - `resolveBoardProject`
>   (`crates/agentum-desktop/ui/src/lib/board-project-resolution.ts:65`) — its
>   repo-keyed GitHub resolution and fail-closed `none` result are the model the
>   Linear resolver must match; embedded code must not revive its standalone
>   legacy tier.
> - `TaskPage` embedded repo seed and keyed Project Hub mount
>   (`crates/agentum-desktop/ui/src/components/project-hub/ProjectHubPage.tsx:65`,
>   `:246`) — retain the repo-keyed remount, but replace mount-time preference
>   with an explicit immutable scope contract.
> - Existing Linear clients/store operations used by `TaskPage` — especially
>   `listLinearProjectIssues`; a bound Linear project can call the narrow project
>   endpoint directly without rebuilding Linear transport or rendering primitives.
> - Global authentication management in `IntegrationsPane`
>   (`crates/agentum-desktop/ui/src/components/settings/IntegrationsPane.tsx:408`)
>   — keep connection checks, credential add/test/disconnect, and CLI status here.
>
> ### Build new
>
> - A repo-owned Linear binding value containing stable workspace ID, project ID,
>   and display metadata, persisted through the existing `Repo` extra-field update
>   path and sanitized as one atomic value.
> - A shared `ProjectIntegrationsSection` mounted only by `RepositoryPane`, composing
>   provider choice, GitHub's existing binding editor, and a Linear project picker.
> - A pure `resolveProjectTaskScope(repo, bindings)` result with explicit
>   `loading | bound | unbound | unavailable` states and no global fallback.
> - An embedded/locked Tasks variant that accepts the resolved scope as input,
>   renders only the bound collection, and rejects mismatched item/action payloads.
> - Generation/scope guards for Linear requests and tests covering repo switches,
>   stale cache/resume data, incomplete bindings, and hostile mismatched payloads.
>
> ## Risks & invariants
>
> - **Critical isolation invariant:** when `TaskPage` is embedded, `repo.id` plus
>   its persisted binding is the complete authority. Global UI state is never an
>   authorization or fallback source.
> - **Fail closed:** loading, missing, malformed, inaccessible, and stale bindings
>   show no external data. Convenience fallback is forbidden on a project surface.
> - **Account vs project ownership:** credentials are account-global; provider,
>   external board/project, and mapping are project-local. Moving credentials into
>   each repo would duplicate secrets and is explicitly not required.
> - **Atomic sibling safety:** updating one repo's integration object must preserve
>   every unknown repo field and all sibling repo records through the existing
>   serialized update queue.
> - **Async race safety:** request cancellation flags alone are insufficient when
>   shared caches outlive a component; response application and actions must compare
>   the captured scope identity to the live one.
> - **Host awareness:** do not regress SSH GitHub binding resolution by substituting
>   a local workdir or omitting `repoId`.
> - This change does not touch the one agent launch path, YOLO translation,
>   push-based terminal streaming, or per-session UUID invariants.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:**
>   - `025-F1-project-integrations-home` — split global account connections from
>     repo-owned configuration; mount the shared Project Settings section and move
>     GitHub binding/status mapping into it.
>   - `025-F2-linear-project-binding` — persist, edit, clear, and reload one Linear
>     workspace/project binding per repo without sibling mutation.
>   - `025-F3-locked-project-task-scope` — resolve the repo scope and render only
>     its bound GitHub/Linear board; remove embedded escape navigation and all
>     global fallbacks.
>   - `025-F4-race-and-action-enforcement` — reset project-local state, ignore stale
>     responses, and validate every embedded read/action against the live scope.
> - **`verify.sh` asserts:** targeted Vitest coverage for repo update sibling
>   preservation, GitHub/Linear scope resolution, embedded control removal,
>   unbound/error fail-closed states, A→B late-response rejection, and mismatched
>   action blocking; `npm run build --prefix crates/agentum-desktop/ui`; relevant
>   repo/binding Rust library tests (and `cargo test --workspace --lib` when the
>   harness budget permits).
> - **`qa.sh` asserts:** connect accounts that can access Linear projects L-A/L-B
>   and GitHub boards G-A/G-B; for each provider, bind Agentum repos A/B to its two
>   different external projects and verify A's Project Hub never renders or
>   navigates to B's board and vice versa; switch during a throttled fetch and
>   verify no stale flash; deep-link/reopen each hub; clear/revoke a binding and
>   verify the honest empty state; run one mutation from each project and confirm
>   only its bound external project changes.
>
> ## Open questions
>
> - No blocking product question. The architect should choose the exact serialized
>   field name/shape for the Linear binding and whether the locked Tasks variant is
>   a dedicated component or a strict `TaskPage` mode; either choice must satisfy
>   the observable boundary and sibling-preservation criteria above.
