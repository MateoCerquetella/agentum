// Spec 016 D2: where a bare "open the board" gesture lands now that the
// sidebar Board entry is gone. Repo-first: the caller's preferred repo, else
// the globally active repo — each only if it still resolves to a LIVE git repo
// (repos can be removed under a stale id) — else the Projects page. D2
// enumerates exactly these two tiers: no first-git-repo fallback. The pure
// resolver is vitest-covered; `openBoardSurface` is the thin store dispatcher
// call sites use so every bare opener stays one line.
import { isGitRepoKind } from '@/shared/repo-kind'
import type { TaskProvider } from '@/shared/task-providers'
import type { Repo } from '@/shared/types'
import { useAppStore } from '@/store'

export function resolveBoardRoute(input: {
  repos: ReadonlyArray<Pick<Repo, 'id' | 'kind'>>
  /** e.g. ChatPage's filed-card repo. */
  preferredRepoId?: string | null
  /** null on cold start (activeRepoId is reset, not persisted). */
  activeRepoId: string | null
}): { kind: 'hub'; repoId: string } | { kind: 'projects' } {
  const liveGitRepoId = (id: string | null | undefined): string | null => {
    if (!id) {
      return null
    }
    const repo = input.repos.find((r) => r.id === id)
    return repo && isGitRepoKind(repo) ? repo.id : null
  }
  const preferred = liveGitRepoId(input.preferredRepoId)
  if (preferred) {
    return { kind: 'hub', repoId: preferred }
  }
  const active = liveGitRepoId(input.activeRepoId)
  if (active) {
    return { kind: 'hub', repoId: active }
  }
  return { kind: 'projects' }
}

/** Dispatch a bare board open: hub Tasks tab when a repo resolves, Projects
 *  page otherwise. `taskSource` seeds the hub's embedded TaskPage tab (a
 *  Linear filed-card must land on the Linear tab, as `openTaskPage` did) —
 *  this is NOT detail-payload threading; detail openers keep calling
 *  `openTaskPage({...})` directly. */
export function openBoardSurface(seed?: {
  preferredRepoId?: string | null
  taskSource?: TaskProvider
}): void {
  const s = useAppStore.getState()
  const route = resolveBoardRoute({
    repos: s.repos,
    preferredRepoId: seed?.preferredRepoId,
    activeRepoId: s.activeRepoId
  })
  if (route.kind === 'hub') {
    s.openProjectHub(
      route.repoId,
      'tasks',
      seed?.taskSource ? { taskSource: seed.taskSource } : undefined
    )
    return
  }
  s.openProjectsPage()
}
