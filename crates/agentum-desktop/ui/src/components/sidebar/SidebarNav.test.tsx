import { describe, expect, it } from 'vitest'
import { getDefaultSettings } from '../../../../shared/constants'
import { navIconClass, shouldShowAgentsButton } from './SidebarNav'

describe('navIconClass', () => {
  it('gives the active nav item the accent token', () => {
    expect(navIconClass(true)).toBe('text-sidebar-accent-foreground')
  })

  it('gives inactive nav items the muted-monochrome token', () => {
    expect(navIconClass(false)).toBe('text-sidebar-foreground/40')
  })

  it('resolves every nav icon to one resting color and one accent — no per-item variance', () => {
    // The whole point of the rework: inactive icons must all share a single class,
    // so a future entry can't reintroduce color noise by hardcoding its own shade.
    const inactive = ['activity', 'harness', 'projects', 'tasks', 'search'].map(() =>
      navIconClass(false)
    )
    expect(new Set(inactive).size).toBe(1)
    expect(navIconClass(true)).not.toBe(navIconClass(false))
  })

  it('uses theme-variable-backed classes, never a hardcoded hex/color (light+dark safe)', () => {
    for (const cls of [navIconClass(true), navIconClass(false)]) {
      expect(cls).toMatch(/^text-sidebar-/)
      expect(cls).not.toMatch(/#|rgb|blue|green|amber|red|purple|emerald|sky|violet/)
    }
  })
})

describe('SidebarNav', () => {
  it('hides the Agents entry while settings are loading', () => {
    expect(shouldShowAgentsButton(null)).toBe(false)
  })

  it('hides the Agents entry while the experimental Agents view is off', () => {
    expect(
      shouldShowAgentsButton({
        ...getDefaultSettings('/tmp'),
        experimentalActivity: false
      })
    ).toBe(false)
  })

  it('shows the Agents entry when the experimental Agents view is on', () => {
    expect(
      shouldShowAgentsButton({
        ...getDefaultSettings('/tmp'),
        experimentalActivity: true
      })
    ).toBe(true)
  })
})
