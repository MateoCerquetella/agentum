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
// per busy agent, so renderer memory scaled linearly with agent count. Trim
// hidden panes to this floor; xterm trims the buffer immediately on the
// option change. Only history that scrolled past this window while the pane
// was hidden is lost — content the user never had on screen, and tmux holds
// the authoritative history server-side. 2000 lines ≈ 2 MB/pane and matches
// what the 512 KB shutdown-capture cap can persist anyway.
export const HIDDEN_PANE_SCROLLBACK_LINES = 2000

export function trimHiddenPaneScrollback(panes: Iterable<ManagedPaneInternal>): void {
  for (const pane of panes) {
    if (pane.configuredScrollback !== undefined) {
      continue // already trimmed
    }
    const configured = pane.terminal.options.scrollback ?? 0
    if (configured <= HIDDEN_PANE_SCROLLBACK_LINES) {
      continue
    }
    pane.configuredScrollback = configured
    pane.terminal.options.scrollback = HIDDEN_PANE_SCROLLBACK_LINES
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
