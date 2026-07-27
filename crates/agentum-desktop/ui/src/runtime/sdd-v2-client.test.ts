import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('./server-http', () => ({
  getBlob: vi.fn(),
  getJson: vi.fn(),
  postJson: vi.fn(),
  qs: (values: Record<string, number>) => `?after=${values.after}`
}))
vi.mock('./server-endpoint', () => ({
  getServerEndpoint: vi.fn(async () => ({
    url: 'http://agentum.test',
    token: 'secret token'
  })),
  wsUrl: vi.fn(async (path: string) => `ws://agentum.test${path}`)
}))
vi.mock('./reconnect-backoff', () => ({ reconnectBackoffMs: () => 1 }))

import { getBlob, getJson, postJson } from './server-http'
import {
  command,
  connectJiraApiToken,
  createSpec,
  createSpecRun,
  getEvents,
  getBrowserEvidenceBlob,
  getSddRemoteCapability,
  listJiraConnections,
  listSpecs,
  previewSddSource,
  redeemJiraOauth,
  selectJiraSite,
  startJiraOauth,
  subscribeSddEvents
} from './sdd-v2-client'

beforeEach(() => vi.clearAllMocks())

describe('Agentum SDD v2 client', () => {
  it('keeps repository and run identities in path segments', async () => {
    vi.mocked(getJson).mockResolvedValueOnce({ specs: [] }).mockResolvedValueOnce({ events: [] })
    vi.mocked(postJson).mockResolvedValue({})

    await listSpecs('repo / one')
    await getEvents('run / one', 42)
    await command('run / one', {
      type: 'pause',
      requestId: 'r',
      expectedRevision: 2
    })

    expect(getJson).toHaveBeenNthCalledWith(1, '/api/sdd/v2/repos/repo%20%2F%20one/specs')
    expect(getJson).toHaveBeenNthCalledWith(2, '/api/sdd/v2/runs/run%20%2F%20one/events?after=42')
    expect(postJson).toHaveBeenCalledWith('/api/sdd/v2/runs/run%20%2F%20one/commands', {
      type: 'pause',
      requestId: 'r',
      expectedRevision: 2
    })
  })

  it('loads immutable evidence bytes through an encoded run/evidence/digest path', async () => {
    vi.mocked(getBlob).mockResolvedValue(new Blob(['png']))
    await getBrowserEvidenceBlob('run / one', 'evidence / one', 'a'.repeat(64))
    expect(getBlob).toHaveBeenCalledWith(
      `/api/sdd/v2/runs/run%20%2F%20one/evidence/evidence%20%2F%20one/blobs/${'a'.repeat(64)}`
    )
  })

  it('probes repository-scoped remote SDD without a local execution fallback', async () => {
    vi.mocked(getJson).mockResolvedValue({
      schemaVersion: 1,
      available: false,
      reason: 'desktop_projection_unavailable',
      localFallback: false
    })

    await getSddRemoteCapability('repo / one', 'custom:remote provider')

    expect(getJson).toHaveBeenCalledWith(
      '/api/sdd/v2/repos/repo%20%2F%20one/remote-capability?provider=custom%3Aremote+provider&baseRef=HEAD'
    )
  })

  it('sends create as an explicit revision-zero idempotent mutation', async () => {
    vi.mocked(postJson).mockResolvedValue({ runId: 'run-1' })
    const input = {
      requestId: 'request-1',
      expectedRevision: 0 as const,
      title: 'Refresh tokens',
      goal: 'Do it safely',
      profile: 'standard' as const,
      control: 'guarded' as const,
      provider: 'codex',
      baseRef: 'HEAD',
      sourceCheckout: 'require_clean' as const
    }
    await createSpec('repo-1', input)
    expect(postJson).toHaveBeenCalledWith('/api/sdd/v2/repos/repo-1/specs', input)
  })

  it('configures a discovered specification through the closed run contract', async () => {
    vi.mocked(postJson).mockResolvedValue({ runId: 'run-1' })
    const input = {
      requestId: 'request-run-1',
      expectedRevision: 4,
      profile: 'high_risk' as const,
      control: 'interactive' as const,
      provider: 'claude',
      baseRef: 'main',
      sourceCheckout: 'snapshot' as const
    }
    await createSpecRun('SPC / one', input)
    expect(postJson).toHaveBeenCalledWith('/api/sdd/v2/specs/SPC%20%2F%20one/runs', input)
  })

  it('previews a typed source without exposing caller-authored provenance', async () => {
    vi.mocked(postJson).mockResolvedValue({ sourceRevision: 'sha256:one' })
    await previewSddSource('repo / one', 'Refresh sessions', {
      type: 'openspec',
      path: 'openspec/changes/refresh-sessions'
    })
    expect(postJson).toHaveBeenCalledWith('/api/sdd/v2/repos/repo%20%2F%20one/sources/preview', {
      title: 'Refresh sessions',
      source: { type: 'openspec', path: 'openspec/changes/refresh-sessions' }
    })
  })

  it('uses closed Jira credential and site-selection contracts', async () => {
    vi.mocked(postJson)
      .mockResolvedValueOnce({ flowId: 'flow-1', revision: 1 })
      .mockResolvedValueOnce({ connection: { connectionId: 'jira-1' } })
      .mockResolvedValueOnce({ connection: { connectionId: 'jira-1' } })
      .mockResolvedValueOnce({ connection: { connectionId: 'jira-local-1' } })
    vi.mocked(getJson).mockResolvedValueOnce({ connections: [] })

    await startJiraOauth('request-start')
    await redeemJiraOauth('request-redeem', 'flow-1', 1)
    await listJiraConnections()
    await selectJiraSite('jira / one', {
      requestId: 'request-site',
      siteId: 'site-1',
      expectedCredentialRevision: 2
    })
    await connectJiraApiToken({
      requestId: 'request-token',
      email: 'operator@example.com',
      apiToken: 'synthetic-test-token',
      siteUrl: 'https://team.atlassian.net',
      acknowledgeRisk: true,
      expectedRevision: 0
    })

    expect(postJson).toHaveBeenNthCalledWith(1, '/api/sdd/v2/integrations/jira/oauth/start', {
      requestId: 'request-start',
      expectedRevision: 0
    })
    expect(postJson).toHaveBeenNthCalledWith(2, '/api/sdd/v2/integrations/jira/oauth/redeem', {
      requestId: 'request-redeem',
      flowId: 'flow-1',
      expectedRevision: 1
    })
    expect(getJson).toHaveBeenCalledWith('/api/sdd/v2/integrations/jira/connections')
    expect(postJson).toHaveBeenNthCalledWith(
      3,
      '/api/sdd/v2/integrations/jira/connections/jira%20%2F%20one/select-site',
      {
        requestId: 'request-site',
        siteId: 'site-1',
        expectedCredentialRevision: 2
      }
    )
    expect(postJson).toHaveBeenNthCalledWith(4, '/api/sdd/v2/integrations/jira/api-token/connect', {
      requestId: 'request-token',
      email: 'operator@example.com',
      apiToken: 'synthetic-test-token',
      siteUrl: 'https://team.atlassian.net',
      acknowledgeRisk: true,
      expectedRevision: 0
    })
  })

  it('binds live events to one repository and resumes from the durable cursor', async () => {
    class FakeWebSocket {
      static instances: FakeWebSocket[] = []
      readonly url: string
      readonly listeners = new Map<string, Array<(event: { data?: string }) => void>>()
      closed = false

      constructor(url: string) {
        this.url = url
        FakeWebSocket.instances.push(this)
      }

      addEventListener(type: string, listener: (event: { data?: string }) => void): void {
        this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener])
      }

      emit(type: string, data?: string): void {
        for (const listener of this.listeners.get(type) ?? []) listener({ data })
      }

      close(): void {
        this.closed = true
      }
    }
    vi.stubGlobal('WebSocket', FakeWebSocket)
    const received: number[] = []
    const unsubscribe = subscribeSddEvents({
      repoId: 'repo / one',
      after: 7,
      onEvent: (event) => received.push(event.cursor)
    })
    await vi.waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1))
    const socket = FakeWebSocket.instances[0]
    expect(socket.url).toContain('repoId=repo+%2F+one')
    expect(socket.url).toContain('after=7')
    expect(socket.url).toContain('token=secret+token')

    socket.emit('message', JSON.stringify({ cursor: 8, repoId: 'repo-2' }))
    socket.emit('message', JSON.stringify({ cursor: 8, repoId: 'repo / one' }))
    socket.emit('message', JSON.stringify({ cursor: 8, repoId: 'repo / one' }))
    expect(received).toEqual([8])

    unsubscribe()
    expect(socket.closed).toBe(true)
  })
})
