// Spec 023 — pins for the pure harness-run helpers (Part A's run↔worktree
// match + Part B's linked-issue read / chip label).
import { describe, expect, it } from 'vitest'
import type { HarnessStatus } from '@/runtime/harness-client'
import {
  deriveGatedRunStages,
  deriveGatedRunSurface,
  findHarnessRunForWorkdir,
  gatedRunBlocker,
  gatedRunHeadline,
  gatedRunSessionTitle,
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

describe('gated-run progress presentation', () => {
  it('shows the concrete blocked role phase and verdict instead of blocked · blocked', () => {
    const run = makeRun({
      state: 'blocked',
      phase: 'blocked',
      blocked_phase: 'authoring',
      phase_attempts: 2,
      gate_summary: 'The goal does not name an observable user outcome.',
      features: { ...makeRun().features, roles: true }
    })
    expect(gatedRunHeadline(run)).toBe('PM spec gate needs attention')
    expect(gatedRunHeadline(run)).not.toContain('blocked · blocked')
    expect(gatedRunBlocker(run)).toContain('observable user outcome')
    expect(deriveGatedRunStages(run).map((stage) => stage.status)).toEqual([
      'blocked',
      'upcoming',
      'upcoming',
      'upcoming',
      'upcoming'
    ])
    expect(gatedRunSessionTitle(run)).toBe('Gated run')
  })

  it('identifies coding, verification, browser QA, blocked, and complete in the headline', () => {
    const cases: Array<{
      label: string
      runState: HarnessStatus['state']
      featureState: HarnessStatus['features']['features'][number]['state']
      expected: string
    }> = [
      {
        label: 'coding',
        runState: 'running',
        featureState: 'coding',
        expected: 'Working on Build the thing'
      },
      {
        label: 'verification',
        runState: 'running',
        featureState: 'verifying',
        expected: 'Verifying Build the thing'
      },
      {
        label: 'browser QA',
        runState: 'running',
        featureState: 'ready_to_test',
        expected: 'Browser QA for Build the thing'
      },
      {
        label: 'blocked',
        runState: 'blocked',
        featureState: 'blocked',
        expected: 'Blocked on Build the thing'
      },
      {
        label: 'complete',
        runState: 'done',
        featureState: 'done',
        expected: 'Gated run complete'
      }
    ]

    for (const testCase of cases) {
      const run = makeRun({
        state: testCase.runState,
        phase: 'executing',
        current_feature: 'F1',
        features: {
          ...makeRun().features,
          features: [
            {
              id: 'F1',
              name: 'Build the thing',
              description: '',
              state: testCase.featureState,
              attempts: 1
            }
          ]
        }
      })
      expect(gatedRunHeadline(run), testCase.label).toBe(testCase.expected)
    }
  })

  it('shows completed/current SDD stages and uses the current task in session copy', () => {
    const run = makeRun({
      phase: 'executing',
      current_feature: 'F1',
      features: {
        ...makeRun().features,
        roles: true,
        features: [
          {
            id: 'F1',
            name: 'Build the tracker surface',
            description: '',
            state: 'coding',
            attempts: 0
          }
        ]
      }
    })
    expect(gatedRunHeadline(run)).toBe('Working on Build the tracker surface')
    expect(gatedRunSessionTitle(run)).toBe('F1 · Gated run')
    expect(deriveGatedRunStages(run).map((stage) => stage.status)).toEqual([
      'complete',
      'complete',
      'complete',
      'active',
      'upcoming'
    ])
  })

  it('prefers a blocked feature error over a role-gate summary', () => {
    const run = makeRun({
      state: 'blocked',
      phase: 'executing',
      current_feature: 'F1',
      gate_summary: 'old role summary',
      features: {
        ...makeRun().features,
        features: [
          {
            id: 'F1',
            name: 'Run tests',
            description: '',
            state: 'blocked',
            attempts: 3,
            last_error: 'npm test failed'
          }
        ]
      }
    })
    expect(gatedRunHeadline(run)).toBe('Blocked on Run tests')
    expect(gatedRunBlocker(run)).toBe('npm test failed')
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
