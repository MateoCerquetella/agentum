type RemoteBrowserKeyboardEvent = {
  key: string
  code?: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
}

export function getRemoteBrowserKeypressKey(event: RemoteBrowserKeyboardEvent): string | null {
  if (event.key.length === 1) {
    return event.key === ' ' ? 'Space' : event.key
  }
  if (event.metaKey || event.ctrlKey || event.altKey) {
    return null
  }
  const supported = new Set([
    'Enter',
    'Backspace',
    'Delete',
    'Tab',
    'Escape',
    'ArrowUp',
    'ArrowDown',
    'ArrowLeft',
    'ArrowRight',
    'Home',
    'End',
    'PageUp',
    'PageDown'
  ])
  return supported.has(event.key) ? event.key : null
}

/** True for the paste chord (Cmd+V / Ctrl+V), as returned by
 *  {@link getRemoteBrowserKeyboardShortcut}. onKeyDown must NOT preventDefault or
 *  emit a keypress for this — a synthetic Cmd/Ctrl+V never triggers a real
 *  clipboard read in headless Chromium; the native paste event is what carries
 *  the text (see {@link getRemoteBrowserInsertText}). */
export function isRemoteBrowserPasteShortcut(shortcut: string | null): boolean {
  return shortcut === 'Meta+v' || shortcut === 'Control+v'
}

/** Build the `browser.insertText` message for pasted clipboard text, or null for
 *  empty text. The text comes from an `onPaste` ClipboardEvent — never
 *  `navigator.clipboard.readText()` (webview permission/blocking risk). The server
 *  maps this to CDP `Input.insertText`, a trusted paste. */
export function getRemoteBrowserInsertText(
  text: string
): { method: 'browser.insertText'; params: { text: string } } | null {
  if (text.length === 0) {
    return null
  }
  return { method: 'browser.insertText', params: { text } }
}

export function getRemoteBrowserKeyboardShortcut(event: RemoteBrowserKeyboardEvent): string | null {
  const modifiers: string[] = []
  if (event.metaKey) {
    modifiers.push('Meta')
  }
  if (event.ctrlKey) {
    modifiers.push('Control')
  }
  if (event.altKey) {
    modifiers.push('Alt')
  }
  const hasShortcutModifier = event.metaKey || event.ctrlKey || event.altKey
  // Why: Ctrl+Shift+R is a browser shortcut, but plain Shift+R should still
  // flow through as printable text for the remote page.
  if (event.shiftKey && (event.key.length !== 1 || hasShortcutModifier)) {
    modifiers.push('Shift')
  }
  if (modifiers.length === 0 || ['Meta', 'Control', 'Alt', 'Shift'].includes(event.key)) {
    return null
  }
  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key
  return `${modifiers.join('+')}+${key}`
}
