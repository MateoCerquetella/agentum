import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import type { HarnessStatus } from '@/runtime/harness-client'
import { getFeatureProgress, resolveHarnessForSession, SddStatusStripView } from './SddStatusStrip'

function makeHarness(overrides: Partial<HarnessStatus> = {}): HarnessStatus {
  return {
    id: '12345678-aaaa-bbbb-cccc-123456789abc',
    workdir: '/repo/worktree',
    state: 'running',
    current_feature: 'F1',
    current_session: null,
    elapsed_secs: 10,
    agent_instructions: '',
    phase: 'executing',
    features: {
      features: [
        { id: 'F1', name: 'First', description: '', state: 'coding', attempts: 0 },
        { id: 'F2', name: 'Second', description: '', state: 'pending', attempts: 0 }
      ],
      max_retries: 3,
      agent_tool: 'cline',
      settle_grace_secs: 8,
      settle_timeout_secs: 1800,
      agent_yolo: true
    },
    ...overrides
  }
}

describe('SDD status strip', () => {
  it('associates feature, QA, and role sessions by the run token', () => {
    const harness = makeHarness()
    for (const name of [
      'harness-F1-12345678',
      'harness-qa-F1-12345678',
      'harness-architect-12345678'
    ]) {
      expect(
        resolveHarnessForSession(
          { id: `session-${name}`, name, workdir: '/repo/worktree' },
          [harness]
        )
      ).toBe(harness)
    }
  })

  it('does not associate an ordinary or same-token session from another worktree', () => {
    const harness = makeHarness()
    expect(
      resolveHarnessForSession(
        { id: 'ordinary', name: 'repo-codex-12345678', workdir: '/repo/worktree' },
        [harness]
      )
    ).toBeNull()
    expect(
      resolveHarnessForSession(
        { id: 'ordinary', name: 'repo-codex-abcd1234', workdir: '/repo/worktree' },
        [harness]
      )
    ).toBeNull()
  })

  it('derives executing progress from the current feature position', () => {
    expect(getFeatureProgress(makeHarness({ current_feature: 'F2' }))).toEqual({
      current: 2,
      total: 2
    })
    expect(getFeatureProgress(makeHarness({ phase: 'review' }))).toBeNull()
  })

  it('renders phase and n/N progress accessibly', () => {
    const markup = renderToStaticMarkup(
      <SddStatusStripView status={makeHarness({ current_feature: 'F2' })} />
    )
    expect(markup).toContain('data-sdd-status-strip')
    expect(markup).toContain('SDD run status: Executing, feature 2 of 2')
    expect(markup).toContain('2/2')
  })
})
