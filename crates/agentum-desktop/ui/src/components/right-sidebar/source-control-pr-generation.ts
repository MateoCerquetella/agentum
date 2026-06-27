import type { PullRequestFieldRevisions } from './useCreatePullRequestDialogFields'

export type PullRequestGenerationFields = {
  base: string
  title: string
  body: string
  draft: boolean
}

export type PullRequestGenerationContext = {
  worktreeId: string | null
  worktreePath: string
  connectionId?: string
  requestId: number
  repoId: string
  branch: string
}

export type PullRequestGenerationStatus = 'idle' | 'running' | 'canceled' | 'failed' | 'succeeded'

export type PullRequestGenerationRecord = {
  context: PullRequestGenerationContext
  seed: PullRequestGenerationFields
  seedFieldRevisions: PullRequestFieldRevisions
  status: PullRequestGenerationStatus
  result: PullRequestGenerationFields | null
  error: string | null
  hydrated: boolean
}

export type PullRequestGenerationRecords = Record<string, PullRequestGenerationRecord>

export function getPullRequestGenerationWorktreeKey(
  worktreeId: string | null | undefined,
  worktreePath: string | null | undefined
): string | null {
  if (worktreeId) {
    return worktreeId
  }
  return worktreePath?.trim() ? worktreePath : null
}

export function getPullRequestGenerationRecordKey({
  worktreeId,
  worktreePath,
  repoId,
  branch
}: {
  worktreeId: string | null | undefined
  worktreePath: string | null | undefined
  repoId: string | null | undefined
  branch: string | null | undefined
}): string | null {
  const worktreeKey = getPullRequestGenerationWorktreeKey(worktreeId, worktreePath)
  if (!worktreeKey || !repoId || !branch) {
    return null
  }
  return JSON.stringify([repoId, worktreeKey, branch])
}

export function arePullRequestGenerationFieldsEqual(
  left: PullRequestGenerationFields,
  right: PullRequestGenerationFields
): boolean {
  return (
    left.base === right.base &&
    left.title === right.title &&
    left.body === right.body &&
    left.draft === right.draft
  )
}

export function shouldApplyPullRequestGenerationResult({
  record,
  requestId
}: {
  record: PullRequestGenerationRecord | null | undefined
  requestId: number
}): boolean {
  return record?.context.requestId === requestId && record.status === 'running'
}

export function shouldHydratePullRequestGenerationResult({
  record
}: {
  record: PullRequestGenerationRecord | null | undefined
}): boolean {
  return record?.status === 'succeeded' && record.result !== null && !record.hydrated
}

export function createRunningPullRequestGenerationRecord(
  context: PullRequestGenerationContext,
  seed: PullRequestGenerationFields,
  seedFieldRevisions: PullRequestFieldRevisions
): PullRequestGenerationRecord {
  return {
    context,
    seed,
    seedFieldRevisions,
    status: 'running',
    result: null,
    error: null,
    hydrated: false
  }
}

export function resolvePullRequestGenerationSuccess({
  record,
  requestId,
  result
}: {
  record: PullRequestGenerationRecord | null | undefined
  requestId: number
  result: PullRequestGenerationFields
}): PullRequestGenerationRecord | null {
  if (!record || record.context.requestId !== requestId || record.status !== 'running') {
    return null
  }
  return {
    ...record,
    status: 'succeeded',
    result,
    error: null,
    hydrated: false
  }
}

export function resolvePullRequestGenerationFailure({
  record,
  requestId,
  error,
  canceled = false
}: {
  record: PullRequestGenerationRecord | null | undefined
  requestId: number
  error: string | null
  canceled?: boolean
}): PullRequestGenerationRecord | null {
  if (!record || record.context.requestId !== requestId || record.status !== 'running') {
    return null
  }
  return {
    ...record,
    status: canceled ? 'canceled' : 'failed',
    result: null,
    error: canceled ? null : error,
    hydrated: false
  }
}

export function resolvePullRequestGenerationCancel(
  record: PullRequestGenerationRecord | null | undefined
): PullRequestGenerationRecord | null {
  if (!record || record.status !== 'running') {
    return null
  }
  return {
    ...record,
    status: 'canceled',
    error: null,
    hydrated: false
  }
}


