// @vitest-environment happy-dom
import React from 'react'
import { act, create, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { getDefaultSettings } from '@/shared/constants'
import type { Repo } from '@/shared/types'

;(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true

const testState = vi.hoisted(() => ({
  platform: { mac: true, windows: false },
  store: {} as Record<string, unknown>
}))

vi.mock('@/store', () => {
  const useAppStore = Object.assign(
    (selector: (state: Record<string, unknown>) => unknown) => selector(testState.store),
    { getState: () => testState.store }
  )
  return { useAppStore }
})

vi.mock('@/components/terminal-pane/pane-helpers', () => ({
  isMacUserAgent: () => testState.platform.mac,
  isWindowsUserAgent: () => testState.platform.windows
}))
vi.mock('@/components/terminal-pane/use-system-prefers-dark', () => ({
  useSystemPrefersDark: () => false
}))
vi.mock('@/hooks/useShortcutLabel', () => ({ useShortcutLabel: () => '⌘,' }))
vi.mock('@/components/confirmation-dialog', () => ({
  useConfirmationDialog: () => async () => true
}))
vi.mock('@/lib/windows-terminal-capabilities', () => ({
  getWindowsTerminalCapabilityOwnerKey: () => 'local',
  useWindowsTerminalCapabilities: () => ({
    isLoading: false,
    wslAvailable: false,
    wslDistros: [],
    pwshAvailable: false,
    gitBashAvailable: false
  })
}))
vi.mock('@/runtime/runtime-hooks-client', () => ({
  checkRuntimeHooks: vi.fn(async () => ({
    hasHooks: false,
    hooks: null,
    mayNeedUpdate: false
  }))
}))
vi.mock('./useGhosttyImport', () => ({ useGhosttyImport: () => ({}) }))

vi.mock('./AgentsPane', () => ({ AgentsPane: () => 'pane:agents' }))
vi.mock('./AccountsPane', () => ({ AccountsPane: () => 'pane:accounts' }))
vi.mock('./McpPane', () => ({ McpPane: () => 'pane:mcp' }))
vi.mock('./OrchestrationPane', () => ({ OrchestrationPane: () => 'pane:orchestration' }))
vi.mock('./BrowserVerificationLoopPane', () => ({
  BrowserVerificationLoopPane: () => 'pane:browser-verification'
}))
vi.mock('./ComputerUsePane', () => ({ ComputerUsePane: () => 'pane:computer-use' }))
vi.mock('./VoicePane', () => ({ VoicePane: () => 'pane:voice' }))
vi.mock('./GeneralPane', () => ({ GeneralPane: () => 'pane:general' }))
vi.mock('./IntegrationsPane', () => ({ IntegrationsPane: () => 'pane:integrations' }))
vi.mock('./GitPane', () => ({ GitPane: () => 'pane:git' }))
vi.mock('./CommitMessageAiPane', () => ({
  CommitMessageAiPane: () => 'pane:commit-message'
}))
vi.mock('./TasksPane', () => ({ TasksPane: () => 'pane:tasks' }))
vi.mock('./TerminalPane', () => ({ TerminalPane: () => 'pane:terminal' }))
vi.mock('./QuickCommandsPane', () => ({ QuickCommandsPane: () => 'pane:quick-commands' }))
vi.mock('./BrowserPane', () => ({ BrowserPane: () => 'pane:browser' }))
vi.mock('./AppearancePane', () => ({ AppearancePane: () => 'pane:appearance' }))
vi.mock('./InputPane', () => ({ InputPane: () => 'pane:input' }))
vi.mock('./NotificationsPane', () => ({ NotificationsPane: () => 'pane:notifications' }))
vi.mock('./ShortcutsPane', () => ({ ShortcutsPane: () => 'pane:shortcuts' }))
vi.mock('../stats/StatsPane', () => ({ StatsPane: () => 'pane:stats' }))
vi.mock('./SshPane', () => ({ SshPane: () => 'pane:ssh' }))
vi.mock('./DeveloperPermissionsPane', () => ({
  DeveloperPermissionsPane: () => 'pane:developer-permissions'
}))
vi.mock('./ExperimentalPane', () => ({ ExperimentalPane: () => 'pane:experimental' }))
vi.mock('./RepositoryPane', () => ({ RepositoryPane: () => 'pane:repository' }))

function textOf(node: ReactTestInstance | string | number): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  return node.children.map((child) => textOf(child as ReactTestInstance | string | number)).join('')
}

function navButton(root: ReactTestInstance, label: string): ReactTestInstance {
  const labels = root.findAllByType('button').map((entry) => textOf(entry).trim())
  const match = root
    .findAllByType('button')
    .filter((entry) => textOf(entry).trim().startsWith(label))
    .sort((left, right) => textOf(left).length - textOf(right).length)[0]
  if (!match) {
    throw new Error(`Settings navigation button not found: ${label}; found ${labels.join(', ')}`)
  }
  return match
}

async function select(root: ReactTestInstance, label: string): Promise<void> {
  await act(async () => {
    navButton(root, label).props.onClick({
      metaKey: false,
      ctrlKey: false,
      shiftKey: false,
      altKey: false
    })
    await Promise.resolve()
  })
}

describe('Settings configuration page mounting', () => {
  beforeEach(() => {
    const repo: Repo = {
      id: 'repo-1',
      path: '/workspace/demo',
      displayName: 'Demo Project',
      badgeColor: '#336699',
      addedAt: 1
    }
    testState.platform.mac = true
    testState.platform.windows = false
    ;(window as unknown as { __AGENTUM_WEB_CLIENT__?: boolean }).__AGENTUM_WEB_CLIENT__ = false
    testState.store = {
      settings: getDefaultSettings('/tmp'),
      keybindings: [],
      repos: [repo],
      settingsNavigationTarget: null,
      settingsSearchInputQuery: '',
      settingsSearchQuery: '',
      updateSettings: vi.fn(async () => undefined),
      fetchSettings: vi.fn(async () => undefined),
      fetchKeybindings: vi.fn(async () => undefined),
      closeSettingsPage: vi.fn(),
      updateRepo: vi.fn(async () => undefined),
      removeProject: vi.fn(async () => undefined),
      clearSettingsTarget: vi.fn(),
      setSettingsSearchQuery: vi.fn((query: string) => {
        testState.store.settingsSearchInputQuery = query
        testState.store.settingsSearchQuery = query
      })
    }
    ;(window as unknown as { api: Record<string, unknown> }).api = {
      settings: { listFonts: vi.fn(async () => []) }
    }
  })

  it('mounts every desktop and per-project configuration page from its sidebar entry', async () => {
    const { default: Settings } = await import('./Settings')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(<Settings />)
    })

    const pages: Array<[string, string]> = [
      ['Agents', 'pane:agents'],
      ['AI Provider Accounts', 'pane:accounts'],
      ['Agents & Automation', 'pane:mcp'],
      ['Voice', 'pane:voice'],
      ['General', 'pane:general'],
      ['Integrations', 'pane:integrations'],
      ['Git & Source Control', 'pane:git'],
      ['Task Sources', 'pane:tasks'],
      ['Terminal', 'pane:terminal'],
      ['Quick Commands', 'pane:quick-commands'],
      ['Browser', 'pane:browser'],
      ['Appearance', 'pane:appearance'],
      ['Input & Editing', 'pane:input'],
      ['Notifications', 'pane:notifications'],
      ['Shortcuts', 'pane:shortcuts'],
      ['Stats & Usage', 'pane:stats'],
      ['SSH Hosts', 'pane:ssh'],
      ['macOS Permissions', 'pane:developer-permissions'],
      ['Experimental', 'pane:experimental'],
      ['Demo Project', 'pane:repository']
    ]

    for (const [label, marker] of pages) {
      await select(renderer!.root, label)
      expect(textOf(renderer!.root)).toContain(marker)
    }

    await select(renderer!.root, 'Agents & Automation')
    expect(textOf(renderer!.root)).toContain('pane:orchestration')
    expect(textOf(renderer!.root)).toContain('pane:browser-verification')
    expect(textOf(renderer!.root)).toContain('pane:computer-use')

    await select(renderer!.root, 'Git & Source Control')
    expect(textOf(renderer!.root)).toContain('pane:commit-message')
    await act(async () => renderer!.unmount())
  })

  it('mounts the web-safe configuration set without desktop-only controls', async () => {
    ;(window as unknown as { __AGENTUM_WEB_CLIENT__?: boolean }).__AGENTUM_WEB_CLIENT__ = true
    testState.platform.mac = false
    const { default: Settings } = await import('./Settings')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(<Settings />)
    })

    for (const label of ['Voice', 'Browser', 'Notifications', 'SSH Hosts', 'macOS Permissions']) {
      expect(
        renderer!.root
          .findAllByType('button')
          .some((entry) => textOf(entry).trim().startsWith(label))
      ).toBe(false)
    }
    await select(renderer!.root, 'Agents & Automation')
    expect(textOf(renderer!.root)).toContain('pane:mcp')
    expect(textOf(renderer!.root)).toContain('pane:orchestration')
    expect(textOf(renderer!.root)).toContain('pane:browser-verification')
    expect(textOf(renderer!.root)).not.toContain('pane:computer-use')
    await act(async () => renderer!.unmount())
  })
})
