import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'

import { RunViewContent } from './SddWorkspaceBar'
import type { SddSnapshot } from '@/runtime/sdd-v2-client'

const snapshot: SddSnapshot = {
  browserEvidence: [],
  spec: {
    specId: 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV',
    repoId: 'repo-1',
    title: 'Refresh access tokens',
    slug: 'spc-01arz3ndektsv4rrffq69g5fav-refresh-access-tokens',
    profile: 'standard',
    control: 'guarded',
    provider: 'codex',
    currentRevision: 2,
    aggregateRevision: 1,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z'
  },
  run: {
    runId: 'run-1',
    specId: 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV',
    repoId: 'repo-1',
    phase: 'specification',
    status: 'waiting',
    aggregateRevision: 1,
    baseRef: 'HEAD',
    baseCommit: '1234567890abcdef',
    branchName: 'agentum/spec',
    authoritativePath: '/data/worktrees/run-1/authoritative',
    workspaceFingerprint: 'fingerprint',
    blocker: null,
    quarantined: 0,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z'
  },
  artifacts: [],
  approval: {
    approvalId: 'approval-1',
    runId: 'run-1',
    purpose: 'specification',
    digest: 'digest',
    requestedRevision: 1,
    requestedBy: 'agent:codex:attempt',
    status: 'pending',
    createdAt: '2026-01-01T00:00:00Z'
  }
}

describe('Run Center views', () => {
  it('renders stable requirements and acceptance criteria from spec.md', () => {
    const html = renderToStaticMarkup(
      <RunViewContent
        view="Spec"
        snapshot={snapshot}
        artifacts={[]}
        events={[]}
        specContent={'# Spec\n\n- RQ-001 Refresh tokens.\n- AC-001 Sessions remain active.'}
        actionPending={false}
        onDecide={vi.fn()}
      />
    )
    expect(html).toContain('RQ-001 Refresh tokens')
    expect(html).toContain('AC-001 Sessions remain active')
  })

  it('shows the exact approval gate and digest before design', () => {
    const html = renderToStaticMarkup(
      <RunViewContent
        view="Overview"
        snapshot={snapshot}
        artifacts={[]}
        events={[]}
        specContent=""
        actionPending={false}
        onDecide={vi.fn()}
      />
    )
    expect(html).toContain('Spec approval required')
    expect(html).toContain('digest')
    expect(html).toContain('Approve')
  })

  it('shows the exact digest without direct approval controls for Autopilot', () => {
    const autopilot = {
      ...snapshot,
      spec: { ...snapshot.spec, control: 'autopilot' as const }
    }
    const html = renderToStaticMarkup(
      <RunViewContent
        view="Overview"
        snapshot={autopilot}
        artifacts={[]}
        events={[]}
        specContent=""
        actionPending={false}
        onDecide={vi.fn()}
      />
    )
    expect(html).toContain('Start authorizes this exact specification')
    expect(html).toContain('digest')
    expect(html).not.toContain('Approve exact digest')
    expect(html).not.toContain('Request changes')
  })

  it('presents attempt-bound browser evidence without exposing raw diagnostics', () => {
    const evidenceSnapshot: SddSnapshot = {
      ...snapshot,
      browserEvidence: [
        {
          evidenceId: 'evidence-1',
          runId: 'run-1',
          attemptId: 'attempt-1',
          grantId: 'grant-1',
          specRevision: 2,
          checkId: 'browser-session',
          manifestSha256: 'a'.repeat(64),
          status: 'passed',
          submittedBy: 'agentum:browser-driver:attempt-1',
          capturedAt: '2026-01-01T00:00:00Z',
          createdAt: '2026-01-01T00:00:01Z',
          evidence: {
            schemaVersion: 1,
            evidenceId: 'evidence-1',
            runId: 'run-1',
            attemptId: 'attempt-1',
            checkId: 'browser-session',
            specRevision: 2,
            capturedAt: '2026-01-01T00:00:00Z',
            workspaceFingerprint: 'f'.repeat(64),
            target: {
              origin: 'http://localhost:3000',
              path: '/[redacted]',
              pathRedacted: true,
              queryRedacted: true
            },
            browser: {
              name: 'chromium',
              version: '130.0',
              viewportWidth: 1440,
              viewportHeight: 900,
              deviceScaleMilli: 1000
            },
            captures: [
              { kind: 'screenshot', sha256: 'b'.repeat(64), byteLength: 1024, mediaType: 'image/png' }
            ],
            assertions: [
              {
                id: 'BV-001',
                status: 'passed',
                acceptanceCriteria: ['AC-001'],
                evidenceSha256: ['b'.repeat(64)]
              }
            ],
            console: {
              coverage: 'none',
              errors: 0,
              warnings: 0,
              transcriptSha256: 'c'.repeat(64)
            },
            network: {
              coverage: 'main_document',
              requests: 1,
              failedRequests: 0,
              transcriptSha256: 'd'.repeat(64)
            }
          },
          blobs: [
            {
              sha256: 'b'.repeat(64),
              byteLength: 1024,
              mediaType: 'image/png',
              storageRelativePath: `evidence/blobs/sha256/bb/${'b'.repeat(64)}`,
              role: 'capture'
            }
          ]
        }
      ]
    }
    const html = renderToStaticMarkup(
      <RunViewContent
        view="Evidence"
        snapshot={evidenceSnapshot}
        artifacts={[]}
        events={[]}
        specContent=""
        actionPending={false}
        onDecide={vi.fn()}
      />
    )
    expect(html).toContain('browser-session')
    expect(html).toContain('Attempt attempt-1')
    expect(html).toContain('main_document')
    expect(html).toContain('Load capture')
    expect(html).not.toContain('raw console secret')
  })
})
