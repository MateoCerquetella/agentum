// Spec 009 F3 — the wiki view state machine's contracts, in dependency order
// of importance:
//   1. THE DISCRIMINATOR PIN (D-A6): a `wiki.updated{ready}` event is a
//      refetch command, NEVER a state flip — `ready` is only reachable from a
//      validated `GET /api/wiki` response.
//   2. Progressive merge: `running` frames grow the TOC, monotonically.
//   3. Scoping: other repos' events and unrelated kinds are inert.
//   4. Socket (re)open ⇒ refetch (why no fallback poll exists, D-A5).
//   5. AC-4 (folded in from the F2 probe test): the probe plan is one repo only.
import { describe, expect, it } from 'vitest'

import type { WikiIndexResponse } from '@/runtime/wiki-client'
import {
  applyWikiEvent,
  commandForSocketOpen,
  prettifySlug,
  wikiProbePlan
} from './wiki-view-state'

const running = (pages: string[]): WikiIndexResponse => ({
  state: 'running',
  sessionId: '00000000-0000-0000-0000-000000000000',
  pages
})

const ready: WikiIndexResponse = {
  state: 'ready',
  schemaVersion: 1,
  generatedAt: 1,
  pages: [{ slug: 'overview', title: 'Overview' }]
}

const frame = (payload: unknown): { kind: string; payload: unknown } => ({
  kind: 'wiki.updated',
  payload
})

describe('applyWikiEvent — discriminator honesty (the load-bearing pin)', () => {
  it('a ready event is a REFETCH command, never a state flip', () => {
    const current = running(['overview'])
    const out = applyWikiEvent(
      current,
      'repo-a',
      frame({ repo_id: 'repo-a', status: 'ready', pages: ['overview', 'architecture'] })
    )
    expect(out.command).toBe('refetch')
    // The state must be untouched — reference-equal, still running.
    expect(out.index).toBe(current)
    expect(out.index?.state).toBe('running')
  })

  it('a failed event is a refetch too (the GET carries the recorded error)', () => {
    const current = running(['overview'])
    const out = applyWikiEvent(current, 'repo-a', frame({ repo_id: 'repo-a', status: 'failed' }))
    expect(out.command).toBe('refetch')
    expect(out.index).toBe(current)
  })
})

describe('applyWikiEvent — progressive running merge', () => {
  it('merges newly written pages into a running state', () => {
    const out = applyWikiEvent(
      running(['overview']),
      'repo-a',
      frame({ repo_id: 'repo-a', status: 'running', pages: ['overview', 'getting-started'] })
    )
    expect(out.command).toBe('none')
    expect(out.index).toEqual(running(['getting-started', 'overview']))
  })

  it('grows monotonically — an early/empty pages frame never shrinks the TOC', () => {
    const current = running(['architecture', 'overview'])
    // The generate request path emits `{running, pages: []}`; a late arrival
    // of that frame (or a scanner frame missing a page) must not contract.
    const out = applyWikiEvent(current, 'repo-a', frame({ repo_id: 'repo-a', status: 'running', pages: [] }))
    expect(out.command).toBe('none')
    expect(out.index).toBe(current) // unchanged, reference-equal
  })

  it('is silent (reference-equal) when the frame brings nothing new', () => {
    const current = running(['overview'])
    const out = applyWikiEvent(
      current,
      'repo-a',
      frame({ repo_id: 'repo-a', status: 'running', pages: ['overview'] })
    )
    expect(out.index).toBe(current)
    expect(out.command).toBe('none')
  })

  it('a running event when the view is NOT running commands a refetch (the GET carries the authoritative sessionId)', () => {
    for (const current of [null, { state: 'empty' } as WikiIndexResponse, ready]) {
      const out = applyWikiEvent(
        current,
        'repo-a',
        frame({ repo_id: 'repo-a', status: 'running', pages: ['overview'] })
      )
      expect(out.command).toBe('refetch')
      expect(out.index).toBe(current)
    }
  })

  it('tolerates a malformed pages payload (non-strings dropped)', () => {
    const out = applyWikiEvent(
      running([]),
      'repo-a',
      frame({ repo_id: 'repo-a', status: 'running', pages: ['overview', 42, null] })
    )
    expect(out.index).toEqual(running(['overview']))
  })
})

describe('applyWikiEvent — scoping', () => {
  it('ignores another repo\'s events entirely', () => {
    const current = running(['overview'])
    const out = applyWikiEvent(
      current,
      'repo-a',
      frame({ repo_id: 'repo-b', status: 'ready', pages: ['x'] })
    )
    expect(out.index).toBe(current)
    expect(out.command).toBe('none')
  })

  it('ignores unrelated event kinds and malformed frames', () => {
    const current = running(['overview'])
    for (const ev of [
      { kind: 'host.metrics', payload: { repo_id: 'repo-a', status: 'ready' } },
      { kind: 'wiki.updated', payload: null },
      { kind: 'wiki.updated', payload: 'nonsense' },
      { kind: 'wiki.updated', payload: { repo_id: 'repo-a', status: 'exploded' } },
      {}
    ]) {
      const out = applyWikiEvent(current, 'repo-a', ev)
      expect(out.index).toBe(current)
      expect(out.command).toBe('none')
    }
  })
})

describe('socket (re)open', () => {
  it('commands a refetch — the reconnect gap heals here, not via a fallback poll', () => {
    expect(commandForSocketOpen()).toBe('refetch')
  })
})

describe('wikiProbePlan (AC-4, folded in from the F2 probe test)', () => {
  it('probes exactly the pinned repo — one entry, nothing else', () => {
    expect(wikiProbePlan('repo-a')).toEqual(['repo-a'])
  })

  it('never grows beyond one entry, whatever the pinned id looks like', () => {
    for (const id of ['x', 'repo-123', 'a1b2c3d4-uuid-ish', 'ssh-host-repo']) {
      const plan = wikiProbePlan(id)
      expect(plan).toHaveLength(1)
      expect(plan[0]).toBe(id)
    }
  })
})

describe('prettifySlug', () => {
  it('kebab → Title Case', () => {
    expect(prettifySlug('getting-started')).toBe('Getting Started')
    expect(prettifySlug('overview')).toBe('Overview')
    expect(prettifySlug('multi-word-page-name')).toBe('Multi Word Page Name')
  })

  it('handles underscores, repeated separators, and degenerate slugs', () => {
    expect(prettifySlug('api_reference')).toBe('Api Reference')
    expect(prettifySlug('a--b__c')).toBe('A B C')
    expect(prettifySlug('---')).toBe('---') // nothing to title-case: pass through
  })
})
