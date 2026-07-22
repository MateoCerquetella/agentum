import type { LinearIssue } from '@/shared/types'
import type { ProjectTaskScope } from './project-task-scope'

export type ProjectTaskScopeGuard = Readonly<{ scopeKey: string; generation: number; repoId: string }>

export function captureProjectTaskScopeGuard(scope: ProjectTaskScope): ProjectTaskScopeGuard | null {
  return scope.status === 'bound' ? Object.freeze({ scopeKey: scope.scopeKey, generation: scope.generation, repoId: scope.repoId }) : null
}

export function isProjectTaskScopeGuardCurrent(guard: ProjectTaskScopeGuard, live: ProjectTaskScope | null | undefined): boolean {
  return live?.status === 'bound' && live.scopeKey === guard.scopeKey && live.generation === guard.generation && live.repoId === guard.repoId
}

export function linearIssueMatchesScope(issue: LinearIssue, scope: Extract<ProjectTaskScope, { status: 'bound'; provider: 'linear' }>): boolean {
  return issue.workspaceId === scope.workspaceId && issue.project?.id === scope.projectId && issue.project.workspaceId === scope.workspaceId && scope.teamIds.includes(issue.team.id)
}

export function linearActionMatchesScope(input: { workspaceId: string; projectId: string; teamId: string }, scope: Extract<ProjectTaskScope, { status: 'bound'; provider: 'linear' }>): boolean {
  return input.workspaceId === scope.workspaceId && input.projectId === scope.projectId && scope.teamIds.includes(input.teamId)
}
