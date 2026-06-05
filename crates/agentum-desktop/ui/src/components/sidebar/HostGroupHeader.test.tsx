import React, { isValidElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, it, expect, vi } from 'vitest'
import { HostGroupHeader } from './HostGroupHeader'
import type { SidebarHost } from './worktree-list-groups'

type InspectableProps = {
  children?: React.ReactNode
  role?: string
  onClick?: () => void
}

function findElement(
  node: React.ReactNode,
  predicate: (props: InspectableProps) => boolean
): React.ReactElement<InspectableProps> | null {
  if (!isValidElement<InspectableProps>(node)) {
    return null
  }
  if (predicate(node.props)) {
    return node
  }
  let match: React.ReactElement<InspectableProps> | null = null
  React.Children.forEach(node.props.children, (child) => {
    if (match) {
      return
    }
    match = findElement(child, predicate)
  })
  return match
}

describe('HostGroupHeader', () => {
  it('renders label, detail line and count', () => {
    const host: SidebarHost = {
      key: 'local',
      kind: 'local',
      label: 'studio',
      detail: 'localhost · Darwin 24.5',
      status: 'reachable'
    }
    const markup = renderToStaticMarkup(
      React.createElement(HostGroupHeader, { host, count: 3, collapsed: false, onToggle: () => {} })
    )
    expect(markup).toContain('studio')
    expect(markup).toContain('localhost · Darwin 24.5')
    expect(markup).toContain('3')
  })

  it('fires onToggle on click', () => {
    const onToggle = vi.fn()
    const host: SidebarHost = { key: 'local', kind: 'local', label: 'studio' }
    // Invoke the component to get its rendered tree (the role="button" root);
    // `createElement(HostGroupHeader, …)` would only expose the component's input
    // props, not its output, so findElement could never reach the clickable div.
    const element = HostGroupHeader({ host, count: 0, collapsed: true, onToggle })
    const button = findElement(element, (props) => props.role === 'button')
    expect(button).not.toBeNull()
    button?.props.onClick?.()
    expect(onToggle).toHaveBeenCalledOnce()
  })
})
