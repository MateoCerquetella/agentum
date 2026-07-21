import {
  GLOBAL_TASK_PROJECT_SCOPE,
  type TaskLinearContext,
  type TaskResumeState
} from './types'

export function taskProjectScopeKey(repoId: string | null): string {
  return repoId ?? GLOBAL_TASK_PROJECT_SCOPE
}

export function resolveLinearContextForRepo(
  resumeState: TaskResumeState | undefined,
  repoId: string | null
): TaskLinearContext | undefined {
  return resumeState?.linearContextByRepo?.[taskProjectScopeKey(repoId)]
}
