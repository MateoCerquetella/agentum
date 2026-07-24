# Tasks — Spec 016 · Board lives inside each project

Developer log (autonomous SDD run, 2026-07-13/14). One entry per slice; gates
recorded with exact numbers. Deviations from `architecture.md` are called out
inline — absence of a note means "implemented as written".

## F1 — per-repo-board-resolution ✅

**Built** (all paths under `crates/agentum-desktop/ui/src/`):

- `shared/github-project-types.ts` — `GitHubProjectSettings` gains optional
  `activeProjectByRepo` (+ legacy-slot doc comment) per §1.1.
- `shared/constants.ts` — stable default gains `activeProjectByRepo: {}` (§1.2).
- `lib/board-project-resolution.ts` (new) — `resolveBoardProject` (pick →
  binding → legacy → none, with `pending` held before the binding/legacy tiers),
  `applyBoardPick`, `clearBoardPick`, exported structural types. Pure, no
  React/store imports. Import path note: architecture's sketch showed
  `'../../../shared/github-project-types'`; the repo's `lib/` convention is the
  `@/shared/...` alias (vite + vitest both resolve it) — used that.
- `lib/board-project-resolution.test.ts` (new) — §2.4 cases 1–13, expressed as
  15 `it` blocks (cases 4, 8, 12 have two assertions-worth of scenarios each;
  case 12's write-path coverage is split across three `it`s).
- `store/slices/github.ts` — `projectBindingByRepo: {}` +
  `setProjectBindingState` (state + setter only; no writer until F2) (§3.2).
- `components/github-project/ProjectViewWrapper.tsx` — `repoId` prop (default
  null); the `:87` global read replaced by the resolver memo (bindingState from
  the store map, module-const `BINDING_ABSENT = {status:'loaded',binding:null}`
  for a missing entry per §3.1's `?? {…}`); `pending` renders the existing
  `ProjectTableSkeleton` (never the legacy flash or the "Choose a project"
  prompt); divergence hint bar (amber banner, bound title from the binding
  entry's `projectTitle`, else `owner/#number`) with "Use bound project" →
  `clearBoardPick` via fresh-`getState()` (§3.5); `repoId` passed to the picker.
- `components/github-project/ProjectPicker.tsx` — `repoId` prop;
  `commitSelection`'s mutate body → `applyBoardPick(prev, repoId, …)` (§3.4);
  everything else (pinned/recent/browse/paste, whole-object spread) untouched.
- `components/TaskPage.tsx` — `<ProjectViewWrapper repoId={embeddedRepoId} />`
  at the Project-mode mount; embedded resolver memo + local-mode effect (ref
  tracks last-applied resolution identity; `none` → 'items', `pending` → no-op)
  (§3.3); resume effect skips the global `taskResumeState.githubMode` when
  embedded; mode buttons gate their `setTaskResumeState` writes on `!embedded`.

**Behavior note (per §6 ordering rule):** F1 alone keeps the hub copy-hack, so
embedded boards resolve via the legacy tier the hack still populates (nothing
writes `projectBindingByRepo` yet → `BINDING_ABSENT` → legacy fallback).

**Gates:**

- `bun run build` — green, 1m 01s.
- `bunx vitest run src/lib/board-project-resolution.test.ts` — 1 file, 15/15.
- `bunx vitest run src/components/new-workspace/work-item-picker-model.test.ts
  src/components/new-workspace/create-workspace-wizard-model.test.ts` (+ the
  resolver file in the same run) — 3 files, 72/72 passed. Wizard model files
  byte-untouched (`git status` clean for `components/new-workspace/`).

**Deviations:** none beyond the import-path note above.

## F2 — hub-binding-retarget ✅

**Built:**

- `components/project-hub/ProjectHubPage.tsx` — the binding effect retargeted
  per §4. Trigger (`repo`, `tab === 'tasks'`, `isGitRepoKind`) + cancellation
  pattern kept; body now: `deriveTrackerBindingTarget({ repo, isGit: true })`
  (import-only from the wizard model — the v0.75.1 host-aware seam, so SSH
  repos thread `connectionId → hostId`), no-target → `loaded/null`; first visit
  writes `{status:'loading'}` (an existing 'loaded' entry is kept while
  refetching — no flicker); `getProjectBinding({ workdir, hostId })` →
  `loaded` with the raw identity+title subset; fetch failure → fail-closed
  `loaded/null` (resolver falls to legacy, `pending` can never wedge). The
  `setTaskResumeState({githubMode:'project'})` and `updateSettings` copy-hack
  calls are DELETED (the 'project'-mode forcing moved to the embedded TaskPage
  in F1 §3.3). The incomplete-identity guard moved into the resolver's
  normalization. `taskDataSeeded` gate/effect and the render gate untouched.
- Note: the old `repo?.path` trigger guard is subsumed by
  `deriveTrackerBindingTarget` (empty path → null target → `loaded/null`) —
  behaviorally identical, one seam instead of two checks.

**Gates:**

- `bun run build` — green, 1m 47s.
- F1 vitest set — 3 files, 72/72 passed.
- `git grep -n 'updateSettings\|setTaskResumeState' --
  src/components/project-hub/ProjectHubPage.tsx` — empty (exit 1).

**Deviations:** none.

## F3 — sidebar-board-removal ✅

**Built:**

- `lib/board-route.ts` + `lib/board-route.test.ts` (new) — `resolveBoardRoute`
  (pure; preferred → active, each guarded live-git-repo, else projects; NO
  first-git-repo fallback) + `openBoardSurface` dispatcher (reads
  `useAppStore.getState()`, dispatches `openProjectHub(repoId, 'tasks',
  {taskSource})` or `openProjectsPage()`). 6 tests: preferred wins, stale
  preferred → active, stale active → projects, non-git excluded, empty repos →
  projects, null ids → projects.
- `store/slices/ui.ts` — `openProjectHub(repoId, tab?, seed?: { taskSource? })`
  merges the seed into `taskPageData` (§5.2, NOT detail threading);
  `projectHubTab` union extended with `'tracker'` (PM risk 5 — now matches
  `ProjectHubPage`'s `HubTab`).
- `components/sidebar/SidebarNav.tsx` — Board button deleted along with ALL of
  its machinery: `openTaskPage` selector, `canBrowseTasks`, `showTasksButton`
  read, the prefetch handler + its `prefetchWorkItems`/`activeRepoId`/
  `defaultTaskViewPreset` selectors, the task-provider derivation
  (`visibleTaskProviders`/`resolvedDefaultTaskSource`) and the
  preflight/linear warm-up effect that existed only to feed the prefetch
  gate, `tasksActive`, and the now-unused imports (Columns3, useRepoMap,
  isGitRepoKind, new-workspace + task-providers helpers).
- `components/settings/AppearancePane.tsx` — dead "Show Tasks Button" toggle
  deleted; the `showTasksButton` settings field itself KEPT
  (`shared/types.ts` / `shared/constants.ts` untouched — persisted-settings
  compat).
- Re-routes (table rows 2–6, every existing gate + preventDefault kept):
  `CommandPalette.tsx` `view-board` → `openBoardSurface()` (id/label/icon
  kept); `App.tsx` `view.tasks` shortcut → gate + preventDefault +
  notifyTerminalCapture unchanged, body → `openBoardSurface()`;
  `hooks/useIpcEvents.ts` `onOpenTasks` → gate unchanged, body →
  `openBoardSurface()`; `ChatPage.tsx` filed-card fallback →
  `openBoardSurface({ preferredRepoId: filedRepoId, taskSource: linear|github })`;
  ChatPage "Open Board" header →
  `openBoardSurface({ preferredRepoId: workspaceId ?? undefined, taskSource: 'github' })`.
- Rows 7–10 UNTOUCHED: ChatPage detail opener (`:481`, openGitHubWorkItem),
  WorktreeCard `:530/:540` (detail payloads), TaskPage internal nav,
  worktree-nav-history.

**Gates:**

- `bun run build` — green, 1m 11s.
- `bunx vitest run src/lib/board-route.test.ts
  src/lib/board-project-resolution.test.ts` — 2 files, 21/21 passed.
- Grep gate: `git grep -n 'openTaskPage(' -- crates/agentum-desktop/ui/src`
  outside TaskPage.tsx/tests = exactly `ChatPage.tsx:481`,
  `WorktreeCard.tsx:530`, `WorktreeCard.tsx:540` — all detail-payload; zero
  bare calls. `git grep -n '"Board"' …/SidebarNav.tsx` — empty.

**Deviations:** none. (The SidebarNav preflight/linear warm-up effect deletion
is within the table-row-1 mandate "prefetch handler … and its now-unused
imports" — the effect fed only the prefetch gate; every other consumer
(TaskPage, hub) triggers the same checks itself on mount.)

## Anchor drift notes (for the tester)

- All architecture `path:line` anchors matched this HEAD after re-location;
  the only nominal drifts were ±1–2 lines (e.g. ChatPage detail opener now at
  `:481`, WorktreeCard rows unchanged). No semantic drift found.
- `shared/*` paths in the architecture resolve to `src/shared/*` in this repo
  (the `../../../../shared` vite alias); imports in new code use `@/shared/…`.
