import { getServerEndpoint, wsUrl } from './server-endpoint'
import { reconnectBackoffMs } from './reconnect-backoff'
import { getBlob, getJson, postJson, qs } from './server-http'

export type SddPhase =
  | 'specification'
  | 'design'
  | 'planning'
  | 'implementation'
  | 'verification'
  | 'review'
  | 'ready'
  | 'delivery'
  | 'completed'

export type SddRunStatus =
  | 'idle'
  | 'queued'
  | 'running'
  | 'waiting'
  | 'retry_scheduled'
  | 'pausing'
  | 'paused'
  | 'blocked'
  | 'canceling'
  | 'canceled'
  | 'failed'
  | 'succeeded'

export type SddSpec = {
  specId: string
  repoId: string
  title: string
  slug: string
  profile: 'standard' | 'high_risk'
  control: 'guarded' | 'interactive' | 'autopilot'
  provider: string
  currentRevision: number
  aggregateRevision: number
  createdAt: string
  updatedAt: string
}

export type SddRun = {
  runId: string
  specId: string
  repoId: string
  phase: SddPhase
  status: SddRunStatus
  aggregateRevision: number
  baseRef: string
  baseCommit: string
  branchName: string
  authoritativePath: string
  workspaceFingerprint: string
  policyJson?: string
  blocker: string | null
  quarantined: number
  createdAt: string
  updatedAt: string
}

export type SddApproval = {
  approvalId: string
  runId: string
  purpose: string
  digest: string
  requestedRevision: number
  requestedBy: string
  status: string
  createdAt: string
}

export type SddArtifactMetadata = {
  artifactRevisionId: string
  runId: string
  specId: string
  kind: string
  revision: number
  specRevision: number
  relativePath: string
  contentHash: string
  submittedBy: string
  evidenceDigest?: string | null
  evidenceManifestHashesJson?: string | null
  createdAt: string
}

export type SddBrowserEvidenceManifest = {
  schemaVersion: number
  evidenceId: string
  runId: string
  attemptId: string
  checkId: string
  specRevision: number
  capturedAt: string
  workspaceFingerprint: string
  target: {
    origin: string
    path: string
    pathRedacted: boolean
    queryRedacted: boolean
  }
  browser: {
    name: string
    version: string
    viewportWidth: number
    viewportHeight: number
    deviceScaleMilli: number
  }
  captures: Array<{
    kind: 'screenshot' | 'dom_snapshot' | 'accessibility_tree' | 'trace'
    sha256: string
    byteLength: number
    mediaType: string
  }>
  assertions: Array<{
    id: string
    status: 'passed' | 'failed'
    acceptanceCriteria: string[]
    evidenceSha256: string[]
  }>
  console: {
    coverage: 'none' | 'main_document' | 'full_context'
    errors: number
    warnings: number
    transcriptSha256: string
  }
  network: {
    coverage: 'none' | 'main_document' | 'full_context'
    requests: number
    failedRequests: number
    transcriptSha256: string
  }
}

export type SddBrowserEvidence = {
  evidenceId: string
  runId: string
  attemptId: string
  grantId: string
  specRevision: number
  checkId: string
  manifestSha256: string
  evidence: SddBrowserEvidenceManifest
  status: 'passed' | 'failed'
  submittedBy: string
  capturedAt: string
  createdAt: string
  blobs: Array<{
    sha256: string
    byteLength: number
    mediaType: string
    storageRelativePath: string
    role: 'capture' | 'console_transcript' | 'network_transcript'
  }>
}

export type SddSnapshot = {
  spec: SddSpec
  run: SddRun
  artifacts: SddArtifactMetadata[]
  approval: SddApproval | null
  browserEvidence: SddBrowserEvidence[]
}

export type SddArtifact = {
  metadata: SddArtifactMetadata
  content: string
  externallyModified: boolean
  actualContentHash: string
}

export type SddEvent = {
  cursor: number
  eventId: string
  repoId: string
  specId: string | null
  runId: string | null
  revision: number
  kind: string
  payload: unknown
  createdAt: string
}

export type CreateSpecInput = {
  requestId: string
  expectedRevision: 0
  title: string
  goal: string
  profile: 'standard' | 'high_risk'
  control: 'guarded' | 'interactive' | 'autopilot'
  provider: string
  baseRef: string
  sourceCheckout: 'require_clean' | 'committed_base' | 'snapshot'
  source?: SddSourceReference
}

export type SddSourceKind =
  'description' | 'socratic' | 'markdown' | 'github' | 'linear' | 'jira' | 'openspec'

export type SddSourceReference =
  | { type: 'socratic'; context: string }
  | { type: 'markdown'; markdown: string }
  | { type: 'github'; url: string; expectedSourceRevision?: string }
  | {
      type: 'linear'
      identifier: string
      connectionId?: string
      expectedSourceRevision?: string
    }
  | {
      type: 'jira'
      connectionId: string
      siteId: string
      key: string
      expectedSourceRevision?: string
    }
  | { type: 'openspec'; path: string; expectedSourceRevision?: string }

export type SddSourcePreview = {
  kind: Exclude<SddSourceKind, 'description'>
  title: string
  markdown: string
  sourceRevision: string
  sourcePath: string
  externalReference?: {
    provider: string
    connectionId: string
    siteId?: string
    externalId: string
    key?: string
    url: string
    sourceRevision: string
  }
  designAvailable: boolean
  taskCount: number
  diagnostics: Array<{
    severity: 'info' | 'warning' | 'error'
    code: string
    message: string
    path?: string
  }>
  previewDigest: string
}

export type SddCapability = {
  id: string
  available: boolean
  reason?: string
  label?: string
  [key: string]: unknown
}

export type SddCapabilities = {
  schemaVersion: number
  providers: SddCapability[]
  providerAliases: Record<string, string>
  localProviderExecution: {
    available: boolean
    boundary: 'local_sandboxed' | 'remote_client_only' | 'unavailable'
    mechanism?: 'bubblewrap' | 'macos_seatbelt'
    reasonCode?: string
    reason?: string
  }
  sources: SddCapability[]
  remoteLifecycle: boolean
  remoteLifecycleReason: string
  remoteWorker: {
    schemaVersion: number
    protocol: 'agentum-sdd-v1'
    projectionReady: boolean
    blockers: string[]
    automaticallyDeployed: boolean
  }
  delivery: boolean
  readyLifecycle: boolean
  browserEvidence: Pick<SddCapability, 'available' | 'reason'>
}

export type SddRemoteCapability = {
  schemaVersion: number
  available: boolean
  reason: string | null
  workerReady?: boolean
  hostId?: string
  repositoryIdentitySha256?: string
  workerVersion?: string
  repositoryRegistered?: boolean
  artifactSetId?: string
  baseCommit?: string
  providerReady?: boolean
  projectionReady?: boolean
  blockers?: string[]
  localFallback?: false
}

export type JiraSite = {
  id: string
  name: string
  url: string
}

export type JiraConnection = {
  connectionId: string
  displayName: string
  sites: JiraSite[]
  selectedSiteId: string
  credentialRevision: number
  authKind: 'oauth' | 'api_token'
  grantedScopes: string[]
  deliveryWriteAuthorized: boolean
}

export type JiraOauthStart = {
  flowId: string
  revision: number
  authorizationUrl: string
  expiresAt: string
}

export type CreateSpecResult = {
  specId: string
  runId: string
  revision: number
  specRevision: number
  phase: SddPhase
  status: SddRunStatus
  nextAction: string
  artifactSetId: string
  authoritativePath: string
  approval: {
    approvalId: string
    purpose: string
    digest: string
    status: string
  }
}

export type CreateSpecRunInput = {
  requestId: string
  expectedRevision: number
  profile: 'standard' | 'high_risk'
  control: 'guarded' | 'interactive' | 'autopilot'
  provider: string
  baseRef: string
  sourceCheckout: 'require_clean' | 'committed_base' | 'snapshot'
}

export type CreateSpecRunResult =
  | {
      specId: string
      runId: string
      revision: number
      specRevision: number
      specAggregateRevision: number
      phase: 'specification'
      status: 'waiting'
      nextAction: string
      authoritativePath: string
      preservedLaterArtifacts: string[]
      downstreamDisposition: 'historical_unapproved_reopen_from_specification'
      approval: {
        approvalId: string
        purpose: 'specification'
        digest: string
        status: 'pending'
      }
    }
  | { run: SddRun; reused: true }

export type SddCommandResult = {
  runId: string
  revision: number
  phase?: SddPhase
  status?: SddRunStatus
  decision?: string
  previewToken?: string
  digest?: string
  expiresAt?: string
  summary?: string
  actions?: SddDeliveryAction[]
}

export type SddDeliveryAction = {
  id: string
  type:
    | 'commit'
    | 'push'
    | 'pull_request'
    | 'tracker_comment'
    | 'tracker_status'
    | 'tracker_field_update'
    | 'release'
    | 'openspec_export'
  label?: string
  description?: string
  enabled?: boolean
  blockedReason?: string
  preview?: unknown
}

export type SddTrackerFieldValue =
  | { type: 'text'; value: string }
  | { type: 'number'; value: number }
  | { type: 'boolean'; value: boolean }
  | { type: 'option'; optionId: string }
  | { type: 'user'; accountId: string }
  | { type: 'clear' }

export type SddDeliveryIntent =
  | { type: 'commit'; message: string }
  | { type: 'push'; remote: string }
  | { type: 'pullRequest'; title: string; body: string; base: string }
  | { type: 'trackerComment'; body: string }
  | { type: 'trackerStatus'; status: string; transitionId?: string }
  | { type: 'trackerFieldUpdate'; fieldId: string; value: SddTrackerFieldValue }
  | {
      type: 'release'
      tag: string
      name: string
      notes: string
      prerelease: boolean
    }
  | { type: 'openSpecExport' }

export async function listSpecs(repoId: string): Promise<SddSpec[]> {
  const response = await getJson<{ specs: SddSpec[] }>(
    `/api/sdd/repos/${encodeURIComponent(repoId)}/specs`
  )
  return response.specs
}

export async function getSddCapabilities(): Promise<SddCapabilities> {
  return getJson('/api/sdd/capabilities')
}

export async function getSddRemoteCapability(
  repoId: string,
  provider: string,
  baseRef = 'HEAD'
): Promise<SddRemoteCapability> {
  const query = new URLSearchParams({ provider, baseRef })
  return getJson(
    `/api/sdd/repos/${encodeURIComponent(repoId)}/remote-capability?${query.toString()}`
  )
}

export async function startJiraOauth(requestId: string): Promise<JiraOauthStart> {
  return postJson('/api/sdd/integrations/jira/oauth/start', {
    requestId,
    expectedRevision: 0
  })
}

export async function redeemJiraOauth(
  requestId: string,
  flowId: string,
  expectedRevision: number
): Promise<JiraConnection> {
  const result = await postJson<{ connection: JiraConnection }>(
    '/api/sdd/integrations/jira/oauth/redeem',
    { requestId, flowId, expectedRevision }
  )
  return result.connection
}

export async function listJiraConnections(): Promise<JiraConnection[]> {
  const result = await getJson<{ connections: JiraConnection[] }>(
    '/api/sdd/integrations/jira/connections'
  )
  return result.connections
}

export async function selectJiraSite(
  connectionId: string,
  input: {
    requestId: string
    siteId: string
    expectedCredentialRevision: number
  }
): Promise<JiraConnection> {
  const result = await postJson<{ connection: JiraConnection }>(
    `/api/sdd/integrations/jira/connections/${encodeURIComponent(connectionId)}/select-site`,
    input
  )
  return result.connection
}

export async function connectJiraApiToken(input: {
  requestId: string
  email: string
  apiToken: string
  siteUrl: string
  acknowledgeRisk: true
  expectedRevision: number
}): Promise<JiraConnection> {
  const result = await postJson<{ connection: JiraConnection }>(
    '/api/sdd/integrations/jira/api-token/connect',
    input
  )
  return result.connection
}

export async function createSpec(
  repoId: string,
  input: CreateSpecInput
): Promise<CreateSpecResult> {
  return postJson(`/api/sdd/repos/${encodeURIComponent(repoId)}/specs`, input)
}

export async function previewSddSource(
  repoId: string,
  title: string,
  source: SddSourceReference
): Promise<SddSourcePreview> {
  return postJson(`/api/sdd/repos/${encodeURIComponent(repoId)}/sources/preview`, {
    title,
    source
  })
}

export async function getSpec(
  specId: string
): Promise<{ spec: SddSpec; run: SddRun | null; runs?: SddRun[] }> {
  return getJson(`/api/sdd/specs/${encodeURIComponent(specId)}`)
}

export async function createSpecRun(
  specId: string,
  input: CreateSpecRunInput
): Promise<CreateSpecRunResult> {
  return postJson(`/api/sdd/specs/${encodeURIComponent(specId)}/runs`, input)
}

export async function getRun(runId: string): Promise<SddSnapshot> {
  return getJson(`/api/sdd/runs/${encodeURIComponent(runId)}`)
}

export async function getArtifacts(runId: string): Promise<SddArtifact[]> {
  const response = await getJson<{ artifacts: SddArtifact[] }>(
    `/api/sdd/runs/${encodeURIComponent(runId)}/artifacts`
  )
  return response.artifacts
}

export async function getBrowserEvidenceBlob(
  runId: string,
  evidenceId: string,
  sha256: string
): Promise<Blob> {
  return getBlob(
    `/api/sdd/runs/${encodeURIComponent(runId)}/evidence/${encodeURIComponent(evidenceId)}/blobs/${encodeURIComponent(sha256)}`
  )
}

export async function getEvents(runId: string, after = 0): Promise<SddEvent[]> {
  const response = await getJson<{ events: SddEvent[] }>(
    `/api/sdd/runs/${encodeURIComponent(runId)}/events${qs({ after })}`
  )
  return response.events
}

export async function command<T extends object>(runId: string, body: T): Promise<SddCommandResult> {
  return postJson(`/api/sdd/runs/${encodeURIComponent(runId)}/commands`, body)
}

export type SddEventSubscription = {
  repoId: string
  after?: number
  onEvent: (event: SddEvent) => void
  onOpen?: () => void
  onError?: () => void
}

/**
 * Subscribe to the durable repository event stream. The cursor advances only
 * after a valid event for the requested repository, so reconnects replay any
 * frame that was not delivered. Calling the returned function permanently
 * closes the socket and cancels a scheduled reconnect.
 */
export function subscribeSddEvents(subscription: SddEventSubscription): () => void {
  let closed = false
  let socket: WebSocket | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  let attempt = 0
  let cursor = Math.max(0, subscription.after ?? 0)

  const scheduleReconnect = (): void => {
    if (closed || reconnectTimer) return
    attempt += 1
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      void connect()
    }, reconnectBackoffMs(attempt))
  }

  const connect = async (): Promise<void> => {
    try {
      const [{ token }, base] = await Promise.all([
        getServerEndpoint(),
        wsUrl('/api/sdd/events')
      ])
      if (closed) return
      const params = new URLSearchParams({
        repoId: subscription.repoId,
        after: String(cursor)
      })
      if (token) params.set('token', token)
      const next = new WebSocket(`${base}?${params.toString()}`)
      socket = next
      next.addEventListener('open', () => {
        if (closed || socket !== next) return
        attempt = 0
        subscription.onOpen?.()
      })
      next.addEventListener('message', (frame) => {
        if (closed || socket !== next || typeof frame.data !== 'string') return
        let event: SddEvent
        try {
          event = JSON.parse(frame.data) as SddEvent
        } catch {
          return
        }
        if (
          event.repoId !== subscription.repoId ||
          !Number.isSafeInteger(event.cursor) ||
          event.cursor <= cursor
        ) {
          return
        }
        cursor = event.cursor
        subscription.onEvent(event)
      })
      next.addEventListener('error', () => subscription.onError?.())
      next.addEventListener('close', () => {
        if (socket !== next) return
        socket = null
        scheduleReconnect()
      })
    } catch {
      subscription.onError?.()
      scheduleReconnect()
    }
  }

  void connect()
  return () => {
    closed = true
    if (reconnectTimer) clearTimeout(reconnectTimer)
    reconnectTimer = null
    const active = socket
    socket = null
    active?.close()
  }
}
