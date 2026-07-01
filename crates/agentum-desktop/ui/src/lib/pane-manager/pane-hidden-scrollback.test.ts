import { describe, expect, it } from 'vitest'
import {
  HIDDEN_PANE_SCROLLBACK_LINES,
  restoreHiddenPaneScrollback,
  trimHiddenPaneScrollback
} from './pane-rendering-control'
import type { ManagedPaneInternal } from './pane-manager-types'

// The xterm side (option change → buffer trim) is upstream behavior; these
// tests pin the freeze/restore state machine — freeze at the CURRENT buffer
// size (never discarding held history), stash-once, restore-once, and never
// shrinking a pane already at or below the floor.
const makePane = (scrollback: number, usedLines = 0, rows = 40): ManagedPaneInternal =>
  ({
    terminal: {
      options: { scrollback },
      rows,
      buffer: { normal: { length: usedLines + rows } }
    }
  }) as unknown as ManagedPaneInternal

describe('hidden pane scrollback freeze', () => {
  it('freezes an above-floor pane at the floor when its buffer is small', () => {
    const pane = makePane(50_000, 100)
    trimHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(HIDDEN_PANE_SCROLLBACK_LINES)
    expect(pane.configuredScrollback).toBe(50_000)

    restoreHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(50_000)
    expect(pane.configuredScrollback).toBeUndefined()
  })

  it('never freezes below the lines already held — pre-hide history survives', () => {
    // 30k lines accumulated while visible: the freeze must sit AT 30k, not
    // at the floor — a lower cap would make xterm discard on-screen history
    // the user already had, unrecoverable on re-show.
    const pane = makePane(50_000, 30_000)
    trimHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(30_000)
    expect(pane.configuredScrollback).toBe(50_000)

    restoreHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(50_000)
  })

  it('skips panes whose buffer already reached the configured cap', () => {
    const pane = makePane(50_000, 50_000)
    trimHiddenPaneScrollback([pane])
    // Freezing at the cap would be a no-op — don't stash, don't touch.
    expect(pane.terminal.options.scrollback).toBe(50_000)
    expect(pane.configuredScrollback).toBeUndefined()
  })

  it('leaves panes configured at or below the floor untouched', () => {
    const pane = makePane(HIDDEN_PANE_SCROLLBACK_LINES)
    trimHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(HIDDEN_PANE_SCROLLBACK_LINES)
    expect(pane.configuredScrollback).toBeUndefined()

    // Restore on an unfrozen pane is a no-op, not a clobber.
    restoreHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(HIDDEN_PANE_SCROLLBACK_LINES)
  })

  it('a second freeze does not overwrite the stashed configured value', () => {
    const pane = makePane(10_000, 0)
    trimHiddenPaneScrollback([pane])
    // e.g. suspendRendering firing again while already hidden.
    trimHiddenPaneScrollback([pane])
    expect(pane.configuredScrollback).toBe(10_000)

    restoreHiddenPaneScrollback([pane])
    expect(pane.terminal.options.scrollback).toBe(10_000)
  })
})
