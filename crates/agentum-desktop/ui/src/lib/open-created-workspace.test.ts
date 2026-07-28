import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Worktree } from '@/shared/types'
import { useAppStore } from '@/store'
import { takePendingSessionPrompt } from '@/lib/pending-session-prompt'
import { openCreatedWorkspace, planCreatedWorkspaceOpen } from './open-created-workspace'

const initialAppStoreState = useAppStore.getState()

afterEach(() => {
  delete (globalThis as { __AGENTUM_WEB_CLIENT__?: boolean }).__AGENTUM_WEB_CLIENT__
  vi.unstubAllGlobals()
  useAppStore.setState(initialAppStoreState, true)
})

function makeWorktree(): Worktree {
  return {
    id: 'repo-1::/workspace/feature',
    repoId: 'repo-1',
    path: '/workspace/feature',
    head: 'abc123',
    branch: 'refs/heads/feature',
    isBare: false,
    isMainWorktree: false,
    displayName: 'feature',
    comment: '',
    linkedIssue: null,
    linkedPR: null,
    linkedLinearIssue: null,
    isArchived: false,
    isUnread: false,
    isPinned: false,
    sortOrder: 0,
    lastActivityAt: 0,
    createdWithAgent: 'codex'
  }
}

function seedStore(worktree: Worktree): void {
  useAppStore.setState({
    repos: [
      {
        id: 'repo-1',
        path: '/workspace/repo',
        displayName: 'repo',
        badgeColor: '#000000',
        addedAt: 0
      }
    ],
    worktreesByRepo: { 'repo-1': [worktree] },
    activeRepoId: 'repo-1',
    activeView: 'terminal',
    tabsByWorktree: {},
    unifiedTabsByWorktree: {},
    groupsByWorktree: {},
    layoutByWorktree: {},
    activeGroupIdByWorktree: {},
    openFiles: [],
    browserTabsByWorktree: {},
    activeFileIdByWorktree: {},
    activeBrowserTabIdByWorktree: {},
    activeTabTypeByWorktree: {},
    activeTabIdByWorktree: {},
    tabBarOrderByWorktree: {},
    pendingStartupByTabId: {},
    settings: {
      agentCmdOverrides: {},
      setupScriptLaunchMode: 'new-tab'
    } as unknown as ReturnType<typeof useAppStore.getState>['settings'],
    markWorktreeVisited: vi.fn(),
    recordWorktreeVisit: vi.fn(),
    refreshGitHubForWorktreeIfStale: vi.fn(),
    revealWorktreeInSidebar: vi.fn()
  })
}

describe('openCreatedWorkspace', () => {
  it('launches the selected agent directly instead of landing on the picker', () => {
    const worktree = makeWorktree()
    seedStore(worktree)

    openCreatedWorkspace({ worktreeId: worktree.id, agent: 'codex' })

    const state = useAppStore.getState()
    const tabs = state.tabsByWorktree[worktree.id] ?? []
    // The agent was launched directly: exactly one terminal tab seeded with the
    // agent launch command — no redundant "Start a session" picker.
    expect(tabs).toHaveLength(1)
    const startup = state.pendingStartupByTabId[tabs[0]!.id]
    expect(startup?.command).toContain('codex')
    // Nothing stashed for the picker — we did not defer to it.
    expect(takePendingSessionPrompt(worktree.id)).toBeUndefined()
  })

  it('lands on the picker and stashes the prompt when no agent was selected', () => {
    const worktree = makeWorktree()
    seedStore(worktree)

    openCreatedWorkspace({ worktreeId: worktree.id, agent: null, prompt: 'implement feature X' })

    const state = useAppStore.getState()
    const tabs = state.tabsByWorktree[worktree.id] ?? []
    // No agent chosen → no terminal launched here; the WorkspaceAgentLauncher
    // picker renders (no surface) and the prompt is stashed for it to deliver.
    expect(tabs).toHaveLength(0)
    expect(takePendingSessionPrompt(worktree.id)).toBe('implement feature X')
  })

})

describe('planCreatedWorkspaceOpen', () => {
  it('agent + prompt launches the agent only (prompt rides as a draft)', () => {
    expect(
      planCreatedWorkspaceOpen({ agent: 'claude', prompt: 'hello', hasIssueCommand: false })
    ).toEqual({ launchAgent: true, stashPrompt: false, runIssueCommand: false })
  })

  it('no agent + prompt stashes the prompt for the picker', () => {
    expect(
      planCreatedWorkspaceOpen({ agent: null, prompt: 'hello', hasIssueCommand: false })
    ).toEqual({ launchAgent: false, stashPrompt: true, runIssueCommand: false })
  })

  it('no agent + whitespace-only prompt stashes nothing', () => {
    expect(
      planCreatedWorkspaceOpen({ agent: null, prompt: '   ', hasIssueCommand: false })
    ).toEqual({ launchAgent: false, stashPrompt: false, runIssueCommand: false })
  })

  it('issueCommand automation runs on the default path', () => {
    expect(planCreatedWorkspaceOpen({ agent: 'claude', hasIssueCommand: true })).toEqual({
      launchAgent: true,
      stashPrompt: false,
      runIssueCommand: true
    })
  })
})
