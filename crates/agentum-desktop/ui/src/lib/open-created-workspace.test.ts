import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Worktree } from '../../../shared/types'
import { useAppStore } from '@/store'
import { takePendingSessionPrompt } from '@/lib/pending-session-prompt'
import { maybeOfferWorkspaceHarnessRun } from '@/lib/workspace-harness-offer'
import { openCreatedWorkspace, planCreatedWorkspaceOpen } from './open-created-workspace'

// Spec 015: the harness-offer runner is fire-and-forget IO — mock it so these
// tests stay network/fs-free and pin only the trigger contract.
vi.mock('@/lib/workspace-harness-offer', () => ({
  maybeOfferWorkspaceHarnessRun: vi.fn(() => Promise.resolve())
}))

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

  it('gated run launches no agent and stashes nothing (spec 005 F1, D2)', () => {
    const worktree = makeWorktree()
    seedStore(worktree)

    openCreatedWorkspace({
      worktreeId: worktree.id,
      agent: 'codex',
      prompt: 'implement feature X',
      gatedRun: true
    })

    const state = useAppStore.getState()
    const tabs = state.tabsByWorktree[worktree.id] ?? []
    // The engine's sessions are the only agents in the worktree: no draft-open…
    expect(tabs).toHaveLength(0)
    // …and no prompt stashed for the picker either.
    expect(takePendingSessionPrompt(worktree.id)).toBeUndefined()
  })
})

// Spec 015 (D2): every create fires the harness-spec detection runner exactly
// once, with the creation context it needs for the D6 gate.
describe('openCreatedWorkspace → harness offer trigger (spec 015)', () => {
  it('fires the runner once per create with { worktreeId, gatedRun: false }', () => {
    const worktree = makeWorktree()
    seedStore(worktree)
    vi.mocked(maybeOfferWorkspaceHarnessRun).mockClear()

    openCreatedWorkspace({ worktreeId: worktree.id, agent: 'codex' })

    expect(maybeOfferWorkspaceHarnessRun).toHaveBeenCalledTimes(1)
    expect(maybeOfferWorkspaceHarnessRun).toHaveBeenCalledWith({
      worktreeId: worktree.id,
      gatedRun: false
    })
  })

  it('passes gatedRun: true through to the runner (D6 input)', () => {
    const worktree = makeWorktree()
    seedStore(worktree)
    vi.mocked(maybeOfferWorkspaceHarnessRun).mockClear()

    openCreatedWorkspace({ worktreeId: worktree.id, agent: 'codex', gatedRun: true })

    expect(maybeOfferWorkspaceHarnessRun).toHaveBeenCalledTimes(1)
    expect(maybeOfferWorkspaceHarnessRun).toHaveBeenCalledWith({
      worktreeId: worktree.id,
      gatedRun: true
    })
  })
})

// Spec 005 F1 (D2): the "suppression flag round-trips" unit pin. A gated
// engine run must skip ALL THREE plain-delivery paths (draft-open, picker
// prompt stash, issueCommand automation); the default path stays exactly
// today's behavior.
describe('planCreatedWorkspaceOpen', () => {
  it('gated run suppresses all three plain-delivery paths', () => {
    expect(
      planCreatedWorkspaceOpen({
        gatedRun: true,
        agent: 'claude',
        prompt: 'do the thing',
        hasIssueCommand: true
      })
    ).toEqual({ launchAgent: false, stashPrompt: false, runIssueCommand: false })
  })

  it('gated run suppresses even without an agent or issueCommand', () => {
    expect(
      planCreatedWorkspaceOpen({
        gatedRun: true,
        agent: null,
        prompt: 'typed prompt',
        hasIssueCommand: false
      })
    ).toEqual({ launchAgent: false, stashPrompt: false, runIssueCommand: false })
  })

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

  it('explicit gatedRun: false behaves like the default path', () => {
    expect(
      planCreatedWorkspaceOpen({
        gatedRun: false,
        agent: null,
        prompt: 'hello',
        hasIssueCommand: true
      })
    ).toEqual({ launchAgent: false, stashPrompt: true, runIssueCommand: true })
  })
})
