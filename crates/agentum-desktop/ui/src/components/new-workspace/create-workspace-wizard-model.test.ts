import { describe, expect, it } from 'vitest'
import {
  buildWizardRecap,
  canLeaveRepoStep,
  resolveWizardAgentOptions,
  wizardNextHint,
  wizardPrimaryLabel,
  WIZARD_FALLBACK_AGENT_IDS
} from './create-workspace-wizard-model'
import type { TuiAgent } from '../../../../shared/types'

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

  it('hints what each step leads to', () => {
    expect(wizardNextHint(1, 'hetzner-01')).toBe('Next: repos on hetzner-01')
    expect(wizardNextHint(2, 'hetzner-01')).toBe('Next: agent & tracker')
    expect(wizardNextHint(3, 'hetzner-01')).toBe('Lands you in a fresh session')
  })
})
