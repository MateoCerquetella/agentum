import React from 'react'
import { act, create, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type {
  SddArtifact,
  SddCapabilities,
  SddEvent,
  SddSnapshot,
  SddSpec
} from '@/runtime/sdd-client'

;(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true

const mocks = vi.hoisted(() => ({
  command: vi.fn(),
  connectJiraApiToken: vi.fn(),
  createSpec: vi.fn(),
  createSpecRun: vi.fn(),
  getArtifacts: vi.fn(),
  getEvents: vi.fn(),
  getRun: vi.fn(),
  getSddCapabilities: vi.fn(),
  getSddRemoteCapability: vi.fn(),
  getSpec: vi.fn(),
  listSpecs: vi.fn(),
  listJiraConnections: vi.fn(),
  previewSddSource: vi.fn(),
  redeemJiraOauth: vi.fn(),
  selectJiraSite: vi.fn(),
  startJiraOauth: vi.fn(),
  subscribeRunCenterSelection: vi.fn(() => () => undefined),
  subscribeSddEvents: vi.fn(),
  toastError: vi.fn(),
  toastSuccess: vi.fn()
}))

vi.mock('@/runtime/sdd-client', () => ({
  command: mocks.command,
  connectJiraApiToken: mocks.connectJiraApiToken,
  createSpec: mocks.createSpec,
  createSpecRun: mocks.createSpecRun,
  getArtifacts: mocks.getArtifacts,
  getEvents: mocks.getEvents,
  getRun: mocks.getRun,
  getSddCapabilities: mocks.getSddCapabilities,
  getSddRemoteCapability: mocks.getSddRemoteCapability,
  getSpec: mocks.getSpec,
  listSpecs: mocks.listSpecs,
  listJiraConnections: mocks.listJiraConnections,
  previewSddSource: mocks.previewSddSource,
  redeemJiraOauth: mocks.redeemJiraOauth,
  selectJiraSite: mocks.selectJiraSite,
  startJiraOauth: mocks.startJiraOauth,
  subscribeRunCenterSelection: mocks.subscribeRunCenterSelection,
  subscribeSddEvents: mocks.subscribeSddEvents
}))

vi.mock('sonner', () => ({
  toast: { error: mocks.toastError, success: mocks.toastSuccess }
}))

vi.mock('@/components/ui/dialog', () => ({
  Dialog: ({ open, children }: { open: boolean; children: React.ReactNode }) =>
    open ? <dialog>{children}</dialog> : null,
  DialogContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogDescription: ({ children }: { children: React.ReactNode }) => <p>{children}</p>,
  DialogFooter: ({ children }: { children: React.ReactNode }) => <footer>{children}</footer>,
  DialogHeader: ({ children }: { children: React.ReactNode }) => <header>{children}</header>,
  DialogTitle: ({ children }: { children: React.ReactNode }) => <h2>{children}</h2>
}))

type EventSubscription = {
  repoId: string
  after?: number
  onOpen?: () => void
  onEvent: (event: SddEvent) => void
}

const SPEC_ID = 'SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV'
const RUN_ID = 'run-01arz3ndektsv4rrffq69g5fav'

const fullCapabilities: SddCapabilities = {
  schemaVersion: 1,
  providers: [
    { id: 'codex', label: 'Codex', available: true },
    {
      id: 'claude',
      label: 'Claude',
      available: false,
      reason: 'Claude CLI not found'
    },
    {
      id: 'agent',
      label: 'Cursor / Agent',
      available: false,
      reason: 'Agent CLI not found'
    },
    {
      id: 'gemini',
      label: 'Gemini',
      available: false,
      reason: 'Gemini CLI not found'
    },
    {
      id: 'hermes',
      label: 'Hermes',
      available: false,
      reason: 'Hermes CLI not found'
    },
    {
      id: 'opencode',
      label: 'OpenCode',
      available: false,
      reason: 'OpenCode CLI not found'
    },
    {
      id: 'aider',
      label: 'Aider',
      available: false,
      reason: 'Aider CLI not found'
    },
    {
      id: 'custom',
      label: 'Custom adapter',
      available: false,
      reason: 'No adapter configured'
    }
  ],
  providerAliases: { cursor: 'agent' },
  localProviderExecution: {
    available: true,
    boundary: 'local_sandboxed',
    mechanism: 'bubblewrap'
  },
  sources: [
    { id: 'description', label: 'Description', available: true },
    { id: 'socratic', label: 'Socratic', available: true },
    { id: 'markdown', label: 'Markdown', available: true, preview: true },
    { id: 'github', label: 'GitHub', available: true, preview: true },
    {
      id: 'linear',
      label: 'Linear',
      available: false,
      reason: 'Secure vault required'
    },
    {
      id: 'jira',
      label: 'Jira Cloud',
      available: false,
      reason: 'OAuth broker required'
    },
    { id: 'openspec', label: 'OpenSpec', available: true, preview: true },
    { id: 'empirical', label: 'Empirical', available: true, preview: true }
  ],
  remoteLifecycle: false,
  remoteLifecycleReason: 'desktop_projection_unavailable',
  remoteWorker: {
    schemaVersion: 1,
    protocol: 'agentum-sdd-v1',
    projectionReady: false,
    blockers: ['desktop_projection_unavailable'],
    automaticallyDeployed: false
  },
  delivery: true,
  readyLifecycle: true,
  browserEvidence: {
    available: false,
    reason: 'Run-bound rich browser evidence is not available in this build.'
  }
}

function spec(): SddSpec {
  return {
    specId: SPEC_ID,
    repoId: 'repo-1',
    title: 'Refresh access tokens',
    slug: 'spc-01arz3ndektsv4rrffq69g5fav-refresh-access-tokens',
    profile: 'standard',
    control: 'guarded',
    provider: 'codex',
    currentRevision: 1,
    aggregateRevision: 1,
    createdAt: '2026-07-26T00:00:00Z',
    updatedAt: '2026-07-26T00:00:00Z'
  }
}

function authoredSnapshot(): SddSnapshot {
  const value = spec()
  return {
    browserEvidence: [],
    spec: value,
    run: {
      runId: RUN_ID,
      specId: SPEC_ID,
      repoId: 'repo-1',
      phase: 'specification',
      status: 'waiting',
      aggregateRevision: 1,
      baseRef: 'HEAD',
      baseCommit: '1234567890abcdef',
      branchName: 'agentum/spc-01arz3ndektsv4rrffq69g5fav-refresh-access-tokens',
      authoritativePath: '/agentum-data/worktrees/repo-1/run-1/authoritative',
      workspaceFingerprint: 'workspace-fingerprint',
      blocker: null,
      quarantined: 0,
      createdAt: '2026-07-26T00:00:00Z',
      updatedAt: '2026-07-26T00:00:00Z'
    },
    artifacts: [],
    approval: {
      approvalId: 'approval-1',
      runId: RUN_ID,
      purpose: 'specification',
      digest: 'sha256:approval-digest',
      requestedRevision: 1,
      requestedBy: 'agent:codex:author-session',
      status: 'pending',
      createdAt: '2026-07-26T00:00:00Z'
    }
  }
}

function snapshotAt(
  phase: SddSnapshot['run']['phase'],
  status: SddSnapshot['run']['status'],
  revision: number
): SddSnapshot {
  const snapshot = authoredSnapshot()
  snapshot.approval = null
  snapshot.run.phase = phase
  snapshot.run.status = status
  snapshot.run.aggregateRevision = revision
  return snapshot
}

function artifact(kind: string, relativePath: string, content: string): SddArtifact {
  return {
    metadata: {
      artifactRevisionId: `${kind}-revision-1`,
      runId: RUN_ID,
      specId: SPEC_ID,
      kind,
      revision: 1,
      specRevision: 1,
      relativePath,
      contentHash: `sha256:${kind}`,
      submittedBy: 'agent:codex:author-session',
      createdAt: '2026-07-26T00:00:00Z'
    },
    content,
    externallyModified: false,
    actualContentHash: `sha256:${kind}`
  }
}

function event(cursor: number, kind: string): SddEvent {
  return {
    cursor,
    eventId: `event-${cursor}`,
    repoId: 'repo-1',
    specId: SPEC_ID,
    runId: RUN_ID,
    revision: cursor,
    kind,
    payload: { durable: true },
    createdAt: '2026-07-26T00:00:00Z'
  }
}

function textOf(node: ReactTestInstance | string | number): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  return node.children.map((child) => textOf(child as ReactTestInstance | string | number)).join('')
}

function button(root: ReactTestInstance, label: string, occurrence = 0): ReactTestInstance {
  const matches = root.findAllByType('button').filter((entry) => textOf(entry).trim() === label)
  const match = matches[occurrence]
  if (!match) throw new Error(`Button not found: ${label}`)
  return match
}

function labeledControl(
  root: ReactTestInstance,
  label: string,
  type: 'input' | 'select' | 'textarea'
): ReactTestInstance {
  const owner = root.findAllByType('label').find((entry) => textOf(entry).trim().startsWith(label))
  if (!owner) throw new Error(`Label not found: ${label}`)
  return owner.findByType(type)
}

async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
    await Promise.resolve()
  })
}

describe('SddWorkspaceBar interaction workflow', () => {
  let serverSnapshot: SddSnapshot | null
  let serverArtifacts: SddArtifact[]
  let serverEvents: SddEvent[]
  let subscription: EventSubscription | null
  let pauseAttempt: number

  beforeEach(() => {
    vi.clearAllMocks()
    serverSnapshot = null
    serverArtifacts = []
    serverEvents = []
    subscription = null
    pauseAttempt = 0
    mocks.getSddCapabilities.mockResolvedValue(fullCapabilities)
    mocks.getSddRemoteCapability.mockResolvedValue({
      schemaVersion: 1,
      available: false,
      reason: 'repository_is_local'
    })
    mocks.previewSddSource.mockResolvedValue({
      kind: 'openspec',
      title: 'Imported refresh change',
      markdown: '# Imported refresh change',
      sourceRevision: 'sha256:source-revision',
      sourcePath: 'openspec/changes/refresh-sessions',
      designAvailable: true,
      taskCount: 2,
      capabilities: ['auth'],
      capabilityCount: 1,
      diagnostics: [],
      previewDigest: 'sha256:preview'
    })
    mocks.listSpecs.mockImplementation(async () => (serverSnapshot ? [serverSnapshot.spec] : []))
    mocks.getSpec.mockImplementation(async () => ({
      spec: structuredClone(serverSnapshot?.spec),
      run: structuredClone(serverSnapshot?.run),
      runs: serverSnapshot ? [structuredClone(serverSnapshot.run)] : []
    }))
    mocks.getRun.mockImplementation(async () => structuredClone(serverSnapshot))
    mocks.getArtifacts.mockImplementation(async () => structuredClone(serverArtifacts))
    mocks.getEvents.mockImplementation(async () => structuredClone(serverEvents))
    mocks.subscribeRunCenterSelection.mockReturnValue(vi.fn())
    mocks.subscribeSddEvents.mockImplementation((next: EventSubscription) => {
      subscription = next
      next.onOpen?.()
      return vi.fn()
    })
    mocks.createSpec.mockImplementation(async (_repoId: string, input: Record<string, unknown>) => {
      serverSnapshot = authoredSnapshot()
      serverArtifacts = [
        artifact(
          'specification',
          `.agentum/specs/${spec().slug}/spec.md`,
          '# Refresh access tokens\n\n- RQ-001 Refresh tokens safely.\n- AC-001 Active sessions remain uninterrupted.'
        ),
        artifact(
          'plan',
          `.agentum/specs/${spec().slug}/plan.json`,
          JSON.stringify({
            tasks: [
              {
                id: 'TSK-001',
                objective: 'Rotate access tokens',
                dependencies: [],
                acceptanceCriteria: ['AC-001'],
                risk: 'medium',
                parallelSafe: false,
                verification: [{ program: 'cargo', args: ['test'] }]
              }
            ]
          })
        ),
        artifact('review', `.agentum/specs/${spec().slug}/review.md`, 'Independent review passed.')
      ]
      serverEvents = [event(1, 'sdd.spec.authored'), event(2, 'sdd.verification.passed')]
      return {
        specId: SPEC_ID,
        runId: RUN_ID,
        revision: 1,
        specRevision: 1,
        phase: 'specification',
        status: 'waiting',
        nextAction: 'Spec approval required',
        artifactSetId: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
        authoritativePath: serverSnapshot.run.authoritativePath,
        approval: {
          approvalId: 'approval-1',
          purpose: 'specification',
          digest: 'sha256:approval-digest',
          status: 'pending'
        },
        input
      }
    })
    mocks.command.mockImplementation(async (_runId: string, body: Record<string, unknown>) => {
      if (!serverSnapshot) throw new Error('missing run')
      const type = body.type
      if (type === 'pause' && pauseAttempt++ === 0) throw new Error('503 provider busy')
      const transition = (
        phase: SddSnapshot['run']['phase'],
        status: SddSnapshot['run']['status']
      ) => {
        const revision = serverSnapshot!.run.aggregateRevision + 1
        serverSnapshot = {
          ...serverSnapshot!,
          run: {
            ...serverSnapshot!.run,
            phase,
            status,
            aggregateRevision: revision
          },
          approval: type === 'decideApproval' ? null : serverSnapshot!.approval
        }
        return { runId: RUN_ID, revision, phase, status }
      }
      if (type === 'decideApproval') {
        return body.decision === 'approve'
          ? transition('implementation', 'running')
          : transition('specification', 'blocked')
      }
      if (type === 'startAuthoring') return transition('specification', 'queued')
      if (type === 'startRun') return transition(serverSnapshot.run.phase, 'running')
      if (type === 'pause') return transition(serverSnapshot.run.phase, 'paused')
      if (type === 'resume' || type === 'resolveBlock') {
        return transition(serverSnapshot.run.phase, 'running')
      }
      if (type === 'retry') return transition(serverSnapshot.run.phase, 'queued')
      if (type === 'reopenPhase') {
        return transition(body.phase as SddSnapshot['run']['phase'], 'paused')
      }
      if (type === 'cancel') return transition(serverSnapshot.run.phase, 'canceled')
      if (type === 'previewDelivery') {
        const result = transition('ready', 'succeeded')
        return {
          ...result,
          previewToken: 'preview-token-bound-to-ready-hash',
          digest: 'sha256:delivery-digest',
          expiresAt: '2026-07-27T00:00:00Z',
          summary: 'Commit locally; push remains blocked.',
          actions: [
            { id: 'commit', type: 'commit', label: 'Commit', enabled: true },
            {
              id: 'push',
              type: 'push',
              label: 'Push',
              enabled: false,
              blockedReason: 'No remote selected'
            }
          ]
        }
      }
      if (type === 'confirmDelivery') return transition('delivery', 'succeeded')
      throw new Error(`unexpected command ${String(type)}`)
    })
  })

  it('authorizes a pending Autopilot spec only through explicit Start', async () => {
    serverSnapshot = authoredSnapshot()
    serverSnapshot.spec.control = 'autopilot'
    mocks.command.mockImplementationOnce(async (_runId: string, body: Record<string, unknown>) => {
      const revision = serverSnapshot!.run.aggregateRevision + 1
      serverSnapshot = {
        ...serverSnapshot!,
        run: {
          ...serverSnapshot!.run,
          phase: 'design',
          status: 'queued',
          aggregateRevision: revision
        },
        approval: null
      }
      return { runId: RUN_ID, revision, phase: 'design', status: 'queued', body }
    })

    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()

    expect(textOf(renderer!.root)).toContain('Start authorizes this exact specification')
    expect(textOf(renderer!.root)).not.toContain('Approve exact digest')
    expect(textOf(renderer!.root)).not.toContain('Request changes')
    await act(async () => button(renderer!.root, 'Start').props.onClick())
    await settle()
    expect(mocks.command).toHaveBeenCalledTimes(1)
    expect(mocks.command.mock.calls[0][1]).toMatchObject({
      type: 'startRun',
      expectedRevision: 1
    })
    expect(mocks.command.mock.calls[0][1].requestId).toMatch(/^[0-9a-f-]{36}$/)
    await act(async () => renderer!.unmount())
  })

  it('configures a discovered spec and keeps imported downstream artifacts historical', async () => {
    const discovered: SddSpec = {
      ...spec(),
      provider: 'unassigned',
      aggregateRevision: 4
    }
    mocks.listSpecs.mockImplementation(async () =>
      serverSnapshot ? [structuredClone(serverSnapshot.spec)] : [structuredClone(discovered)]
    )
    mocks.getSpec.mockImplementation(async () =>
      serverSnapshot
        ? {
            spec: structuredClone(serverSnapshot.spec),
            run: structuredClone(serverSnapshot.run)
          }
        : { spec: structuredClone(discovered), run: null }
    )
    mocks.createSpecRun.mockImplementation(async (_specId: string, input: Record<string, unknown>) => {
      serverSnapshot = authoredSnapshot()
      serverSnapshot.spec = {
        ...discovered,
        provider: String(input.provider),
        profile: input.profile as SddSpec['profile'],
        control: input.control as SddSpec['control'],
        aggregateRevision: 5
      }
      serverArtifacts = [
        artifact(
          'specification',
          `.agentum/specs/${discovered.slug}/spec.md`,
          '# Refresh access tokens\n\n- RQ-001 Preserve imported intent.\n- AC-001 Imported intent is reviewed.'
        ),
        artifact(
          'plan',
          `.agentum/specs/${discovered.slug}/plan.json`,
          JSON.stringify({
            tasks: [
              {
                id: 'T-HISTORICAL',
                objective: 'Do not schedule this imported task',
                dependencies: [],
                acceptanceCriteria: ['AC-001'],
                risk: 'unknown',
                parallelSafe: false,
                verification: []
              }
            ]
          })
        ),
        artifact(
          'review',
          `.agentum/specs/${discovered.slug}/review.md`,
          'Historical review only.'
        )
      ]
      for (const entry of serverArtifacts.slice(1)) {
        entry.metadata.submittedBy = 'agentum:filesystem-discovery:attempt-1'
      }
      return {
        specId: discovered.specId,
        runId: RUN_ID,
        revision: 1,
        specRevision: discovered.currentRevision,
        specAggregateRevision: 5,
        phase: 'specification',
        status: 'waiting',
        nextAction: 'Spec approval required',
        authoritativePath: serverSnapshot.run.authoritativePath,
        preservedLaterArtifacts: ['plan.json', 'review.md'],
        downstreamDisposition: 'historical_unapproved_reopen_from_specification',
        approval: {
          approvalId: 'approval-1',
          purpose: 'specification',
          digest: 'sha256:approval-digest',
          status: 'pending'
        }
      }
    })

    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    expect(textOf(renderer!.root)).toContain('Configure a run for Refresh access tokens')
    expect(textOf(renderer!.root)).toContain('Rich browser evidence unavailable')

    await act(async () => button(renderer!.root, 'Configure Run').props.onClick())
    expect(textOf(renderer!.root)).toContain(
      'Repository-discovered design, plan, decisions, and review files stay historical'
    )
    await act(async () => button(renderer!.root, 'Create Run').props.onClick())
    await settle()

    expect(mocks.createSpecRun).toHaveBeenCalledTimes(1)
    expect(mocks.createSpecRun.mock.calls[0][0]).toBe(discovered.specId)
    expect(mocks.createSpecRun.mock.calls[0][1]).toMatchObject({
      expectedRevision: 4,
      profile: 'standard',
      control: 'guarded',
      provider: 'codex',
      baseRef: 'HEAD',
      sourceCheckout: 'require_clean'
    })
    expect(mocks.createSpecRun.mock.calls[0][1].requestId).toMatch(/^[0-9a-f-]{36}$/)
    expect(textOf(renderer!.root)).toContain('Spec approval required')

    await act(async () => button(renderer!.root, 'Plan').props.onClick())
    expect(textOf(renderer!.root)).toContain('Historical imported plan.json is preserved')
    expect(textOf(renderer!.root)).not.toContain('Do not schedule this imported task')
    await act(async () => button(renderer!.root, 'Tasks').props.onClick())
    expect(textOf(renderer!.root)).toContain('Historical imported plan.json task DAG is preserved')
    await act(async () => button(renderer!.root, 'Review').props.onClick())
    expect(textOf(renderer!.root)).toContain('Historical imported review.md is preserved')
    expect(textOf(renderer!.root)).not.toContain('Historical review only.')
    await act(async () => renderer!.unmount())
  })

  it('drives authoring, exact approval, recovery, durable refresh, every view, and delivery', async () => {
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()

    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    const sourceInputs = renderer!.root
      .findAllByType('input')
      .filter((entry) => entry.props.name === 'sdd-source')
    expect(sourceInputs.map((entry) => entry.props.value)).toEqual([
      'description',
      'socratic',
      'markdown',
      'github',
      'linear',
      'jira',
      'openspec',
      'empirical'
    ])
    expect(sourceInputs.map((entry) => entry.props.disabled)).toEqual([
      false,
      false,
      false,
      false,
      true,
      true,
      false,
      false
    ])
    expect(textOf(renderer!.root)).toContain('OAuth broker required')
    expect(textOf(renderer!.root)).toContain('Secure vault required')

    const provider = labeledControl(renderer!.root, 'Provider', 'select')
    expect(provider.findAllByType('option').map((entry) => entry.props.value)).toEqual([
      'codex',
      'claude',
      'agent',
      'gemini',
      'hermes',
      'opencode',
      'aider',
      'custom'
    ])
    expect(provider.findAllByType('option').filter((entry) => entry.props.disabled)).toHaveLength(7)
    expect(
      labeledControl(renderer!.root, 'Profile', 'select')
        .findAllByType('option')
        .map((entry) => entry.props.value)
    ).toEqual(['standard', 'high_risk'])
    expect(
      labeledControl(renderer!.root, 'Control', 'select')
        .findAllByType('option')
        .map((entry) => entry.props.value)
    ).toEqual(['guarded', 'interactive', 'autopilot'])

    await act(async () => {
      labeledControl(renderer!.root, 'Title', 'input').props.onChange({
        target: { value: 'discard me' }
      })
      button(renderer!.root, 'Cancel').props.onClick()
    })
    expect(mocks.createSpec).not.toHaveBeenCalled()
    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    expect(labeledControl(renderer!.root, 'Title', 'input').props.value).toBe('')

    await act(async () => {
      labeledControl(renderer!.root, 'Title', 'input').props.onChange({
        target: { value: 'Refresh access tokens' }
      })
    })
    await act(async () => {
      labeledControl(renderer!.root, 'Goal', 'textarea').props.onChange({
        target: {
          value: 'Refresh access tokens without interrupting active sessions'
        }
      })
    })
    await act(async () => button(renderer!.root, 'Create & Author').props.onClick())
    await settle()

    const createInput = mocks.createSpec.mock.calls[0][1]
    expect(mocks.createSpec.mock.calls[0][0]).toBe('repo-1')
    expect(createInput).toMatchObject({
      expectedRevision: 0,
      title: 'Refresh access tokens',
      goal: 'Refresh access tokens without interrupting active sessions',
      profile: 'standard',
      control: 'guarded',
      provider: 'codex',
      baseRef: 'HEAD',
      sourceCheckout: 'require_clean'
    })
    expect(createInput).not.toHaveProperty('source')
    expect(createInput.requestId).toMatch(/^[0-9a-f-]{36}$/)
    expect(textOf(renderer!.root)).toContain('RQ-001 Refresh tokens safely')
    expect(textOf(renderer!.root)).toContain('AC-001 Active sessions remain uninterrupted')

    const tabs = renderer!.root.findAll((entry) => entry.props.role === 'tab')
    expect(tabs).toHaveLength(7)
    expect(tabs.map((entry) => entry.props.tabIndex)).toEqual([-1, 0, -1, -1, -1, -1, -1])
    expect(new Set(tabs.map((entry) => entry.props['aria-controls'])).size).toBe(1)
    expect(tabs.every((entry) => typeof entry.props.id === 'string')).toBe(true)
    const tabPanel = renderer!.root.findByProps({ role: 'tabpanel' })
    expect(tabPanel.props['aria-labelledby']).toBe(tabs[1].props.id)

    await act(async () => button(renderer!.root, 'Overview').props.onClick())
    expect(textOf(renderer!.root)).toContain('Spec approval required')
    expect(textOf(renderer!.root)).toContain('sha256:approval-digest')
    await act(async () => button(renderer!.root, 'Approve exact digest').props.onClick())
    await settle()
    const approvalCommand = mocks.command.mock.calls[0][1]
    expect(approvalCommand).toMatchObject({
      type: 'decideApproval',
      expectedRevision: 1,
      approvalId: 'approval-1',
      digest: 'sha256:approval-digest',
      decision: 'approve'
    })
    expect(approvalCommand.requestId).toMatch(/^[0-9a-f-]{36}$/)

    await act(async () => button(renderer!.root, 'Pause').props.onClick())
    await settle()
    expect(mocks.toastError).toHaveBeenCalledWith('503 provider busy')
    const firstPause = mocks.command.mock.calls[1][1]
    await act(async () => button(renderer!.root, 'Pause').props.onClick())
    await settle()
    const retriedPause = mocks.command.mock.calls[2][1]
    expect(retriedPause.requestId).toBe(firstPause.requestId)
    expect(retriedPause.expectedRevision).toBe(2)
    await act(async () => button(renderer!.root, 'Resume').props.onClick())
    await settle()
    expect(mocks.command.mock.calls[3][1]).toMatchObject({
      type: 'resume',
      expectedRevision: 3
    })

    serverSnapshot = {
      ...serverSnapshot!,
      run: {
        ...serverSnapshot!.run,
        status: 'blocked',
        blocker: 'Verification command exceeded its bounded output limit.',
        aggregateRevision: 5
      }
    }
    serverEvents.push(event(5, 'sdd.run.blocked'))
    await act(async () => {
      subscription?.onEvent(serverEvents.at(-1)!)
      await new Promise((resolve) => globalThis.setTimeout(resolve, 180))
    })
    expect(textOf(renderer!.root)).toContain(
      'Verification command exceeded its bounded output limit.'
    )
    await act(async () => button(renderer!.root, 'Resolve block').props.onClick())
    await settle()
    expect(mocks.command.mock.calls[4][1]).toMatchObject({
      type: 'resolveBlock',
      expectedRevision: 5
    })

    serverSnapshot = {
      ...serverSnapshot!,
      run: {
        ...serverSnapshot!.run,
        phase: 'ready',
        status: 'succeeded',
        blocker: null,
        aggregateRevision: 7
      }
    }
    serverEvents.push(event(7, 'sdd.review.ready'))
    await act(async () => {
      subscription?.onEvent(serverEvents.at(-1)!)
      await new Promise((resolve) => globalThis.setTimeout(resolve, 180))
    })
    expect(button(renderer!.root, 'Preview delivery').props.disabled).toBe(false)

    const expectedViews: Record<string, string> = {
      Overview: 'Preview delivery',
      Spec: 'RQ-001 Refresh tokens safely',
      Plan: 'TSK-001',
      Tasks: 'Rotate access tokens',
      Evidence: 'sdd.verification.passed',
      Review: 'Independent review passed.',
      Activity: 'sdd.review.ready'
    }
    for (const [view, expected] of Object.entries(expectedViews)) {
      await act(async () => button(renderer!.root, view).props.onClick())
      expect(textOf(renderer!.root)).toContain(expected)
    }

    await act(async () => renderer!.unmount())
    const listCountBeforeRestart = mocks.listSpecs.mock.calls.length
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    expect(mocks.listSpecs.mock.calls.length).toBeGreaterThan(listCountBeforeRestart)
    expect(textOf(renderer!.root)).toContain('Refresh access tokens')
    expect(textOf(renderer!.root)).toContain('Ready')

    await act(async () => button(renderer!.root, 'Preview delivery').props.onClick())
    await settle()
    expect(mocks.command.mock.calls[5][1]).toEqual({
      type: 'previewDelivery',
      requestId: expect.stringMatching(/^[0-9a-f-]{36}$/),
      expectedRevision: 7,
      actions: [{ type: 'commit', message: 'Agentum: Refresh access tokens' }]
    })
    expect(textOf(renderer!.root)).toContain('Hash-bound delivery preview')
    expect(textOf(renderer!.root)).toContain('sha256:delivery-digest')
    expect(textOf(renderer!.root)).toContain('No remote selected')
    const deliveryChecks = renderer!.root
      .findByProps({ 'aria-label': 'Delivery preview' })
      .findAllByType('input')
      .filter((entry) => entry.props.type === 'checkbox')
    expect(deliveryChecks.map((entry) => [entry.props.checked, entry.props.disabled])).toEqual([
      [true, false],
      [false, true]
    ])
    await act(async () =>
      button(renderer!.root, 'Confirm selected delivery actions').props.onClick()
    )
    await settle()
    expect(mocks.command.mock.calls[6][1]).toMatchObject({
      type: 'confirmDelivery',
      expectedRevision: 8,
      previewToken: 'preview-token-bound-to-ready-hash',
      actions: ['commit']
    })
    expect(textOf(renderer!.root)).not.toContain('Hash-bound delivery preview')
    await act(async () => renderer!.unmount())
  })

  it('previews and revision-binds an OpenSpec source before create', async () => {
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    const openspec = renderer!.root
      .findAllByType('input')
      .find((entry) => entry.props.name === 'sdd-source' && entry.props.value === 'openspec')
    expect(openspec).toBeDefined()
    await act(async () => openspec!.props.onChange())
    await act(async () =>
      labeledControl(renderer!.root, 'Title', 'input').props.onChange({
        target: { value: 'Refresh sessions' }
      })
    )
    await act(async () =>
      labeledControl(renderer!.root, 'Change path', 'input').props.onChange({
        target: { value: 'openspec/changes/refresh-sessions' }
      })
    )
    await act(async () => button(renderer!.root, 'Preview source').props.onClick())
    await settle()
    expect(mocks.previewSddSource).toHaveBeenCalledWith('repo-1', 'Refresh sessions', {
      type: 'openspec',
      path: 'openspec/changes/refresh-sessions',
      expectedSourceRevision: undefined
    })
    expect(textOf(renderer!.root)).toContain('Immutable openspec snapshot')
    expect(textOf(renderer!.root)).toContain('design available')

    await act(async () => button(renderer!.root, 'Create & Author').props.onClick())
    await settle()
    expect(mocks.createSpec.mock.calls[0][1]).toMatchObject({
      source: {
        type: 'openspec',
        path: 'openspec/changes/refresh-sessions',
        expectedSourceRevision: 'sha256:source-revision'
      }
    })
    await act(async () => renderer!.unmount())
  })

  it('previews and revision-binds an Empirical feature with visible capability metadata', async () => {
    mocks.previewSddSource.mockResolvedValue({
      kind: 'empirical',
      title: 'Durable report export',
      markdown: '# Imported Empirical feature',
      sourceRevision: 'sha256:empirical-source-revision',
      sourcePath: '.empirical/specs/add-report-export',
      designAvailable: true,
      taskCount: 3,
      capabilities: ['report-export'],
      capabilityCount: 1,
      diagnostics: [
        {
          severity: 'info',
          code: 'empirical_artifact_intake_only',
          message: 'Agentum remains authoritative for execution and delivery.'
        }
      ],
      previewDigest: 'sha256:empirical-preview'
    })
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    const empirical = renderer!.root
      .findAllByType('input')
      .find((entry) => entry.props.name === 'sdd-source' && entry.props.value === 'empirical')
    expect(empirical).toBeDefined()
    expect(empirical!.props.disabled).toBe(false)
    await act(async () => empirical!.props.onChange())
    await act(async () =>
      labeledControl(renderer!.root, 'Title', 'input').props.onChange({
        target: { value: 'Durable report export' }
      })
    )
    await act(async () =>
      labeledControl(renderer!.root, 'Feature path', 'input').props.onChange({
        target: { value: '.empirical/specs/add-report-export' }
      })
    )
    await act(async () => button(renderer!.root, 'Preview source').props.onClick())
    await settle()
    expect(mocks.previewSddSource).toHaveBeenCalledWith('repo-1', 'Durable report export', {
      type: 'empirical',
      path: '.empirical/specs/add-report-export',
      expectedSourceRevision: undefined
    })
    const previewText = textOf(renderer!.root)
    expect(previewText).toContain('Immutable empirical snapshot')
    expect(previewText).toContain('3 imported tasks')
    expect(previewText).toContain('1 capability')
    expect(previewText).toContain('report-export')
    expect(previewText).toContain('design available')
    expect(previewText).toContain('Agentum remains authoritative for execution and delivery.')

    await act(async () => button(renderer!.root, 'Create & Author').props.onClick())
    await settle()
    expect(mocks.createSpec.mock.calls[0][1]).toMatchObject({
      source: {
        type: 'empirical',
        path: '.empirical/specs/add-report-export',
        expectedSourceRevision: 'sha256:empirical-source-revision'
      }
    })
    await act(async () => renderer!.unmount())
  })

  it('submits pasted Markdown through the closed source contract', async () => {
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    const markdown = renderer!.root
      .findAllByType('input')
      .find((entry) => entry.props.name === 'sdd-source' && entry.props.value === 'markdown')
    await act(async () => markdown!.props.onChange())
    await act(async () =>
      labeledControl(renderer!.root, 'Title', 'input').props.onChange({
        target: { value: 'Refresh sessions' }
      })
    )
    await act(async () =>
      renderer!.root.findByType('textarea').props.onChange({
        target: { value: '# Goal\n\nKeep active sessions online.' }
      })
    )
    await act(async () => button(renderer!.root, 'Create & Author').props.onClick())
    await settle()
    expect(mocks.createSpec.mock.calls[0][1]).toMatchObject({
      source: {
        type: 'markdown',
        markdown: '# Goal\n\nKeep active sessions online.'
      }
    })
    await act(async () => renderer!.unmount())
  })

  it('binds cancellation to the current aggregate revision', async () => {
    serverSnapshot = authoredSnapshot()
    serverSnapshot.approval = null
    serverSnapshot.run.phase = 'implementation'
    serverSnapshot.run.status = 'running'
    serverSnapshot.run.aggregateRevision = 11
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    await act(async () => button(renderer!.root, 'Cancel').props.onClick())
    await settle()
    expect(mocks.command).toHaveBeenCalledTimes(1)
    expect(mocks.command.mock.calls[0][1]).toMatchObject({
      type: 'cancel',
      expectedRevision: 11
    })
    expect(mocks.command.mock.calls[0][1].requestId).toMatch(/^[0-9a-f-]{36}$/)
    expect(textOf(renderer!.root)).toContain('canceled')
    await act(async () => renderer!.unmount())
  })

  it('submits requested changes against the exact pending approval digest', async () => {
    serverSnapshot = authoredSnapshot()
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()

    await act(async () => button(renderer!.root, 'Request changes').props.onClick())
    const reason = labeledControl(renderer!.root, 'Required changes', 'textarea')
    await act(async () => reason.props.onChange({ target: { value: 'Add an expired-token AC.' } }))
    await act(async () => button(renderer!.root, 'Request changes').props.onClick())
    await settle()

    expect(mocks.command).toHaveBeenCalledTimes(1)
    expect(mocks.command.mock.calls[0][1]).toMatchObject({
      type: 'decideApproval',
      expectedRevision: 1,
      approvalId: 'approval-1',
      digest: 'sha256:approval-digest',
      decision: 'reject',
      reason: 'Add an expired-token AC.'
    })
    expect(mocks.command.mock.calls[0][1].requestId).toMatch(/^[0-9a-f-]{36}$/)
    expect(textOf(renderer!.root)).toContain('blocked')
    await act(async () => renderer!.unmount())
  })

  it('offers and revision-binds every remaining lifecycle action', async () => {
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    const cases: Array<{
      phase: SddSnapshot['run']['phase']
      status: SddSnapshot['run']['status']
      revision: number
      label: string
      type: string
    }> = [
      {
        phase: 'specification',
        status: 'idle',
        revision: 21,
        label: 'Re-author',
        type: 'startAuthoring'
      },
      {
        phase: 'design',
        status: 'idle',
        revision: 31,
        label: 'Start',
        type: 'startRun'
      },
      {
        phase: 'verification',
        status: 'failed',
        revision: 41,
        label: 'Retry',
        type: 'retry'
      }
    ]

    for (const current of cases) {
      serverSnapshot = snapshotAt(current.phase, current.status, current.revision)
      mocks.command.mockClear()
      let renderer: ReactTestRenderer
      await act(async () => {
        renderer = create(
          <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
        )
      })
      await settle()
      await act(async () => button(renderer!.root, current.label).props.onClick())
      await settle()
      expect(mocks.command).toHaveBeenCalledTimes(1)
      expect(mocks.command.mock.calls[0][1]).toMatchObject({
        type: current.type,
        expectedRevision: current.revision
      })
      await act(async () => renderer!.unmount())
    }

    serverSnapshot = snapshotAt('ready', 'succeeded', 51)
    mocks.command.mockClear()
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    const phase = renderer!.root.findByProps({
      'aria-label': 'Phase to reopen'
    })
    await act(async () => phase.props.onChange({ target: { value: 'planning' } }))
    await act(async () => button(renderer!.root, 'Reopen').props.onClick())
    await settle()
    expect(mocks.command).toHaveBeenCalledTimes(1)
    expect(mocks.command.mock.calls[0][1]).toMatchObject({
      type: 'reopenPhase',
      expectedRevision: 51,
      phase: 'planning'
    })
    await act(async () => renderer!.unmount())
  })

  it('hides unsupported post-authoring actions and explains the checkpoint boundary', async () => {
    mocks.getSddCapabilities.mockResolvedValue({
      ...fullCapabilities,
      delivery: false,
      readyLifecycle: false
    })
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    const cases: Array<{
      snapshot: SddSnapshot
      unavailable: string[]
      available?: string
    }> = [
      {
        snapshot: snapshotAt('ready', 'succeeded', 61),
        unavailable: ['Preview delivery', 'Reopen']
      },
      {
        snapshot: snapshotAt('design', 'idle', 62),
        unavailable: ['Start']
      },
      {
        snapshot: snapshotAt('verification', 'failed', 63),
        unavailable: ['Retry']
      },
      {
        snapshot: snapshotAt('implementation', 'paused', 64),
        unavailable: ['Resume']
      },
      {
        snapshot: snapshotAt('implementation', 'blocked', 65),
        unavailable: ['Resolve block']
      },
      {
        snapshot: snapshotAt('specification', 'idle', 66),
        unavailable: ['Start', 'Retry', 'Preview delivery', 'Reopen'],
        available: 'Re-author'
      }
    ]

    for (const current of cases) {
      serverSnapshot = current.snapshot
      let renderer: ReactTestRenderer
      await act(async () => {
        renderer = create(
          <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
        )
      })
      await settle()
      expect(textOf(renderer!.root)).toContain(
        'This build supports the authoring checkpoint only; design through Ready is unavailable.'
      )
      for (const label of current.unavailable) {
        expect(
          renderer!.root.findAllByType('button').filter((entry) => textOf(entry).trim() === label)
        ).toHaveLength(0)
      }
      if (current.available)
        expect(button(renderer!.root, current.available).props.disabled).toBe(false)
      await act(async () => renderer!.unmount())
    }
  })

  it('fails closed when provider and source capabilities cannot be verified', async () => {
    mocks.getSddCapabilities.mockRejectedValue(new Error('capability service offline'))
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    expect(textOf(renderer!.root)).toContain(
      'capability service offline. Creation is disabled until capabilities can be verified.'
    )
    expect(button(renderer!.root, 'Create & Author').props.disabled).toBe(true)
    expect(mocks.createSpec).not.toHaveBeenCalled()
    await act(async () => renderer!.unmount())

    serverSnapshot = authoredSnapshot()
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    expect(textOf(renderer!.root)).toContain(
      'Run Center capabilities could not be verified. New work, resume, retry, approval, and delivery actions are disabled; pause and cancel remain available.'
    )
    expect(button(renderer!.root, 'Approve exact digest').props.disabled).toBe(true)
    expect(
      renderer!.root.findAllByType('button').filter((entry) => textOf(entry).trim() === 'Re-author')
    ).toHaveLength(0)
    expect(button(renderer!.root, 'Cancel').props.disabled).toBe(false)
    await act(async () => renderer!.unmount())
  })

  it('shows the Windows remote-client-only boundary and disables local authoring', async () => {
    mocks.getSddCapabilities.mockResolvedValue({
      ...fullCapabilities,
      localProviderExecution: {
        available: false,
        boundary: 'remote_client_only',
        reasonCode: 'windows_agentum_sandbox_unavailable',
        reason:
          'Windows local SDD is disabled because Agentum does not provide a restricted-token/AppContainer filesystem sandbox; provider-native sandbox flags and process-tree cancellation are not isolation.'
      }
    })
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    expect(textOf(renderer!.root)).toContain('restricted-token/AppContainer filesystem sandbox')
    expect(textOf(renderer!.root)).toContain('provider-native sandbox flags')
    expect(button(renderer!.root, 'Create & Author').props.disabled).toBe(true)
    expect(mocks.createSpec).not.toHaveBeenCalled()
    await act(async () => renderer!.unmount())
  })

  it('creates through a verified remote worker when restart-safe projection is ready', async () => {
    mocks.getSddRemoteCapability.mockResolvedValue({
      schemaVersion: 1,
      available: true,
      workerReady: true,
      hostId: '00000000-0000-4000-8000-000000000001',
      repositoryIdentitySha256: 'a'.repeat(64),
      workerVersion: '0.97.0',
      repositoryRegistered: true,
      artifactSetId: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
      baseCommit: 'b'.repeat(40),
      providerReady: true,
      projectionReady: true,
      blockers: [],
      localFallback: false,
      reason: null
    })
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-remote" projectName="remote-shop" presentation="page" />
      )
    })
    await settle()
    expect(mocks.getSddRemoteCapability).toHaveBeenCalledWith('repo-remote', 'codex', 'HEAD')

    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    await act(async () => {
      labeledControl(renderer!.root, 'Title', 'input').props.onChange({
        target: { value: 'Remote refresh' }
      })
    })
    await act(async () => {
      labeledControl(renderer!.root, 'Goal', 'textarea').props.onChange({
        target: { value: 'Refresh remotely' }
      })
    })
    expect(button(renderer!.root, 'Create & Author').props.disabled).toBe(false)
    await act(async () => button(renderer!.root, 'Create & Author').props.onClick())
    await settle()
    expect(mocks.createSpec).toHaveBeenCalledWith(
      'repo-remote',
      expect.objectContaining({
        title: 'Remote refresh',
        goal: 'Refresh remotely',
        provider: 'codex',
        baseRef: 'HEAD',
        sourceCheckout: 'require_clean'
      })
    )
    await act(async () => renderer!.unmount())
  })

  it('does not let a stale command response regress the visible aggregate revision', async () => {
    serverSnapshot = snapshotAt('implementation', 'running', 71)
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()

    mocks.command.mockReset()
    mocks.command
      .mockResolvedValueOnce({
        runId: RUN_ID,
        revision: 70,
        phase: 'implementation',
        status: 'running'
      })
      .mockImplementationOnce(async (_runId: string, body: Record<string, unknown>) => ({
        runId: RUN_ID,
        revision: Number(body.expectedRevision) + 1,
        phase: 'implementation',
        status: 'paused'
      }))
    mocks.getRun.mockRejectedValue(new Error('refresh temporarily unavailable'))

    await act(async () => button(renderer!.root, 'Pause').props.onClick())
    await settle()
    await act(async () => button(renderer!.root, 'Pause').props.onClick())
    await settle()
    expect(mocks.command.mock.calls[0][1]).toMatchObject({
      expectedRevision: 71
    })
    expect(mocks.command.mock.calls[1][1]).toMatchObject({
      expectedRevision: 71
    })
    expect(textOf(renderer!.root)).toContain('The command succeeded. Refresh Run Center')
    await act(async () => renderer!.unmount())
  })

  it('binds Linear and Jira intake to capability-selected secure connections', async () => {
    const capabilities = structuredClone(fullCapabilities)
    capabilities.sources = capabilities.sources.map((source) => {
      if (source.id === 'linear') {
        return { ...source, available: true, preview: true, connectionId: 'linear-workspace-1' }
      }
      if (source.id === 'jira') {
        return {
          ...source,
          available: true,
          preview: true,
          brokerConfigured: true,
          connection: {
            connectionId: 'jira-account-1',
            displayName: 'Example',
            sites: [{ id: 'site-1', name: 'Example', url: 'https://example.atlassian.net' }],
            selectedSiteId: 'site-1',
            credentialRevision: 1,
            authKind: 'oauth',
            grantedScopes: ['read:jira-work', 'write:jira-work', 'offline_access'],
            deliveryWriteAuthorized: true
          }
        }
      }
      return source
    })
    mocks.getSddCapabilities.mockResolvedValue(capabilities)
    mocks.previewSddSource.mockResolvedValue({
      kind: 'linear',
      title: 'Imported item',
      markdown: '# Imported item',
      sourceRevision: 'sha256:work-item',
      sourcePath: 'ENG-1',
      designAvailable: false,
      taskCount: 0,
      capabilities: [],
      capabilityCount: 0,
      diagnostics: [],
      previewDigest: 'sha256:preview'
    })
    const { default: SddWorkspaceBar } = await import('./SddWorkspaceBar')
    let renderer: ReactTestRenderer
    await act(async () => {
      renderer = create(
        <SddWorkspaceBar repoId="repo-1" projectName="demo-shop" presentation="page" />
      )
    })
    await settle()
    await act(async () => button(renderer!.root, 'New Spec', 0).props.onClick())
    await act(async () =>
      labeledControl(renderer!.root, 'Title', 'input').props.onChange({
        target: { value: 'Imported work item' }
      })
    )
    const sourceRadio = (kind: string): ReactTestInstance =>
      renderer!.root
        .findAllByType('input')
        .find((entry) => entry.props.name === 'sdd-source' && entry.props.value === kind)!

    await act(async () => sourceRadio('linear').props.onChange())
    await act(async () =>
      labeledControl(renderer!.root, 'Linear URL or key', 'input').props.onChange({
        target: { value: 'ENG-1' }
      })
    )
    await act(async () => button(renderer!.root, 'Preview source').props.onClick())
    await settle()
    expect(mocks.previewSddSource).toHaveBeenLastCalledWith('repo-1', 'Imported work item', {
      type: 'linear',
      identifier: 'ENG-1',
      connectionId: 'linear-workspace-1',
      expectedSourceRevision: undefined
    })

    await act(async () => sourceRadio('jira').props.onChange())
    await act(async () =>
      labeledControl(renderer!.root, 'Jira URL or key', 'input').props.onChange({
        target: { value: 'OPS-7' }
      })
    )
    await act(async () => button(renderer!.root, 'Preview source').props.onClick())
    await settle()
    expect(mocks.previewSddSource).toHaveBeenLastCalledWith('repo-1', 'Imported work item', {
      type: 'jira',
      connectionId: 'jira-account-1',
      siteId: 'site-1',
      key: 'OPS-7',
      expectedSourceRevision: undefined
    })
    await act(async () => renderer!.unmount())
  })
})
