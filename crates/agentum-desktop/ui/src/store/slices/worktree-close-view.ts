import type { AppState } from '../types'

type ActiveView = AppState['activeView']

/**
 * The view to land on after the ACTIVE worktree is closed / deselected.
 *
 * Why: closing the active worktree nulls `activeWorktreeId` but historically left
 * `activeView` on `'terminal'`, so Mission Control rendered through the
 * `terminal && !activeWorktreeId` fallback (App.tsx). Unlike the real `'activity'`
 * view, that fallback is NOT in `RIGHT_SIDEBAR_SUPPRESSED_VIEWS`, so the right
 * sidebar stays mounted and squeezes the dashboard's fixed `grid-cols-3` — the
 * "squished / wrong width" bug. Landing on `'activity'` puts Mission Control in
 * the right-sidebar-suppressed layout.
 *
 * Only redirect FROM the workspace (`'terminal'`) view — nulling the active
 * worktree while the user is on settings/tasks/projects/etc. must not yank them
 * away. Selecting a worktree restores `'terminal'` (repos.ts), so there is no
 * sticky-`'activity'` hazard for a deselect-then-reselect sequence.
 */
export function viewAfterWorktreeClose(
  removedActiveWorktree: boolean,
  currentView: ActiveView
): ActiveView {
  return removedActiveWorktree && currentView === 'terminal' ? 'activity' : currentView
}
