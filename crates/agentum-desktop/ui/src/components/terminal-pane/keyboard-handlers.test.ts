// src/renderer/src/components/terminal-pane/keyboard-handlers.test.ts
import { describe, it, expect } from 'vitest'
import {
  dispatchPaneChordInput,
  matchFileSearchShortcut,
  matchSearchNavigate
} from './keyboard-handlers'

function makeKeyEvent(
  overrides: Partial<{
    key: string
    metaKey: boolean
    ctrlKey: boolean
    shiftKey: boolean
    altKey: boolean
    repeat: boolean
  }>
): Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'repeat'> {
  return {
    key: 'g',
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    repeat: false,
    ...overrides
  }
}

describe('matchSearchNavigate', () => {
  const isMac = true
  const searchState = { query: 'hello', caseSensitive: false, regex: false }

  it('returns "next" for Cmd+G on macOS', () => {
    const e = makeKeyEvent({ metaKey: true })
    expect(matchSearchNavigate(e, isMac, true, searchState)).toBe('next')
  })

  it('returns "previous" for Cmd+Shift+G on macOS', () => {
    const e = makeKeyEvent({ metaKey: true, shiftKey: true })
    expect(matchSearchNavigate(e, isMac, true, searchState)).toBe('previous')
  })

  it('returns null when search is closed', () => {
    const e = makeKeyEvent({ metaKey: true })
    expect(matchSearchNavigate(e, isMac, false, searchState)).toBeNull()
  })

  it('returns null when query is empty', () => {
    const e = makeKeyEvent({ metaKey: true })
    expect(
      matchSearchNavigate(e, isMac, true, { query: '', caseSensitive: false, regex: false })
    ).toBeNull()
  })

  it('returns null for wrong key', () => {
    const e = makeKeyEvent({ metaKey: true, key: 'f' })
    expect(matchSearchNavigate(e, isMac, true, searchState)).toBeNull()
  })

  it('returns null when alt is pressed', () => {
    const e = makeKeyEvent({ metaKey: true, altKey: true })
    expect(matchSearchNavigate(e, isMac, true, searchState)).toBeNull()
  })

  it('returns "next" for Ctrl+G on Linux/Windows', () => {
    const e = makeKeyEvent({ ctrlKey: true })
    expect(matchSearchNavigate(e, false, true, searchState)).toBe('next')
  })

  it('returns "previous" for Ctrl+Shift+G on Linux/Windows', () => {
    const e = makeKeyEvent({ ctrlKey: true, shiftKey: true })
    expect(matchSearchNavigate(e, false, true, searchState)).toBe('previous')
  })

  it('returns null for Ctrl+G on macOS (wrong modifier)', () => {
    const e = makeKeyEvent({ ctrlKey: true })
    expect(matchSearchNavigate(e, true, true, searchState)).toBeNull()
  })
})

describe('dispatchPaneChordInput', () => {
  it('sends through the paneTransport for a local PTY pane', () => {
    const sent: string[] = []
    const transports = new Map([
      [1, { sendInput: (d: string) => { sent.push(d); return true } }]
    ])
    const bindings = new Map<number, { sendChordInput?: (d: string) => void }>()
    expect(dispatchPaneChordInput(1, '\x1b[1;3D', transports, bindings)).toBe('transport')
    expect(sent).toEqual(['\x1b[1;3D'])
  })

  // Regression: server-session panes (the default — tmux over WS) have NO entry
  // in paneTransportsRef, so the chord must fall through to the binding instead
  // of being dropped. This is the exact bug that killed word-nav/erase in agent
  // sessions.
  it('routes through the binding for a server-session pane with no transport', () => {
    const chord: string[] = []
    const transports = new Map<number, { sendInput: (d: string) => unknown }>()
    const bindings = new Map([[1, { sendChordInput: (d: string) => chord.push(d) }]])
    expect(dispatchPaneChordInput(1, '\x17', transports, bindings)).toBe('session')
    expect(chord).toEqual(['\x17'])
  })

  it('reports dropped when neither a transport nor a chord-capable binding exists', () => {
    const transports = new Map<number, { sendInput: (d: string) => unknown }>()
    const bindings = new Map<number, { sendChordInput?: (d: string) => void }>()
    expect(dispatchPaneChordInput(1, '\x17', transports, bindings)).toBe('dropped')
  })
})

describe('matchFileSearchShortcut', () => {
  it('matches Cmd+Shift+F on macOS', () => {
    expect(
      matchFileSearchShortcut(makeKeyEvent({ key: 'F', metaKey: true, shiftKey: true }), 'darwin')
    ).toBe(true)
  })

  it('matches Ctrl+Shift+F on Linux/Windows', () => {
    expect(
      matchFileSearchShortcut(makeKeyEvent({ key: 'F', ctrlKey: true, shiftKey: true }), 'linux')
    ).toBe(true)
  })

  it('rejects repeats, alt, and the wrong platform modifier', () => {
    expect(
      matchFileSearchShortcut(
        makeKeyEvent({ key: 'F', metaKey: true, shiftKey: true, repeat: true }),
        'darwin'
      )
    ).toBe(false)
    expect(
      matchFileSearchShortcut(
        makeKeyEvent({ key: 'F', metaKey: true, shiftKey: true, altKey: true }),
        'darwin'
      )
    ).toBe(false)
    expect(
      matchFileSearchShortcut(makeKeyEvent({ key: 'F', ctrlKey: true, shiftKey: true }), 'darwin')
    ).toBe(false)
  })

  it('follows customized file-search bindings', () => {
    const overrides = { 'sidebar.search.toggle': ['Ctrl+Alt+S'] }

    expect(
      matchFileSearchShortcut(
        makeKeyEvent({ key: 's', ctrlKey: true, altKey: true }),
        'linux',
        overrides
      )
    ).toBe(true)
    expect(
      matchFileSearchShortcut(
        makeKeyEvent({ key: 'F', ctrlKey: true, shiftKey: true }),
        'linux',
        overrides
      )
    ).toBe(false)
  })

  it('lets terminal-first pass the file-search shortcut through to the terminal', () => {
    expect(
      matchFileSearchShortcut(
        makeKeyEvent({ key: 'F', metaKey: true, shiftKey: true }),
        'darwin',
        undefined,
        'terminal-first'
      )
    ).toBe(false)
  })

  it('does not match when file search is disabled', () => {
    expect(
      matchFileSearchShortcut(makeKeyEvent({ key: 'F', metaKey: true, shiftKey: true }), 'darwin', {
        'sidebar.search.toggle': []
      })
    ).toBe(false)
  })
})
