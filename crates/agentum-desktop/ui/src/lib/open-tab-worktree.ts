// Resolve which worktree a CLI/agent-driven `agentum_browser {op:"open"}` tab
// should open in. The agentum server tags the calling agent's MCP URL with the
// directory it runs in (`?worktree=<path>`, see routes/sessions/provision.rs),
// the `/mcp` handler threads it into the tool args as `worktreeId`, and the
// desktop bridge forwards it here as `hint`. The hint is a filesystem PATH,
// while the renderer keys worktrees by `<repoId>::<path>` — so we match by the
// path portion. Without this the renderer fell back to the active worktree, and
// a page an agent opened on one project surfaced under whichever project the UI
// happened to be focused on.

/** Minimal shape this resolver needs from a known worktree. */
export interface WorktreeCandidate {
  id: string
  path?: string | null
}

function stripTrailingSlashes(path: string): string {
  return path.replace(/\/+$/, '')
}

/**
 * Resolve the target worktree id for an opened tab. Order:
 *   1. exact id match — a caller that already passes a full `<repoId>::<path>`;
 *   2. by path portion — the server tags a bare path, and a `<repoId>::<path>`
 *      hint can carry a repoId that differs from ours for the same checkout;
 *      either way the path after `::` (or the whole hint) pins the worktree;
 *   3. the UI's active worktree — no usable hint (prior behavior, no regression).
 * Returns `null` only when there's no hint AND no active worktree.
 */
export function resolveOpenTabWorktreeId(
  hint: string | null | undefined,
  candidates: ReadonlyArray<WorktreeCandidate>,
  activeWorktreeId: string | null
): string | null {
  const trimmed = typeof hint === 'string' ? hint.trim() : ''
  if (trimmed) {
    if (candidates.some((wt) => wt.id === trimmed)) {
      return trimmed
    }
    const separator = trimmed.indexOf('::')
    const hintPath = stripTrailingSlashes(
      separator === -1 ? trimmed : trimmed.slice(separator + 2)
    )
    if (hintPath) {
      const byPath = candidates.find(
        (wt) => stripTrailingSlashes(wt.path ?? '') === hintPath
      )
      if (byPath) {
        return byPath.id
      }
    }
  }
  return activeWorktreeId
}
