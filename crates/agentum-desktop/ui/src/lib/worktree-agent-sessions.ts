import type { Session } from '@/runtime/agentum-server-client'
import { splitWorktreeIdForFilesystem } from '@/shared/worktree-id'

/** A running agent session on a worktree that the "Send to an agent" menu can target.
 *  Sourced from the SERVER's session list, so it includes tmux/MCP/board-spawned agents
 *  — not just ones the desktop has open as terminal tabs (the gap that made the menu
 *  show "No running agents here" even with an agent running on the worktree). */
export type WorktreeAgentSession = {
  sessionId: string
  label: string
  tool: string
}

// Plain shells aren't agents — "Send to an agent" shouldn't list them.
const SHELL_TOOLS = new Set(['terminal', 'bash', 'shell', 'zsh', 'fish', 'sh'])

/** Filter the server session list to the running AGENT sessions on `worktreeId`.
 *
 *  - worktree match: the UI worktree id is `<repoId>::<path>` (folder projects append a
 *    `::workspace:<uuid>` suffix); reduce it to the bare path and match a session whose
 *    `worktree_path`/`workdir` is that path (or a subdirectory). Mirrors the server's own
 *    `@worktree:<id>` resolver and `cdp_browser::canonical_worktree_key`.
 *  - alive: has a tmux pane (`tmux_target`) and isn't stopped/crashed.
 *  - agent: not a plain shell, so the menu lists agents, not terminals.
 *
 *  Pure: the caller fetches `listSessions()` and passes the result in.
 */
export function deriveWorktreeAgentSessions(
  sessions: readonly Session[],
  worktreeId: string
): WorktreeAgentSession[] {
  const worktreePath = normalizePath(
    splitWorktreeIdForFilesystem(worktreeId)?.worktreePath ?? worktreeId
  )
  if (!worktreePath) {
    return []
  }
  return sessions
    .filter(
      (s) =>
        (s.tmux_target ?? null) !== null &&
        s.status !== 'stopped' &&
        s.status !== 'crashed' &&
        !SHELL_TOOLS.has(s.tool.toLowerCase()) &&
        sessionOnWorktree(s, worktreePath)
    )
    .map((s) => ({ sessionId: s.id, label: agentLabel(s), tool: s.tool }))
}

function sessionOnWorktree(s: Session, worktreePath: string): boolean {
  const wp = normalizePath(s.worktree_path ?? '')
  const wd = normalizePath(s.workdir)
  return (
    wp === worktreePath ||
    wd === worktreePath ||
    (wp !== '' && wp.startsWith(`${worktreePath}/`)) ||
    wd.startsWith(`${worktreePath}/`)
  )
}

function normalizePath(p: string): string {
  return p.trim().replace(/[\\/]+$/g, '')
}

function agentLabel(s: Session): string {
  const tool = s.tool.length > 0 ? s.tool.charAt(0).toUpperCase() + s.tool.slice(1) : 'Agent'
  const name = s.name?.trim()
  return name && name.length > 0 && name !== s.tool ? `${tool} · ${name}` : tool
}
