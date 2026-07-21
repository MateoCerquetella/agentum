import type { BoardProjectResolution } from '@/lib/board-project-resolution'

/**
 * A project hub always stays on the GitHub Project surface when its repo
 * resolution changes. Bound repos render their board; unbound repos render
 * that repo's honest picker/empty state. Falling through to Items would hide
 * the scoped resolution and make a repo switch look like the previous board
 * leaked into the new project.
 */
export function embeddedGithubModeForResolution(
  _resolution: Exclude<BoardProjectResolution, { source: 'pending' }>
): 'project' {
  return 'project'
}
