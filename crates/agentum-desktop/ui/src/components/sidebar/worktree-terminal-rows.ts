import { tabHasLivePty } from '@/lib/tab-has-live-pty'
import type { TerminalTab } from '@/shared/types'

/** A plain-terminal entry rendered under a worktree card, beside the agent
 *  rows. Plain = a terminal tab that is NOT already surfaced as an agent row. */
export type WorktreeTerminalRow = {
  tabId: string
  title: string
  /** Whether the tab currently has a live PTY. Used only for a subtle visual
   *  hint — NOT a visibility gate: a freshly-created terminal (ptyId still
   *  null while its PTY spawns) must list immediately, which was the bug. */
  hasLivePty: boolean
}

/** Display label for a terminal row: an explicit custom name wins; otherwise
 *  the live title (OSC-updated), falling back to the stable default label. */
function terminalRowTitle(tab: TerminalTab): string {
  const custom = tab.customTitle?.trim()
  if (custom) {
    return custom
  }
  const title = tab.title?.trim()
  if (title) {
    return title
  }
  return tab.defaultTitle?.trim() || 'Terminal'
}

/**
 * Derive the plain-terminal rows for a worktree card. Plain terminals are tabs
 * with no agent running — they would otherwise never appear in the sidebar
 * (the inline list only emitted agent rows), so creating a new terminal left
 * it invisible there.
 *
 * Deliberately NOT gated on a live PTY: a brand-new terminal carries
 * `ptyId: null` until the main TerminalPane spawns its PTY, and must still be
 * listed. (Worktree-card *visibility* under "hide sleeping workspaces" is a
 * separate, live-PTY-gated concern handled in visible-worktrees.ts.)
 */
export function buildWorktreeTerminalRows(args: {
  tabs: TerminalTab[]
  /** Tab ids already represented by an agent row — excluded to avoid dupes. */
  agentTabIds: ReadonlySet<string>
  ptyIdsByTabId?: Record<string, string[]>
}): WorktreeTerminalRow[] {
  const ptyIdsByTabId = args.ptyIdsByTabId ?? {}
  return args.tabs
    .filter((tab) => !args.agentTabIds.has(tab.id))
    .slice()
    .sort((a, b) => a.sortOrder - b.sortOrder || a.createdAt - b.createdAt)
    .map((tab) => ({
      tabId: tab.id,
      title: terminalRowTitle(tab),
      hasLivePty: tabHasLivePty(ptyIdsByTabId, tab.id)
    }))
}
