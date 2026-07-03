import { describe, expect, it } from 'vitest'
import {
  deriveIssueSideEffectGate,
  describeIssueSideEffectSkip,
  type IssueSideEffectSkipReason
} from './issue-side-effect-gate'

describe('deriveIssueSideEffectGate', () => {
  const issue = (url: string) => ({ type: 'issue', url })

  it('passes for a github.com issue on a local repo', () => {
    const gate = deriveIssueSideEffectGate(
      issue('https://github.com/MateoCerquetella/agentum/issues/237'),
      null
    )
    expect(gate).toEqual({
      eligible: true,
      slug: { owner: 'MateoCerquetella', repo: 'agentum' },
      number: 237
    })
  })

  it('accepts www.github.com and trailing paths', () => {
    const gate = deriveIssueSideEffectGate(issue('https://www.github.com/o/r/issues/9/'), undefined)
    expect(gate.eligible).toBe(true)
  })

  it('fails with no-linked-item when nothing is linked', () => {
    expect(deriveIssueSideEffectGate(null, null)).toEqual({
      eligible: false,
      reason: 'no-linked-item'
    })
    expect(deriveIssueSideEffectGate(undefined, null)).toEqual({
      eligible: false,
      reason: 'no-linked-item'
    })
  })

  it('fails with not-an-issue for PRs/MRs', () => {
    expect(
      deriveIssueSideEffectGate({ type: 'pr', url: 'https://github.com/o/r/pull/1' }, null)
    ).toEqual({ eligible: false, reason: 'not-an-issue' })
  })

  it('fails with not-github-url for empty, api-form, and non-github urls', () => {
    // The Chat filed-card / stub paths can carry an empty or api-form URL —
    // exactly the silent-skip inputs bug 2 was about.
    expect(deriveIssueSideEffectGate(issue(''), null)).toEqual({
      eligible: false,
      reason: 'not-github-url'
    })
    expect(
      deriveIssueSideEffectGate(issue('https://api.github.com/repos/o/r/issues/237'), null)
    ).toEqual({ eligible: false, reason: 'not-github-url' })
    expect(deriveIssueSideEffectGate(issue('https://gitlab.com/o/r/-/issues/2'), null)).toEqual({
      eligible: false,
      reason: 'not-github-url'
    })
    // A github.com PULL url on an item claiming to be an issue is rejected too.
    expect(deriveIssueSideEffectGate(issue('https://github.com/o/r/pull/3'), null)).toEqual({
      eligible: false,
      reason: 'not-github-url'
    })
  })

  it('fails with remote-repo when the repo has a connectionId', () => {
    expect(
      deriveIssueSideEffectGate(issue('https://github.com/o/r/issues/1'), 'ssh-conn-1')
    ).toEqual({ eligible: false, reason: 'remote-repo' })
  })
})

describe('describeIssueSideEffectSkip', () => {
  it('names the action and the reason', () => {
    expect(describeIssueSideEffectSkip('scaffold-spec', 'no-linked-item')).toBe(
      'Spec scaffold skipped: no issue is linked to this workspace anymore.'
    )
    expect(describeIssueSideEffectSkip('start-gated-run', 'remote-repo')).toBe(
      'Gated run not started: the selected repo is remote (SSH) — this runs locally only.'
    )
  })

  // Spec 008 F1 #4 (AC 1): EVERY skip reason must produce a distinct, non-empty
  // toast on the start-gated-run route — no reason may fall through silently.
  // Enumerated so adding a reason without copy fails this test loudly.
  it('has a distinct, non-empty message for every skip reason on the start route', () => {
    const reasons: IssueSideEffectSkipReason[] = [
      'no-linked-item',
      'not-an-issue',
      'not-github-url',
      'remote-repo'
    ]
    const messages = reasons.map((reason) => describeIssueSideEffectSkip('start-gated-run', reason))
    for (const message of messages) {
      expect(message.startsWith('Gated run not started: ')).toBe(true)
      // Non-empty reason copy after the prefix — never a bare label.
      expect(message.length).toBeGreaterThan('Gated run not started: '.length)
    }
    // All four are pairwise distinct (no two reasons collapse to one message).
    expect(new Set(messages).size).toBe(reasons.length)
  })
})
