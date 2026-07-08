import { describe, expect, it } from 'vitest'
import {
  buildWizardRecap,
  canLeaveRepoStep,
  deriveUnifiedTrackerStatus,
  deriveWizardComposerSeed,
  resolveWizardAgentOptions,
  wizardBaseBranchTriggerLabel,
  wizardPrimaryLabel,
  WIZARD_FALLBACK_AGENT_IDS
} from './create-workspace-wizard-model'
import type { PickerProjectRef } from './work-item-picker-model'
import type { LinkedWorkItemSummary } from '@/lib/new-workspace'
import type { TuiAgent } from '../../../../shared/types'

const PROJECT: PickerProjectRef = { owner: 'acme', ownerType: 'organization', number: 7 }

describe('canLeaveRepoStep', () => {
  it('allows advancing when a repo is chosen and no connection is pending', () => {
    expect(canLeaveRepoStep({ repoId: 'r1', requiresConnection: false })).toBe(true)
  })
  it('blocks when no repo is chosen', () => {
    expect(canLeaveRepoStep({ repoId: '', requiresConnection: false })).toBe(false)
  })
  it('blocks a remote repo that still needs an SSH connection', () => {
    expect(canLeaveRepoStep({ repoId: 'r1', requiresConnection: true })).toBe(false)
  })
})

describe('resolveWizardAgentOptions', () => {
  it('returns the enabled subset of the detected set', () => {
    const detected: TuiAgent[] = ['claude', 'codex']
    expect(resolveWizardAgentOptions({ detectedAgentIds: detected })).toEqual(['claude', 'codex'])
  })

  it('drops disabled agents from the detected set', () => {
    const detected: TuiAgent[] = ['claude', 'codex']
    expect(
      resolveWizardAgentOptions({ detectedAgentIds: detected, disabledTuiAgents: ['codex'] })
    ).toEqual(['claude'])
  })

  it('falls back to the enabled catalog defaults when detection is null', () => {
    expect(resolveWizardAgentOptions({ detectedAgentIds: null })).toEqual(WIZARD_FALLBACK_AGENT_IDS)
  })

  it('falls back when the detected set is empty (fresh host, nothing installed)', () => {
    expect(resolveWizardAgentOptions({ detectedAgentIds: [] })).toEqual(WIZARD_FALLBACK_AGENT_IDS)
  })

  it('filters the fallback by disabled agents', () => {
    expect(
      resolveWizardAgentOptions({ detectedAgentIds: null, disabledTuiAgents: ['codex'] })
    ).toEqual(['claude', 'gemini'])
  })
})

describe('buildWizardRecap', () => {
  it('shows only the host on step 1', () => {
    expect(buildWizardRecap({ step: 1, hostLabel: 'hetzner-01' })).toBe('hetzner-01')
  })

  it('adds repo and worktree once past step 1', () => {
    expect(
      buildWizardRecap({
        step: 2,
        hostLabel: 'hetzner-01',
        repoDisplayName: 'agentum',
        worktreeName: 'feat/token-refresh'
      })
    ).toBe('hetzner-01  ·  agentum · feat/token-refresh')
  })

  it('omits a blank worktree name', () => {
    expect(
      buildWizardRecap({ step: 2, hostLabel: 'hetzner-01', repoDisplayName: 'agentum', worktreeName: '  ' })
    ).toBe('hetzner-01  ·  agentum')
  })

  it('adds the agent once past step 2', () => {
    expect(
      buildWizardRecap({
        step: 3,
        hostLabel: 'hetzner-01',
        repoDisplayName: 'agentum',
        worktreeName: 'feat',
        agent: 'claude'
      })
    ).toBe('hetzner-01  ·  agentum · feat  ·  claude')
  })

  it('never dangles a separator when the repo is unknown', () => {
    expect(buildWizardRecap({ step: 2, hostLabel: 'this mac', repoDisplayName: null })).toBe(
      'this mac'
    )
  })
})

describe('footer copy', () => {
  it('labels the last step as create', () => {
    expect(wizardPrimaryLabel(3)).toBe('Create workspace')
    expect(wizardPrimaryLabel(1)).toBe('Continue')
    expect(wizardPrimaryLabel(2)).toBe('Continue')
  })
})

describe('wizardBaseBranchTriggerLabel', () => {
  it('shows the chosen base branch when set', () => {
    expect(wizardBaseBranchTriggerLabel('feature/x', 'main')).toBe('feature/x')
  })
  it('falls back to the resolved default ref', () => {
    expect(wizardBaseBranchTriggerLabel(undefined, 'origin/main')).toBe('origin/main')
    expect(wizardBaseBranchTriggerLabel('   ', 'develop')).toBe('develop')
  })
  it('falls back to a generic hint when no default is known', () => {
    expect(wizardBaseBranchTriggerLabel(undefined, null)).toBe('default branch')
    expect(wizardBaseBranchTriggerLabel(undefined, '  ')).toBe('default branch')
  })
})

describe('deriveUnifiedTrackerStatus', () => {
  // The load-bearing AC 3 invariant: the merged section's status reads from the
  // SAME resolved Project the picker lists from, so "no tracker" is impossible
  // to show while issues are available.
  it('never reports "none" when a Project resolves', () => {
    for (const status of ['idle', 'loading', 'failed'] as const) {
      for (const optionCount of [0, 1, 5]) {
        expect(
          deriveUnifiedTrackerStatus({ resolved: PROJECT, status, optionCount }).kind
        ).not.toBe('none')
      }
    }
    // ...and the only path to "none" is a null resolution.
    expect(deriveUnifiedTrackerStatus({ resolved: null, status: 'idle', optionCount: 0 })).toEqual({
      kind: 'none'
    })
  })

  it('reports "connecting" while a resolved Project is loading', () => {
    expect(
      deriveUnifiedTrackerStatus({ resolved: PROJECT, status: 'loading', optionCount: 0 })
    ).toEqual({ kind: 'connecting' })
  })

  it('reports "unavailable" when a resolved Project failed to load (still connected)', () => {
    expect(
      deriveUnifiedTrackerStatus({ resolved: PROJECT, status: 'failed', optionCount: 0 })
    ).toEqual({ kind: 'unavailable' })
  })

  it('reports "connected-empty" when a resolved Project loaded zero open issues', () => {
    expect(
      deriveUnifiedTrackerStatus({ resolved: PROJECT, status: 'idle', optionCount: 0 })
    ).toEqual({ kind: 'connected-empty' })
  })

  it('reports "connected" with the issue count when a resolved Project loaded issues', () => {
    expect(
      deriveUnifiedTrackerStatus({ resolved: PROJECT, status: 'idle', optionCount: 3 })
    ).toEqual({ kind: 'connected', issueCount: 3 })
  })

  it('reports "none" regardless of status/count when no Project resolves', () => {
    expect(
      deriveUnifiedTrackerStatus({ resolved: null, status: 'failed', optionCount: 0 })
    ).toEqual({ kind: 'none' })
    expect(
      deriveUnifiedTrackerStatus({ resolved: null, status: 'loading', optionCount: 0 })
    ).toEqual({ kind: 'none' })
  })
})

describe('deriveWizardComposerSeed (spec 013 F4 single front door)', () => {
  const LINKED: LinkedWorkItemSummary = {
    type: 'issue',
    number: 42,
    title: 'Fix the sidebar flicker',
    url: 'https://github.com/acme/agentum/issues/42'
  }

  it('defaults every field for a plain open (no lost capability, no phantom arming)', () => {
    const seed = deriveWizardComposerSeed({})
    expect(seed.initialName).toBe('')
    expect(seed.initialRepoId).toBeUndefined()
    expect(seed.initialLinkedWorkItem).toBeNull()
    expect(seed.initialWorkspaceStatus).toBeUndefined()
    expect(seed.initialBaseBranch).toBeUndefined()
    expect(seed.telemetrySource).toBeUndefined()
    // Absent startGatedRun ⇒ the toggle prop is not seeded (stays default off).
    expect('initialStartGatedRun' in seed).toBe(false)
  })

  it('honors every opinionated field a caller can pass', () => {
    const seed = deriveWizardComposerSeed({
      prefilledName: 'fix-login',
      initialRepoId: 'repo-1',
      linkedWorkItem: LINKED,
      initialBaseBranch: 'develop',
      initialWorkspaceStatus: 'doing',
      startGatedRun: true,
      telemetrySource: 'sidebar'
    })
    expect(seed.initialName).toBe('fix-login')
    expect(seed.initialRepoId).toBe('repo-1')
    expect(seed.initialLinkedWorkItem).toBe(LINKED)
    expect(seed.initialBaseBranch).toBe('develop')
    expect(seed.initialWorkspaceStatus).toBe('doing')
    expect(seed.telemetrySource).toBe('sidebar')
    // Armed open ⇒ the toggle opens already armed (inv. 4, via initialStartGatedRunProp).
    expect(seed).toHaveProperty('initialStartGatedRun', true)
  })

  it('does not arm the gated-run toggle when startGatedRun is false/absent', () => {
    expect('initialStartGatedRun' in deriveWizardComposerSeed({ startGatedRun: false })).toBe(false)
    expect('initialStartGatedRun' in deriveWizardComposerSeed({ prefilledName: 'x' })).toBe(false)
  })
})
