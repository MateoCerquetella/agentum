import type { GitHubProjectSettings } from './github-project-types'
import {
  GLOBAL_TASK_PROJECT_SCOPE,
  type TaskLinearContext,
  type TaskResumeState
} from './types'

export function taskProjectScopeKey(repoId: string | null): string {
  return repoId ?? GLOBAL_TASK_PROJECT_SCOPE
}

export function resolveActiveProjectForRepo(
  settings: GitHubProjectSettings | undefined,
  repoId: string | null
): NonNullable<GitHubProjectSettings['activeProject']> | null {
  return settings?.activeProjectByRepo?.[taskProjectScopeKey(repoId)] ?? null
}

export function resolveLinearContextForRepo(
  resumeState: TaskResumeState | undefined,
  repoId: string | null
): TaskLinearContext | undefined {
  return resumeState?.linearContextByRepo?.[taskProjectScopeKey(repoId)]
}
