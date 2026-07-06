// Pure row model for the Projects page (#274). Extracted so the card content
// logic is vitest-testable without jsdom (the UI package convention).

/** The minimal repo shape the page needs — structurally compatible with the
 *  store's `Repo` without importing it. */
export type ProjectsPageRepo = {
  id: string
  displayName: string
  path: string
  /** SSH target ID for remote repos. null/undefined = local. */
  connectionId?: string | null
}

export type ProjectsPageRow = {
  id: string
  name: string
  path: string
  remote: boolean
  /** Registered workspaces (worktrees) for the repo — 0 when none/unknown. */
  worktrees: number
}

/**
 * One card per repo, in store order (the order the user added them — stable
 * and predictable, matching the old sidebar group). Remote repos are flagged
 * so the card can show an SSH badge instead of pretending the path is local.
 */
export function projectsPageRows(
  repos: readonly ProjectsPageRepo[],
  worktreeCounts: Readonly<Record<string, number>>
): ProjectsPageRow[] {
  return repos.map((repo) => ({
    id: repo.id,
    name: repo.displayName,
    path: repo.path,
    remote: repo.connectionId != null,
    worktrees: worktreeCounts[repo.id] ?? 0
  }))
}
