# Right-click to remove a project from the workspace — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Right-clicking a project (repo) header row in the desktop sidebar opens the same actions menu the `⋯` ellipsis button already shows — including **Remove Project**.

**Architecture:** Mirror the app's existing cursor-anchored right-click pattern (`WorktreeContextMenu`). Extract the ellipsis menu's items into one shared `renderProjectActionItems(repo)` closure so both entry points stay in sync. A single hoisted state in `VirtualizedWorktreeViewport` drives one list-level `DropdownMenu` anchored at the cursor. The new decision logic (when to open, macOS ctrl-click click-suppression) lives in a small pure module that is unit-tested. No backend/store changes — the removal pipeline (`handleRemoveProject` → `RemoveFolderDialog` → `removeProject` → `DELETE /api/repos/{id}`) is reused unchanged.

**Tech Stack:** React 18 + TypeScript, Radix `DropdownMenu`, Vite, Vitest (node environment, `renderToStaticMarkup`).

## Global Constraints

- All paths are relative to `crates/agentum-desktop/ui` unless stated otherwise. Run all `npx`/`npm` commands from that directory.
- Vitest runs in the **node** environment (no jsdom). Tests use pure functions or `renderToStaticMarkup` — never `fireEvent`/`@testing-library` (not a dependency).
- No `test`/`lint`/`typecheck` npm scripts exist. Per-file tests: `npx vitest run <path>`. Whole-UI compile gate: `npm run build`.
- Touch only the new right-click entry point. Do **not** change the `⋯` button behavior, the worktree right-click menu, the store, server routes, or any group/host header.
- Right-click handling applies **only** to project rows: the guard is `row.repo && groupBy === 'repo'`.
- `groupBy` has type `WorktreeGroupBy` (`'none' | 'workspace-status' | 'repo' | 'pr-status' | 'host'`) from `./worktree-list-groups`.
- Reuse `CLOSE_ALL_CONTEXT_MENUS_EVENT` exported from `./WorktreeContextMenu` (do not invent a new event string).

---

### Task 1: Pure decision helpers + unit tests

The new, genuinely-branching logic, isolated into a pure module so it can be unit-tested in the node vitest environment. Matches the codebase's `shouldXxx`-predicate idiom (see `WorktreeContextMenu.tsx`).

**Files:**
- Create: `src/components/sidebar/project-context-menu.ts`
- Test: `src/components/sidebar/project-context-menu.test.ts`

**Interfaces:**
- Produces (consumed by Task 3):
  - `type ProjectContextMenuTarget = { repo: Repo; x: number; y: number }`
  - `getProjectContextMenuTarget(args: { groupBy: WorktreeGroupBy; repo: Repo | null | undefined; clientX: number; clientY: number }): ProjectContextMenuTarget | null`
  - `shouldSuppressProjectHeaderClick(openedAt: number | null, now: number): boolean`

- [ ] **Step 1: Write the failing test**

Create `src/components/sidebar/project-context-menu.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import type { Repo } from '../../../../shared/types'
import {
  getProjectContextMenuTarget,
  shouldSuppressProjectHeaderClick
} from './project-context-menu'

function makeRepo(overrides: Partial<Repo> = {}): Repo {
  return {
    id: 'repo-1',
    path: '/tmp/repo-1',
    displayName: 'Repo One',
    badgeColor: '#fff',
    addedAt: 0,
    ...overrides
  } as Repo
}

describe('getProjectContextMenuTarget', () => {
  it('returns the cursor-anchored target for a repo row in repo grouping', () => {
    const repo = makeRepo()
    expect(
      getProjectContextMenuTarget({ groupBy: 'repo', repo, clientX: 120, clientY: 240 })
    ).toEqual({ repo, x: 120, y: 240 })
  })

  it('returns null when grouping is not "repo"', () => {
    const repo = makeRepo()
    expect(
      getProjectContextMenuTarget({ groupBy: 'host', repo, clientX: 1, clientY: 2 })
    ).toBeNull()
    expect(
      getProjectContextMenuTarget({ groupBy: 'none', repo, clientX: 1, clientY: 2 })
    ).toBeNull()
  })

  it('returns null when there is no repo (group/host header row)', () => {
    expect(
      getProjectContextMenuTarget({ groupBy: 'repo', repo: null, clientX: 1, clientY: 2 })
    ).toBeNull()
    expect(
      getProjectContextMenuTarget({ groupBy: 'repo', repo: undefined, clientX: 1, clientY: 2 })
    ).toBeNull()
  })
})

describe('shouldSuppressProjectHeaderClick', () => {
  it('never suppresses when no menu open was recorded', () => {
    expect(shouldSuppressProjectHeaderClick(null, 1000)) .toBe(false)
  })

  it('suppresses the click that immediately follows opening the menu', () => {
    expect(shouldSuppressProjectHeaderClick(1000, 1000)).toBe(true)
    expect(shouldSuppressProjectHeaderClick(1000, 1499)).toBe(true)
    expect(shouldSuppressProjectHeaderClick(1000, 1500)).toBe(true)
  })

  it('does not suppress once the suppression window has elapsed', () => {
    expect(shouldSuppressProjectHeaderClick(1000, 1501)).toBe(false)
  })

  it('does not suppress a click that predates the recorded open (clock skew)', () => {
    expect(shouldSuppressProjectHeaderClick(1000, 999)).toBe(false)
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/components/sidebar/project-context-menu.test.ts`
Expected: FAIL — `Failed to resolve import "./project-context-menu"` (module does not exist yet).

- [ ] **Step 3: Write the minimal implementation**

Create `src/components/sidebar/project-context-menu.ts`:

```ts
// Why: isolate the right-click open-decision and the macOS ctrl-click
// click-suppression window as pure functions so they are unit-testable in the
// node vitest environment (no DOM). Mirrors the shouldXxx predicates in
// WorktreeContextMenu.tsx.
import type { Repo } from '../../../../shared/types'
import type { WorktreeGroupBy } from './worktree-list-groups'

export type ProjectContextMenuTarget = { repo: Repo; x: number; y: number }

// macOS ctrl-click fires `contextmenu` AND a follow-up primary `click`. This
// window lets the row swallow that one trailing click so opening the menu does
// not also toggle/select the project. Same 500ms budget as WorktreeContextMenu.
const PROJECT_HEADER_CLICK_SUPPRESSION_MS = 500

export function getProjectContextMenuTarget(args: {
  groupBy: WorktreeGroupBy
  repo: Repo | null | undefined
  clientX: number
  clientY: number
}): ProjectContextMenuTarget | null {
  if (args.groupBy !== 'repo' || args.repo == null) {
    return null
  }
  return { repo: args.repo, x: args.clientX, y: args.clientY }
}

export function shouldSuppressProjectHeaderClick(
  openedAt: number | null,
  now: number
): boolean {
  if (openedAt == null) {
    return false
  }
  const elapsed = now - openedAt
  return elapsed >= 0 && elapsed <= PROJECT_HEADER_CLICK_SUPPRESSION_MS
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/components/sidebar/project-context-menu.test.ts`
Expected: PASS — 2 suites, 7 tests passing.

- [ ] **Step 5: Commit**

```bash
git add src/components/sidebar/project-context-menu.ts src/components/sidebar/project-context-menu.test.ts
git commit -m "feat(sidebar): pure helpers for project-row right-click menu"
```

---

### Task 2: Extract the project `⋯` menu items into one shared closure

Pure refactor: lift the ellipsis menu's `DropdownMenuItem`s into a `renderProjectActionItems(repo)` closure inside `VirtualizedWorktreeViewport`, then render it from the existing `⋯` menu. No behavior change — this is the single source both entry points will use, so they cannot drift. Verified by the existing render test staying green and the UI still building.

**Files:**
- Modify: `src/components/sidebar/WorktreeList.tsx` (define helper near other in-component callbacks; replace ellipsis `DropdownMenuContent` body at `:2909–3000`)

**Interfaces:**
- Consumes: in-scope props/helpers already destructured in `VirtualizedWorktreeViewport` — `handleOpenRepoSettings`, `handleOpenWorktreeVisibility`, `handleCreateGroupFromRepo`, `handleMoveProjectToGroup`, `handleRemoveProjectFromGroup`, `handleRemoveProject`, `projectGroups`; module helpers `isGitRepoKind`, `getRepositoryIconSectionId`, `getWorktreeVisibilityMenuLabel`; icons `SlidersHorizontal`, `Shapes`, `Eye`, `FolderPlus`, `FolderInput`, `CircleX`, `Trash2` (all already imported).
- Produces (consumed by Task 3): `renderProjectActionItems(repo: Repo): React.ReactNode`

- [ ] **Step 1: Define the shared render helper**

Inside `VirtualizedWorktreeViewport` (after the existing `useState`/`useCallback` block, e.g. near line ~865, before the `return (`), add. `Repo` is already imported in this file:

```tsx
// Why: both the project header ⋯ button and the new right-click context menu
// render the exact same actions, so the item list lives in one closure to
// prevent the two entry points from drifting apart.
const renderProjectActionItems = (repo: Repo) => (
  <>
    <DropdownMenuItem onSelect={() => handleOpenRepoSettings(repo.id)}>
      <SlidersHorizontal className="size-3.5" />
      Project Settings
    </DropdownMenuItem>
    <DropdownMenuItem
      onSelect={() => handleOpenRepoSettings(repo.id, getRepositoryIconSectionId(repo.id))}
    >
      <Shapes className="size-3.5" />
      Change Project Icon
    </DropdownMenuItem>
    {isGitRepoKind(repo) ? (
      <DropdownMenuItem onSelect={() => handleOpenWorktreeVisibility(repo.id)}>
        <Eye className="size-3.5" />
        {getWorktreeVisibilityMenuLabel(repo)}
      </DropdownMenuItem>
    ) : null}
    <DropdownMenuItem onSelect={() => handleCreateGroupFromRepo(repo)}>
      <FolderPlus className="size-3.5" />
      New group from project
    </DropdownMenuItem>
    {projectGroups.length > 0 ? (
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>
          <FolderInput className="size-3.5" />
          Move to group
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          {projectGroups.map((group) => (
            <DropdownMenuItem
              key={group.id}
              disabled={repo.projectGroupId === group.id}
              onSelect={() => handleMoveProjectToGroup(repo, group.id)}
            >
              <span className="max-w-48 truncate">{group.name}</span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuSubContent>
      </DropdownMenuSub>
    ) : null}
    {repo.projectGroupId ? (
      <DropdownMenuItem onSelect={() => handleRemoveProjectFromGroup(repo)}>
        <CircleX className="size-3.5" />
        Remove from group
      </DropdownMenuItem>
    ) : null}
    <DropdownMenuSeparator />
    <DropdownMenuItem variant="destructive" onSelect={() => handleRemoveProject(repo)}>
      <Trash2 className="size-3.5" />
      Remove Project
    </DropdownMenuItem>
  </>
)
```

- [ ] **Step 2: Replace the ellipsis menu body to use the helper**

In the `⋯` menu `DropdownMenuContent` (`:2903–3001`), replace the inner item block (`:2909–3000`) with a single call. The surrounding `<DropdownMenuContent align="end" side="bottom" sideOffset={6} onClick={(event) => event.stopPropagation()}>` … `</DropdownMenuContent>` stays. New body:

```tsx
<DropdownMenuContent
  align="end"
  side="bottom"
  sideOffset={6}
  onClick={(event) => event.stopPropagation()}
>
  {renderProjectActionItems(row.repo)}
</DropdownMenuContent>
```

(`row.repo` is non-null in this branch — it is already guarded by `{row.repo && groupBy === 'repo' ? (` at `:2880`.)

- [ ] **Step 3: Verify no regression in the existing render test**

Run: `npx vitest run src/components/sidebar/WorktreeList.lineage-child-card.test.ts`
Expected: PASS — same test count as before the change (the static markup of always-rendered rows/headers is unchanged; the ⋯ dropdown content is not part of static markup either way).

- [ ] **Step 4: Verify the UI still builds**

Run: `npm run build`
Expected: Vite build completes with exit code 0 (imports resolve, JSX compiles).

- [ ] **Step 5: Commit**

```bash
git add src/components/sidebar/WorktreeList.tsx
git commit -m "refactor(sidebar): extract project ⋯ menu items into shared closure"
```

---

### Task 3: Wire right-click on the project header row + list-level menu

Add the new entry point: `onContextMenu` on the project header row opens a single cursor-anchored `DropdownMenu` rendered once at the viewport root, showing `renderProjectActionItems`. Coordinate closing with other menus and suppress the macOS ctrl-click follow-up click.

**Files:**
- Modify: `src/components/sidebar/WorktreeList.tsx` (import line `:129`; add state/ref/effect in `VirtualizedWorktreeViewport`; header div `:2714`; mount menu before container close `:3454`)

**Interfaces:**
- Consumes (from Task 1): `getProjectContextMenuTarget`, `shouldSuppressProjectHeaderClick`, `ProjectContextMenuTarget`.
- Consumes (from Task 2): `renderProjectActionItems(repo)`.
- Consumes (existing): `CLOSE_ALL_CONTEXT_MENUS_EVENT` from `./WorktreeContextMenu`; Radix `DropdownMenu`, `DropdownMenuTrigger`, `DropdownMenuContent` (already imported).

- [ ] **Step 1: Add imports**

At `:129`, extend the existing default import to also pull the event constant:

```tsx
import WorktreeContextMenu, { CLOSE_ALL_CONTEXT_MENUS_EVENT } from './WorktreeContextMenu'
```

Add the helpers import alongside the other `./`-sibling imports (e.g. near the `worktree-list-groups` import):

```tsx
import {
  getProjectContextMenuTarget,
  shouldSuppressProjectHeaderClick,
  type ProjectContextMenuTarget
} from './project-context-menu'
```

- [ ] **Step 2: Add hoisted state + suppression ref + close-coordination effect**

Inside `VirtualizedWorktreeViewport`, next to the other `useState` hooks (~`:850`):

```tsx
// Single open-at-a-time right-click menu for project header rows. Hoisted to
// the viewport (not per-row) so a virtualized list renders only one menu.
const [projectContextMenu, setProjectContextMenu] =
  useState<ProjectContextMenuTarget | null>(null)
// Timestamp of the last right-click open, used to swallow the macOS ctrl-click
// follow-up `click` so opening the menu does not also toggle the project row.
const projectHeaderContextOpenedAtRef = useRef<number | null>(null)
```

Add an effect near the other `useEffect`s (the exact location is not load-bearing; place it after the state above is in scope):

```tsx
// Why: closing coordination — right-clicking any other surface dispatches
// CLOSE_ALL_CONTEXT_MENUS_EVENT; dismiss our project menu when it fires.
useEffect(() => {
  const closeProjectMenu = () => setProjectContextMenu(null)
  window.addEventListener(CLOSE_ALL_CONTEXT_MENUS_EVENT, closeProjectMenu)
  return () => window.removeEventListener(CLOSE_ALL_CONTEXT_MENUS_EVENT, closeProjectMenu)
}, [])
```

- [ ] **Step 3: Wire `onContextMenu` + click-suppression on the header row**

On the project header `div` (`:2714`), add an `onContextMenu` handler and guard the existing `onClick`. The current `onClick` starts at `:2761`:

```tsx
onClick={(event) => {
  if (
    projectIdForHeader &&
    onProjectSelectionGesture(event, projectIdForHeader)
  ) {
    event.preventDefault()
    event.stopPropagation()
    return
  }
  toggleGroupWithScrollAnchor(row.key)
}}
```

Replace it with (prepend the suppression check; rest unchanged):

```tsx
onClick={(event) => {
  // Swallow the macOS ctrl-click that just opened the right-click menu.
  if (shouldSuppressProjectHeaderClick(projectHeaderContextOpenedAtRef.current, Date.now())) {
    projectHeaderContextOpenedAtRef.current = null
    event.preventDefault()
    event.stopPropagation()
    return
  }
  projectHeaderContextOpenedAtRef.current = null
  if (
    projectIdForHeader &&
    onProjectSelectionGesture(event, projectIdForHeader)
  ) {
    event.preventDefault()
    event.stopPropagation()
    return
  }
  toggleGroupWithScrollAnchor(row.key)
}}
onContextMenu={(event) => {
  const target = getProjectContextMenuTarget({
    groupBy,
    repo: row.repo,
    clientX: event.clientX,
    clientY: event.clientY
  })
  if (!target) {
    return
  }
  event.preventDefault()
  projectHeaderContextOpenedAtRef.current = Date.now()
  window.dispatchEvent(new Event(CLOSE_ALL_CONTEXT_MENUS_EVENT))
  setProjectContextMenu(target)
}}
```

- [ ] **Step 4: Mount the cursor-anchored menu once at the viewport root**

Immediately before the closing `</div>` of the `data-worktree-sidebar-container` root (`:3454`), add:

```tsx
{/* Right-click context menu for project header rows. Rendered once; a hidden
    fixed-positioned trigger anchors Radix's menu at the cursor. */}
<DropdownMenu
  open={projectContextMenu != null}
  onOpenChange={(open) => {
    if (!open) {
      setProjectContextMenu(null)
    }
  }}
  modal={false}
>
  <DropdownMenuTrigger asChild>
    <button
      aria-hidden
      tabIndex={-1}
      className="pointer-events-none fixed size-px opacity-0"
      style={{ left: projectContextMenu?.x ?? 0, top: projectContextMenu?.y ?? 0 }}
    />
  </DropdownMenuTrigger>
  <DropdownMenuContent align="start" sideOffset={0} onClick={(event) => event.stopPropagation()}>
    {projectContextMenu ? renderProjectActionItems(projectContextMenu.repo) : null}
  </DropdownMenuContent>
</DropdownMenu>
```

- [ ] **Step 5: Verify existing render test + build**

Run: `npx vitest run src/components/sidebar/WorktreeList.lineage-child-card.test.ts`
Expected: PASS — unchanged test count (the menu only mounts content when `projectContextMenu != null`, which is never true in the static render fixture; the hidden trigger button adds no asserted-on markup).

Run: `npm run build`
Expected: Vite build completes with exit code 0.

- [ ] **Step 6: Commit**

```bash
git add src/components/sidebar/WorktreeList.tsx
git commit -m "feat(sidebar): right-click a project row to open its actions menu"
```

---

### Task 4: Manual verification in the running desktop app

Automated interaction tests are not feasible here (node vitest, no jsdom/testing-library), so confirm the behavior in the real app once.

**Files:** none (verification only)

- [ ] **Step 1: Launch the desktop app**

From the repo root of the worktree:

```bash
npm run dev --prefix crates/agentum-desktop/ui   # serves the UI
# In another shell, run the Tauri shell if needed, or use the existing dev flow.
```

(If a project `run`/`verify` skill or script exists, prefer it. The goal is a window showing the sidebar with at least one project and `groupBy === 'repo'`.)

- [ ] **Step 2: Verify the four acceptance behaviors**

1. Right-click a **project header row** → a context menu appears at the cursor containing **Remove Project** (plus Project Settings, Change Project Icon, New group from project, etc. — identical to the `⋯` button).
2. Click **Remove Project** → the existing `RemoveFolderDialog` confirmation appears; confirming removes the project from the sidebar (and `~/.agentum/repos.json`).
3. Right-clicking the row does **not** collapse/expand or select the project (ctrl-click suppression works on macOS).
4. Right-click a **project-group header** or a **host header** → no project context menu appears (guard holds).

- [ ] **Step 3: Record the result**

Note pass/fail for each of the four behaviors in the PR description / completion summary. If any fail, return to the relevant task — do not claim completion.

---

## Self-Review

**1. Spec coverage**
- Right-click opens same menu as `⋯` → Task 2 (shared closure) + Task 3 (wiring). ✓
- Cursor-anchored, single hoisted state, virtualized-safe → Task 3 Steps 2 & 4. ✓
- macOS ctrl-click guard → Task 1 (`shouldSuppressProjectHeaderClick`) + Task 3 Step 3. ✓
- Cross-menu close coordination → Task 3 Step 2 effect + Step 3 dispatch. ✓
- Only `groupBy === 'repo'` project rows → Task 1 (`getProjectContextMenuTarget` guard) + Task 4 behavior 4. ✓
- No server/store changes → reuse `handleRemoveProject`; no such files in any task's Files list. ✓
- Testing via pure helpers + manual → Task 1 + Task 4. ✓

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; commands have expected output. ✓

**3. Type consistency:** `ProjectContextMenuTarget` defined in Task 1, consumed by name in Task 3. `getProjectContextMenuTarget`/`shouldSuppressProjectHeaderClick` signatures identical across Tasks 1 and 3. `renderProjectActionItems(repo: Repo)` defined in Task 2, called with `row.repo` (Task 2) and `projectContextMenu.repo` (Task 3), both `Repo`. `CLOSE_ALL_CONTEXT_MENUS_EVENT` sourced from `./WorktreeContextMenu` in both dispatch and listen. ✓
