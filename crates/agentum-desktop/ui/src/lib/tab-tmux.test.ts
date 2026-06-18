import { describe, it, expect } from 'vitest'
import { isTabTmuxBacked } from './tab-tmux'

const LEAF_A = '11111111-1111-4111-8111-111111111111'
const LEAF_B = '22222222-2222-4222-8222-222222222222'

describe('isTabTmuxBacked', () => {
  it('is false when no panes are recorded as tmux-backed', () => {
    expect(isTabTmuxBacked({}, 'tab-1')).toBe(false)
  })

  it('is true when a pane of this tab is tmux-backed', () => {
    const map = { [`tab-1:${LEAF_A}`]: true } as Record<string, true>
    expect(isTabTmuxBacked(map, 'tab-1')).toBe(true)
  })

  it('is false for a local PTY tab whose paneKey is absent from the map', () => {
    // A local PTY pane never records itself, so its tab stays icon-less even
    // when OTHER tabs are tmux-backed. This is the regression guard against the
    // previous persistTmux-based false positive.
    const map = { [`tmux-tab:${LEAF_A}`]: true } as Record<string, true>
    expect(isTabTmuxBacked(map, 'pty-tab')).toBe(false)
  })

  it('does not match on a tabId that is a prefix of another tabId', () => {
    // 'tab-1' must not match a pane belonging to 'tab-10' — the ':' separator
    // is part of the prefix so substring collisions cannot leak the icon.
    const map = { [`tab-10:${LEAF_A}`]: true } as Record<string, true>
    expect(isTabTmuxBacked(map, 'tab-1')).toBe(false)
  })

  it('is true when ANY of a tab’s split panes is tmux-backed', () => {
    const map = {
      [`tab-1:${LEAF_A}`]: true,
      [`tab-1:${LEAF_B}`]: true
    } as Record<string, true>
    expect(isTabTmuxBacked(map, 'tab-1')).toBe(true)
  })
})
