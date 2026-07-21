import { describe, expect, it } from 'vitest'
import {
  getRemoteBrowserKeypressKey,
  getRemoteBrowserKeyboardShortcut,
  getRemoteBrowserInsertText,
  isRemoteBrowserPasteShortcut
} from './remote-browser-keyboard'

function keyboardEvent(
  overrides: Partial<Parameters<typeof getRemoteBrowserKeyboardShortcut>[0]>
): Parameters<typeof getRemoteBrowserKeyboardShortcut>[0] {
  return {
    key: 'r',
    code: 'KeyR',
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...overrides
  }
}

describe('remote browser keyboard serialization', () => {
  it('serializes modified letter shortcuts', () => {
    expect(getRemoteBrowserKeyboardShortcut(keyboardEvent({ ctrlKey: true }))).toBe('Control+r')
  })

  it('preserves Shift on modified letter shortcuts', () => {
    expect(
      getRemoteBrowserKeyboardShortcut(keyboardEvent({ key: 'R', ctrlKey: true, shiftKey: true }))
    ).toBe('Control+Shift+r')
  })

  it('keeps plain shifted printable input as text input', () => {
    const event = keyboardEvent({ key: 'R', shiftKey: true })

    expect(getRemoteBrowserKeyboardShortcut(event)).toBeNull()
    expect(getRemoteBrowserKeypressKey(event)).toBe('R')
  })

  it('detects the paste chord (Cmd/Ctrl+V) so onKeyDown can defer to onPaste', () => {
    // Cmd+V and Ctrl+V serialize to the paste shortcut…
    expect(getRemoteBrowserKeyboardShortcut(keyboardEvent({ key: 'v', metaKey: true }))).toBe(
      'Meta+v'
    )
    expect(getRemoteBrowserKeyboardShortcut(keyboardEvent({ key: 'v', ctrlKey: true }))).toBe(
      'Control+v'
    )
    expect(isRemoteBrowserPasteShortcut('Meta+v')).toBe(true)
    expect(isRemoteBrowserPasteShortcut('Control+v')).toBe(true)
    // …but a plain 'v' or reload is NOT a paste chord.
    expect(isRemoteBrowserPasteShortcut('Meta+r')).toBe(false)
    expect(isRemoteBrowserPasteShortcut(null)).toBe(false)
  })

  it('builds a browser.insertText message for pasted text (empty text is dropped)', () => {
    expect(getRemoteBrowserInsertText('hello world')).toEqual({
      method: 'browser.insertText',
      params: { text: 'hello world' }
    })
    // Empty clipboard → no message (never a stray browser.keypress {key:"v"}).
    expect(getRemoteBrowserInsertText('')).toBeNull()
  })
})
