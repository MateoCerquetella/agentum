# Right-click to remove a project from the workspace

**Date:** 2026-06-16
**Branch:** `feat/remove-project-rightclick`
**Status:** Approved design — ready for implementation plan

## Problem

Removing a project (repo) from the agentum desktop sidebar is currently
only reachable via the project header's left-click `⋯` (ellipsis) button,
or by right-clicking one of the project's *worktree* cards ("Remove Project
from Agentum"). Right-clicking the **project header row itself** does
nothing. Users expect right-click on the project to offer the same actions
the `⋯` button does — this is standard desktop behavior.

## Goal

Right-clicking a project (repo) header row in the sidebar opens the **same
actions menu** the `⋯` ellipsis button already shows — which includes
**Remove Project**. Selecting it runs the existing removal pipeline
unchanged.

## Non-goals (YAGNI)

- No server/backend changes. The removal pipeline already exists end-to-end:
  `RemoveFolderDialog` (confirm) → `removeProject()` store action →
  `DELETE /api/repos/{id}` (rewrites `~/.agentum/repos.json`).
- No new menu items, and no changes to the `⋯` button or the worktree
  right-click menu behavior.
- No right-click handling for **project-group** headers or **host** headers
  — only project (repo) rows, i.e. when `row.repo && groupBy === 'repo'`.
- No multi-select batch behavior for the new right-click (the `⋯` menu is
  single-project; we match it).

## Existing surface (verified)

| Piece | Location |
| --- | --- |
| Project header row `div` (`role="button"`, `data-repo-header-id`) | `crates/agentum-desktop/ui/src/components/sidebar/WorktreeList.tsx:2714` |
| Project `⋯` menu (items to mirror) | `WorktreeList.tsx:2880–3003` |
| `handleRemoveProject` (+ other project handlers) passed into the row component as props | `WorktreeList.tsx:421, 806, 4460` |
| Established right-click pattern (cursor-anchored, hidden positioned trigger) | `WorktreeContextMenu.tsx:504–546` |
| Cross-menu close coordination event | `CLOSE_ALL_CONTEXT_MENUS_EVENT = 'agentum-close-all-context-menus'` (`WorktreeContextMenu.tsx:61`) |
| macOS ctrl-click follow-up-click suppression | `WorktreeContextMenu.tsx:97–101, 472–490` |
| Confirm dialog | `RemoveFolderDialog.tsx` |
| Store action | `store/slices/repos.ts` `removeProject(projectId)` |
| Client API | `runtime/server-repo-client.ts` `reposRemove(repoId)` |

The `⋯` menu items (the set to mirror): **Project Settings**, **Change
Project Icon**, optional **worktree-visibility toggle** (git repos only),
**New group from project**, **Move to group** (submenu, when groups exist),
**Remove from group** (when in a group), separator, **Remove Project**
(destructive).

## Design

Mirror the app's existing cursor-anchored right-click pattern
(`WorktreeContextMenu`), reusing the exact menu items the `⋯` button renders
so the two entry points can never drift apart.

### 1. Extract the menu items (no duplication)

Inside the row-rendering component in `WorktreeList.tsx`, extract the
ellipsis menu's item block (`:2909–3000`) into a local render helper:

```tsx
const renderProjectActionItems = (repo: Repo) => (<>… existing items …</>)
```

It closes over the handlers already in scope (`handleRemoveProject`,
`handleOpenRepoSettings`, `handleCreateGroupFromRepo`,
`handleMoveProjectToGroup`, `handleRemoveProjectFromGroup`,
`handleOpenWorktreeVisibility`), `projectGroups`, and the helpers
(`isGitRepoKind`, `getRepositoryIconSectionId`,
`getWorktreeVisibilityMenuLabel`) — so no prop-drilling is needed. The
existing `⋯` `DropdownMenuContent` body becomes `{renderProjectActionItems(row.repo)}`.

### 2. One hoisted right-click state

Only one context menu is open at a time across the whole virtualized list,
so hoist a single piece of state in the row-rendering component:

```ts
const [projectContextMenu, setProjectContextMenu] =
  useState<{ repo: Repo; x: number; y: number } | null>(null)
```

Store the whole `Repo` (not just an id) so no lookup is needed when the
menu renders.

### 3. Add `onContextMenu` to the project header row

On the header `div` (`:2714`), only when `row.repo && groupBy === 'repo'`:

```tsx
onContextMenu={(event) => {
  if (!(row.repo && groupBy === 'repo')) return
  event.preventDefault()
  window.dispatchEvent(new Event(CLOSE_ALL_CONTEXT_MENUS_EVENT))
  setProjectContextMenu({ repo: row.repo, x: event.clientX, y: event.clientY })
}}
```

`onContextMenu` (right button) does not collide with the existing left-click
`onClick` (collapse toggle / selection gesture).

### 4. Render one cursor-anchored menu at the list root

A single controlled `DropdownMenu` rendered once (a sibling of the
virtualized rows), anchored at the cursor via a hidden `position: fixed`
trigger — mirroring `WorktreeContextMenu`'s hidden-trigger approach but in
viewport coordinates (since it is list-level, not per-row):

```tsx
<DropdownMenu
  open={projectContextMenu != null}
  onOpenChange={(open) => { if (!open) setProjectContextMenu(null) }}
  modal={false}
>
  <DropdownMenuTrigger asChild>
    <button aria-hidden tabIndex={-1}
      className="pointer-events-none fixed size-px opacity-0"
      style={{ left: projectContextMenu?.x ?? 0, top: projectContextMenu?.y ?? 0 }} />
  </DropdownMenuTrigger>
  <DropdownMenuContent align="start" sideOffset={0}>
    {projectContextMenu ? renderProjectActionItems(projectContextMenu.repo) : null}
  </DropdownMenuContent>
</DropdownMenu>
```

### 5. Cross-menu close coordination

Subscribe to `CLOSE_ALL_CONTEXT_MENUS_EVENT` and clear
`projectContextMenu` when it fires, so right-clicking a worktree (or any
other surface that dispatches the event) dismisses an open project menu and
vice-versa. The `onContextMenu` handler already dispatches the event on open.

### 6. macOS ctrl-click guard

On macOS, ctrl-click fires `contextmenu` and can also fire `click`, which
would toggle the row's collapse. Mirror `WorktreeContextMenu`'s
suppression: record the open timestamp and swallow the immediately
following `click` on the row (within ~500ms) so opening the menu never also
toggles the project. Implement via an `onClickCapture` guard on the row (or
the existing wrapper) consistent with `suppressOpeningPointerEvent`.

## Data flow

```
right-click project header div (WorktreeList.tsx:2714)
  → onContextMenu: preventDefault + dispatch CLOSE_ALL + set {repo,x,y}
  → list-level DropdownMenu opens at cursor, renders renderProjectActionItems(repo)
  → user selects "Remove Project"
  → handleRemoveProject(repo)   [existing]
  → RemoveFolderDialog confirm  [existing]
  → removeProject(repo.id)      [existing store action]
  → DELETE /api/repos/{id}      [existing server route]
```

Everything from `handleRemoveProject` onward is unchanged and already
tested.

## Error handling

No new error paths: the new code only opens a menu and delegates to existing
handlers. The downstream removal flow (dialog, store action, server route)
already owns its own error handling and is unchanged.

## Testing

New `crates/agentum-desktop/ui/src/components/sidebar/WorktreeList.project-context-menu.test.tsx`,
mirroring the existing `WorktreeList.*` test harness/mocks:

1. **Opens the menu:** render the list with one repo (`groupBy === 'repo'`),
   fire a `contextmenu` event on `[data-repo-header-id]`, assert a menu item
   "Remove Project" is present.
2. **Wires the action:** select "Remove Project", assert the injected
   `handleRemoveProject` mock is called with the repo.
3. **No accidental toggle:** assert the row's collapse toggle does **not**
   fire as a result of the right-click (guards the ctrl-click suppression).
4. (If cheap) **Parity:** assert the right-click menu and `⋯` menu render the
   same item labels for the same repo.

Baseline before implementing: the existing sidebar vitest suite passes
(known-unrelated failures — `@xterm/addon-ligatures` import in ~7 files — are
documented and excluded from the targeted run).

## Risks / notes

- **Virtualized list:** the menu must be rendered once at list level, not per
  row, to avoid N hidden triggers; state is hoisted accordingly.
- **`groupBy` modes:** the guard `row.repo && groupBy === 'repo'` ensures
  group/host headers are untouched.
- **No drift:** because both entry points call `renderProjectActionItems`,
  any future menu change updates both at once.
