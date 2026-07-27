import { describe, expect, it } from 'vitest'

import type { SddEvent, SddSnapshot } from '@/runtime/sdd-v2-client'
import {
  appendDurableEvent,
  availableRunActions,
  snapshotBelongsToRepository,
  sourceGoal
} from './run-center-model'

function snapshot(overrides: Partial<SddSnapshot['run']> = {}): SddSnapshot {
  return {
    browserEvidence: [],
    spec: {
      specId: 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV',
      repoId: 'repo-1',
      title: 'Tokens',
      slug: 'tokens',
      profile: 'standard',
      control: 'guarded',
      provider: 'codex',
      currentRevision: 1,
      aggregateRevision: 1,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z'
    },
    run: {
      runId: 'run-1',
      specId: 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV',
      repoId: 'repo-1',
      phase: 'implementation',
      status: 'running',
      aggregateRevision: 1,
      baseRef: 'HEAD',
      baseCommit: 'abc',
      branchName: 'agentum/spec',
      authoritativePath: '/data/run',
      workspaceFingerprint: 'fingerprint',
      blocker: null,
      quarantined: 0,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      ...overrides
    },
    artifacts: [],
    approval: null
  }
}

describe('Run Center state model', () => {
  it('rejects snapshots whose repository, spec, or run identity drifted', () => {
    expect(snapshotBelongsToRepository(snapshot(), 'repo-1', 'run-1')).toBe(true)
    expect(snapshotBelongsToRepository(snapshot({ repoId: 'repo-2' }), 'repo-1', 'run-1')).toBe(false)
    expect(snapshotBelongsToRepository(snapshot(), 'repo-1', 'run-2')).toBe(false)
  })

  it('never exposes approval-bypass commands while an approval is pending', () => {
    const value = snapshot({ phase: 'specification', status: 'waiting' })
    value.approval = {
      approvalId: 'approval-1',
      runId: 'run-1',
      purpose: 'specification',
      digest: 'digest',
      requestedRevision: 1,
      requestedBy: 'agent:codex:attempt',
      status: 'pending',
      createdAt: '2026-01-01T00:00:00Z'
    }
    expect(availableRunActions(value)).toEqual(['cancel'])
  })

  it('uses explicit Start as the only Autopilot specification authorization action', () => {
    const value = snapshot({ phase: 'specification', status: 'waiting' })
    value.spec.control = 'autopilot'
    value.approval = {
      approvalId: 'approval-1',
      runId: 'run-1',
      purpose: 'specification',
      digest: 'digest',
      requestedRevision: 1,
      requestedBy: 'agent:codex:attempt',
      status: 'pending',
      createdAt: '2026-01-01T00:00:00Z'
    }
    expect(availableRunActions(value)).toEqual(['startRun', 'cancel'])
  })

  it('offers only state-valid recovery and delivery actions', () => {
    expect(availableRunActions(snapshot())).toEqual(['pause', 'cancel'])
    expect(availableRunActions(snapshot({ status: 'failed' }))).toEqual(['retry', 'cancel'])
    expect(availableRunActions(snapshot({ status: 'blocked' }))).toEqual(['resolveBlock', 'cancel'])
    expect(
      availableRunActions(snapshot({ phase: 'specification', status: 'blocked' }))
    ).toEqual(['startAuthoring', 'cancel'])
    expect(availableRunActions(snapshot({ phase: 'design', status: 'queued' }))).toEqual([
      'startRun',
      'pause',
      'cancel'
    ])
    expect(availableRunActions(snapshot({ phase: 'ready', status: 'waiting' }))).toEqual([
      'previewDelivery',
      'cancel'
    ])
    expect(availableRunActions(snapshot({ phase: 'ready', status: 'succeeded' }))).toEqual([
      'previewDelivery',
      'cancel'
    ])
  })

  it('deduplicates and repository-scopes durable run events', () => {
    const event = {
      cursor: 4,
      eventId: 'event-4',
      repoId: 'repo-1',
      specId: null,
      runId: 'run-1',
      revision: 2,
      kind: 'sdd.run.started',
      payload: {},
      createdAt: '2026-01-01T00:00:00Z'
    } satisfies SddEvent
    expect(appendDurableEvent([], event, 'run-2')).toEqual([])
    expect(appendDurableEvent([event], event, 'run-1')).toEqual([event])
  })

  it('derives a non-empty authoring goal from an external source only', () => {
    expect(sourceGoal('github', '', 'https://github.com/a/b/issues/1')).toContain('https://')
    expect(sourceGoal('description', '', '')).toBe('')
  })
})
