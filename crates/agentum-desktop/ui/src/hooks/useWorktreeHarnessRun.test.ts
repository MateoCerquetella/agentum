import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { FeatureState, HarnessEvent, HarnessStatus } from '@/runtime/harness-client'
import {
  beginWorktreeHarnessSnapshotRequest,
  createWorktreeHarnessSnapshotState,
  resolveWorktreeHarnessSnapshotRequest,
  selectWorktreeHarnessSnapshot,
  useWorktreeHarnessRun
} from './useWorktreeHarnessRun'

const io = vi.hoisted(() => ({
  getHarnessStatus: vi.fn(),
  listHarnesses: vi.fn(),
  subscribeHarnessEvents: vi.fn(),
  onEvent: null as ((event: HarnessEvent) => void) | null,
  onConnected: null as (() => void) | null,
  closeStream: vi.fn()
}))

const reactState = vi.hoisted(() => ({
  slots: [] as unknown[],
  index: 0,
  effectsEnabled: false,
  cleanups: [] as Array<() => void>
}))

vi.mock('react', () => ({
  useCallback: <T extends (...args: never[]) => unknown>(callback: T) => callback,
  useEffect: (effect: () => void | (() => void)) => {
    if (!reactState.effectsEnabled) return
    const cleanup = effect()
    if (typeof cleanup === 'function') reactState.cleanups.push(cleanup)
  },
  useRef: <T>(initial: T) => {
    const index = reactState.index
    reactState.index += 1
    if (!(index in reactState.slots)) reactState.slots[index] = { current: initial }
    return reactState.slots[index] as { current: T }
  },
  useState: <T>(initial: T | (() => T)) => {
    const index = reactState.index
    reactState.index += 1
    if (!(index in reactState.slots)) {
      reactState.slots[index] =
        typeof initial === 'function' ? (initial as () => T)() : initial
    }
    const setState = (next: T | ((current: T) => T)): void => {
      const current = reactState.slots[index] as T
      reactState.slots[index] =
        typeof next === 'function'
          ? (next as (current: T) => T)(current)
          : next
    }
    return [reactState.slots[index] as T, setState] as const
  }
}))

vi.mock('@/runtime/harness-client', () => ({
  getHarnessStatus: io.getHarnessStatus,
  listHarnesses: io.listHarnesses,
  subscribeHarnessEvents: io.subscribeHarnessEvents
}))

vi.mock('@/lib/harness-run', () => ({
  findHarnessRunForWorkdir: (
    runs: HarnessStatus[],
    workdir: string,
    worktreeId: string | undefined,
    allowLegacyLocalPathFallback: boolean
  ) => {
    if (worktreeId) {
      return (
        runs.find((run) => run.worktree_id === worktreeId) ??
        (allowLegacyLocalPathFallback
          ? runs.find(
              (run) =>
                !run.worktree_id &&
                run.workdir.replace(/\/+$/, '') === workdir.replace(/\/+$/, '')
            )
          : undefined)
      )
    }
    return allowLegacyLocalPathFallback
      ? runs.find((run) => run.workdir.replace(/\/+$/, '') === workdir.replace(/\/+$/, ''))
      : undefined
  }
}))

function makeRun(overrides: Partial<HarnessStatus> = {}): HarnessStatus {
  return {
    id: 'run-1',
    workdir: '/workspace/feature',
    state: 'running',
    phase: 'executing',
    current_feature: 'F1',
    features: {
      features: [
        {
          id: 'F1',
          name: 'Build the thing',
          description: '',
          state: 'coding',
          attempts: 0
        }
      ],
      max_retries: 2,
      agent_tool: 'claude',
      settle_grace_secs: 8,
      settle_timeout_secs: 1200,
      agent_yolo: true
    },
    elapsed_secs: 0,
    agent_instructions: '',
    ...overrides
  }
}

function makeSnapshotRun(
  workdir: string,
  featureState: FeatureState,
  overrides: Partial<HarnessStatus> = {}
): HarnessStatus {
  const run = makeRun({
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
    ...overrides
  })
  run.features.features[0].state = featureState
  return run
}

function renderHook(
  workdir: string | undefined,
  runEffects: boolean,
  worktreeId?: string,
  allowLegacyLocalPathFallback = true
) {
  reactState.index = 0
  reactState.effectsEnabled = runEffects
  const result = useWorktreeHarnessRun(
    workdir,
    worktreeId,
    allowLegacyLocalPathFallback
  )
  reactState.effectsEnabled = false
  return result
}

function mountHook(
  workdir: string | undefined,
  worktreeId?: string,
  allowLegacyLocalPathFallback = true
) {
  return renderHook(workdir, true, worktreeId, allowLegacyLocalPathFallback)
}

function readHook(
  workdir: string | undefined,
  worktreeId?: string,
  allowLegacyLocalPathFallback = true
) {
  return renderHook(workdir, false, worktreeId, allowLegacyLocalPathFallback)
}

function cleanupEffects(): void {
  for (const cleanup of reactState.cleanups.splice(0)) cleanup()
}

async function flushAsyncWork(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

beforeEach(() => {
  cleanupEffects()
  reactState.slots = []
  reactState.index = 0
  reactState.effectsEnabled = false
  io.onEvent = null
  io.onConnected = null
  vi.clearAllMocks()
  io.subscribeHarnessEvents.mockImplementation(
    (
      onEvent: (event: HarnessEvent) => void,
      onConnected?: () => void
    ) => {
      io.onEvent = onEvent
      io.onConnected = onConnected ?? null
      return Promise.resolve({ close: io.closeStream })
    }
  )
})

describe('useWorktreeHarnessRun', () => {
  it('hydrates a selected worktree from the harness list without an event', async () => {
    const run = makeRun()
    io.listHarnesses.mockResolvedValue([run])

    mountHook('/workspace/feature')
    await flushAsyncWork()

    expect(io.listHarnesses).toHaveBeenCalledTimes(1)
    expect(io.getHarnessStatus).not.toHaveBeenCalled()
    expect(readHook('/workspace/feature').run).toBe(run)
  })

  it('hydrates by worktree id when two hosts expose the same path', async () => {
    const first = makeRun({
      id: 'run-a',
      worktree_id: 'repo-a::/srv/shared',
      host_id: 'host-a',
      workdir: '/srv/shared'
    })
    const second = makeRun({
      id: 'run-b',
      worktree_id: 'repo-b::/srv/shared',
      host_id: 'host-b',
      workdir: '/srv/shared'
    })
    io.listHarnesses.mockResolvedValue([first, second])

    mountHook('/srv/shared', 'repo-b::/srv/shared', false)
    await flushAsyncWork()

    expect(readHook('/srv/shared', 'repo-b::/srv/shared', false).run).toBe(second)
  })

  it('does not hydrate an SSH worktree from a legacy local run at the same path', async () => {
    const legacyLocal = makeRun({ id: 'legacy-local', workdir: '/srv/shared' })
    io.listHarnesses.mockResolvedValue([legacyLocal])

    mountHook('/srv/shared', 'remote-repo::/srv/shared', false)
    await flushAsyncWork()

    expect(readHook('/srv/shared', 'remote-repo::/srv/shared', false).run).toBeUndefined()
  })

  it('refreshes the matched run for every status lifecycle event', async () => {
    const initial = makeRun()
    io.listHarnesses.mockResolvedValue([initial])
    const events: HarnessEvent[] = [
      { type: 'state_changed', harness_id: 'run-1', state: 'verifying' },
      {
        type: 'feature_state_changed',
        harness_id: 'run-1',
        feature_id: 'F1',
        state: 'verifying'
      },
      {
        type: 'phase_changed',
        harness_id: 'run-1',
        from: 'executing',
        to: 'review'
      },
      {
        type: 'gate_result',
        harness_id: 'run-1',
        role: 'reviewer',
        passed: true,
        attempt: 1,
        summary: 'passed'
      },
      { type: 'harness_completed', harness_id: 'run-1', success: true }
    ]
    const snapshots = events.map((_, index) =>
      makeRun({ elapsed_secs: index + 1, state: index === events.length - 1 ? 'done' : 'running' })
    )
    for (const snapshot of snapshots) {
      io.getHarnessStatus.mockResolvedValueOnce(snapshot)
    }

    mountHook('/workspace/feature')
    await flushAsyncWork()

    for (const [index, event] of events.entries()) {
      io.onEvent?.(event)
      await flushAsyncWork()
      expect(readHook('/workspace/feature').run).toBe(snapshots[index])
    }
    expect(io.getHarnessStatus).toHaveBeenCalledTimes(events.length)
    expect(io.getHarnessStatus).toHaveBeenCalledWith('run-1')
  })

  it('keeps the last snapshot while connected and lagged reconciliation completes', async () => {
    const initial = makeRun({ elapsed_secs: 1 })
    const connected = makeRun({ elapsed_secs: 2 })
    const reconciled = makeRun({ elapsed_secs: 3 })
    const connectedRead = deferred<HarnessStatus[]>()
    const laggedRead = deferred<HarnessStatus[]>()
    io.listHarnesses
      .mockResolvedValueOnce([initial])
      .mockImplementationOnce(() => connectedRead.promise)
      .mockImplementationOnce(() => laggedRead.promise)

    mountHook('/workspace/feature')
    await flushAsyncWork()
    expect(readHook('/workspace/feature').run).toBe(initial)

    io.onConnected?.()
    expect(readHook('/workspace/feature').run).toBe(initial)
    connectedRead.resolve([connected])
    await flushAsyncWork()
    expect(readHook('/workspace/feature').run).toBe(connected)

    io.onEvent?.({ type: 'lagged', skipped: 2 })
    expect(readHook('/workspace/feature').run).toBe(connected)
    laggedRead.resolve([reconciled])
    await flushAsyncWork()
    expect(readHook('/workspace/feature').run).toBe(reconciled)
  })

  it('does not expose a previous or absent workspace run', async () => {
    const first = makeRun({ workdir: '/workspace/first' })
    const secondRead = deferred<HarnessStatus[]>()
    io.listHarnesses
      .mockResolvedValueOnce([first])
      .mockImplementationOnce(() => secondRead.promise)

    mountHook('/workspace/first')
    await flushAsyncWork()
    expect(readHook('/workspace/first').run).toBe(first)

    cleanupEffects()
    mountHook('/workspace/absent')
    expect(readHook('/workspace/absent').run).toBeUndefined()

    secondRead.resolve([])
    await flushAsyncWork()
    expect(readHook('/workspace/absent').run).toBeUndefined()
  })
})

describe('worktree harness snapshot continuity', () => {
  it('keeps the matched snapshot selected while gate refreshes resolve in order', () => {
    const workdir = '/workspace/feature'
    const coding = makeSnapshotRun(workdir, 'coding')
    const verifying = makeSnapshotRun(workdir, 'verifying')
    const readyToTest = makeSnapshotRun(workdir, 'ready_to_test')
    let state = createWorktreeHarnessSnapshotState()

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 1)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 1, coding, undefined, true)

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 2)
    expect(selectWorktreeHarnessSnapshot(state, workdir, undefined, true)).toBe(coding)
    state = resolveWorktreeHarnessSnapshotRequest(
      state,
      workdir,
      2,
      verifying,
      undefined,
      true
    )
    expect(selectWorktreeHarnessSnapshot(state, workdir, undefined, true)).toBe(verifying)

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 3)
    expect(selectWorktreeHarnessSnapshot(state, workdir, undefined, true)).toBe(verifying)
    state = resolveWorktreeHarnessSnapshotRequest(
      state,
      workdir,
      3,
      readyToTest,
      undefined,
      true
    )
    expect(selectWorktreeHarnessSnapshot(state, workdir, undefined, true)).toBe(readyToTest)
  })

  it('selects the owning snapshot when switching away and back and clears an unmatched worktree', () => {
    const owningWorkdir = '/workspace/feature'
    const unmatchedWorkdir = '/workspace/unmatched'
    const latest = makeSnapshotRun(owningWorkdir, 'done', { state: 'done', phase: 'done' })
    let state = createWorktreeHarnessSnapshotState()

    state = beginWorktreeHarnessSnapshotRequest(state, owningWorkdir, 1)
    state = resolveWorktreeHarnessSnapshotRequest(
      state,
      owningWorkdir,
      1,
      latest,
      undefined,
      true
    )

    expect(
      selectWorktreeHarnessSnapshot(state, unmatchedWorkdir, undefined, true)
    ).toBeUndefined()
    state = beginWorktreeHarnessSnapshotRequest(state, unmatchedWorkdir, 2)
    state = resolveWorktreeHarnessSnapshotRequest(
      state,
      unmatchedWorkdir,
      2,
      undefined,
      undefined,
      true
    )
    expect(
      selectWorktreeHarnessSnapshot(state, unmatchedWorkdir, undefined, true)
    ).toBeUndefined()
    expect(
      selectWorktreeHarnessSnapshot(state, `${owningWorkdir}/`, undefined, true)
    ).toBe(latest)
  })

  it('rejects a stale response and a snapshot owned by another normalized workdir', () => {
    const workdir = '/workspace/feature'
    const initial = makeSnapshotRun(workdir, 'coding')
    const stale = makeSnapshotRun(workdir, 'verifying')
    const latest = makeSnapshotRun(workdir, 'done', { state: 'done', phase: 'done' })
    const foreign = makeSnapshotRun('/workspace/other', 'blocked')
    let state = createWorktreeHarnessSnapshotState()

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 1)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 1, initial, undefined, true)
    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 2)
    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 3)
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 3, latest, undefined, true)

    const afterLatest = state
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 2, stale, undefined, true)
    expect(state).toBe(afterLatest)
    expect(selectWorktreeHarnessSnapshot(state, workdir, undefined, true)).toBe(latest)

    state = beginWorktreeHarnessSnapshotRequest(state, workdir, 4)
    const beforeForeign = state
    state = resolveWorktreeHarnessSnapshotRequest(state, workdir, 4, foreign, undefined, true)
    expect(state).toBe(beforeForeign)
    expect(selectWorktreeHarnessSnapshot(state, workdir, undefined, true)).toBe(latest)
    expect(
      selectWorktreeHarnessSnapshot(state, foreign.workdir, undefined, true)
    ).toBeUndefined()
  })
})
