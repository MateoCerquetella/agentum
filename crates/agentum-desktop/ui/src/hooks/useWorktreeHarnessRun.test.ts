import { describe, expect, it } from 'vitest'
import type { FeatureState, HarnessStatus } from '@/runtime/harness-client'
import {
  beginWorktreeHarnessSnapshotRequest,
  createWorktreeHarnessSnapshotState,
  resolveWorktreeHarnessSnapshotRequest,
  selectWorktreeHarnessSnapshot
} from './useWorktreeHarnessRun'

function makeRun(
  workdir: string,
  featureState: FeatureState,
  overrides: Partial<HarnessStatus> = {}
): HarnessStatus {
  return {
    id: `run-${workdir.split('/').slice(-1)[0]}`,
    workdir,
    state:
      featureState === 'verifying'
        ? 'verifying'
        : featureState === 'blocked'
          ? 'blocked'
          : 'running',
    phase: featureState === 'blocked' ? 'blocked' : 'executing',
    blocked_phase: featureState === 'blocked' ? 'executing' : null,
    current_feature: 'F1',
    features: {
      features: [
        {
          id: 'F1',
          name: 'Keep the topbar visible',
          description: '',
          state: featureState,
          attempts: 0
        }
      ],
      max_retries: 3,
      agent_tool: 'codex',
      settle_grace_secs: 8,
      settle_timeout_secs: 1800,
      agent_yolo: true,
      roles: true
    },
    elapsed_secs: 1,
    agent_instructions: '',
    ...overrides
  }
}

describe('worktree harness snapshot continuity', () => {
  it('keeps the matched snapshot selected while gate refreshes resolve in order', () => {
    const workdir = '/workspace/feature'
    const coding = makeRun(workdir, 'coding')
    const verifying = makeRun(workdir, 'verifying')
    const readyToTest = makeRun(workdir, 'ready_to_test')
    let state = createWorktreeHarnessSnapshotState()

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 1)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 1, coding)

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 2)
    expect(selectWorktreeHarnessSnapshot(state, workdir)).toBe(coding)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 2, verifying)
    expect(selectWorktreeHarnessSnapshot(state, workdir)).toBe(verifying)

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 3)
    expect(selectWorktreeHarnessSnapshot(state, workdir)).toBe(verifying)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 3, readyToTest)
    expect(selectWorktreeHarnessSnapshot(state, workdir)).toBe(readyToTest)
  })

  it('selects the owning snapshot when switching away and back and clears an unmatched worktree', () => {
    const owningWorkdir = '/workspace/feature'
    const unmatchedWorkdir = '/workspace/unmatched'
    const latest = makeRun(owningWorkdir, 'done', { state: 'done', phase: 'done' })
    let state = createWorktreeHarnessSnapshotState()

    state = beginWorktreeHarnessSnapshotRequest(state, owningWorkdir, 1)
    state = resolveWorktreeHarnessSnapshotRequest(state, owningWorkdir, 1, latest)

    expect(selectWorktreeHarnessSnapshot(state, unmatchedWorkdir)).toBeUndefined()
    state = beginWorktreeHarnessSnapshotRequest(state, unmatchedWorkdir, 2)
    state = resolveWorktreeHarnessSnapshotRequest(state, unmatchedWorkdir, 2, undefined)
    expect(selectWorktreeHarnessSnapshot(state, unmatchedWorkdir)).toBeUndefined()
    expect(selectWorktreeHarnessSnapshot(state, `${owningWorkdir}/`)).toBe(latest)
  })

  it('rejects a stale response and a snapshot owned by another normalized workdir', () => {
    const workdir = '/workspace/feature'
    const initial = makeRun(workdir, 'coding')
    const stale = makeRun(workdir, 'verifying')
    const latest = makeRun(workdir, 'done', { state: 'done', phase: 'done' })
    const foreign = makeRun('/workspace/other', 'blocked')
    let state = createWorktreeHarnessSnapshotState()

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 1)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 1, initial)
    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 2)
    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 3)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 3, latest)

    const afterLatest = state
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 2, stale)
    expect(state).toBe(afterLatest)
    expect(selectWorktreeHarnessSnapshot(state, workdir)).toBe(latest)

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 4)
    const beforeForeign = state
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 4, foreign)
    expect(state).toBe(beforeForeign)
    expect(selectWorktreeHarnessSnapshot(state, workdir)).toBe(latest)
    expect(selectWorktreeHarnessSnapshot(state, foreign.workdir)).toBeUndefined()
  })
})
