import { describe, expect, it } from 'vitest'
import {
  buildWizardRecap,
  canLeaveRepoStep,
  deriveWizardTracker,
  parseRemoteSlug,
  resolveWizardAgentOptions,
  wizardBaseBranchTriggerLabel,
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

describe('parseRemoteSlug', () => {
  it('parses an HTTPS GitHub remote', () => {
    expect(parseRemoteSlug('https://github.com/acme/agentum.git')).toEqual({
      host: 'github.com',
      slug: 'acme/agentum',
      provider: 'github'
    })
  })
  it('parses an scp-like SSH GitHub remote', () => {
    expect(parseRemoteSlug('git@github.com:acme/agentum.git')).toEqual({
      host: 'github.com',
      slug: 'acme/agentum',
      provider: 'github'
    })
  })
  it('parses a GitLab remote (incl. nested groups)', () => {
    expect(parseRemoteSlug('https://gitlab.com/group/sub/app.git')).toEqual({
      host: 'gitlab.com',
      slug: 'group/sub/app',
      provider: 'gitlab'
    })
  })
  it('treats a self-hosted remote as provider "other" keeping the host', () => {
    expect(parseRemoteSlug('git@git.mycorp.com:team/app.git')).toEqual({
      host: 'git.mycorp.com',
      slug: 'team/app',
      provider: 'other'
    })
  })
  it('returns null for empty, non-slug, or unparseable input', () => {
    expect(parseRemoteSlug(undefined)).toBeNull()
    expect(parseRemoteSlug('')).toBeNull()
    expect(parseRemoteSlug('not a url')).toBeNull()
    expect(parseRemoteSlug('https://github.com/onlyowner')).toBeNull()
  })
})

describe('deriveWizardTracker', () => {
  it('detects a tracker from a parseable remote', () => {
    expect(
      deriveWizardTracker({
        remoteUrl: 'git@github.com:acme/agentum.git',
        requiresConnection: false,
        isGit: true
      })
    ).toEqual({
      kind: 'detected',
      provider: 'github',
      label: 'GitHub',
      host: 'github.com',
      slug: 'acme/agentum'
    })
  })
  it('reports "disconnected" when the repo still needs a connection and has no readable remote', () => {
    expect(
      deriveWizardTracker({ remoteUrl: null, requiresConnection: true, isGit: true })
    ).toEqual({ kind: 'disconnected' })
  })
  it('reports "none" for a git repo with no remote', () => {
    expect(
      deriveWizardTracker({ remoteUrl: undefined, requiresConnection: false, isGit: true })
    ).toEqual({ kind: 'none' })
  })
  it('reports "none" for a non-git folder regardless of remote', () => {
    expect(
      deriveWizardTracker({
        remoteUrl: 'git@github.com:acme/agentum.git',
        requiresConnection: false,
        isGit: false
      })
    ).toEqual({ kind: 'none' })
  })
})
