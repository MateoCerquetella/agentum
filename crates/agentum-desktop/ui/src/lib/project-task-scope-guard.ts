import type { LinearIssue } from '@/shared/types'
import type { ProjectTrackerConfig } from '@/shared/project-tracker-config'
import type { ProjectTaskScope } from './project-task-scope'

export type ProjectTaskScopeGuard = Readonly<{ scopeKey: string; generation: number; repoId: string }>

export function captureProjectTaskScopeGuard(scope: ProjectTaskScope): ProjectTaskScopeGuard | null {
  return scope.status === 'bound' ? Object.freeze({ scopeKey: scope.scopeKey, generation: scope.generation, repoId: scope.repoId }) : null
}

export function isProjectTaskScopeGuardCurrent(guard: ProjectTaskScopeGuard, live: ProjectTaskScope | null | undefined): boolean {
  return live?.status === 'bound' && live.scopeKey === guard.scopeKey && live.generation === guard.generation && live.repoId === guard.repoId
}

/** Revalidate a captured task scope against the project's canonical tracker.
 * Repository-scoped GitHub trackers deliberately use the `repository` marker
 * in place of a Project id, so they must not be treated as a missing binding. */
export function projectTaskScopeGuardMatchesTracker(
  guard: ProjectTaskScopeGuard,
  tracker: ProjectTrackerConfig | null | undefined
): boolean {
  if (!tracker || tracker.repoId !== guard.repoId) return false
  try {
    const key = JSON.parse(guard.scopeKey) as unknown
    if (!Array.isArray(key) || key[0] !== guard.repoId) return false
    if (key[1] === 'linear') {
      return (
        tracker.provider === 'linear' &&
        tracker.linear?.workspaceId === key[2] &&
        tracker.linear.scope?.kind === 'project' &&
        tracker.linear.scope.id === key[3]
      )
    }
    if (key[1] !== 'github') return false
    if (
      tracker.provider !== 'github' ||
      tracker.github?.repositorySlug !== key[2]
    ) {
      return false
    }
    return key[3] === 'repository'
      ? tracker.github.projectBinding === undefined
      : tracker.github.projectBinding?.projectId === key[3]
  } catch {
    return false
  }
}

export function linearIssueMatchesScope(issue: LinearIssue, scope: Extract<ProjectTaskScope, { status: 'bound'; provider: 'linear' }>): boolean {
  return issue.workspaceId === scope.workspaceId && issue.project?.id === scope.projectId && issue.project.workspaceId === scope.workspaceId && scope.teamIds.includes(issue.team.id)
}

export function linearActionMatchesScope(input: { workspaceId: string; projectId: string; teamId: string }, scope: Extract<ProjectTaskScope, { status: 'bound'; provider: 'linear' }>): boolean {
  return input.workspaceId === scope.workspaceId && input.projectId === scope.projectId && scope.teamIds.includes(input.teamId)
}
