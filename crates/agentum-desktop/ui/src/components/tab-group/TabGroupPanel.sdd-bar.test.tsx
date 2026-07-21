// Regression pin for issue #313's invisibility bugs: the SDD bar must render
// in the SPLIT-GROUP panel — the render path real workspaces actually use.
// v0.72.0/v0.72.1 shipped the bar only inside Terminal.tsx's legacy no-layout
// fallback, which never renders once a worktree has a root group (always),
// so the feature was invisible despite green builds. This test renders the
// real TabGroupPanel and asserts the buttons are in its markup.
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import type { TerminalTab } from '../../../../shared/types'

const agentForTab = vi.fn<() => string | null>(() => 'claude')

vi.mock('@/lib/use-tab-agent', () => ({
  useTabAgent: () => agentForTab()
}))

vi.mock('@dnd-kit/core', () => ({
  useDroppable: () => ({ setNodeRef: () => {} })
}))

vi.mock('../tab-bar/TabBar', () => ({ default: () => null }))
vi.mock('../tab-bar/TabBarQuickCommandsButton', () => ({
  TabBarQuickCommandsButton: () => null
}))
vi.mock('../terminal-pane/CloseTerminalDialog', () => ({ default: () => null }))
vi.mock('../terminal-pane/use-running-terminal-close-guard', () => ({
  useRunningTerminalCloseGuard: () => ({
    requestClose: () => {},
    dialog: {
      open: false,
      dontAskAgain: false,
      onDontAskAgainChange: () => {},
      onCancel: () => {},
      onConfirm: () => {}
    }
  })
}))

// No network at render time: the loop snapshot/events wiring lives in effects,
// which renderToStaticMarkup never runs.
vi.mock('@/runtime/sdd-client', () => ({
  listSddPlaybooks: vi.fn(),
  injectSddPlaybook: vi.fn(),
  getSddLoop: vi.fn(),
  setSddLoop: vi.fn()
}))
vi.mock('@/runtime/server-events-bus', () => ({
  subscribeServerEvents: () => () => {}
}))

const TAB: TerminalTab = {
  id: 'tab-1',
  ptyId: null,
  worktreeId: 'wt-1',
  title: '✳ Make /sdd commands',
  customTitle: null,
  color: null,
  sortOrder: 0,
  createdAt: 1
}

const STORE_STATE = {
  rightSidebarOpen: false,
  sidebarOpen: true,
  // The bar resolves its server session from the pane's registered ptyId.
  ptyIdsByTabId: { 'tab-1': ['server:0a1b2c3d:leaf-1'] }
}

vi.mock('@/store', () => ({
  useAppStore: (selector: (s: typeof STORE_STATE) => unknown) => selector(STORE_STATE)
}))

const commands = new Proxy({}, { get: () => () => {} })

vi.mock('./useTabGroupWorkspaceModel', () => ({
  useTabGroupWorkspaceModel: () => ({
    activeTab: { id: 'unified-1', entityId: 'tab-1', contentType: 'terminal' },
    browserItems: [],
    commands,
    editorItems: [],
    tabBarOrder: [],
    terminalTabs: [TAB],
    groupTabs: [],
    expandedPaneByTabId: {}
  })
}))

async function renderPanel(): Promise<string> {
  const { default: TabGroupPanel } = await import('./TabGroupPanel')
  return renderToStaticMarkup(
    <TabGroupPanel
      groupId="group-1"
      worktreeId="wt-1"
      isFocused={false}
      hasSplitGroups={false}
      touchesRightEdge={true}
      touchesLeftEdge={true}
      reserveClosedExplorerToggleSpace={false}
      reserveCollapsedSidebarHeaderSpace={false}
    />
  )
}

describe('TabGroupPanel SDD bar (issue #313)', () => {
  it('renders the SDD buttons for a terminal tab running an agent', async () => {
    agentForTab.mockReturnValue('claude')
    const html = await renderPanel()
    for (const label of ['Spec', 'Spec Socratic', 'Continue', 'Status', 'Loop']) {
      expect(html).toContain(label)
    }
  })

  it('renders no SDD bar when the tab is a plain shell', async () => {
    agentForTab.mockReturnValue(null)
    const html = await renderPanel()
    expect(html).not.toContain('Spec Socratic')
    expect(html).not.toContain('SDD')
  })

  // Issue #349: the bar can be dismissed (X) and restored from a slim chip.
  it('offers a dismiss control on the expanded bar', async () => {
    agentForTab.mockReturnValue('claude')
    const html = await renderPanel()
    expect(html).toContain('Hide the SDD bar')
  })

  it('collapses to the restore chip when the preference is set', async () => {
    agentForTab.mockReturnValue('claude')
    const store: Record<string, string> = { agentum_sdd_bar_collapsed: '1' }
    vi.stubGlobal('localStorage', {
      getItem: (key: string) => store[key] ?? null,
      setItem: (key: string, value: string) => {
        store[key] = value
      },
      removeItem: (key: string) => {
        delete store[key]
      }
    })
    try {
      const html = await renderPanel()
      expect(html).not.toContain('Spec Socratic')
      expect(html).toContain('Show the SDD bar')
      // The restore chip must be discoverable, not a near-invisible ghost: a
      // visible label brings the bar back (issue #349 follow-up — a dismissed
      // bar was effectively unfindable).
      expect(html).toContain('Show bar')
    } finally {
      vi.unstubAllGlobals()
    }
  })
})
