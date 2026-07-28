// Why: reproduce the real Codex spinner bug from a captured live session. Codex
// animates its OSC title with braille frames ("⠴ testi") but interleaves a
// status-less bare frame ("testi") between them *while still working*. Each bare
// frame makes detectAgentStatusFromTitle return null, which would collapse the
// sidebar spinner to idle for that instant — so the spinner flickers and never
// reads as steadily working without a distracting spinner.
// The transport must hold a transient working→non-working blip and only commit
// to idle when the bare title is sustained (a real turn end). This pins that
// contract at the transport level, where runtimePaneTitlesByTabId is fed.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const ESC = '\x1b'
const BEL = '\x07'
// Real frames captured from the Codex pane log (worktree folder "testi").
const braille = (frame: string): string => `${ESC}]0;${frame} testi${BEL}`
const bare = (): string => `${ESC}]0;testi${BEL}`

// Must match/exceed WORKING_TITLE_HOLD_MS in pty-transport.ts (300ms).
const HOLD_MS = 300

describe('createIpcPtyTransport — Codex spinner flicker absorption', () => {
  const originalWindow = (globalThis as { window?: typeof window }).window
  let onData: ((payload: { id: string; data: string }) => void) | null = null

  beforeEach(() => {
    vi.resetModules()
    vi.useFakeTimers()
    onData = null
    ;(globalThis as { window: typeof window }).window = {
      ...originalWindow,
      api: {
        ...originalWindow?.api,
        pty: {
          ...originalWindow?.api?.pty,
          spawn: vi.fn().mockResolvedValue({ id: 'pty-codex' }),
          write: vi.fn(),
          resize: vi.fn(),
          kill: vi.fn(),
          onData: vi.fn((cb: (payload: { id: string; data: string }) => void) => {
            onData = cb
            return () => {}
          }),
          onReplay: vi.fn(() => () => {}),
          onExit: vi.fn(() => () => {})
        }
      }
    } as unknown as typeof window
  })

  afterEach(() => {
    vi.useRealTimers()
    if (originalWindow) {
      ;(globalThis as { window: typeof window }).window = originalWindow
    } else {
      delete (globalThis as { window?: typeof window }).window
    }
  })

  it('absorbs a bare frame interleaved between braille frames (spinner stays working)', async () => {
    const { createIpcPtyTransport } = await import('./pty-transport')
    const onTitleChange = vi.fn()
    const transport = createIpcPtyTransport({ onTitleChange })
    await transport.attach({ existingPtyId: 'pty-codex', callbacks: {} })

    // Codex working animation: braille, then a bare "testi" mid-turn, then more braille.
    onData?.({ id: 'pty-codex', data: braille('⠴') })
    onData?.({ id: 'pty-codex', data: bare() }) // transient null frame, still working
    onData?.({ id: 'pty-codex', data: braille('⠦') })
    vi.advanceTimersByTime(5) // drain side effects

    const applied = onTitleChange.mock.calls.map((c) => c[0])
    expect(applied).toContain('⠴ testi')
    expect(applied).toContain('⠦ testi')
    // The interleaved bare frame must NOT have collapsed the working state.
    expect(applied).not.toContain('testi')

    transport.disconnect()
  })

  it('commits to idle when the bare frame is sustained (real turn end)', async () => {
    const { createIpcPtyTransport } = await import('./pty-transport')
    const onTitleChange = vi.fn()
    const transport = createIpcPtyTransport({ onTitleChange })
    await transport.attach({ existingPtyId: 'pty-codex', callbacks: {} })

    onData?.({ id: 'pty-codex', data: braille('⠴') })
    vi.advanceTimersByTime(5)
    expect(onTitleChange.mock.calls.map((c) => c[0])).toContain('⠴ testi')

    onTitleChange.mockClear()
    onData?.({ id: 'pty-codex', data: bare() }) // turn ended — bare title persists
    vi.advanceTimersByTime(5) // drain; bare frame is held, not applied yet
    expect(onTitleChange.mock.calls.map((c) => c[0])).not.toContain('testi')

    vi.advanceTimersByTime(HOLD_MS + 50) // hold expires → idle commits
    expect(onTitleChange.mock.calls.map((c) => c[0])).toContain('testi')

    transport.disconnect()
  })
})
