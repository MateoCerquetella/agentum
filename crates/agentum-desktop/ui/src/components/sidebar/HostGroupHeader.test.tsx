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

  it('renders the tmux-sessions affordance and fires onOpenTmuxSessions without toggling', () => {
    const onToggle = vi.fn()
    const onOpenTmuxSessions = vi.fn()
    const host: SidebarHost = { key: 'ssh:conn-1', kind: 'ssh', label: 'omarchy' }
    const element = HostGroupHeader({
      host,
      count: 2,
      collapsed: false,
      onToggle,
      onOpenTmuxSessions
    })
    const action = findElement(
      element,
      (props) => (props as { 'aria-label'?: string })['aria-label'] === 'Tmux sessions on omarchy'
    )
    expect(action).not.toBeNull()
    const onClick = action?.props.onClick as unknown as
      | ((event: { stopPropagation: () => void }) => void)
      | undefined
    onClick?.({ stopPropagation: () => {} })
    expect(onOpenTmuxSessions).toHaveBeenCalledOnce()
    // Clicking the tmux action must not collapse the host section.
    expect(onToggle).not.toHaveBeenCalled()
  })

  it('omits the tmux-sessions affordance when no handler is provided', () => {
    const host: SidebarHost = { key: 'local', kind: 'local', label: 'studio' }
    const markup = renderToStaticMarkup(
      React.createElement(HostGroupHeader, {
        host,
        count: 1,
        collapsed: false,
        onToggle: () => {}
      })
    )
    expect(markup).not.toContain('Tmux sessions')
  })

  it('replaces the detail line with an unreachable line + Reconnect when the ssh host is down', () => {
    const host: SidebarHost = {
      key: 'ssh:conn-1',
      kind: 'ssh',
      label: 'freebee',
      detail: 'ssh · Linux 6.9',
      status: 'down',
      sshStatus: 'error',
      connectionId: 'conn-1'
    }
    const markup = renderToStaticMarkup(
      React.createElement(HostGroupHeader, {
        host,
        count: 1,
        collapsed: false,
        onToggle: () => {},
        onReconnect: () => {}
      })
    )
    expect(markup).toContain('Host unreachable')
    expect(markup).toContain('Reconnect')
    // The transport line takes the detail line's slot while the host is down.
    expect(markup).not.toContain('ssh · Linux 6.9')
  })

  it('labels a deliberate disconnect as Disconnected, not unreachable', () => {
    const host: SidebarHost = {
      key: 'ssh:conn-1',
      kind: 'ssh',
      label: 'freebee',
      status: 'down',
      sshStatus: 'disconnected',
      connectionId: 'conn-1'
    }
    const markup = renderToStaticMarkup(
      React.createElement(HostGroupHeader, {
        host,
        count: 1,
        collapsed: false,
        onToggle: () => {},
        onReconnect: () => {}
      })
    )
    expect(markup).toContain('Disconnected')
    expect(markup).not.toContain('Host unreachable')
  })

  it('shows the transport label while reconnecting and hides the Reconnect action', () => {
    const host: SidebarHost = {
      key: 'ssh:conn-1',
      kind: 'ssh',
      label: 'freebee',
      status: 'connecting',
      sshStatus: 'reconnecting',
      connectionId: 'conn-1'
    }
    const markup = renderToStaticMarkup(
      React.createElement(HostGroupHeader, {
        host,
        count: 1,
        collapsed: false,
        onToggle: () => {},
        onReconnect: () => {}
      })
    )
    expect(markup).toContain('Reconnecting')
    expect(markup).not.toContain('>Reconnect<')
  })

  it('fires onReconnect without toggling the section', () => {
    const onToggle = vi.fn()
    const onReconnect = vi.fn()
    const host: SidebarHost = {
      key: 'ssh:conn-1',
      kind: 'ssh',
      label: 'freebee',
      status: 'down',
      sshStatus: 'reconnection-failed',
      connectionId: 'conn-1'
    }
    const element = HostGroupHeader({
      host,
      count: 1,
      collapsed: false,
      onToggle,
      onReconnect
    })
    const action = findElement(
      element,
      (props) => (props as { 'aria-label'?: string })['aria-label'] === 'Reconnect to freebee'
    )
    expect(action).not.toBeNull()
    const onClick = action?.props.onClick as unknown as
      | ((event: { stopPropagation: () => void }) => void)
      | undefined
    onClick?.({ stopPropagation: () => {} })
    expect(onReconnect).toHaveBeenCalledOnce()
    expect(onToggle).not.toHaveBeenCalled()
  })

  it('keeps the plain detail line when the ssh host is reachable', () => {
    const host: SidebarHost = {
      key: 'ssh:conn-1',
      kind: 'ssh',
      label: 'freebee',
      detail: 'ssh · Linux 6.9',
      status: 'reachable',
      sshStatus: 'connected',
      connectionId: 'conn-1'
    }
    const markup = renderToStaticMarkup(
      React.createElement(HostGroupHeader, {
        host,
        count: 1,
        collapsed: false,
        onToggle: () => {},
        onReconnect: () => {}
      })
    )
    expect(markup).toContain('ssh · Linux 6.9')
    expect(markup).not.toContain('Host unreachable')
    expect(markup).not.toContain('Reconnect')
  })
})
