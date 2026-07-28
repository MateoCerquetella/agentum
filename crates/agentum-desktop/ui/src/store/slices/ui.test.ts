/* eslint-disable max-lines */
import { createStore, type StoreApi } from 'zustand/vanilla'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { getDefaultUIState } from '@/shared/constants'
import type {
  GitHubWorkItem,
  PersistedUIState,
  Worktree,
  WorktreeCardProperty
} from '@/shared/types'
import { createUISlice, normalizePersistedGroupBy } from './ui'
import { createWorktreeNavHistorySlice } from './worktree-nav-history'
import { createSettingsSearchState } from './settings-search-state'
import type { AppState } from '../types'
import type { FeatureInteractionState } from '@/shared/feature-interactions'
import { makePaneKey } from '@/shared/stable-pane-id'

const mocks = vi.hoisted(() => ({
  sendBracketedPasteToRunningAgent: vi.fn(),
  submitPromptToAgentTab: vi.fn(),
  activateTabAndFocusPane: vi.fn(),
  track: vi.fn(),
  toastMessage: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn()
}))

vi.mock('@/lib/agent-paste-draft', () => ({
  sendBracketedPasteToRunningAgent: mocks.sendBracketedPasteToRunningAgent,
  submitPromptToAgentTab: mocks.submitPromptToAgentTab
}))

vi.mock('@/lib/activate-tab-and-focus-pane', () => ({
  activateTabAndFocusPane: mocks.activateTabAndFocusPane
}))

vi.mock('@/lib/telemetry', () => ({
  track: mocks.track
}))

vi.mock('sonner', () => ({
  toast: {
    message: mocks.toastMessage,
    success: mocks.toastSuccess,
    error: mocks.toastError
  }
}))

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

beforeEach(() => {
  mocks.sendBracketedPasteToRunningAgent.mockReset()
  mocks.submitPromptToAgentTab.mockReset()
  mocks.activateTabAndFocusPane.mockReset()
  mocks.track.mockReset()
  mocks.toastMessage.mockReset()
  mocks.toastSuccess.mockReset()
  mocks.toastError.mockReset()
})

function createUIStore(): StoreApi<AppState> {
  // Only the UI slice, repo/worktree ids, and right sidebar width fallback are
  // needed for these tests. The worktree-nav-history slice is also included
  // because page opens record view visits.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return createStore<any>()((...args: any[]) => ({
    repos: [],
    worktreesByRepo: {},
    activeRepoId: null,
    projectBindingByRepo: {},
    setActiveRepo: (repoId: string | null) => args[0]({ activeRepoId: repoId }),
    rightSidebarOpen: false,
    rightSidebarWidth: 280,
    ...createSettingsSearchState(args[0]),
    ...createWorktreeNavHistorySlice(...(args as Parameters<typeof createWorktreeNavHistorySlice>)),
    ...createUISlice(...(args as Parameters<typeof createUISlice>))
  })) as unknown as StoreApi<AppState>
}

function makeWorktree(id: string): Worktree {
  return { id } as unknown as Worktree
}

function makeGitHubWorkItem(overrides: Partial<GitHubWorkItem> = {}): GitHubWorkItem {
  return {
    id: 'pr-95',
    type: 'pr',
    number: 95,
    title: 'feat: add file upload command',
    state: 'open',
    url: 'https://github.com/acme/repo/pull/95',
    labels: [],
    updatedAt: '2026-05-20T00:00:00.000Z',
    author: 'octocat',
    repoId: 'repo-1',
    ...overrides
  }
}

function makePersistedUI(overrides: Partial<PersistedUIState> = {}): PersistedUIState {
  return {
    ...getDefaultUIState(),
    ...overrides
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((res) => {
    resolve = res
  })
  return { promise, resolve }
}

describe('createUISlice agent send target mode', () => {
  const worktreeId = 'wt-1'
  const tabId = 'tab-1'
  const readyLeafId = '11111111-1111-4111-8111-111111111111'
  const workingLeafId = '22222222-2222-4222-8222-222222222222'
  const readyPaneKey = makePaneKey(tabId, readyLeafId)
  const workingPaneKey = makePaneKey(tabId, workingLeafId)

  function seedAgentSendState(store: StoreApi<AppState>): void {
    const now = Date.now()
    store.setState({
      tabsByWorktree: {
        [worktreeId]: [
          {
            id: tabId,
            worktreeId,
            ptyId: 'fallback-pty',
            title: 'Terminal 1',
            customTitle: null,
            color: null,
            sortOrder: 0,
            createdAt: now
          }
        ]
      },
      terminalLayoutsByTabId: {
        [tabId]: {
          root: {
            type: 'split',
            direction: 'vertical',
            first: { type: 'leaf', leafId: readyLeafId },
            second: { type: 'leaf', leafId: workingLeafId }
          },
          activeLeafId: readyLeafId,
          expandedLeafId: null,
          ptyIdsByLeafId: {
            [readyLeafId]: 'pty-ready',
            [workingLeafId]: 'pty-working'
          }
        }
      },
      agentStatusByPaneKey: {
        [readyPaneKey]: {
          state: 'done',
          prompt: 'previous',
          updatedAt: now,
          stateStartedAt: now,
          agentType: 'codex',
          paneKey: readyPaneKey,
          stateHistory: []
        },
        [workingPaneKey]: {
          state: 'working',
          prompt: 'busy',
          updatedAt: now,
          stateStartedAt: now,
          agentType: 'codex',
          paneKey: workingPaneKey,
          stateHistory: []
        }
      }
    } as Partial<AppState>)
  }

  it('opens target mode with derived eligible and disabled pane keys', () => {
    const store = createUIStore()
    seedAgentSendState(store)

    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'diff-notes',
      prompt: 'Review this',
      label: 'All unsent notes',
      launchSource: 'notes_send'
    })

    expect(store.getState().agentSendPopoverTargetMode).toMatchObject({
      id: 'send-1',
      eligiblePaneKeys: [readyPaneKey],
      disabledPaneKeys: {
        [workingPaneKey]: 'Agent is working'
      },
      status: 'open'
    })
    expect(store.getState().pendingRevealWorktree).toMatchObject({
      worktreeId,
      behavior: 'auto',
      highlight: true
    })
  })

  it('does not reveal the sidebar when the current workspace has no eligible targets', () => {
    const store = createUIStore()
    seedAgentSendState(store)
    // Make every agent ineligible (mid-turn) so there is genuinely nothing to
    // send to. A tabless-but-idle agent is now eligible (the send path activates
    // its tab), so "no eligible targets" needs an actually-unsendable state.
    const now = Date.now()
    store.setState({
      agentStatusByPaneKey: {
        [readyPaneKey]: {
          state: 'working',
          prompt: 'busy',
          updatedAt: now,
          stateStartedAt: now,
          agentType: 'codex',
          paneKey: readyPaneKey,
          stateHistory: []
        },
        [workingPaneKey]: {
          state: 'working',
          prompt: 'busy',
          updatedAt: now,
          stateStartedAt: now,
          agentType: 'codex',
          paneKey: workingPaneKey,
          stateHistory: []
        }
      }
    } as Partial<AppState>)

    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'browser-annotations',
      prompt: 'Review this',
      label: 'Browser annotations',
      launchSource: 'notes_send'
    })

    expect(store.getState().agentSendPopoverTargetMode).toMatchObject({
      id: 'send-1',
      eligiblePaneKeys: [],
      disabledPaneKeys: {
        [readyPaneKey]: 'Agent is working',
        [workingPaneKey]: 'Agent is working'
      }
    })
    expect(store.getState().pendingRevealWorktree).toBeNull()
  })

  it('sends to the live leaf PTY, runs delivery callback, tracks followup, and closes', async () => {
    const store = createUIStore()
    const onPromptDelivered = vi.fn()
    seedAgentSendState(store)
    mocks.sendBracketedPasteToRunningAgent.mockResolvedValue(true)
    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'diff-notes',
      prompt: 'Review this',
      label: 'All unsent notes',
      launchSource: 'notes_send',
      onPromptDelivered
    })

    await expect(store.getState().sendPromptToSidebarAgentTarget(readyPaneKey)).resolves.toBe(true)

    expect(mocks.sendBracketedPasteToRunningAgent).toHaveBeenCalledWith({
      ptyId: 'pty-ready',
      content: 'Review this'
    })
    expect(onPromptDelivered).toHaveBeenCalledTimes(1)
    expect(mocks.track).toHaveBeenCalledWith('agent_prompt_sent', {
      agent_kind: 'codex',
      launch_source: 'notes_send',
      request_kind: 'followup'
    })
    expect(mocks.toastSuccess).toHaveBeenCalledWith('Sent to Codex')
    expect(store.getState().agentSendPopoverTargetMode).toBeNull()
  })

  it('activates the tab and submits when the eligible target has no live PTY', async () => {
    const store = createUIStore()
    seedAgentSendState(store)
    // Drop the live ptys so the ready agent is eligible-but-tabless: the send
    // path must activate its tab, then submit once the pty spawns.
    store.setState({
      terminalLayoutsByTabId: {
        [tabId]: {
          root: {
            type: 'split',
            direction: 'vertical',
            first: { type: 'leaf', leafId: readyLeafId },
            second: { type: 'leaf', leafId: workingLeafId }
          },
          activeLeafId: readyLeafId,
          expandedLeafId: null,
          ptyIdsByLeafId: {}
        }
      }
    } as Partial<AppState>)
    mocks.submitPromptToAgentTab.mockResolvedValue(true)

    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'browser-annotations',
      prompt: 'Review this',
      label: 'Browser annotations',
      launchSource: 'notes_send'
    })

    await expect(store.getState().sendPromptToSidebarAgentTarget(readyPaneKey)).resolves.toBe(true)

    expect(mocks.activateTabAndFocusPane).toHaveBeenCalledWith(tabId, readyLeafId)
    expect(mocks.submitPromptToAgentTab).toHaveBeenCalledWith({
      tabId,
      content: 'Review this'
    })
    expect(mocks.sendBracketedPasteToRunningAgent).not.toHaveBeenCalled()
    expect(mocks.toastSuccess).toHaveBeenCalledWith('Sent to Codex')
    expect(store.getState().agentSendPopoverTargetMode).toBeNull()
  })

  it('keeps target mode open and does not run delivery callback when send fails', async () => {
    const store = createUIStore()
    const onPromptDelivered = vi.fn()
    seedAgentSendState(store)
    mocks.sendBracketedPasteToRunningAgent.mockResolvedValue(false)
    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'diff-notes',
      prompt: 'Review this',
      label: 'All unsent notes',
      launchSource: 'notes_send',
      onPromptDelivered
    })

    await expect(store.getState().sendPromptToSidebarAgentTarget(readyPaneKey)).resolves.toBe(false)

    expect(onPromptDelivered).not.toHaveBeenCalled()
    expect(mocks.track).not.toHaveBeenCalled()
    expect(mocks.toastError).toHaveBeenCalledWith("Couldn't send to Codex", {
      description: 'Terminal is no longer available'
    })
    expect(store.getState().agentSendPopoverTargetMode).toMatchObject({
      id: 'send-1',
      status: 'error',
      error: 'Terminal is no longer available'
    })
  })

  it('does not send to a working agent row', async () => {
    const store = createUIStore()
    seedAgentSendState(store)
    mocks.sendBracketedPasteToRunningAgent.mockResolvedValue(true)
    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'browser-annotations',
      prompt: 'Review this',
      label: 'Browser annotations',
      launchSource: 'notes_send'
    })

    await expect(store.getState().sendPromptToSidebarAgentTarget(workingPaneKey)).resolves.toBe(
      false
    )

    expect(mocks.sendBracketedPasteToRunningAgent).not.toHaveBeenCalled()
    expect(mocks.toastSuccess).not.toHaveBeenCalled()
    expect(store.getState().agentSendPopoverTargetMode).toMatchObject({
      id: 'send-1',
      status: 'open'
    })
  })

  it('does not let an older send close a reopened popover with the same id', async () => {
    const store = createUIStore()
    const write = deferred<boolean>()
    seedAgentSendState(store)
    mocks.sendBracketedPasteToRunningAgent.mockReturnValue(write.promise)
    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'diff-notes',
      prompt: 'Review this',
      label: 'All unsent notes',
      launchSource: 'notes_send'
    })

    const send = store.getState().sendPromptToSidebarAgentTarget(readyPaneKey)
    store.getState().closeAgentSendPopoverTargetMode('send-1')
    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'diff-notes',
      prompt: 'Review this again',
      label: 'All unsent notes',
      launchSource: 'notes_send'
    })
    const reopenedMode = store.getState().agentSendPopoverTargetMode

    write.resolve(true)
    await expect(send).resolves.toBe(true)

    expect(store.getState().agentSendPopoverTargetMode).toBe(reopenedMode)
    expect(store.getState().agentSendPopoverTargetMode).toMatchObject({
      id: 'send-1',
      prompt: 'Review this again',
      status: 'open'
    })
  })

  it('does not retarget the same popover while a send is in progress', async () => {
    const store = createUIStore()
    const write = deferred<boolean>()
    seedAgentSendState(store)
    mocks.sendBracketedPasteToRunningAgent.mockReturnValue(write.promise)
    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'diff-notes',
      prompt: 'Review this',
      label: 'This file',
      launchSource: 'notes_send'
    })

    const send = store.getState().sendPromptToSidebarAgentTarget(readyPaneKey)
    const sendingMode = store.getState().agentSendPopoverTargetMode
    store.getState().openAgentSendPopoverTargetMode({
      id: 'send-1',
      worktreeId,
      source: 'diff-notes',
      prompt: 'Review everything',
      label: 'All unsent notes',
      launchSource: 'notes_send'
    })

    expect(store.getState().agentSendPopoverTargetMode).toBe(sendingMode)
    expect(store.getState().agentSendPopoverTargetMode).toMatchObject({
      id: 'send-1',
      prompt: 'Review this',
      status: 'sending',
      sendingPaneKey: readyPaneKey
    })

    write.resolve(true)
    await expect(send).resolves.toBe(true)
  })
})

describe('createUISlice hydratePersistedUI', () => {
  it('defaults fresh and absent grouping to operational without overwriting explicit choices', () => {
    expect(getDefaultUIState().groupBy).toBe('operational')
    expect(createUIStore().getState().groupBy).toBe('operational')
    expect(normalizePersistedGroupBy(undefined)).toBe('operational')
    expect(normalizePersistedGroupBy('corrupt')).toBe('operational')
    expect(normalizePersistedGroupBy('parent')).toBe('host')
    for (const explicit of [
      'operational',
      'host',
      'repo',
      'workspace-status',
      'pr-status',
      'none'
    ] as const) {
      expect(normalizePersistedGroupBy(explicit)).toBe(explicit)
    }
  })

  it('defaults persisted right sidebar visibility to open', () => {
    expect(getDefaultUIState().rightSidebarOpen).toBe(true)
  })

  it('defaults to showing sleeping workspaces', () => {
    const store = createUIStore()

    expect(store.getState().showSleepingWorkspaces).toBe(true)
  })

  it('preserves the current right sidebar width when older persisted UI omits it', () => {
    const store = createUIStore()

    store.setState({ rightSidebarWidth: 360 })
    store.getState().hydratePersistedUI({
      ...makePersistedUI(),
      rightSidebarWidth: undefined as unknown as number
    })

    expect(store.getState().rightSidebarWidth).toBe(360)
  })

  it('hydrates a persisted closed right sidebar preference', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(makePersistedUI({ rightSidebarOpen: false }))

    expect(store.getState().rightSidebarOpen).toBe(false)
  })

  it('hydrates a persisted open right sidebar preference', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(makePersistedUI({ rightSidebarOpen: true }))

    expect(store.getState().rightSidebarOpen).toBe(true)
  })

  it('hydrates a persisted right sidebar tab preference', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(makePersistedUI({ rightSidebarTab: 'checks' }))

    expect(store.getState().rightSidebarTab).toBe('checks')
  })

  it('hydrates persisted per-worktree dotfile visibility', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        showDotfilesByWorktree: {
          'repo-1::/repo': false,
          'repo-2::/repo': true
        }
      })
    )

    expect(store.getState().showDotfilesByWorktree).toEqual({
      'repo-1::/repo': false,
      'repo-2::/repo': true
    })
  })

  it('drops invalid persisted per-worktree dotfile visibility entries', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        showDotfilesByWorktree: {
          'repo-1::/repo': false,
          'repo-2::/repo': 'nope',
          constructor: false
        } as never
      })
    )

    expect(store.getState().showDotfilesByWorktree).toEqual({ 'repo-1::/repo': false })
  })

  it('stores only per-worktree dotfile visibility opt-outs', () => {
    const store = createUIStore()

    store.getState().setShowDotfilesForWorktree('repo-1::/repo', false)
    expect(store.getState().showDotfilesByWorktree).toEqual({ 'repo-1::/repo': false })

    store.getState().setShowDotfilesForWorktree('repo-1::/repo', true)
    expect(store.getState().showDotfilesByWorktree).toEqual({})
  })

  it('toggles per-worktree dotfile visibility independently', () => {
    const store = createUIStore()

    store.getState().toggleShowDotfilesForWorktree('repo-1::/repo')
    store.getState().toggleShowDotfilesForWorktree('repo-2::/repo')
    store.getState().toggleShowDotfilesForWorktree('repo-2::/repo')

    expect(store.getState().showDotfilesByWorktree).toEqual({ 'repo-1::/repo': false })
  })

  it('falls back to explorer for invalid persisted right sidebar tabs', () => {
    const store = createUIStore()

    store
      .getState()
      .hydratePersistedUI(
        makePersistedUI({ rightSidebarTab: 'bogus' as PersistedUIState['rightSidebarTab'] })
      )

    expect(store.getState().rightSidebarTab).toBe('explorer')
  })

  it('clamps persisted sidebar widths into the supported range', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        sidebarWidth: 100,
        rightSidebarWidth: 100
      })
    )

    expect(store.getState().sidebarWidth).toBe(220)
    expect(store.getState().rightSidebarWidth).toBe(220)
  })

  it('preserves right sidebar widths above the former 500px cap', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        sidebarWidth: 260,
        rightSidebarWidth: 900
      })
    )

    // Left sidebar stays capped; right sidebar now allows wide drag targets
    // so long file names remain readable.
    expect(store.getState().sidebarWidth).toBe(260)
    expect(store.getState().rightSidebarWidth).toBe(900)
  })

  it('falls back to existing sidebar widths when persisted values are not finite', () => {
    const store = createUIStore()

    store.getState().setSidebarWidth(320)
    store.setState({ rightSidebarWidth: 360 })

    store.getState().hydratePersistedUI(
      makePersistedUI({
        sidebarWidth: Number.NaN,
        rightSidebarWidth: Number.POSITIVE_INFINITY
      })
    )

    expect(store.getState().sidebarWidth).toBe(320)
    expect(store.getState().rightSidebarWidth).toBe(360)
  })

  it('does not restore the retired active-only filter from persisted UI state', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        showActiveOnly: true
      })
    )

    expect(store.getState().showActiveOnly).toBe(false)
  })

  it('restores the new hide-sleeping filter from persisted UI state', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        hideSleepingWorkspaces: true
      })
    )

    expect(store.getState().showSleepingWorkspaces).toBe(false)
  })

  it('ignores legacy hidden-sleeping preference so existing users start with sleeping visible', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        showSleepingWorkspaces: false
      })
    )

    expect(store.getState().showSleepingWorkspaces).toBe(true)
  })

  it('ignores the legacy show-inactive filter so existing users start with sleeping visible', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        showSleepingWorkspaces: undefined,
        showInactiveWorkspaces: false
      })
    )

    expect(store.getState().showSleepingWorkspaces).toBe(true)
  })

  it('restores the hide-default-branch filter from persisted UI state', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        hideDefaultBranchWorkspace: true
      })
    )

    expect(store.getState().hideDefaultBranchWorkspace).toBe(true)
  })

  it('hides the default-branch workspace by default and respects an explicit persisted choice', () => {
    // New baseline: brand-new profiles (no persisted value) start hidden.
    expect(getDefaultUIState().hideDefaultBranchWorkspace).toBe(true)

    // An explicit persisted choice is respected verbatim — no forced migration.
    // A profile that opted to show the primary keeps it shown.
    const shown = createUIStore()
    shown.getState().hydratePersistedUI(makePersistedUI({ hideDefaultBranchWorkspace: false }))
    expect(shown.getState().hideDefaultBranchWorkspace).toBe(false)

    // A profile that opted to hide it keeps it hidden.
    const hidden = createUIStore()
    hidden.getState().hydratePersistedUI(makePersistedUI({ hideDefaultBranchWorkspace: true }))
    expect(hidden.getState().hideDefaultBranchWorkspace).toBe(true)
  })

  it('restores fixed card properties during hydration', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        worktreeCardProperties: ['inline-agents']
      })
    )

    expect(store.getState().worktreeCardProperties).toEqual(['status', 'unread', 'inline-agents'])
  })

  it('adds the default-on Ports + I/O status items once for older persisted UI', () => {
    const setUI = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('window', { api: { ui: { set: setUI } } })
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        statusBarItems: ['claude', 'gemini'],
        _portsStatusBarDefaultAdded: false
      })
    )

    expect(store.getState().statusBarItems).toEqual(['claude', 'gemini', 'ports', 'io'])
    expect(setUI).toHaveBeenCalledWith({
      statusBarItems: ['claude', 'gemini', 'ports', 'io'],
      _portsStatusBarDefaultAdded: true,
      _ioStatusBarDefaultAdded: true
    })
  })

  it('adds the default-on I/O status item once even after the Ports migration ran', () => {
    const setUI = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('window', { api: { ui: { set: setUI } } })
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        statusBarItems: ['claude', 'gemini', 'ports'],
        _portsStatusBarDefaultAdded: true
      })
    )

    expect(store.getState().statusBarItems).toEqual(['claude', 'gemini', 'ports', 'io'])
    expect(setUI).toHaveBeenCalledWith({
      statusBarItems: ['claude', 'gemini', 'ports', 'io'],
      _portsStatusBarDefaultAdded: true,
      _ioStatusBarDefaultAdded: true
    })
  })

  it('preserves user-hidden Ports + I/O status items after both one-shot migrations ran', () => {
    const setUI = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('window', { api: { ui: { set: setUI } } })
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        statusBarItems: ['claude', 'gemini'],
        _portsStatusBarDefaultAdded: true,
        _ioStatusBarDefaultAdded: true
      })
    )

    expect(store.getState().statusBarItems).toEqual(['claude', 'gemini'])
    expect(setUI).not.toHaveBeenCalled()
  })

  it('clamps persisted workspace board column width', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        workspaceBoardColumnWidth: 900
      })
    )

    expect(store.getState().workspaceBoardColumnWidth).toBe(520)
  })

  it('hydrates workspace board column layout', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        workspaceBoardColumnLayout: 'fit'
      })
    )

    expect(store.getState().workspaceBoardColumnLayout).toBe('fit')
  })

  it('defaults invalid workspace board column layout to full width', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        workspaceBoardColumnLayout: 'compact' as never
      })
    )

    expect(store.getState().workspaceBoardColumnLayout).toBe('full')
  })

  it('hydrates a valid Kagi session link', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        browserKagiSessionLink: 'https://kagi.com/search?token=secret&q=%s'
      })
    )

    expect(store.getState().browserKagiSessionLink).toBe('https://kagi.com/search?token=secret')
  })

  it('drops an invalid Kagi session link during hydration', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        browserKagiSessionLink: 'https://example.com/search?token=secret'
      })
    )

    expect(store.getState().browserKagiSessionLink).toBeNull()
  })

  it('retires legacy custom sidekick assets during pet-state hydration', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        petVisible: undefined,
        petId: undefined,
        petSize: undefined,
        customPets: undefined,
        sidekickVisible: false,
        sidekickId: 'custom-pet',
        sidekickSize: 240,
        customSidekicks: [
          {
            id: 'custom-pet',
            label: 'Legacy pet',
            fileName: 'custom-pet.webp',
            mimeType: 'image/webp',
            kind: 'image'
          }
        ]
      })
    )

    expect(store.getState().petVisible).toBe(false)
    expect(store.getState().petId).toBe('agentum-agent')
    expect(store.getState().petSize).toBe(240)
    expect(store.getState().customPets).toEqual([])
  })

  it('sanitizes task resume state field-by-field during hydration', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        taskResumeState: {
          githubMode: 'project',
          githubItemsPreset: 'invalid',
          githubItemsQuery: 42,
          linearPreset: 'completed',
          linearQuery: 'label:bug'
        } as unknown as PersistedUIState['taskResumeState']
      })
    )

    expect(store.getState().taskResumeState).toEqual({
      githubMode: 'project',
      linearPreset: 'completed',
      linearQuery: 'label:bug'
    })
  })

  it('restores acknowledgedAgentsByPaneKey from persisted UI state', () => {
    const now = 1_700_000_000_000
    vi.useFakeTimers()
    vi.setSystemTime(now)

    try {
      const store = createUIStore()

      store.getState().hydratePersistedUI(
        makePersistedUI({
          acknowledgedAgentsByPaneKey: { 'tab-a:0': now, 'tab-b:1': now - 5_000 }
        })
      )

      expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({
        'tab-a:0': now,
        'tab-b:1': now - 5_000
      })
    } finally {
      vi.useRealTimers()
    }
  })

  it('falls back to an empty ack map when persisted UI omits acknowledgedAgentsByPaneKey', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(makePersistedUI())

    expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({})
  })

  it('falls back to an empty ack map when persisted acknowledgedAgentsByPaneKey is null', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        acknowledgedAgentsByPaneKey:
          null as unknown as PersistedUIState['acknowledgedAgentsByPaneKey']
      })
    )

    expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({})
  })

  it('falls back to an empty ack map when persisted acknowledgedAgentsByPaneKey is a string', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        acknowledgedAgentsByPaneKey:
          'oops' as unknown as PersistedUIState['acknowledgedAgentsByPaneKey']
      })
    )

    expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({})
  })

  it('falls back to an empty ack map when persisted acknowledgedAgentsByPaneKey is an array', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        acknowledgedAgentsByPaneKey: [
          'a',
          'b'
        ] as unknown as PersistedUIState['acknowledgedAgentsByPaneKey']
      })
    )

    expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({})
  })

  it('drops non-number / non-finite / non-positive entries from acknowledgedAgentsByPaneKey', () => {
    const now = 1_700_000_000_000
    vi.useFakeTimers()
    vi.setSystemTime(now)

    try {
      const store = createUIStore()

      store.getState().hydratePersistedUI(
        makePersistedUI({
          acknowledgedAgentsByPaneKey: {
            'tab-a:0': now,
            'tab-b:1': now - 1000,
            'tab-c:2': 'not-a-number',
            'tab-d:3': Number.NaN,
            'tab-e:4': Number.POSITIVE_INFINITY,
            'tab-f:5': -1
          } as unknown as Record<string, number>
        })
      )

      expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({
        'tab-a:0': now,
        'tab-b:1': now - 1000
      })
    } finally {
      vi.useRealTimers()
    }
  })

  it('prunes acknowledgedAgentsByPaneKey entries older than the 7-day TTL during hydration', () => {
    // HYDRATE_MAX_AGE_MS lives in src/renderer/src/store/slices/ui.ts and matches
    // the constant in src/main/agent-hooks/server.ts.
    const SEVEN_DAYS_MS = 7 * 24 * 60 * 60 * 1000
    const now = 1_700_000_000_000
    vi.useFakeTimers()
    vi.setSystemTime(now)

    try {
      const store = createUIStore()

      store.getState().hydratePersistedUI(
        makePersistedUI({
          acknowledgedAgentsByPaneKey: {
            'tab-recent:0': now,
            'tab-old:1': now - SEVEN_DAYS_MS - 1
          }
        })
      )

      expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({
        'tab-recent:0': now
      })
    } finally {
      // The shared afterEach restores mocks/globals but not timers, so clean up
      // here to avoid leaking fake timers into subsequent tests.
      vi.useRealTimers()
    }
  })

  it('drops prototype-pollution keys from acknowledgedAgentsByPaneKey during hydration', () => {
    const now = 1_700_000_000_000
    vi.useFakeTimers()
    vi.setSystemTime(now)

    try {
      const store = createUIStore()
      const malicious: Record<string, number> = {}
      // Object.defineProperty so these land as own enumerable properties rather
      // than getting silently re-routed to Object.prototype by the JS engine.
      Object.defineProperty(malicious, '__proto__', {
        value: now,
        enumerable: true,
        configurable: true,
        writable: true
      })
      Object.defineProperty(malicious, 'constructor', {
        value: now,
        enumerable: true,
        configurable: true,
        writable: true
      })
      Object.defineProperty(malicious, 'prototype', {
        value: now,
        enumerable: true,
        configurable: true,
        writable: true
      })
      malicious['tab-safe:0'] = now

      store.getState().hydratePersistedUI(
        makePersistedUI({
          acknowledgedAgentsByPaneKey: malicious
        })
      )

      expect(store.getState().acknowledgedAgentsByPaneKey).toEqual({
        'tab-safe:0': now
      })
    } finally {
      vi.useRealTimers()
    }
  })

  it('merges and persists partial task resume updates', () => {
    const setUI = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('window', { api: { ui: { set: setUI } } })
    const store = createUIStore()

    store.setState({ taskResumeState: { githubMode: 'project', linearPreset: 'all' } })
    store.getState().setTaskResumeState({ githubItemsPreset: 'my-prs' })

    const expected = { githubMode: 'project', linearPreset: 'all', githubItemsPreset: 'my-prs' }
    expect(store.getState().taskResumeState).toEqual(expected)
    expect(setUI).toHaveBeenCalledWith({ taskResumeState: expected })
  })

  it('persists and clears Linear contexts under only the active repo', () => {
    const setUI = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('window', { api: { ui: { set: setUI } } })
    const store = createUIStore()
    const contextX = { kind: 'project' as const, id: 'project-x', workspaceId: 'workspace-1' }
    const contextY = { kind: 'view' as const, id: 'view-y', workspaceId: 'workspace-1' }

    store.setState({ activeRepoId: 'repo-x' })
    store.getState().setTaskResumeState({ linearContext: contextX })
    store.setState({ activeRepoId: 'repo-y' })
    store.getState().setTaskResumeState({ linearContext: contextY })
    store.setState({ activeRepoId: 'repo-x' })
    store.getState().setTaskResumeState({ linearContext: undefined })

    expect(store.getState().taskResumeState).toMatchObject({
      linearContextByRepo: { 'repo-y': contextY }
    })
    expect(store.getState().taskResumeState?.linearContextByRepo?.['repo-x']).toBeUndefined()
    expect(store.getState().taskResumeState?.linearContext).toBeUndefined()
  })

  it('hydrates legacy Linear context safely without treating it as repo-scoped', () => {
    const store = createUIStore()
    store.setState({ repos: [{ id: 'repo-x' }] as AppState['repos'] })

    expect(() =>
      store.getState().hydratePersistedUI(
        makePersistedUI({
          taskResumeState: {
            linearContext: { kind: 'project', id: 'legacy', workspaceId: 'workspace-1' }
          }
        })
      )
    ).not.toThrow()
    expect(store.getState().taskResumeState?.linearContext?.id).toBe('legacy')
    expect(store.getState().taskResumeState?.linearContextByRepo).toBeUndefined()
  })

  it('sanitizes scoped Linear contexts and prunes deleted repos on hydrate', () => {
    const store = createUIStore()
    store.setState({ repos: [{ id: 'repo-x' }] as AppState['repos'] })
    store.getState().hydratePersistedUI(
      makePersistedUI({
        taskResumeState: {
          linearContextByRepo: {
            'repo-x': { kind: 'project', id: 'project-x', workspaceId: 'workspace-1' },
            'repo-deleted': { kind: 'project', id: 'stale', workspaceId: 'workspace-1' },
            global: { kind: 'view', id: 'global-view', workspaceId: 'workspace-1', model: 'issue' }
          }
        }
      })
    )

    expect(store.getState().taskResumeState?.linearContextByRepo).toEqual({
      'repo-x': { kind: 'project', id: 'project-x', workspaceId: 'workspace-1' },
      global: { kind: 'view', id: 'global-view', workspaceId: 'workspace-1', model: 'issue' }
    })
  })

  it('keeps fixed card properties when toggling Agent activity', () => {
    const setUI = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('window', { api: { ui: { set: setUI } } })
    const store = createUIStore()

    store.setState({ worktreeCardProperties: ['inline-agents'] })
    store.getState().toggleWorktreeCardProperty('inline-agents')

    const expected: WorktreeCardProperty[] = ['status', 'unread']
    expect(store.getState().worktreeCardProperties).toEqual(expected)
    expect(setUI).toHaveBeenCalledWith({ worktreeCardProperties: expected })
  })

  it('persists the agent activity display mode', () => {
    const setUI = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('window', { api: { ui: { set: setUI } } })
    const store = createUIStore()

    store.getState().setAgentActivityDisplayMode('full')

    expect(store.getState().agentActivityDisplayMode).toBe('full')
    expect(setUI).toHaveBeenCalledWith({ agentActivityDisplayMode: 'full' })
  })

  it('normalizes invalid persisted agent activity display modes', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        agentActivityDisplayMode: 'bogus' as PersistedUIState['agentActivityDisplayMode']
      })
    )

    expect(store.getState().agentActivityDisplayMode).toBe('compact')
  })
})

describe('createUISlice settings navigation', () => {
  it('prefetches the restored default task source when provider settings drifted', () => {
    const store = createUIStore()
    const prefetchWorkItems = vi.fn()
    const prefetchLinearIssues = vi.fn()

    store.setState({
      repos: [
        {
          id: 'repo-1',
          path: '/repo',
          displayName: 'Repo',
          badgeColor: 'blue',
          addedAt: 1,
          kind: 'git'
        }
      ],
      settings: {
        visibleTaskProviders: ['linear'],
        defaultTaskSource: 'github',
        defaultTaskViewPreset: 'all'
      } as unknown as AppState['settings'],
      linearStatus: { connected: true } as AppState['linearStatus'],
      preflightStatus: { glab: { installed: false } } as AppState['preflightStatus'],
      prefetchWorkItems,
      prefetchLinearIssues
    } as unknown as Partial<AppState>)

    store.getState().openTaskPage()

    expect(prefetchWorkItems).toHaveBeenCalledWith(
      'repo-1',
      '/repo',
      expect.any(Number),
      'is:issue is:open'
    )
    expect(prefetchLinearIssues).not.toHaveBeenCalled()
  })

  it('returns to the tasks page after visiting settings from an in-progress draft', () => {
    const store = createUIStore()

    store.getState().openTaskPage({ preselectedRepoId: 'repo-1' })
    store.getState().openSettingsPage()

    expect(store.getState().activeView).toBe('settings')
    expect(store.getState().previousViewBeforeSettings).toBe('tasks')

    store.getState().closeSettingsPage()

    expect(store.getState().activeView).toBe('tasks')
  })

  it('keeps the original return target when settings is reopened while already visible', () => {
    const store = createUIStore()

    store.getState().openTaskPage()
    store.getState().openSettingsPage()
    store.getState().openSettingsPage()

    expect(store.getState().previousViewBeforeSettings).toBe('tasks')

    store.getState().closeSettingsPage()

    expect(store.getState().activeView).toBe('tasks')
  })

  it('clears transient settings search when opening settings', () => {
    const store = createUIStore()

    store.setState({ settingsSearchInputQuery: 'terminal', settingsSearchQuery: 'terminal' })
    store.getState().openSettingsPage()

    expect(store.getState().activeView).toBe('settings')
    expect(store.getState().settingsSearchInputQuery).toBe('')
    expect(store.getState().settingsSearchQuery).toBe('')
  })
})

describe('createUISlice new workspace draft', () => {
  it('preserves Linear linked work item metadata and context', () => {
    const store = createUIStore()

    store.getState().setNewWorkspaceDraft({
      repoId: 'repo-1',
      name: 'Fix launch context handoff',
      prompt: '',
      note: '',
      attachments: [],
      linkedWorkItem: {
        type: 'issue',
        number: 0,
        title: 'Fix launch context handoff',
        url: 'https://linear.app/acme/issue/ENG-123/fix-launch-context-handoff',
        linearIdentifier: 'ENG-123',
        linkedContext: {
          provider: 'linear',
          version: 1,
          renderedText: 'Identifier: ENG-123'
        }
      },
      agent: 'claude',
      linkedIssue: '',
      linkedPR: null,
      linkedGitLabIssue: null,
      linkedGitLabMR: null
    })

    expect(store.getState().newWorkspaceDraft?.linkedWorkItem).toMatchObject({
      linearIdentifier: 'ENG-123',
      linkedContext: {
        provider: 'linear',
        version: 1,
        renderedText: 'Identifier: ENG-123'
      }
    })
  })

  it('keeps older linked work item drafts without Linear context fields valid', () => {
    const store = createUIStore()

    store.getState().setNewWorkspaceDraft({
      repoId: 'repo-1',
      name: 'Legacy issue',
      prompt: '',
      note: '',
      attachments: [],
      linkedWorkItem: {
        type: 'issue',
        number: 42,
        title: 'Legacy issue',
        url: 'https://github.com/acme/repo/issues/42'
      },
      agent: 'claude',
      linkedIssue: '42',
      linkedPR: null,
      linkedGitLabIssue: null,
      linkedGitLabMR: null
    })

    expect(store.getState().newWorkspaceDraft?.linkedWorkItem).toEqual({
      type: 'issue',
      number: 42,
      title: 'Legacy issue',
      url: 'https://github.com/acme/repo/issues/42'
    })
  })
})

describe('createUISlice project hub scoping', () => {
  it('atomically invalidates only the target repo binding when switching projects', () => {
    const store = createUIStore()
    const agentumBinding = {
      status: 'loaded' as const,
      binding: {
        projectOwner: 'MateoCerquetella',
        projectOwnerType: 'user',
        projectNumber: 2
      }
    }

    // Model the dangerous runtime state directly: Freebee has a stale cached
    // Agentum binding before the sidebar switch begins.
    store.setState({
      activeRepoId: 'agentum',
      activeView: 'project',
      projectHubTab: 'tasks',
      taskPageData: { preselectedRepoId: 'agentum' },
      projectBindingByRepo: {
        agentum: agentumBinding,
        freebee: agentumBinding
      }
    })

    store.getState().openProjectHub('freebee', 'tasks')

    expect(store.getState()).toMatchObject({
      activeRepoId: 'freebee',
      activeView: 'project',
      projectHubTab: 'tasks',
      taskPageData: { preselectedRepoId: 'freebee' },
      projectBindingByRepo: {
        agentum: agentumBinding,
        freebee: { status: 'loading' }
      }
    })
  })
})

describe('createUISlice page navigation history', () => {
  it('records and rewinds Tasks visits on close', () => {
    const store = createUIStore()
    store.setState({ worktreesByRepo: { 'repo-1': [makeWorktree('a')] } })

    store.getState().recordWorktreeVisit('a')
    store.getState().openTaskPage()
    expect(store.getState().worktreeNavHistory).toEqual(['a', 'tasks'])
    expect(store.getState().worktreeNavHistoryIndex).toBe(1)

    store.getState().closeTaskPage()
    expect(store.getState().activeView).toBe('activity')
    expect(store.getState().worktreeNavHistoryIndex).toBe(0)
  })

  it('rewinds Tasks detail visits on close', () => {
    const store = createUIStore()
    const workItem = makeGitHubWorkItem()
    store.setState({ worktreesByRepo: { 'repo-1': [makeWorktree('a')] } })

    store.getState().recordWorktreeVisit('a')
    store.getState().openTaskPage({ taskSource: 'github', openGitHubWorkItem: workItem })
    expect(store.getState().worktreeNavHistory).toEqual([
      'a',
      'tasks',
      { kind: 'task-detail', source: 'github', workItem, initialTab: undefined }
    ])
    expect(store.getState().worktreeNavHistoryIndex).toBe(2)

    store.getState().closeTaskPage()
    expect(store.getState().activeView).toBe('activity')
    expect(store.getState().taskPageData).toEqual({})
    expect(store.getState().githubTaskDrawerWorkItem).toBeNull()
    expect(store.getState().worktreeNavHistoryIndex).toBe(0)
  })

  it('skips the whole Tasks detail stack on close', () => {
    const store = createUIStore()
    const workItem = makeGitHubWorkItem()
    store.setState({ worktreesByRepo: { 'repo-1': [makeWorktree('a')] } })

    store.getState().recordWorktreeVisit('a')
    store.getState().openTaskPage({ taskSource: 'github', openGitHubWorkItem: workItem })
    store.getState().openTaskPage({ taskSource: 'linear' })
    expect(store.getState().worktreeNavHistory).toEqual([
      'a',
      'tasks',
      { kind: 'task-detail', source: 'github', workItem, initialTab: undefined },
      'tasks'
    ])

    store.getState().closeTaskPage()
    expect(store.getState().activeView).toBe('activity')
    expect(store.getState().worktreeNavHistoryIndex).toBe(0)
  })
})

describe('createUISlice feature tips', () => {
  it('marks feature tips seen and persists them once', () => {
    const setMock = vi.fn(() => Promise.resolve())
    vi.stubGlobal('window', {
      api: {
        ui: {
          set: setMock
        }
      }
    })
    const store = createUIStore()

    store.getState().markFeatureTipsSeen(['voice-dictation'])
    store.getState().markFeatureTipsSeen(['voice-dictation'])

    expect(store.getState().featureTipsSeenIds).toEqual(['voice-dictation'])
    expect(setMock).toHaveBeenCalledTimes(1)
    expect(setMock).toHaveBeenCalledWith({ featureTipsSeenIds: ['voice-dictation'] })
  })

  it('normalizes persisted feature tip ids during hydration', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        featureTipsSeenIds: ['voice-dictation', 'unknown', 'voice-dictation'] as never
      })
    )

    expect(store.getState().featureTipsSeenIds).toEqual(['voice-dictation'])
  })
})

describe('createUISlice feature interactions', () => {
  it('normalizes persisted feature interaction records during hydration', () => {
    const store = createUIStore()

    store.getState().hydratePersistedUI(
      makePersistedUI({
        featureInteractions: {
          tasks: { firstInteractedAt: 100 },
          browser: { firstInteractedAt: Number.NaN },
          unknown: { firstInteractedAt: 200 }
        } as unknown as FeatureInteractionState
      })
    )

    expect(store.getState().featureInteractions).toEqual({
      tasks: { firstInteractedAt: 100, interactionCount: 1 },
    })
  })

  it('records feature interaction counts and persists each interaction', () => {
    const setMock = vi.fn(() => Promise.resolve())
    vi.stubGlobal('window', {
      api: {
        ui: {
          set: setMock
        }
      }
    })
    const now = 1_700_000_000_000
    vi.useFakeTimers()
    vi.setSystemTime(now)

    try {
      const store = createUIStore()
      store.getState().hydratePersistedUI(makePersistedUI())
      setMock.mockClear()

      store.getState().recordFeatureInteraction('tasks')
      store.getState().recordFeatureInteraction('tasks')

      const expected: FeatureInteractionState = {
        tasks: { firstInteractedAt: now, interactionCount: 2 }
      }
      expect(store.getState().featureInteractions).toEqual(expected)
      expect(setMock).toHaveBeenCalledTimes(2)
      expect(setMock).toHaveBeenCalledWith({ featureInteractions: expected })
    } finally {
      vi.useRealTimers()
    }
  })

  it('uses the main-owned feature interaction increment API when available', async () => {
    const recordFeatureInteractionMock = vi.fn(() =>
      Promise.resolve(
        makePersistedUI({
          featureInteractions: {
            tasks: { firstInteractedAt: 100, interactionCount: 3 }
          }
        })
      )
    )
    const setMock = vi.fn(() => Promise.resolve())
    vi.stubGlobal('window', {
      api: {
        ui: {
          recordFeatureInteraction: recordFeatureInteractionMock,
          set: setMock
        }
      }
    })
    const store = createUIStore()
    store.getState().hydratePersistedUI(
      makePersistedUI({
        featureInteractions: {
          tasks: { firstInteractedAt: 100, interactionCount: 2 }
        }
      })
    )
    setMock.mockClear()

    store.getState().recordFeatureInteraction('tasks')
    await Promise.resolve()

    expect(recordFeatureInteractionMock).toHaveBeenCalledWith('tasks')
    expect(setMock).not.toHaveBeenCalled()
    expect(store.getState().featureInteractions.tasks).toEqual({
      firstInteractedAt: 100,
      interactionCount: 3
    })
  })

  it('keeps newer optimistic interaction counts when persistence responses resolve out of order', async () => {
    const pending: ((ui: PersistedUIState) => void)[] = []
    const recordFeatureInteractionMock = vi.fn(
      () =>
        new Promise<PersistedUIState>((resolve) => {
          pending.push(resolve)
        })
    )
    vi.stubGlobal('window', {
      api: {
        ui: {
          recordFeatureInteraction: recordFeatureInteractionMock,
          set: vi.fn(() => Promise.resolve())
        }
      }
    })
    const store = createUIStore()
    store.getState().hydratePersistedUI(makePersistedUI())

    store.getState().recordFeatureInteraction('tasks')
    store.getState().recordFeatureInteraction('tasks')

    pending[1](
      makePersistedUI({
        featureInteractions: {
          tasks: { firstInteractedAt: 100, interactionCount: 2 }
        }
      })
    )
    await Promise.resolve()
    pending[0](
      makePersistedUI({
        featureInteractions: {
          tasks: { firstInteractedAt: 100, interactionCount: 1 }
        }
      })
    )
    await Promise.resolve()

    expect(store.getState().featureInteractions.tasks).toEqual({
      firstInteractedAt: 100,
      interactionCount: 2
    })
  })

  it('does not record interactions before persisted UI has hydrated', () => {
    const setMock = vi.fn(() => Promise.resolve())
    vi.stubGlobal('window', {
      api: {
        ui: {
          set: setMock
        }
      }
    })
    const store = createUIStore()

    store.getState().recordFeatureInteraction('tasks')

    expect(store.getState().featureInteractions).toEqual({})
    expect(setMock).not.toHaveBeenCalled()
  })
})
