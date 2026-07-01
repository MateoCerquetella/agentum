import type { ManagedPaneInternal } from './pane-manager-types'
import { safeFit } from './pane-tree-ops'
import { attachWebgl, disposeWebgl, markComplexScriptOutput } from './pane-webgl-renderer'
import { reattachWebglIfNeeded } from './pane-webgl-reattach'

export function setPaneGpuRenderingState(
  panes: Map<number, ManagedPaneInternal>,
  paneId: number,
  enabled: boolean
): void {
  const pane = panes.get(paneId)
  if (!pane) {
    return
  }
  pane.gpuRenderingEnabled = enabled
  if (!enabled) {
    disposeWebgl(pane, { refreshDimensions: true })
    return
  }
  if (pane.webglAttachmentDeferred || pane.webglDisabledAfterContextLoss) {
    return
  }
  if (!pane.webglAddon) {
    attachWebgl(pane)
    safeFit(pane)
  }
}

export function markPaneComplexScriptOutput(
  panes: Map<number, ManagedPaneInternal>,
  paneId: number
): void {
  const pane = panes.get(paneId)
  if (pane) {
    markComplexScriptOutput(pane)
  }
}

export function suspendPaneRendering(panes: Iterable<ManagedPaneInternal>): void {
  for (const pane of panes) {
    pane.webglAttachmentDeferred = true
    disposeWebgl(pane)
  }
}

export function resumePaneRendering(panes: Iterable<ManagedPaneInternal>): void {
  for (const pane of panes) {
    pane.webglAttachmentDeferred = false
    reattachWebglIfNeeded(pane)
  }
}

// Hidden panes keep receiving (throttled) output, and each one grows toward
// the configured scrollback cap — at the 50k-line default that's tens of MB
// per busy agent, so renderer memory scaled linearly with agent count. While
// hidden, freeze each pane's scrollback at its CURRENT size (floored here):
// growth stops, so a fleet of hidden fresh agents stays small, but nothing
// the user already had on screen is discarded at hide time — reducing the
// cap below the used line count makes xterm drop those rows immediately and
// no re-show can recover them. New hidden output beyond the frozen size
// recycles the oldest lines, the same eviction it would eventually hit at
// the full cap; tmux holds the authoritative history server-side.
export const HIDDEN_PANE_SCROLLBACK_LINES = 2000

export function trimHiddenPaneScrollback(panes: Iterable<ManagedPaneInternal>): void {
  for (const pane of panes) {
    if (pane.configuredScrollback !== undefined) {
      continue // already frozen
    }
    const configured = pane.terminal.options.scrollback ?? 0
    if (configured <= HIDDEN_PANE_SCROLLBACK_LINES) {
      continue
    }
    // Lines currently held above the viewport — the history that must
    // survive the freeze. buffer.normal: the alternate screen has no
    // scrollback, and the freeze must size to the buffer that does.
    const used = Math.max(0, pane.terminal.buffer.normal.length - pane.terminal.rows)
    const frozen = Math.min(configured, Math.max(HIDDEN_PANE_SCROLLBACK_LINES, used))
    if (frozen >= configured) {
      continue // buffer already at the configured cap — nothing to freeze
    }
    pane.configuredScrollback = configured
    pane.terminal.options.scrollback = frozen
  }
}

// Restore BEFORE draining any hidden-output backlog into the terminal, so
// recovered bytes land in the full-size buffer instead of being trimmed.
export function restoreHiddenPaneScrollback(panes: Iterable<ManagedPaneInternal>): void {
  for (const pane of panes) {
    if (pane.configuredScrollback === undefined) {
      continue
    }
    pane.terminal.options.scrollback = pane.configuredScrollback
    pane.configuredScrollback = undefined
  }
}
