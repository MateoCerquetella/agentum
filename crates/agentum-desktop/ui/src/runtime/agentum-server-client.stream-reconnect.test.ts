// Regression test for the desktop bug where a REMOTE (SSH) session pane showed
// "[agentum: session stream closed]" and never recovered after a transient drop
// — while the TUI rode through the same drops. openSessionStream must reconnect
// like the TUI's open_terminal_stream, giving up only when the session is gone
// or the token is rejected. See server-session-terminal.ts / terminal/api.rs.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { openSessionStream } from './agentum-server-client'

vi.mock('./server-endpoint', () => ({
  getServerEndpoint: vi.fn(async () => ({ token: 'tok' })),
  wsUrl: vi.fn(async (path: string) => `ws://localhost${path}`),
  apiUrl: vi.fn(async (path: string) => `http://localhost${path}`)
}))

vi.mock('./io-meter', () => ({
  record: vi.fn(),
  LOCAL_HOST_KEY: 'local'
}))

/** Minimal stand-in for the browser WebSocket the code constructs. Captures
 *  listeners so the test can drive open/message/close, and records sent frames. */
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

  /** Server-initiated drop (the WS just closing, not via our close()). */
  fireDrop(): void {
    this.readyState = FakeWebSocket.CLOSED
    this.emit('close', {})
  }

  fireMessage(buf: ArrayBuffer): void {
    this.emit('message', { data: buf })
  }
}

// 200 = session still there → keep retrying; 404/401/403 = give up.
let fetchStatus = 200

beforeEach(() => {
  vi.useFakeTimers()
  FakeWebSocket.instances = []
  fetchStatus = 200
  vi.stubGlobal('WebSocket', FakeWebSocket)
  vi.stubGlobal('fetch', vi.fn(async () => ({ status: fetchStatus })))
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
  vi.clearAllMocks()
})

const handlers = () => ({
  onData: vi.fn(),
  onClose: vi.fn(),
  onReconnecting: vi.fn()
})

describe('openSessionStream reconnect', () => {
  it('reconnects on a transient drop instead of firing onClose', async () => {
    const h = handlers()
    await openSessionStream('sess-1', { cols: 80, rows: 24 }, h)

    expect(FakeWebSocket.instances).toHaveLength(1)
    FakeWebSocket.instances[0].fireOpen()

    // Server-side tail dies → the WS closes. This must NOT be terminal.
    FakeWebSocket.instances[0].fireDrop()
    expect(h.onReconnecting).toHaveBeenCalledWith(1)
    expect(h.onClose).not.toHaveBeenCalled()

    // After backoff + the "is the session gone?" probe (200 → no), it reconnects.
    await vi.advanceTimersByTimeAsync(1000)
    expect(FakeWebSocket.instances).toHaveLength(2)
    expect(h.onClose).not.toHaveBeenCalled()
  })

  it('gives up (onClose) when the session is gone (404)', async () => {
    const h = handlers()
    await openSessionStream('sess-1', { cols: 80, rows: 24 }, h)
    FakeWebSocket.instances[0].fireOpen()

    fetchStatus = 404
    FakeWebSocket.instances[0].fireDrop()
    await vi.advanceTimersByTimeAsync(1000)

    expect(h.onClose).toHaveBeenCalledTimes(1)
    expect(FakeWebSocket.instances).toHaveLength(1) // never reconnected
  })

  it('does NOT fire onClose when the caller disposes the stream', async () => {
    const h = handlers()
    const stream = await openSessionStream('sess-1', { cols: 80, rows: 24 }, h)
    FakeWebSocket.instances[0].fireOpen()

    stream.close()
    await vi.advanceTimersByTimeAsync(1000)

    expect(h.onClose).not.toHaveBeenCalled()
    expect(h.onReconnecting).not.toHaveBeenCalled()
    expect(FakeWebSocket.instances).toHaveLength(1)
  })

  it('routes keystrokes to the live socket across a reconnect', async () => {
    const h = handlers()
    const stream = await openSessionStream('sess-1', { cols: 80, rows: 24 }, h)
    const first = FakeWebSocket.instances[0]
    first.fireOpen()

    stream.send('a')
    // First frame is the initial resize (sent on open); the keystroke follows.
    expect(first.sent.some((f) => f instanceof Uint8Array)).toBe(true)

    first.fireDrop()
    await vi.advanceTimersByTimeAsync(1000)
    const second = FakeWebSocket.instances[1]
    second.fireOpen()

    stream.send('b')
    expect(second.sent.some((f) => f instanceof Uint8Array)).toBe(true)
  })

  it('fires onReconnected only after recovering from a drop, not on first connect', async () => {
    // The recovery signal the desktop needs: a reconnect that SUCCEEDS proves
    // the host is reachable again, so the sidebar/file-tree (which failed during
    // the outage) can refresh. The very first connect is not a recovery.
    const h = { ...handlers(), onReconnected: vi.fn() }
    await openSessionStream('sess-1', { cols: 80, rows: 24 }, h)
    FakeWebSocket.instances[0].fireOpen()
    expect(h.onReconnected).not.toHaveBeenCalled()

    FakeWebSocket.instances[0].fireDrop()
    await vi.advanceTimersByTimeAsync(1000)
    // The reconnect socket opening = recovered.
    FakeWebSocket.instances[1].fireOpen()
    expect(h.onReconnected).toHaveBeenCalledTimes(1)
  })

  it('requestRepaint reconnects fresh (no resume) and is not treated as a reconnect', async () => {
    // The blank-pane self-heal: force a full re-snapshot in place. A normal
    // reconnect would send resume=true; a repaint must NOT (it needs the whole
    // snapshot), and it must not look like a recovery (no onReconnecting).
    const h = handlers()
    const stream = await openSessionStream('sess-1', { cols: 80, rows: 24 }, h)
    FakeWebSocket.instances[0].fireOpen() // connectedOnce = true

    stream.requestRepaint()

    expect(FakeWebSocket.instances).toHaveLength(2)
    expect(FakeWebSocket.instances[1].url).not.toContain('resume=true')
    expect(h.onReconnecting).not.toHaveBeenCalled()
    expect(h.onClose).not.toHaveBeenCalled()
  })

  it('passes resume=true on reconnect but not on the first connect', async () => {
    const h = handlers()
    await openSessionStream('sess-1', { cols: 80, rows: 24 }, h)
    expect(FakeWebSocket.instances[0].url).not.toContain('resume=true')

    FakeWebSocket.instances[0].fireOpen()
    FakeWebSocket.instances[0].fireDrop()
    await vi.advanceTimersByTimeAsync(1000)

    expect(FakeWebSocket.instances[1].url).toContain('resume=true')
  })
})
