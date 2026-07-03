import { describe, expect, it } from 'vitest'
import {
  firstStartGatedRunBlocker,
  type StartGatedRunPreconditionState
} from './start-gated-run-precondition'

// Spec 008 F1 #2 (AC 1, the #226 chat-origin edge): an ARMED gated run that
// trips the composer's submit guard must name the unmet precondition instead of
// returning silently. These pin that firstStartGatedRunBlocker is never silent
// on a real block and never fires a false positive when all is well.

const ok: StartGatedRunPreconditionState = {
  repoId: 'repo-1',
  workspaceSeedName: 'add-widget',
  hasSelectedRepo: true,
  selectedRepoRequiresConnection: false,
  shouldWaitForSetupCheck: false,
  shouldWaitForIssueAutomationCheck: false,
  requiresExplicitSetupChoice: false,
  hasSetupDecision: false,
  sparseError: null
}

describe('firstStartGatedRunBlocker', () => {
  it('returns null when every precondition is met', () => {
    expect(firstStartGatedRunBlocker(ok)).toBeNull()
  })

  it('names the repo blocker for the #226 empty-repoId chat-origin edge', () => {
    expect(firstStartGatedRunBlocker({ ...ok, repoId: '' })).toBe(
      'Pick a repo before starting a gated run.'
    )
    expect(firstStartGatedRunBlocker({ ...ok, repoId: null })).toBe(
      'Pick a repo before starting a gated run.'
    )
    expect(firstStartGatedRunBlocker({ ...ok, hasSelectedRepo: false })).toBe(
      'Pick a repo before starting a gated run.'
    )
  })

  it('names each other unmet precondition (never a silent block)', () => {
    expect(firstStartGatedRunBlocker({ ...ok, workspaceSeedName: '' })).toMatch(/name the workspace/i)
    expect(firstStartGatedRunBlocker({ ...ok, selectedRepoRequiresConnection: true })).toMatch(
      /connect/i
    )
    expect(firstStartGatedRunBlocker({ ...ok, shouldWaitForSetupCheck: true })).toMatch(
      /checking project setup/i
    )
    expect(firstStartGatedRunBlocker({ ...ok, shouldWaitForIssueAutomationCheck: true })).toMatch(
      /checking project setup/i
    )
    expect(
      firstStartGatedRunBlocker({ ...ok, requiresExplicitSetupChoice: true, hasSetupDecision: false })
    ).toMatch(/setup option/i)
    expect(firstStartGatedRunBlocker({ ...ok, sparseError: 'bad sparse dir' })).toBe('bad sparse dir')
  })

  it('reports the first blocker in guard order (repo before name)', () => {
    expect(firstStartGatedRunBlocker({ ...ok, repoId: '', workspaceSeedName: '' })).toBe(
      'Pick a repo before starting a gated run.'
    )
  })
})
