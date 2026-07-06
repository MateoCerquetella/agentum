// Pure row model for the sidebar Projects group (spec 009 F1, D-A1).
// Extracted from SidebarProjectsNav so the active-row logic is vitest-testable
// without jsdom (the UI package convention: interactive logic lives in pure
// modules; components are only exercised by `vite build`).

/** The minimal repo shape the Projects group needs — structurally compatible
 *  with the store's `Repo` without importing it. */
export type ProjectsNavRepo = {
  id: string
  displayName: string
}

export type ProjectsNavRow = {
  id: string
  label: string
  active: boolean
}

/**
 * One row per repo, in store order. A row is active only when the Project Hub
 * is the current view AND it is showing this repo — `activeRepoId` alone is
 * not enough (it also tracks the active workspace's repo on other views, and
 * highlighting a project row while the user is on, say, a terminal would read
 * as a wrong location indicator).
 */
export function projectsNavRows(
  repos: readonly ProjectsNavRepo[],
  activeView: string,
  activeRepoId: string | null | undefined
): ProjectsNavRow[] {
  return repos.map((repo) => ({
    id: repo.id,
    label: repo.displayName,
    active: activeView === 'project' && activeRepoId === repo.id
  }))
}
