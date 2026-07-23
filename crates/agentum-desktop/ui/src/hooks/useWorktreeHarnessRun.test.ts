import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { HarnessEvent, HarnessStatus } from '@/runtime/harness-client'
import { useWorktreeHarnessRun } from './useWorktreeHarnessRun'

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
  useState: <T>(initial: T) => {
    const index = reactState.index
    reactState.index += 1
    if (!(index in reactState.slots)) reactState.slots[index] = initial
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
  findHarnessRunForWorkdir: (runs: HarnessStatus[], workdir: string) =>
    runs.find((run) => run.workdir.replace(/\/+$/, '') === workdir.replace(/\/+$/, ''))
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

function renderHook(workdir: string | undefined, runEffects: boolean) {
  reactState.index = 0
  reactState.effectsEnabled = runEffects
  const result = useWorktreeHarnessRun(workdir)
  reactState.effectsEnabled = false
  return result
}

function mountHook(workdir: string | undefined) {
  return renderHook(workdir, true)
}

function readHook(workdir: string | undefined) {
  return renderHook(workdir, false)
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
