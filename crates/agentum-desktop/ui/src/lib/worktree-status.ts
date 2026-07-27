import { detectAgentStatusFromTitle } from '@/lib/agent-status'
import { tabHasLivePty } from '@/lib/tab-has-live-pty'
import type { ServerWorktreeLiveActivity } from '@/lib/server-worktree-activity-map'
import type { TerminalTab } from '@/shared/types'

export type WorktreeStatus = 'active' | 'working' | 'permission' | 'done' | 'inactive'

const STATUS_LABELS: Record<WorktreeStatus, string> = {
  active: 'Active',
  working: 'Working',
  permission: 'Needs permission',
  done: 'Done',
  inactive: 'Inactive'
}

export function getWorktreeStatus(
  tabs: Pick<TerminalTab, 'id' | 'title'>[],
  browserTabs: { id: string }[],
  ptyIdsByTabId: Record<string, string[]>,
  runtimePaneTitlesByTabId: Record<string, Record<number, string>> = {}
): WorktreeStatus {
  // Why: liveness gates every promotion. tab.ptyId is the wake-hint sessionId
  // preserved across sleep (so wake can reattach to the same daemon history
  // dir / relay session) — it is *not* a liveness signal. ptyIdsByTabId is the
  // source of truth: sleep clears it to []; pty.spawn writes it; pty.kill
  // clears it. Reading the live-pty map keeps the dot honest after sleep,
  // crash, or any path where the wake-hint outlives the actual PTY.
  const liveTabs = tabs.filter((tab) => tabHasLivePty(ptyIdsByTabId, tab.id))

  // Why: a split-pane tab can host multiple concurrent agents, but `tab.title`
  // only reflects the most-recently-focused pane (see onActivePaneChange in
  // use-terminal-pane-lifecycle.ts). Reading just `tab.title` causes the
  // sidebar spinner to follow the focused pane instead of the aggregate tab
  // state — e.g. clicking an idle Claude pane while Codex is still working in
  // another pane would collapse the spinner to solid green. Consult per-pane
  // titles first (same pattern as countWorkingAgentsForTab) and only fall back
  // to `tab.title` for tabs that have no mounted panes yet.
  const hasStatus = (status: 'permission' | 'working'): boolean =>
    liveTabs.some((tab) => tabHasStatus(tab, runtimePaneTitlesByTabId, status))

  if (hasStatus('permission')) {
    return 'permission'
  }
  if (hasStatus('working')) {
    return 'working'
  }
  if (liveTabs.length > 0 || browserTabs.length > 0) {
    // Why: browser-only worktrees are still active from the user's point of
    // view even when they have no PTY-backed terminal. The sidebar filter
    // already treats them as active, so every navigation surface must reuse
    // that rule instead of showing a misleading inactive dot.
    return 'active'
  }
  return 'inactive'
}

function tabHasStatus(
  tab: Pick<TerminalTab, 'id' | 'title'>,
  runtimePaneTitlesByTabId: Record<string, Record<number, string>>,
  status: 'permission' | 'working'
): boolean {
  const paneTitles = runtimePaneTitlesByTabId[tab.id]
  if (paneTitles && Object.keys(paneTitles).length > 0) {
    for (const title of Object.values(paneTitles)) {
      if (detectAgentStatusFromTitle(title) === status) {
        return true
      }
    }
    return false
  }
  return detectAgentStatusFromTitle(tab.title) === status
}

export function getWorktreeStatusLabel(status: WorktreeStatus): string {
  return STATUS_LABELS[status]
}

/**
 * Apply the WorktreeCard priority overlay (permission > working > done >
 * heuristic) on top of the title-heuristic base. Live PTY liveness still gates
 * title-derived working/permission, but explicit agent rows are allowed to
 * promote the dot: if the sidebar shows a running/completed/blocking inline
 * agent row, the worktree status must agree with that visible row. Sleep
 * cleanup owns removing stale retained rows; once they are gone, no promotion
 * occurs.
 *
 * Argument semantics (sourced by the WorktreeCard caller from the store):
 * - `tabs`, `browserTabs`: the worktree's terminal and browser tabs.
 * - `ptyIdsByTabId`: live-PTY map narrowed to this worktree (the liveness
 *   gate; see tabHasLivePty).
 * - `runtimePaneTitlesByTabId`: per-tab pane title map narrowed to this
 *   worktree (used by the title-heuristic for split-pane spinners).
 * - `hasPermission`: any fresh hook entry in {blocked, waiting} for a tab in
 *   this worktree.
 * - `hasLiveWorking`: any fresh hook entry in {working} for a tab in this
 *   worktree.
 * - `hasLiveDone`: any fresh hook entry in {done} for a tab in this worktree.
 * - `hasRetainedDone`: any retained-agent snapshot scoped to this worktreeId.
 * - `isAlive`: a backing tmux session for this worktree is alive on the host
 *   (server status 'running'), even with no pane mounted. Server-authoritative,
 *   so it survives an app relaunch where the renderer-local heuristic is cold.
 * - `liveActivity`: the watchdog's live activity verdict for the backing
 *   session ('working' | 'awaiting' | 'idle'), also pane-independent.
 */
export function resolveWorktreeStatus(args: {
  tabs: Pick<TerminalTab, 'id' | 'title'>[]
  browserTabs: { id: string }[]
  ptyIdsByTabId: Record<string, string[]>
  runtimePaneTitlesByTabId?: Record<string, Record<number, string>>
  hasPermission: boolean
  hasLiveWorking: boolean
  hasLiveDone: boolean
  hasRetainedDone: boolean
  isAlive?: boolean
  liveActivity?: ServerWorktreeLiveActivity
}): WorktreeStatus {
  const heuristic = getWorktreeStatus(
    args.tabs,
    args.browserTabs,
    args.ptyIdsByTabId,
    args.runtimePaneTitlesByTabId ?? {}
  )
  // Why: the watchdog's "blocked on the user" verdict is pane-independent and
  // outranks everything else, mirroring args.hasPermission — after relaunch it's
  // the only "needs you" signal for an unmounted worktree.
  if (args.hasPermission || args.liveActivity === 'awaiting') {
    return 'permission'
  }
  // Why: heuristic 'permission' must outrank heuristic 'working' (a tab can
  // be "working" while another pane in the same tab is blocked on permission;
  // the user-actionable signal wins). Both this branch and the args.hasPermission
  // branch above return 'permission' — they're separated only because
  // args.hasPermission must also outrank heuristic 'working' below.
  if (heuristic === 'permission') {
    return 'permission'
  }
  // Why: restored-but-unfocused cards may have the startup hook snapshot before
  // their terminal pane mounts and repopulates runtimePaneTitlesByTabId.
  // Trust the fresh explicit working row — or the watchdog's pane-independent
  // 'working' verdict — so those cards stay yellow on restart.
  if (args.hasLiveWorking || args.liveActivity === 'working' || heuristic === 'working') {
    return 'working'
  }
  if (args.hasLiveDone || args.hasRetainedDone) {
    return 'done'
  }
  // Why: a worktree whose backing tmux session is still alive on the host is
  // "running" even when no pane is mounted and the renderer-local heuristic has
  // gone cold after relaunch — promote it out of the misleading inactive dot.
  // Lowest priority: a live working/permission signal above already won.
  if (args.isAlive && heuristic === 'inactive') {
    return 'active'
  }
  return heuristic
}
