// #360 — the board lives inside each project: the active GitHub Project is
// resolved per repo, with the legacy single global `activeProject` kept as a
// read-only migration fallback. These two helpers are the ONLY place that
// resolution/write policy lives, so every surface (hub Tasks tab, board
// picker, project view) agrees on it.
import type { GitHubProjectSettings } from './github-project-types'

export type ActiveProjectRef = NonNullable<GitHubProjectSettings['activeProject']>

/** Resolve the active GitHub Project for a repo scope. Per-repo binding wins;
 *  the legacy global `activeProject` is the migration fallback (a board picked
 *  before #360 keeps showing everywhere until a repo binds its own). A `null`
 *  repo scope (the multi-repo board) resolves straight to the legacy value. */
export function resolveActiveProject(
  gh: Pick<GitHubProjectSettings, 'activeProject' | 'activeProjectByRepo'> | null | undefined,
  repoId: string | null | undefined
): ActiveProjectRef | null {
  if (!gh) {
    return null
  }
  if (repoId) {
    const bound = gh.activeProjectByRepo?.[repoId]
    if (bound) {
      return bound
    }
  }
  return gh.activeProject ?? null
}

/** Record a project selection. With a repo scope the write lands ONLY in the
 *  per-repo map — the legacy global stays untouched (read-only fallback), so
 *  picking a board inside project A can never change project B's board.
 *  Without a repo scope (the multi-repo board) the legacy global keeps its
 *  old persistence — that surface has no repo to key by. */
export function withActiveProjectSelection(
  prev: GitHubProjectSettings,
  repoId: string | null | undefined,
  ref: ActiveProjectRef
): GitHubProjectSettings {
  if (!repoId) {
    return { ...prev, activeProject: ref }
  }
  return {
    ...prev,
    activeProjectByRepo: { ...(prev.activeProjectByRepo ?? {}), [repoId]: ref }
  }
}

/** True when `ref` is already the resolved active project for the scope —
 *  callers use this to skip a redundant settings write. */
export function isActiveProjectFor(
  gh: Pick<GitHubProjectSettings, 'activeProject' | 'activeProjectByRepo'> | null | undefined,
  repoId: string | null | undefined,
  ref: ActiveProjectRef
): boolean {
  const active = resolveActiveProject(gh, repoId)
  return (
    active != null &&
    active.owner === ref.owner &&
    active.ownerType === ref.ownerType &&
    active.number === ref.number
  )
}
