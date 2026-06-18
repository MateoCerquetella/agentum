// Tests for the in-agentum CDP screencast transport (009c-3): URL/token/query
// building, control-frame handling, binary-frame passthrough, input
// serialization, and reconnect/backoff. Mirrors the FakeWebSocket pattern in
// agentum-server-client.stream-reconnect.test.ts.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { openCdpScreencast } from './cdp-screencast-client'

vi.mock('./server-endpoint', () => ({
  getServerEndpoint: vi.fn(async () => ({ url: 'http://localhost:9999', token: 'tok' })),
  wsUrl: vi.fn(async (path: string) => `ws://localhost:9999${path}`)
}))

class FakeWebSocket {
  static OPEN = 1
  static CLOSED = 3
  static instances: FakeWebSocket[] = []

  url: string
  binaryType = ''
  readyState = 0
  sent: unknown[] = []
  private listeners: Record<string, ((ev: unknown) => void)[]> = {}

  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }

  addEventListener(type: string, cb: (ev: unknown) => void): void {
    ;(this.listeners[type] ??= []).push(cb)
  }

  send(data: unknown): void {
    this.sent.push(data)
  }

  close(): void {
    this.readyState = FakeWebSocket.CLOSED
    this.emit('close', {})
  }

  private emit(type: string, ev: unknown): void {
    for (const cb of this.listeners[type] ?? []) {
      cb(ev)
    }
  }

  fireOpen(): void {
    this.readyState = FakeWebSocket.OPEN
    this.emit('open', {})
  }

  fireDrop(): void {
    this.readyState = FakeWebSocket.CLOSED
    this.emit('close', {})
  }

  fireText(text: string): void {
    this.emit('message', { data: text })
  }

  fireBinary(buf: ArrayBuffer): void {
    this.emit('message', { data: buf })
  }
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeWebSocket.instances = []
  vi.stubGlobal('WebSocket', FakeWebSocket)
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
  vi.clearAllMocks()
})

const handlers = () => ({
  onReady: vi.fn(),
  onBinary: vi.fn(),
  onError: vi.fn(),
  onClose: vi.fn(),
  onReconnecting: vi.fn()
})

describe('openCdpScreencast', () => {
  it('builds the URL with token + screencast query params', async () => {
    await openCdpScreencast(
      { cdpPort: 9201, format: 'png', quality: 50, everyNthFrame: 1 },
      handlers()
    )
    const url = FakeWebSocket.instances[0].url
    expect(url).toContain('ws://localhost:9999/api/cdp-browser/screencast')
    expect(url).toContain('token=tok')
    expect(url).toContain('cdpPort=9201')
    expect(url).toContain('format=png')
    expect(url).toContain('quality=50')
    expect(url).toContain('everyNthFrame=1')
  })

  it('forwards binary frames as Uint8Array and fires onReady on the ready control', async () => {
    const h = handlers()
    await openCdpScreencast({}, h)
    const sock = FakeWebSocket.instances[0]
    sock.fireOpen()

    sock.fireText(JSON.stringify({ type: 'ready', format: 'jpeg' }))
    expect(h.onReady).toHaveBeenCalledWith({ format: 'jpeg' })

    const buf = new Uint8Array([0x62, 1, 1, 1]).buffer
    sock.fireBinary(buf)
    expect(h.onBinary).toHaveBeenCalledTimes(1)
    expect(h.onBinary.mock.calls[0][0]).toBeInstanceOf(Uint8Array)
  })

  it('serializes input as {method,params} JSON over the socket', async () => {
    const sub = await openCdpScreencast({}, handlers())
    const sock = FakeWebSocket.instances[0]
    sock.fireOpen()

    sub.sendInput('browser.mouseMove', { worktree: 'id:w', page: 'p', x: 10, y: 20 })
    expect(sock.sent).toHaveLength(1)
    expect(JSON.parse(sock.sent[0] as string)).toEqual({
      method: 'browser.mouseMove',
      params: { worktree: 'id:w', page: 'p', x: 10, y: 20 }
    })
  })

  it('drops input sent while the socket is not open (no throw, no queue)', async () => {
    const sub = await openCdpScreencast({}, handlers())
    const sock = FakeWebSocket.instances[0]
    // Not opened yet (readyState 0).
    sub.sendInput('browser.keypress', { key: 'a' })
    expect(sock.sent).toHaveLength(0)
  })

  it('surfaces a server error control frame', async () => {
    const h = handlers()
    await openCdpScreencast({}, h)
    FakeWebSocket.instances[0].fireText(JSON.stringify({ type: 'error', message: 'boom' }))
    expect(h.onError).toHaveBeenCalledWith('boom')
  })

  it('reconnects on a transient drop instead of firing onClose', async () => {
    const h = handlers()
    await openCdpScreencast({}, h)
    FakeWebSocket.instances[0].fireOpen()

    FakeWebSocket.instances[0].fireDrop()
    expect(h.onReconnecting).toHaveBeenCalledWith(1)
    expect(h.onClose).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(600)
    expect(FakeWebSocket.instances).toHaveLength(2)
    expect(h.onClose).not.toHaveBeenCalled()
  })

  it('gives up (onClose) after exhausting reconnect attempts', async () => {
    const h = handlers()
    await openCdpScreencast({}, h)
    // Drop repeatedly without a successful open → backoff escalates, then caps.
    for (let i = 0; i < 6; i += 1) {
      const sock = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
      sock.fireDrop()
      await vi.advanceTimersByTimeAsync(5_000)
    }
    expect(h.onClose).toHaveBeenCalledTimes(1)
  })

  it('a clean end control is terminal — no reconnect', async () => {
    const h = handlers()
    await openCdpScreencast({}, h)
    const sock = FakeWebSocket.instances[0]
    sock.fireOpen()

    sock.fireText(JSON.stringify({ type: 'end' }))
    expect(h.onClose).toHaveBeenCalledTimes(1)

    sock.fireDrop()
    await vi.advanceTimersByTimeAsync(5_000)
    // No new socket: end disposed the stream.
    expect(FakeWebSocket.instances).toHaveLength(1)
    expect(h.onReconnecting).not.toHaveBeenCalled()
  })

  it('close() tears down without firing onClose or reconnecting', async () => {
    const h = handlers()
    const sub = await openCdpScreencast({}, h)
    FakeWebSocket.instances[0].fireOpen()

    sub.close()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(h.onClose).not.toHaveBeenCalled()
    expect(FakeWebSocket.instances).toHaveLength(1)
  })
})
