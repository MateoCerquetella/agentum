// Spec 023 — pins for the pure harness-run helpers (Part A's run↔worktree
// match + Part B's linked-issue read / chip label).
import { describe, expect, it } from 'vitest'
import type { HarnessStatus } from '@/runtime/harness-client'
import {
  deriveGatedRunSurface,
  findHarnessRunForWorkdir,
  linkedIssueLabel,
  runLinkedIssue
} from './harness-run'

const ISSUE_URL = 'https://github.com/o/r/issues/42'

function makeRun(overrides: Partial<HarnessStatus> = {}): HarnessStatus {
  return {
    id: 'run-1',
    workdir: '/workspace/feature',
    state: 'running',
    features: {
      features: [],
      max_retries: 2,
      agent_tool: 'claude',
      settle_grace_secs: 8,
      settle_timeout_secs: 1200,
      agent_yolo: true
    },
    elapsed_secs: 0,
    agent_instructions: '',
    ...overrides
  }
}

function stampedFeatures(stamps: Array<{ provider?: string; url?: string }>) {
  return stamps.map((s, i) => ({
    id: `F${i + 1}`,
    name: `Feature ${i + 1}`,
    description: '',
    state: 'pending' as const,
    attempts: 0,
    ...(s.provider !== undefined ? { tracker_provider: s.provider } : {}),
    ...(s.url !== undefined ? { tracker_url: s.url } : {})
  }))
}

describe('findHarnessRunForWorkdir', () => {
  it('matches the run whose workdir is the worktree path', () => {
    const run = makeRun()
    expect(findHarnessRunForWorkdir([makeRun({ id: 'other', workdir: '/elsewhere' }), run], '/workspace/feature')).toBe(run)
  })

  it('normalizes trailing slashes on BOTH sides before comparing', () => {
    const run = makeRun({ workdir: '/workspace/feature/' })
    expect(findHarnessRunForWorkdir([run], '/workspace/feature')).toBe(run)
    expect(findHarnessRunForWorkdir([makeRun()], '/workspace/feature/')).toBeDefined()
  })

  it('returns undefined when no run owns the workdir', () => {
    expect(findHarnessRunForWorkdir([makeRun()], '/nope')).toBeUndefined()
    expect(findHarnessRunForWorkdir([], '/workspace/feature')).toBeUndefined()
  })
})

describe('runLinkedIssue (mirrors shared_tracker_provenance)', () => {
  it('returns the first feature carrying BOTH provider and url', () => {
    const run = makeRun({
      features: {
        ...makeRun().features,
        features: stampedFeatures([{ provider: 'github', url: ISSUE_URL }])
      }
    })
    expect(runLinkedIssue(run)).toBe(ISSUE_URL)
  })

  it('skips half-stamped features (provider-only / url-only)', () => {
    const run = makeRun({
      features: {
        ...makeRun().features,
        features: stampedFeatures([
          { provider: 'github' },
          { url: 'https://github.com/o/r/issues/1' },
          { provider: 'github', url: ISSUE_URL }
        ])
      }
    })
    expect(runLinkedIssue(run)).toBe(ISSUE_URL)
  })

  it('is null once unlinked (no stamps anywhere)', () => {
    const run = makeRun({
      features: { ...makeRun().features, features: stampedFeatures([{}, {}]) }
    })
    expect(runLinkedIssue(run)).toBeNull()
  })
})

describe('linkedIssueLabel', () => {
  it('shortens a GitHub issue URL to its number', () => {
    expect(linkedIssueLabel(ISSUE_URL)).toBe('#42')
    expect(linkedIssueLabel('https://github.com/o/r/issues/7?notification_reason=mention')).toBe('#7')
  })

  it('keeps non-issue identifiers readable as-is', () => {
    expect(linkedIssueLabel('ENG-123')).toBe('ENG-123')
  })
})

describe('deriveGatedRunSurface (AC 1–3)', () => {
  it('pending + no run snapshot yet → starting (the create-beat gap)', () => {
    expect(
      deriveGatedRunSurface({ pendingGatedRun: true, harness: undefined, hasAttachableSession: false })
    ).toBe('starting')
  })

  it('pending + booting run (no session) → starting, reflecting the live state', () => {
    for (const state of ['idle', 'init_verifying', 'running', 'verifying', 'awaiting_confirmation'] as const) {
      expect(
        deriveGatedRunSurface({
          pendingGatedRun: true,
          harness: makeRun({ state }),
          hasAttachableSession: false
        })
      ).toBe('starting')
    }
  })

  it('pending + engine session present → session (clears once attachable)', () => {
    expect(
      deriveGatedRunSurface({
        pendingGatedRun: true,
        harness: makeRun({ current_session: 'sess-1' }),
        hasAttachableSession: false
      })
    ).toBe('session')
  })

  it('pending + halted run → picker (done / failed / blocked never read as starting)', () => {
    for (const state of ['done', 'failed', 'blocked'] as const) {
      expect(
        deriveGatedRunSurface({
          pendingGatedRun: true,
          harness: makeRun({ state }),
          hasAttachableSession: false
        })
      ).toBe('picker')
    }
  })

  it('no pending slice → picker (incl. the non-ownership fallback, AC 3)', () => {
    expect(
      deriveGatedRunSurface({ pendingGatedRun: false, harness: makeRun(), hasAttachableSession: false })
    ).toBe('picker')
    expect(
      deriveGatedRunSurface({ pendingGatedRun: false, harness: undefined, hasAttachableSession: false })
    ).toBe('picker')
  })

  it('an attachable session always wins → session', () => {
    expect(
      deriveGatedRunSurface({ pendingGatedRun: true, harness: makeRun(), hasAttachableSession: true })
    ).toBe('session')
    expect(
      deriveGatedRunSurface({ pendingGatedRun: false, harness: undefined, hasAttachableSession: true })
    ).toBe('session')
  })
})
