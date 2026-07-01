import { describe, expect, it } from 'vitest'
import {
  HIDDEN_PANE_SCROLLBACK_LINES,
  restoreHiddenPaneScrollback,
  trimHiddenPaneScrollback
} from './pane-rendering-control'
import type { ManagedPaneInternal } from './pane-manager-types'

// The xterm side (option change → buffer trim) is upstream behavior; these
// tests pin the trim/restore state machine — stash-once, restore-once, and
// never shrinking a pane already at or below the floor.
const makePane = (scrollback: number): ManagedPaneInternal =>
  ({ terminal: { options: { scrollback } } }) as unknown as ManagedPaneInternal

describe('hidden pane scrollback trim', () => {
  it('trims above-floor panes and restores the configured value', () => {
    const pane = makePane(50_000)
    trimHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(HIDDEN_PANE_SCROLLBACK_LINES)
    expect(pane.configuredScrollback).toBe(50_000)

    restoreHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(50_000)
    expect(pane.configuredScrollback).toBeUndefined()
  })

  it('leaves panes at or below the floor untouched', () => {
    const pane = makePane(HIDDEN_PANE_SCROLLBACK_LINES)
    trimHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(HIDDEN_PANE_SCROLLBACK_LINES)
    expect(pane.configuredScrollback).toBeUndefined()

    // Restore on an untrimmed pane is a no-op, not a clobber.
    restoreHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(HIDDEN_PANE_SCROLLBACK_LINES)
  })

  it('a second trim does not overwrite the stashed configured value', () => {
    const pane = makePane(10_000)
    trimHiddenPaneScrollback([pane])
    // e.g. suspendRendering firing again while already hidden.
    trimHiddenPaneScrollback([pane])
    expect(pane.configuredScrollback).toBe(10_000)

    restoreHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(10_000)
  })
})
