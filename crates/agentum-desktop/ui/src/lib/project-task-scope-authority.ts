import type { ProjectTaskScopeGuard } from './project-task-scope-guard'

const liveByRepo = new Map<string, ProjectTaskScopeGuard>()

export function publishProjectTaskScopeAuthority(guard: ProjectTaskScopeGuard): () => void {
  liveByRepo.set(guard.repoId, guard)
  return () => {
    if (sameGuard(liveByRepo.get(guard.repoId), guard)) liveByRepo.delete(guard.repoId)
  }
}

export function isLiveProjectTaskScopeAuthority(required: ProjectTaskScopeGuard): boolean {
  return sameGuard(liveByRepo.get(required.repoId), required)
}

export function clearProjectTaskScopeAuthoritiesForTests(): void {
  liveByRepo.clear()
}

export async function runGuardedProjectTaskAction<T>(
  isCurrent: () => boolean,
  action: () => Promise<T>,
  applyCurrentResult: (result: T) => void
): Promise<boolean> {
  if (!isCurrent()) return false
  const result = await action()
  if (!isCurrent()) return false
  applyCurrentResult(result)
  return true
}

function sameGuard(a: ProjectTaskScopeGuard | undefined, b: ProjectTaskScopeGuard): boolean {
  return Boolean(a && a.scopeKey === b.scopeKey && a.generation === b.generation && a.repoId === b.repoId)
}
