---
schema: 1
id: SPC-0Y0VGQRHNT3QKJ0FHWMMH55V5T
revision: 1
title: Board lives inside each project
source: legacy-import:ai/specs/016-board-per-project/spec.md@sha256:4bdc867fa86c10c14259b62a0cad535d5b0661cea4effffc5776afa5e499f30d
---

# Board lives inside each project

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

> # Spec 016 — Board lives inside each project
>
> - **Number:** 016
> - **Status:** Done
> - **Surface:** `crates/agentum-desktop/ui` (React SPA; no server changes expected)
> - **Author:** Claude (from Mateo's ask, GitHub issue #360)
> - **Date:** 2026-07-13
> - **Issue:** [#360 — Board lives inside each project: remove sidebar Board, per-project tracker binding (incl. SSH)](https://github.com/MateoCerquetella/agentum/issues/360)
>
> > ⚠️ **Base-branch warning:** this spec was researched against `origin/develop`
> > (all `path:line` refs below are develop refs). The worktree the spec was
> > drafted in sits at v0.57.0 — **182 commits behind** — so implementation must
> > happen in a fresh worktree off `origin/develop`, re-locating lines before
> > editing (same hazard that bit the v0.70.0 wizard work).
>
> ## Problem
>
> The sidebar Board is one global surface bound to one global
> `settings.githubProjects.activeProject` — two projects cannot show their own
> boards, and switching projects means re-picking the tracker every time. The
> Project Hub's Tasks tab looks per-project but is faked: on tab open it *copies*
> the repo's tracker binding into the global `activeProject`, so opening project
> A's hub silently changes what every other board surface shows. For SSH-hosted
> repos even that fails — the hub loads the binding without `hostId`, so the
> server tries to read git origin from a local path that doesn't exist and the
> board never appears.
>
> ## Goal
>
> The board becomes a genuinely per-project surface inside each project's hub —
> sidebar Board entry removed, board view resolved per repo (binding-aware,
> SSH-aware), legacy global `activeProject` kept as a read-only migration
> fallback.
>
> ## Users / personas
>
> - **Mateo (multi-project operator):** drives agents across several repos from
>   one cockpit, each tracked by a different GitHub Project. Feels the pain every
>   time he opens a second project's hub and the first project's board is
>   silently replaced.
> - **SSH-project operator:** added a repo that lives on a remote host; its hub
>   Tasks tab shows the coarse issue Kanban instead of the bound Projects board,
>   with no error explaining why.
>
> ## Acceptance criteria
>
> 1. `SidebarNav` renders no Board entry; the board renders inside
>    Projects → hub → Tasks tab. (`SidebarNav.tsx` Board button ~`:184-253`
>    deleted.)
> 2. With project A bound/picked to GitHub Project X and project B to Project Y,
>    opening each hub's Tasks tab renders that project's own board — and doing so
>    **emits no settings write** to `githubProjects.activeProject` (the copy-hack
>    effect in `ProjectHubPage.tsx:82-123` is gone).
> 3. Picking a board inside project A's hub persists
>    `githubProjects.activeProjectByRepo[A.id]` and leaves both the legacy
>    `activeProject` and every other repo's entry byte-unchanged (sibling-field
>    preservation per `ProjectPicker.tsx:184`). When the pick differs from the
>    repo's server binding, the board shows a non-blocking hint naming the bound
>    project (status transitions still write to the binding) with a one-click
>    "Use bound project" that deletes the per-repo entry.
> 4. With only the legacy global `activeProject` set (fresh upgrade, no per-repo
>    entry, no binding), the hub board still renders that project — the legacy
>    slot is read as a fallback and never written by new code.
> 5. For a repo with `hostId` set (SSH), the hub board resolution calls
>    `getProjectBinding({ workdir, hostId })` so the slug resolves on the remote
>    host; a bound SSH repo renders its board.
> 6. Every **bare board opener** — command palette "Board"
>    (`CommandPalette.tsx:108`), the `view.tasks` shortcut (`App.tsx:1285`), the
>    native-menu Open Tasks IPC handler (`useIpcEvents.ts:823`), and ChatPage's
>    repo-scoped board links (`ChatPage.tsx:503/523`) — routes to
>    `openProjectHub(repoId, 'tasks')` when a repo resolves (payload
>    `preselectedRepoId`, else a live `activeRepoId`) and `openProjectsPage()`
>    otherwise, preserving the existing has-git-repo gate. **Work-item detail
>    openers** (`ChatPage.tsx:480`, `WorktreeCard.tsx:530/540`, TaskPage-internal
>    navigation) keep calling `openTaskPage({…detail payload})` unchanged this
>    slice. Grep gate: every remaining `openTaskPage(` call site outside
>    `TaskPage.tsx` (and tests) passes `openGitHubWorkItem` or `openLinearIssue`;
>    zero bare `openTaskPage()` calls remain.
> 7. `bun run build` (ui) green; new pure resolution logic covered by vitest
>    (`bunx vitest` — NOT bare tsc; `shared/*` is a vite alias).
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** per-repo board resolution + persistence in UI settings; hub wiring
>   (binding-first, hostId-aware); sidebar entry removal + nav re-routing;
>   migration fallback read of legacy `activeProject`.
> - **Out:**
>   - No server/Rust changes — the binding API is already host-aware
>     (`routes/github_projects.rs:318`, resolver #315). If a server gap surfaces,
>     that's a new issue.
>   - No change to tracker *transitions* (spec 010's zero-call-site Projects
>     write stays binding-driven, untouched).
>   - No new `Repo` field — the binding stays in settings keyed by `Repo.id`
>     (issue's explicit call) and server-side `bindings.json` keyed by slug.
>   - No deletion of `TaskPage` / the `activeView === 'tasks'` route internals —
>     only its global entry points; the component keeps serving the hub embed.
>   - No Linear-side per-project binding (GitHub Projects only, matching #360).
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - **Server binding API, host-aware** — `GET/PUT/DELETE
>   /api/github/project-binding` + `/discover`
>   (`crates/agentum-server/src/routes/github_projects.rs:33,318`); slug
>   resolution accepts `host_id` (#315 resolver). SSH support is a *client
>   threading* fix, not a server feature.
> - **Typed client** — `getProjectBinding({ workdir, slug?, hostId? })`
>   (`ui/src/runtime/github-projects-client.ts:124`), plus put/delete/discover.
> - **Hub Tracker tab** — `ProjectBindingEditor.tsx` already lets a project
>   configure its binding (`ProjectHubPage.tsx:46-50`, tab `'tracker'`).
> - **Hub Tasks embed** — `<TaskPage key={repo.id} embedded />`
>   (`ProjectHubPage.tsx:237`) with `preselectedRepoId` seeding (`:59-74`); the
>   binding-load effect (`:82-123`) is **retargeted** (write per-repo state, pass
>   `hostId`), not rewritten.
> - **Board view + picker** — `ProjectViewWrapper.tsx` (global read at `:87`)
>   mounted at `TaskPage.tsx:4062`; `ProjectPicker.tsx` (global write at
>   `:186,212`). Both are *modified to key by repo*, not rebuilt.
> - **Resolution-precedence precedent** — `CreateWorkspaceWizard.tsx:136-140`
>   (spec 011 F2) already resolves per-repo binding first, global `activeProject`
>   as fallback; the new resolver generalizes this exact order.
> - **Hub navigation** — `openProjectHub(repoId, tab)`
>   (`ui/src/store/slices/ui.ts:508,929`) is the re-route target for all Board
>   deep links. (Note: `projectHubTab` union at `ui.ts:503` already drifted from
>   `ProjectHubPage`'s `HubTab` — extend, don't fork.)
> - **Settings shape** — `GitHubProjectSettings`
>   (`ui/src/shared/github-project-types.ts:224-234`) and the stable-shape
>   default in `shared/constants.ts:317`.
>
> ### Build new
>
> - `activeProjectByRepo: Record<string, ProjectRef>` field on
>   `GitHubProjectSettings` (legacy `activeProject` kept, read-only).
> - A **pure resolver** (new `ui/src/lib/` module, vitest-covered):
>   `resolveBoardProject(repoId, settings, binding)` with precedence
>   **explicit per-repo pick → server binding → legacy global**.
> - Repo-context threading: embedded `TaskPage` passes the hub repo to
>   `ProjectViewWrapper`/`ProjectPicker` (prop or store selector) so reads/writes
>   key by `repo.id`; `hostId` threaded into the binding load.
> - Sidebar Board removal + re-pointing of `openTaskPage()` callers to
>   `openProjectHub(repoId, 'tasks')` / Projects page.
>
> ## Risks & invariants
>
> - **Sibling-field settings writes:** `ProjectPicker` deliberately spreads the
>   whole `githubProjects` object so pinned/recent/lastView survive
>   (`ProjectPicker.tsx:184`); the per-repo map write must keep that discipline
>   or user state is wiped.
> - **Spec 012 wizard fallback:** `CreateWorkspaceWizard.tsx:140` reads the
>   legacy `activeProject` — it must keep working (ideally switched to the new
>   resolver) or workspace-creation issue-picking regresses.
> - **Design rule (v0.59.1):** sidebar never accumulates project-scoped
>   surfaces; this spec *enforces* it — do not reintroduce any per-repo rows.
> - **Stale-base hazard:** implement off fresh `origin/develop`; every line ref
>   above drifts (documented lesson from v0.70.0).
> - **Architecture principles untouched:** no launch-path, YOLO, or streaming
>   surface is anywhere near this slice; server is read-only for this work.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:**
>   1. `per-repo-board-resolution` — settings field + pure resolver + wire
>      `ProjectViewWrapper`/`ProjectPicker` to repo context; legacy fallback.
>   2. `hub-binding-retarget` — hub effect writes per-repo state (no global
>      write), passes `hostId`; remove the copy hack.
>   3. `sidebar-board-removal` — delete the rail entry, re-route
>      palette/App/ChatPage callers, audit `openTaskPage()`.
> - **`verify.sh` asserts:** `bun run build --prefix crates/agentum-desktop/ui`
>   green; `bunx vitest` green on the new resolver suite (per-repo pick beats
>   binding beats legacy; unknown repo falls through; no write-path returns the
>   legacy slot); `git grep -n 'openTaskPage('` shows every caller outside
>   `TaskPage.tsx` and test files passes a work-item detail payload
>   (`openGitHubWorkItem`/`openLinearIssue`); zero bare `openTaskPage()` calls
>   remain.
> - **`qa.sh` asserts (browser):** sidebar has no Board button; bind project A →
>   X and project B → Y, flip between hubs, each shows its own board and
>   `activeProject` in persisted settings is unchanged; legacy-only settings
>   still render a board; an SSH repo with a binding renders its board.
>
> ## Locked decisions (PM, 2026-07-13 — autonomous run)
>
> - **D1 — pick-wins + divergence hint.** Displayed board resolves **explicit
>   per-repo pick → server binding → legacy global**. When pick ≠ binding, show
>   a non-blocking hint naming the bound project with one-click "Use bound
>   project" (deletes `activeProjectByRepo[repoId]`). Rejected
>   pick-updates-binding: `ProjectBindingEditor.handleSave`
>   (`ProjectBindingEditor.tsx:224-254`) requires a `/discover` round + complete
>   5-phase status mapping (`mappingComplete` gate at `:225`) — a browse action
>   must not coerce the automation write path. Rejected binding-wins: makes the
>   picker dead UI for bound repos. Transitions keep writing to the binding
>   (spec 010 untouched).
> - **D2 — bare openers route repo-first, Projects-page fallback.**
>   `openProjectHub(repoId, 'tasks')` when a repo resolves (payload
>   `preselectedRepoId`, else live `activeRepoId` — set at `ui.ts:934`,
>   `lib/worktree-activation.ts:172`, but null on cold start
>   (`settings.ts:42`)); else `openProjectsPage()` (`ui.ts:1132-1139`).
>   Preserve the has-git-repo gate (`App.tsx:1282`, `useIpcEvents.ts:820`).
> - **D3 — the `activeView === 'tasks'` branch STAYS, and is not dead code.**
>   Work-item **detail** openers (`WorktreeCard.tsx:530/540`, `ChatPage.tsx:480`),
>   TaskPage-internal navigation (`TaskPage.tsx:576/798/853/3245`), and
>   nav-history replay (`worktree-nav-history.ts:233-238` calls
>   `setActiveView('tasks')` directly) all land there by design. Re-homing
>   detail views into the hub is blocked today (`openProjectHub` wipes
>   `taskPageData` at `ui.ts:935-941`) and is a separate future spec — not a
>   mechanical follow-up.
>
> ## PM risks for the architect
>
> 1. `openProjectHub` wipes `taskPageData` to `{preselectedRepoId}`
>    (`ui.ts:935-941`) — do NOT attempt detail-payload threading this slice.
> 2. The hub effect's `setTaskResumeState({ githubMode: 'project' })`
>    (`ProjectHubPage.tsx:97`) is a **global** slot: with pick-wins it must fire
>    whenever the resolver yields a project (pick OR binding) and must not leak
>    stale mode into an unbound repo's hub — decide where it lives (likely the
>    resolver consumer, keyed by repo).
> 3. `ProjectPicker.commitSelection` retarget must be surgical: keep `recent` +
>    `lastViewByProject` writes global (project-keyed, repo-agnostic;
>    `ProjectPicker.tsx:194-217`); redirect only the active-slot write to
>    `activeProjectByRepo[repoId]`; feed the picker's `activeProject` display
>    prop (`:37-42`) the *resolved* per-repo value.
> 4. Nav-history: re-routed bare openers stop recording `'tasks'` entries
>    (`openProjectHub` records none) while old entries still replay into the
>    standalone view — confirm back/forward UX after a hub-routed board visit.
> 5. `projectHubTab` union (`ui.ts:503`) lacks `'tracker'` vs
>    `ProjectHubPage.tsx:30` — extend, don't fork.
> 6. SSH verified: hub effect calls `getProjectBinding({ workdir: repo.path })`
>    with no `hostId` (`ProjectHubPage.tsx:84`); client accepts it
>    (`github-projects-client.ts:124-136`) — pure client threading.
> 7. Stale `activeRepoId`: D2's hub route must verify the id still resolves in
>    `s.repos` before calling `openProjectHub` (repos can be removed under a
>    stale id); resolver no-result must fall back to the plain issue Kanban, and
>    the forced `githubMode:'project'` must never leak into that fallback.
