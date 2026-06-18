// Blank-pane self-heal: a remote idle pane whose connect snapshot never landed
// on screen (lost in an xterm reflow, or an empty server snapshot under SSH
// channel pressure) has no live bytes to recover it — so if the terminal is
// still visually blank after a grace window, bindServerSessionTerminal forces a
// bounded number of fresh re-snapshots. A pane that actually painted never fires.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const requestRepaint = vi.fn()
const requestRedraw = vi.fn()
// Capture the handlers bindServerSessionTerminal passes so a test can drive
// lifecycle callbacks (e.g. onReconnected) the real WS would fire.
let capturedHandlers: { onReconnected?: () => void } | undefined
vi.mock('./agentum-server-client', () => ({
  openSessionStream: vi.fn(async (_id: string, _size: unknown, handlers: { onReconnected?: () => void }) => {
    capturedHandlers = handlers
    return {
      send: vi.fn(),
      resize: vi.fn(),
      requestRepaint,
      requestRedraw,
      close: vi.fn()
    }
  })
}))
vi.mock('./server-host-client', () => ({
  markHostConnectedFromHostKey: vi.fn(),
  markHostReconnectingFromHostKey: vi.fn()
}))

import { bindServerSessionTerminal } from './server-session-terminal'

// Minimal xterm Terminal stub: a single buffer line that is either empty
// (blank pane) or has content, plus the listener subs bind attaches.
function makeTerm(blank: boolean): import('@xterm/xterm').Terminal {
  const line = blank ? '' : 'hello'
  return {
    cols: 80,
    rows: 24,
    buffer: {
      active: {
        length: 1,
        getLine: () => ({ translateToString: () => line })
      }
    },
    write: vi.fn(),
    onData: vi.fn(() => ({ dispose: vi.fn() })),
    onResize: vi.fn(() => ({ dispose: vi.fn() }))
  } as unknown as import('@xterm/xterm').Terminal
}

beforeEach(() => {
  vi.useFakeTimers()
  requestRepaint.mockClear()
  requestRedraw.mockClear()
})
afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('blank-pane self-heal', () => {
  it('repaints when the pane is still blank after the grace window', async () => {
    await bindServerSessionTerminal('s', makeTerm(true), { hostKey: 'ssh:t' })
    expect(requestRepaint).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(6000)
    expect(requestRepaint).toHaveBeenCalledTimes(1)
  })

  it('does NOT repaint a pane that painted content', async () => {
    await bindServerSessionTerminal('s', makeTerm(false), { hostKey: 'ssh:t' })
    await vi.advanceTimersByTimeAsync(6000)
    expect(requestRepaint).not.toHaveBeenCalled()
  })

  it('stops after the bounded number of repaint attempts', async () => {
    await bindServerSessionTerminal('s', makeTerm(true), { hostKey: 'ssh:t' })
    await vi.advanceTimersByTimeAsync(6000) // attempt 1
    await vi.advanceTimersByTimeAsync(6000) // attempt 2
    await vi.advanceTimersByTimeAsync(6000) // bounded out — no 3rd
    expect(requestRepaint).toHaveBeenCalledTimes(2)
  })

  it('cancels the watchdog on dispose', async () => {
    const binding = await bindServerSessionTerminal('s', makeTerm(true), { hostKey: 'ssh:t' })
    binding.dispose()
    await vi.advanceTimersByTimeAsync(6000)
    expect(requestRepaint).not.toHaveBeenCalled()
  })
})

describe('reconnect redraw heal', () => {
  it('forces a redraw when the stream reconnects (suspend/resume path)', async () => {
    await bindServerSessionTerminal('s', makeTerm(false), { hostKey: 'ssh:t' })
    expect(requestRedraw).not.toHaveBeenCalled()
    // Simulate the WS recovering after a drop — the server has already replayed
    // the (possibly broadcast-corrupted) resume delta by now.
    capturedHandlers?.onReconnected?.()
    expect(requestRedraw).toHaveBeenCalledTimes(1)
  })

  it('exposes a manual forceRedraw that drives the same heal', async () => {
    const binding = await bindServerSessionTerminal('s', makeTerm(false), { hostKey: 'ssh:t' })
    binding.forceRedraw()
    expect(requestRedraw).toHaveBeenCalledTimes(1)
  })
})
