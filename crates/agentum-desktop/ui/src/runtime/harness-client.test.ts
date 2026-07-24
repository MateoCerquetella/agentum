// Spec 023 Part B — the unlink client call (AC 7's "via runtime/harness-client.ts").
// `server-endpoint` is mocked so no loopback server is needed; `fetch` is
// stubbed per test.
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  restartHarness,
  startGatedWork,
  startHarness,
  subscribeHarnessEvents,
  unlinkHarnessIssue
} from './harness-client'

vi.mock('./server-endpoint', () => ({
  apiUrl: vi.fn((p: string) => Promise.resolve(p)),
  wsUrl: vi.fn((p: string) => Promise.resolve(p)),
  getServerEndpoint: vi.fn(() => Promise.resolve({ token: null }))
}))

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('unlinkHarnessIssue', () => {
  it('POSTs the dedicated unlink route for the run id', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(new Response('', { status: 200 })))
    vi.stubGlobal('fetch', fetchMock)
    await unlinkHarnessIssue('run-1')
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('/api/harness/run-1/unlink-issue')
    expect(init.method).toBe('POST')
  })

  it('rejects with the server detail on a 404 (unknown run)', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response('harness gone not found', { status: 404 })))
    )
    await expect(unlinkHarnessIssue('gone')).rejects.toThrow('harness 404')
  })
})

describe('worktree-scoped Harness requests', () => {
  it('registers a Harness with authoritative worktree identity', async () => {
    const fetchMock = vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify({ harness_id: 'run-1' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' }
        })
      )
    )
    vi.stubGlobal('fetch', fetchMock)

    await startHarness({
      workdir: '/srv/project feature',
      worktreeId: 'repo-1::/srv/project feature'
    })

    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(JSON.parse(String(init.body))).toEqual({
      workdir: '/srv/project feature',
      worktreeId: 'repo-1::/srv/project feature'
    })
  })

  it('starts gated work with the same authoritative identity', async () => {
    const fetchMock = vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            harnessId: 'run-1',
            specId: '42-feature',
            specExisted: false,
            planned: 1,
            runStarted: true,
            alreadyRunning: false
          }),
          { status: 200, headers: { 'Content-Type': 'application/json' } }
        )
      )
    )
    vi.stubGlobal('fetch', fetchMock)

    await startGatedWork({
      workdir: '/srv/project feature',
      worktreeId: 'repo-1::/srv/project feature',
      number: 42,
      slug: 'acme/widgets',
      agentTool: 'codex'
    })

    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(JSON.parse(String(init.body))).toMatchObject({
      workdir: '/srv/project feature',
      worktreeId: 'repo-1::/srv/project feature',
      number: '42',
      slug: 'acme/widgets',
      agentTool: 'codex'
    })
  })
})

describe('restartHarness', () => {
  const status = (overrides: Record<string, unknown> = {}): Record<string, unknown> => ({
    id: 'run-1',
    workdir: '/srv/project',
    state: 'failed',
    features: {
      features: [],
      max_retries: 2,
      agent_tool: 'claude',
      settle_grace_secs: 8,
      settle_timeout_secs: 1200,
      agent_yolo: true
    },
    elapsed_secs: 1,
    agent_instructions: '',
    ...overrides
  })

  it('does not resolve at the 202 boundary and waits for a visible worker', async () => {
    const responses = [
      new Response('', { status: 202 }),
      Response.json(status()),
      Response.json(status({ state: 'init_verifying' })),
      Response.json(status({ state: 'running', current_session: 'session-1' }))
    ]
    const fetchMock = vi.fn(() => Promise.resolve(responses.shift()!))
    vi.stubGlobal('fetch', fetchMock)

    const restarted = await restartHarness('run-1', { timeoutMs: 1_000, pollIntervalMs: 0 })

    expect(restarted.current_session).toBe('session-1')
    expect(fetchMock).toHaveBeenCalledTimes(4)
    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/harness/run-1/run')
  })

  it('rejects when an accepted retry fails again before launching a worker', async () => {
    const responses = [
      new Response('', { status: 202 }),
      Response.json(status({ state: 'init_verifying' })),
      Response.json(status())
    ]
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(responses.shift()!)))

    await expect(
      restartHarness('run-1', { timeoutMs: 1_000, pollIntervalMs: 0 })
    ).rejects.toThrow('failed again before a worker started')
  })

  it('rejects a retry that never produces observable progress', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn()
        .mockResolvedValueOnce(new Response('', { status: 202 }))
        .mockResolvedValue(Response.json(status()))
    )

    await expect(
      restartHarness('run-1', { timeoutMs: 0, pollIntervalMs: 0 })
    ).rejects.toThrow('no worker started within 0s')
  })
})


describe('subscribeHarnessEvents', () => {
  it('notifies subscribers after each successful event-stream connection', async () => {
    vi.useFakeTimers()

    type SocketListener = (event: { data?: unknown }) => void
    class FakeWebSocket {
      static instances: FakeWebSocket[] = []
      readonly listeners = new Map<string, SocketListener[]>()
      readonly close = vi.fn()

      constructor(readonly url: string) {
        FakeWebSocket.instances.push(this)
      }

      addEventListener(type: string, listener: SocketListener): void {
        const listeners = this.listeners.get(type) ?? []
        listeners.push(listener)
        this.listeners.set(type, listeners)
      }

      emit(type: string, data?: unknown): void {
        for (const listener of this.listeners.get(type) ?? []) {
          listener({ data })
        }
      }
    }

    vi.stubGlobal('WebSocket', FakeWebSocket)
    const onConnected = vi.fn()
    const stream = await subscribeHarnessEvents(vi.fn(), onConnected)

    expect(FakeWebSocket.instances).toHaveLength(1)
    FakeWebSocket.instances[0].emit('open')
    expect(onConnected).toHaveBeenCalledTimes(1)

    FakeWebSocket.instances[0].emit('close')
    await vi.runOnlyPendingTimersAsync()
    expect(FakeWebSocket.instances).toHaveLength(2)
    FakeWebSocket.instances[1].emit('open')
    expect(onConnected).toHaveBeenCalledTimes(2)

    stream.close()
    expect(FakeWebSocket.instances[1].close).toHaveBeenCalledTimes(1)
    FakeWebSocket.instances[1].emit('close')
    await vi.runOnlyPendingTimersAsync()
    expect(FakeWebSocket.instances).toHaveLength(2)
  })
})
