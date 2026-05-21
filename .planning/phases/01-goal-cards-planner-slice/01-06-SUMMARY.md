---
phase: 01-goal-cards-planner-slice
plan: "01-06"
subsystem: dashboard
tags: [dashboard, svelte, ui, goal-composer, parent-cue, css, fleet-board]
dependency-graph:
  requires:
    - "01-03-SUMMARY.md (board column rules + gate rejection shape)"
    - "01-04-SUMMARY.md (POST /api/board/goals endpoint)"
  provides:
    - "GoalComposer.svelte (persistent input bar above the board)"
    - "submitGoal() store action in board.ts"
    - "api.createGoal() in api.ts"
    - "parent-cue chip on child cards (Ticket.svelte)"
    - ".lbl.goal + .lbl.parent-cue + .pill.filter CSS rules"
    - "Column filter pill narrowing cards to a goal's children"
  affects:
    - "dashboard/src/routes/board/+page.svelte (GoalComposer + filter logic)"
    - "dashboard/src/lib/themes/_design.css (new chip + pill rules)"
    - "dashboard/src/lib/api.ts (TicketLbl union + createGoal)"
    - "dashboard/src/lib/components/dashboard/Ticket.svelte (tk-foot chips)"
tech-stack:
  added:
    - "GoalComposer.svelte: Svelte 5 component with $state/$derived.by() + auto-grow textarea"
  patterns:
    - "boardIsEmpty derived from $board.data.column_order (all columns empty check)"
    - "boardFilter keyed by laneColKey (lane::colKey compound) for lane-scoped filtering"
    - "onParentCueClick toggles filter + opens parent goal in edit dialog simultaneously"
    - "colItems = colFilter ? items.filter(parent_goal_id|id match) : all items"
key-files:
  created:
    - "dashboard/src/lib/components/GoalComposer.svelte"
  modified:
    - "dashboard/src/lib/api.ts"
    - "dashboard/src/lib/stores/board.ts"
    - "dashboard/src/lib/components/dashboard/Ticket.svelte"
    - "dashboard/src/lib/themes/_design.css"
    - "dashboard/src/routes/board/+page.svelte"
decisions:
  - "boardFilter uses compound laneColKey (laneKey:colKey) to keep filter lane-scoped — the fleet board shows multiple lanes simultaneously and a board-wide filter would be confusing"
  - "colFilter also shows the goal card itself (it.id === colFilter.goalId) so the user can see the parent alongside its children"
  - "TicketLbl union extended to include 'goal' — the server sends lbl=goal on planner-spawned cards; this is a type-level truth"
  - "onclick|stopPropagation (Svelte 4 syntax) replaced with onclick={(e)=>{e.stopPropagation();...}} for Svelte 5 compatibility"
metrics:
  duration: "~60 minutes"
  completed: "2026-05-21T14:22:00Z"
  tasks-completed: 2
  files-changed: 5
---

# Phase 01 Plan 06: Dashboard goal slice — GoalComposer, parent-cue chip, filter pill Summary

GoalComposer.svelte persistent input bar with Svelte 5 runes, submitGoal store action, parent-cue ↳ AG-{id} chip on child cards, .lbl.goal coral styling, and lane-scoped column filter pill on /board.

## Tasks Completed

| # | Task | Commit | Key Files |
|---|------|--------|-----------|
| 1 | api.createGoal + submitGoal + GoalComposer.svelte | e1d12c0 | api.ts, board.ts, GoalComposer.svelte |
| 2 | Route + CSS — board page wiring, parent-cue, filter pill, CSS | 4e1c514 | +page.svelte, Ticket.svelte, _design.css, api.ts |

## What Was Built

### Task 1: api + store + component

**`dashboard/src/lib/api.ts`** — added `parent_goal_id?: number | null` to `BoardItem`; extended `TicketLbl` union with `'goal'`; added `api.createGoal(text, opts?)` calling `POST /api/board/goals`.

**`dashboard/src/lib/stores/board.ts`** — added `submitGoal(text, opts?)` which delegates to `api.createGoal`. On success the new card arrives via the `/api/events` WS; no manual store write needed.

**`dashboard/src/lib/components/GoalComposer.svelte`** — persistent input bar above the board:
- 56px compact / 220px empty-state heights per UI-SPEC
- Empty-state mode: `boardIsEmpty = $derived.by(() => data?.column_order.every(col => columns[col]?.length === 0))` — shows eyebrow + heading + body copy when board has no cards
- Keyboard: Cmd/Ctrl+Enter always submits; plain Enter submits only when single-line (`!text.includes('\n')`)
- Auto-grow: textarea expands up to 160px via `scrollHeight`
- Error handling: 400 gate rejection → "Your todo column needs: {labels}. Add them in Settings → Column rules." (verbatim); 5xx → "Couldn't reach the planner. Check the daemon and try again." (verbatim)
- Error block: `role="alert" aria-live="polite"`, 4px `--crash` left border, dismiss × button
- Mobile: textarea above button at ≤720px, font-size 16px on textarea (prevents iOS zoom)

### Task 2: route + CSS

**`dashboard/src/routes/board/+page.svelte`** — wired GoalComposer between toolbar and board:
- `boardFilter: Record<string, { goalId: number } | null>` state keyed by `${laneKey}:${colKey}`
- `onParentCueClick(laneColKey, parentGoalId)`: opens parent goal in BoardItemDialog + toggles filter
- `onColKeyDown(laneColKey, e)`: Esc clears the column filter
- `colItems` computed per column: filtered to `parent_goal_id === goalId || id === goalId` when filter active
- Filter pill in `.col-h`: `<span class="pill filter">Filter: AG-{goalId} ↓ <button class="dismiss">×</button></span>`
- All `<Ticket>` renders now pass `onParentCueClick` callback

**`dashboard/src/lib/components/dashboard/Ticket.svelte`** — added:
- `isChildCard = $derived(tk.parent_goal_id != null && tk.lbl !== 'goal' && onParentCueClick != null)`
- `.tk-foot` block for child cards: `<button class="lbl parent-cue">↳ AG-{id}</button>` with `onclick={(e)=>{e.stopPropagation();...}}`
- `.tk-foot` block for goal cards: `<span class="lbl goal">GOAL</span>`

**`dashboard/src/lib/themes/_design.css`** — added:
- `.ticket .tk-foot .lbl.goal { color: var(--cta); border-color: rgba(243,100,88,0.4); }`
- `.ticket .tk-foot .lbl.parent-cue` with cursor:pointer + `--link` hover/focus
- `.col-h .pill.filter` with `border-color: var(--link)` + dismiss button hover
- `@media (max-width: 720px)` rule: parent-cue min-height 28px tap target

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Svelte 5 incompatible onclick|stopPropagation modifier**
- **Found during:** Task 2 (svelte-check error)
- **Issue:** `onclick|stopPropagation` is Svelte 4 event modifier syntax; Svelte 5 does not support `|` modifiers
- **Fix:** Replaced with `onclick={(e) => { e.stopPropagation(); onParentCueClick?.(tk.parent_goal_id!); }}`
- **Files modified:** `dashboard/src/lib/components/dashboard/Ticket.svelte`
- **Commit:** 4e1c514

**2. [Rule 2 - Missing critical type] 'goal' missing from TicketLbl union**
- **Found during:** Task 2 (svelte-check type error — `TicketLbl` had no overlap with `'goal'`)
- **Issue:** `tk.lbl !== 'goal'` and `tk.lbl === 'goal'` comparisons were flagged as always-true/always-false because `TicketLbl = 'bug' | 'feat' | 'chore' | 'spike'` did not include `'goal'`. Server sends `lbl=goal` on planner-spawned cards (D-02).
- **Fix:** Added `'goal'` to `TicketLbl` union in `api.ts`
- **Files modified:** `dashboard/src/lib/api.ts`
- **Commit:** 4e1c514

**3. [Rule 3 - Infrastructure] Worktree dashboard build needed fresh .svelte-kit**
- **Found during:** Task 2 (`npm run build --prefix dashboard` from worktree)
- **Issue:** The `.svelte-kit` symlink pointing to the main project contained absolute paths that didn't resolve correctly in the worktree context
- **Fix:** Removed symlink, ran `vite build` directly from worktree's dashboard dir — this generated a fresh `.svelte-kit/` and `build/` within the worktree
- **Impact:** Build exits 0; `dashboard/build/` exists with correct SPA bundle

## Known Stubs

None. All data paths are wired: GoalComposer → `submitGoal` → `api.createGoal` → `POST /api/board/goals`. The returned card lands via the `/api/events` WS which the existing `events.ts` bridge handles.

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: user-input | GoalComposer.svelte | Goal text POSTed as `title` — server must sanitize; client does no sanitization beyond trim (correct) |

## Verification

- `svelte-check`: 373 files, **0 errors**, 1 warning (a11y non-interactive div with keyboard handler — intentional UX for Esc-to-clear-filter)
- `npm run build` (vite): exits 0, writes `dashboard/build/`
- `cargo build --release -p agentum-server`: rebakes the embedded SPA (in progress at summary write time)

## Self-Check: PASSED

- GoalComposer.svelte created: FOUND at `dashboard/src/lib/components/GoalComposer.svelte`
- api.createGoal added: FOUND in `dashboard/src/lib/api.ts`
- submitGoal added: FOUND in `dashboard/src/lib/stores/board.ts`
- parent-cue chip in Ticket.svelte: FOUND at lines 113-136
- .lbl.goal CSS: FOUND in `dashboard/src/lib/themes/_design.css`
- Task 1 commit e1d12c0: FOUND
- Task 2 commit 4e1c514: FOUND
