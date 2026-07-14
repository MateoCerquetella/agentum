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

## F2 — hub-binding-retarget

_(pending)_

## F3 — sidebar-board-removal

_(pending)_
