// Spec 023 Part B — the unlink client call (AC 7's "via runtime/harness-client.ts").
// `server-endpoint` is mocked so no loopback server is needed; `fetch` is
// stubbed per test.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { subscribeHarnessEvents, unlinkHarnessIssue } from './harness-client'

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
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
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
