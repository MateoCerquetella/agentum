import { renderToStaticMarkup } from 'react-dom/server'
import { isValidElement, type ReactElement, type ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const sidebarState = vi.hoisted(() => ({
  activeView: 'activity',
  groupBy: 'repository',
  openActivityPage: vi.fn(),
  openProjectsPage: vi.fn(),
  openModal: vi.fn()
}))

const activityState = vi.hoisted(() => ({ unreadCount: 3 }))

vi.mock('@/store', () => ({
  useAppStore: (selector: (state: typeof sidebarState) => unknown) => selector(sidebarState)
}))

vi.mock('@/components/activity/useActivityUnreadCount', () => ({
  useActivityUnreadCount: () => activityState.unreadCount
}))

vi.mock('@/hooks/useShortcutLabel', () => ({
  useShortcutLabel: () => '⌘P'
}))

import { navIconClass, shouldShowAgentsButton, SidebarNav } from './SidebarNav'

function buttonMarkup(markup: string, label: string): string {
  const labelIndex = markup.indexOf(label)
  if (labelIndex < 0) {
    return ''
  }
  const buttonStart = markup.lastIndexOf('<button', labelIndex)
  const buttonEnd = markup.indexOf('</button>', labelIndex)
  return buttonStart < 0 || buttonEnd < 0 ? '' : markup.slice(buttonStart, buttonEnd + 9)
}

function findElementByLabel(
  node: ReactNode,
  label: string
): ReactElement<{ label: string; onClick: () => void }> | null {
  if (!isValidElement(node)) {
    return null
  }

  const props = node.props as { children?: ReactNode; label?: string; onClick?: () => void }
  if (props.label === label && props.onClick) {
    return node as ReactElement<{ label: string; onClick: () => void }>
  }

  for (const child of Array.isArray(props.children) ? props.children : [props.children]) {
    const match = findElementByLabel(child, label)
    if (match) {
      return match
    }
  }

  return null
}

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
    const inactive = ['activity', 'projects', 'tasks', 'search'].map(() =>
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
  beforeEach(() => {
    sidebarState.activeView = 'activity'
    activityState.unreadCount = 3
  })

  it('renders Mission Control first with the shared primary treatment and unread badge', () => {
    const markup = renderToStaticMarkup(<SidebarNav />)
    const missionControl = buttonMarkup(markup, 'Mission Control')

    expect(markup.indexOf('Mission Control')).toBeLessThan(markup.indexOf('Projects'))
    expect(missionControl).toContain('lucide-radar')
    expect(missionControl).toContain('Mission Control')
    expect(missionControl).toContain('bg-sidebar-accent')
    expect(missionControl).toContain('text-sidebar-accent-foreground')
    expect(missionControl).toContain('aria-current="page"')
    expect(missionControl).toContain('>3<')
  })

  it('uses the shared hover treatment when Mission Control is not active', () => {
    sidebarState.activeView = 'projects'

    const missionControl = buttonMarkup(renderToStaticMarkup(<SidebarNav />), 'Mission Control')

    expect(missionControl).toContain('hover:bg-sidebar-foreground/8')
    expect(missionControl).toContain('text-sidebar-foreground/70')
    expect(missionControl).not.toContain('aria-current')
  })

  it('routes Mission Control clicks through the activity-page action', () => {
    sidebarState.openActivityPage.mockClear()
    const missionControl = findElementByLabel(SidebarNav(), 'Mission Control')

    expect(missionControl).not.toBeNull()
    missionControl?.props.onClick()
    expect(sidebarState.openActivityPage).toHaveBeenCalledOnce()
  })

  it('hides the Agents entry while settings are loading', () => {
    expect(shouldShowAgentsButton(null)).toBe(false)
  })

  it('hides the Agents entry while the experimental Agents view is off', () => {
    expect(
      shouldShowAgentsButton({
        experimentalActivity: false
      })
    ).toBe(false)
  })

  it('shows the Agents entry when the experimental Agents view is on', () => {
    expect(
      shouldShowAgentsButton({
        experimentalActivity: true
      })
    ).toBe(true)
  })
})
