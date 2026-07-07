// Pure row model for the Projects page (#274, host grouping + search #279).
// Extracted so the card content, name filter, and host grouping are all
// vitest-testable without jsdom (the UI package convention).

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
  /** Host label the row groups under — 'Local' for local repos, otherwise the
   *  SSH target's user-facing label (falling back to 'Remote host'). */
  host: string
  /** Registered workspaces (worktrees) for the repo — 0 when none/unknown. */
  worktrees: number
}

export type ProjectsHostGroup = {
  host: string
  rows: ProjectsPageRow[]
}

/** Group label for local (non-SSH) repos — also the group we float to the top. */
export const LOCAL_HOST_LABEL = 'Local'
/** Shown when a remote repo's connection has no resolved label yet (matches the
 *  sidebar's WorktreeList fallback). */
export const REMOTE_HOST_FALLBACK = 'Remote host'

/**
 * One card per repo, in store order (the order the user added them — stable
 * and predictable, matching the old sidebar group). Remote repos are flagged
 * so the card can show an SSH badge instead of pretending the path is local,
 * and carry a `host` label resolved from the connection's SSH target label.
 */
export function projectsPageRows(
  repos: readonly ProjectsPageRepo[],
  worktreeCounts: Readonly<Record<string, number>>,
  sshLabels?: ReadonlyMap<string, string>
): ProjectsPageRow[] {
  return repos.map((repo) => {
    const remote = repo.connectionId != null
    return {
      id: repo.id,
      name: repo.displayName,
      path: repo.path,
      remote,
      host: remote
        ? (sshLabels?.get(repo.connectionId as string) ?? REMOTE_HOST_FALLBACK)
        : LOCAL_HOST_LABEL,
      worktrees: worktreeCounts[repo.id] ?? 0
    }
  })
}

/**
 * Narrow rows to those whose project name contains `query` as a
 * case-insensitive substring. A blank/whitespace-only query is a no-op (returns
 * a copy of all rows). No fuzzy matching, no path/session-name search (#279).
 */
export function filterProjectsRows(
  rows: readonly ProjectsPageRow[],
  query: string
): ProjectsPageRow[] {
  const needle = query.trim().toLowerCase()
  if (needle === '') return [...rows]
  return rows.filter((row) => row.name.toLowerCase().includes(needle))
}

/**
 * Bucket rows by their host label into ordered sections — 'Local' first, then
 * each remote host in first-seen (store) order. A group only exists when it has
 * at least one row, so a host with no matches after filtering never renders a
 * header (the page relies on this to hide empty sections, #279).
 */
export function groupProjectsRowsByHost(
  rows: readonly ProjectsPageRow[]
): ProjectsHostGroup[] {
  const order: string[] = []
  const byHost = new Map<string, ProjectsPageRow[]>()
  for (const row of rows) {
    let bucket = byHost.get(row.host)
    if (!bucket) {
      bucket = []
      byHost.set(row.host, bucket)
      order.push(row.host)
    }
    bucket.push(row)
  }
  // Float Local to the top without disturbing the store order of the remotes.
  const ordered = byHost.has(LOCAL_HOST_LABEL)
    ? [LOCAL_HOST_LABEL, ...order.filter((host) => host !== LOCAL_HOST_LABEL)]
    : order
  return ordered.map((host) => ({ host, rows: byHost.get(host)! }))
}
