import { useEffect, useSyncExternalStore } from 'react'

// Why: browser pages render in a Tauri **native child webview** that the Rust
// shell overlays on the window. A native webview always paints ABOVE the entire
// HTML/DOM layer, so CSS `z-index` cannot lift app chrome above it — any DOM
// overlay that opens over the browser region (a dropdown menu, popover, context
// menu, select, dialog/sheet) would be hidden BEHIND the page.
//
// The fix is to hide the native webview while such an overlay is open and
// restore it when the last one closes. This module is the shared, ref-counted
// signal: the overlay primitives (`ui/dialog`, `ui/dropdown-menu`, …) raise a
// lease while their portaled content is mounted (Radix only mounts content
// while open), and `NativeBrowserPagePane` subscribes to hide/show accordingly.
//
// Ref-counted because overlays nest (a dropdown inside a dialog) and several can
// be open at once; the webview must stay hidden until the count returns to zero.
// Mirrors the external-store shape of `browser-automation-visibility.ts`.

let openOverlayCount = 0
let version = 0
const listeners = new Set<() => void>()

function emitChange(): void {
  version += 1
  for (const listener of listeners) {
    listener()
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function getSnapshot(): number {
  return version
}

function getServerSnapshot(): number {
  return 0
}

/**
 * Register one open DOM overlay. Returns a release function that must be called
 * exactly once when the overlay closes/unmounts. The native browser webview is
 * hidden while the count is > 0.
 */
export function acquireNativeBrowserOverlay(): () => void {
  openOverlayCount += 1
  if (openOverlayCount === 1) {
    emitChange()
  }
  let released = false
  return () => {
    if (released) {
      return
    }
    released = true
    openOverlayCount = Math.max(0, openOverlayCount - 1)
    if (openOverlayCount === 0) {
      emitChange()
    }
  }
}

/** Is at least one DOM overlay currently open (so the native webview must hide)? */
export function isNativeBrowserOverlayOpen(): boolean {
  return openOverlayCount > 0
}

/**
 * React hook for an overlay primitive: hold a suppression lease for the lifetime
 * of the calling component. Because Radix mounts portaled content only while the
 * overlay is open, mounting === open, so a bare mount/unmount effect is the
 * correct open/close signal.
 */
export function useSuppressNativeBrowserWhileOpen(): void {
  useEffect(() => acquireNativeBrowserOverlay(), [])
}

/** Subscribe to whether any overlay is open (for the native webview pane). */
export function useNativeBrowserOverlayOpen(): boolean {
  useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
  return openOverlayCount > 0
}
