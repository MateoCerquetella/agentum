import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import {
  Check,
  ChevronDown,
  ChevronUp,
  CircleAlert,
  FilePlus2,
  Loader2,
  RefreshCw,
  RotateCcw,
  X
} from 'lucide-react'
import { toast } from 'sonner'

import { api } from '@/tauri'
import { subscribeNewSpecPrefill } from '@/lib/sdd-new-spec-entry'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import {
  command,
  connectJiraApiToken,
  createSpec,
  createSpecRun,
  getArtifacts,
  getBrowserEvidenceBlob,
  getEvents,
  getRun,
  getSddCapabilities,
  getSddRemoteCapability,
  getSpec,
  listSpecs,
  listJiraConnections,
  previewSddSource,
  redeemJiraOauth,
  selectJiraSite,
  startJiraOauth,
  subscribeSddEvents,
  type SddArtifact,
  type SddBrowserEvidence,
  type SddCommandResult,
  type SddCapabilities,
  type SddDeliveryAction,
  type SddDeliveryIntent,
  type SddEvent,
  type SddPhase,
  type SddRemoteCapability,
  type SddRun,
  type SddSnapshot,
  type SddSourceKind,
  type SddSourcePreview,
  type SddSourceReference,
  type SddSpec,
  type JiraConnection,
  type JiraOauthStart
} from '@/runtime/sdd-client'
import {
  SOURCE_OPTIONS,
  appendDurableEvent,
  approvalLabel,
  availableRunActions,
  parsePlanTasks,
  phaseLabel,
  selectableDeliveryActions,
  snapshotBelongsToRepository,
  sourceGoal,
  sourceNeedsReference,
  type RunAction
} from './run-center-model'

const PHASES: { id: SddPhase; label: string }[] = [
  { id: 'specification', label: 'Spec' },
  { id: 'design', label: 'Design' },
  { id: 'planning', label: 'Plan' },
  { id: 'implementation', label: 'Build' },
  { id: 'verification', label: 'Verify' },
  { id: 'review', label: 'Review' },
  { id: 'ready', label: 'Ready' },
  { id: 'delivery', label: 'Deliver' },
  { id: 'completed', label: 'Done' }
]

const VIEWS = ['Overview', 'Spec', 'Plan', 'Tasks', 'Evidence', 'Review', 'Activity'] as const
type RunView = (typeof VIEWS)[number]

const PROVIDER_LABELS: Record<string, string> = {
  codex: 'Codex',
  claude: 'Claude',
  agent: 'Cursor / Agent',
  cursor: 'Cursor / Agent',
  gemini: 'Gemini',
  hermes: 'Hermes',
  opencode: 'OpenCode',
  aider: 'Aider'
}

function providerOptions(capabilities: SddCapabilities | null): SddCapabilities['providers'] {
  if (capabilities) return capabilities.providers
  return Object.entries(PROVIDER_LABELS)
    .filter(([id]) => id !== 'cursor')
    .map(([id, label]) => ({
      id,
      label,
      available: false,
      reason: 'Checking capability'
    }))
}

function providerAvailable(
  capabilities: SddCapabilities | null,
  provider: string | undefined
): boolean {
  if (!capabilities || !provider) return false
  const canonical = capabilities.providerAliases[provider] ?? provider
  return capabilities.providers.some(
    (capability) => capability.id === canonical && capability.available === true
  )
}

function repositorySddAvailable(capability: SddRemoteCapability | null): boolean {
  return (
    capability !== null &&
    (capability.reason === 'repository_is_local' || capability.available === true)
  )
}

function remoteCapabilityMessage(capability: SddRemoteCapability): string {
  if (capability.reason === 'desktop_projection_unavailable') {
    return 'The remote worker may be ready, but Agentum could not establish its restart-safe Run Center checkpoint and artifact projection. Agentum will not fall back to a local provider.'
  }
  if (capability.reason === 'worker_version_mismatch') {
    return 'The remote worker version does not match this Agentum release. Agentum will not fall back to a local provider.'
  }
  if (capability.reason === 'remote_subsystem_unavailable') {
    return 'The fixed agentum-sdd-v1 SSH subsystem could not be verified. Agentum will not fall back to a local provider.'
  }
  return `Remote SDD is unavailable (${capability.reason}). Agentum will not fall back to a local provider.`
}

type Props = {
  repoId: string
  projectName: string
  presentation?: 'bar' | 'page'
  initiallyExpanded?: boolean
}

export type NewSpecDraft = {
  title: string
  goal: string
  sourceKind: SddSourceKind
  sourceReference: string
  profile: 'standard' | 'high_risk'
  control: 'guarded' | 'interactive' | 'autopilot'
  provider: string
  baseRef: string
  sourceCheckout: 'require_clean' | 'committed_base' | 'snapshot'
}

const EMPTY_DRAFT: NewSpecDraft = {
  title: '',
  goal: '',
  sourceKind: 'description',
  sourceReference: '',
  profile: 'standard',
  control: 'guarded',
  provider: 'codex',
  baseRef: 'HEAD',
  sourceCheckout: 'require_clean'
}

type RunConfigurationDraft = Pick<
  NewSpecDraft,
  'profile' | 'control' | 'provider' | 'baseRef' | 'sourceCheckout'
>

const EMPTY_RUN_CONFIGURATION: RunConfigurationDraft = {
  profile: 'standard',
  control: 'guarded',
  provider: 'codex',
  baseRef: 'HEAD',
  sourceCheckout: 'require_clean'
}

type DeliveryPreview = {
  previewToken: string
  digest?: string
  expiresAt?: string
  summary?: string
  actions: SddDeliveryAction[]
}

type DeliveryIntentDraft = {
  commit: boolean
  commitMessage: string
  push: boolean
  remote: string
  pullRequest: boolean
  pullRequestTitle: string
  pullRequestBody: string
  pullRequestBase: string
  trackerComment: boolean
  trackerCommentBody: string
  trackerStatus: boolean
  trackerStatusName: string
  trackerField: boolean
  trackerFieldId: string
  trackerFieldValue: string
  release: boolean
  releaseTag: string
  releaseName: string
  releaseNotes: string
  prerelease: boolean
  openSpecExport: boolean
}

const EMPTY_DELIVERY_INTENT: DeliveryIntentDraft = {
  commit: true,
  commitMessage: '',
  push: false,
  remote: 'origin',
  pullRequest: false,
  pullRequestTitle: '',
  pullRequestBody: '',
  pullRequestBase: 'main',
  trackerComment: false,
  trackerCommentBody: '',
  trackerStatus: false,
  trackerStatusName: '',
  trackerField: false,
  trackerFieldId: '',
  trackerFieldValue: '',
  release: false,
  releaseTag: '',
  releaseName: '',
  releaseNotes: '',
  prerelease: false,
  openSpecExport: false
}

function deliveryIntentsFromDraft(
  draft: DeliveryIntentDraft,
  specTitle: string
): SddDeliveryIntent[] {
  const actions: SddDeliveryIntent[] = []
  if (draft.commit) {
    actions.push({
      type: 'commit',
      message: draft.commitMessage.trim() || `Agentum: ${specTitle.trim()}`
    })
  }
  if (draft.push) actions.push({ type: 'push', remote: draft.remote.trim() || 'origin' })
  if (draft.pullRequest) {
    actions.push({
      type: 'pullRequest',
      title: draft.pullRequestTitle.trim() || specTitle.trim(),
      body: draft.pullRequestBody.trim(),
      base: draft.pullRequestBase.trim() || 'main'
    })
  }
  if (draft.trackerComment) {
    actions.push({
      type: 'trackerComment',
      body: draft.trackerCommentBody.trim()
    })
  }
  if (draft.trackerStatus) {
    actions.push({
      type: 'trackerStatus',
      status: draft.trackerStatusName.trim()
    })
  }
  if (draft.trackerField) {
    actions.push({
      type: 'trackerFieldUpdate',
      fieldId: draft.trackerFieldId.trim(),
      value: { type: 'text', value: draft.trackerFieldValue }
    })
  }
  if (draft.release) {
    actions.push({
      type: 'release',
      tag: draft.releaseTag.trim(),
      name: draft.releaseName.trim() || draft.releaseTag.trim(),
      notes: draft.releaseNotes.trim(),
      prerelease: draft.prerelease
    })
  }
  if (draft.openSpecExport) actions.push({ type: 'openSpecExport' })
  return actions
}

function capabilityConnection(
  capability: SddCapabilities['sources'][number] | undefined
): JiraConnection | null {
  const connection = capability?.connection
  if (!connection || typeof connection !== 'object') return null
  const value = connection as Partial<JiraConnection>
  return typeof value.connectionId === 'string' && typeof value.selectedSiteId === 'string'
    ? (value as JiraConnection)
    : null
}

function sourceFromDraft(
  draft: NewSpecDraft,
  expectedSourceRevision?: string,
  capability?: SddCapabilities['sources'][number]
): SddSourceReference | undefined {
  const reference = draft.sourceReference.trim()
  switch (draft.sourceKind) {
    case 'description':
      return undefined
    case 'socratic':
      return { type: 'socratic', context: draft.goal.trim() }
    case 'markdown':
      return { type: 'markdown', markdown: draft.goal.trim() }
    case 'github':
      return { type: 'github', url: reference, expectedSourceRevision }
    case 'linear':
      return {
        type: 'linear',
        identifier: reference,
        ...(typeof capability?.connectionId === 'string'
          ? { connectionId: capability.connectionId }
          : {}),
        expectedSourceRevision
      }
    case 'jira': {
      const connection = capabilityConnection(capability)
      return {
        type: 'jira',
        connectionId: connection?.connectionId ?? '',
        siteId: connection?.selectedSiteId ?? '',
        key: reference,
        expectedSourceRevision
      }
    }
    case 'openspec':
      return { type: 'openspec', path: reference, expectedSourceRevision }
  }
}

export default function SddWorkspaceBar({
  repoId,
  projectName,
  presentation = 'bar',
  initiallyExpanded = false
}: Props): React.JSX.Element {
  const instanceId = useId()
  const [expanded, setExpanded] = useState(initiallyExpanded || presentation === 'page')
  const [newSpecOpen, setNewSpecOpen] = useState(false)
  const [configureRunOpen, setConfigureRunOpen] = useState(false)
  const [draft, setDraft] = useState<NewSpecDraft>(EMPTY_DRAFT)
  const [runConfiguration, setRunConfiguration] = useState<RunConfigurationDraft>(
    EMPTY_RUN_CONFIGURATION
  )
  const [creating, setCreating] = useState(false)
  const [configuringRun, setConfiguringRun] = useState(false)
  const [loading, setLoading] = useState(true)
  const [refreshWarning, setRefreshWarning] = useState<string | null>(null)
  const [snapshot, setSnapshot] = useState<SddSnapshot | null>(null)
  const [artifacts, setArtifacts] = useState<SddArtifact[]>([])
  const [events, setEvents] = useState<SddEvent[]>([])
  const [specs, setSpecs] = useState<SddSpec[]>([])
  const [runs, setRuns] = useState<SddRun[]>([])
  const [selectedSpecId, setSelectedSpecId] = useState('')
  const [selectedRunId, setSelectedRunId] = useState('')
  const [view, setView] = useState<RunView>('Overview')
  const [actionPending, setActionPending] = useState<string | null>(null)
  const [deliveryPreview, setDeliveryPreview] = useState<DeliveryPreview | null>(null)
  const [deliverySelections, setDeliverySelections] = useState<string[]>([])
  const [deliveryIntent, setDeliveryIntent] = useState<DeliveryIntentDraft>(EMPTY_DELIVERY_INTENT)
  const [reopenPhase, setReopenPhase] = useState<SddPhase>('specification')
  const [liveConnected, setLiveConnected] = useState(false)
  const [capabilities, setCapabilities] = useState<SddCapabilities | null>(null)
  const [capabilitiesError, setCapabilitiesError] = useState<string | null>(null)
  const [remoteCapability, setRemoteCapability] = useState<SddRemoteCapability | null>(null)
  const [remoteCapabilityError, setRemoteCapabilityError] = useState<string | null>(null)
  const [sourcePreview, setSourcePreview] = useState<SddSourcePreview | null>(null)
  const [sourcePreviewing, setSourcePreviewing] = useState(false)
  const [jiraOauth, setJiraOauth] = useState<JiraOauthStart | null>(null)
  const [jiraConnections, setJiraConnections] = useState<JiraConnection[]>([])
  const [jiraConnecting, setJiraConnecting] = useState(false)

  const epochRef = useRef(0)
  const repoRef = useRef(repoId)
  const selectedRunRef = useRef('')
  const eventCursorRef = useRef(0)
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const mutationRequestIdsRef = useRef(new Map<string, string>())
  const createAttemptRef = useRef<{
    signature: string
    requestId: string
  } | null>(null)
  const configureRunAttemptRef = useRef<{
    signature: string
    requestId: string
  } | null>(null)
  const sourcePreviewRequestRef = useRef(0)
  const remoteCapabilityRequestRef = useRef(0)
  repoRef.current = repoId
  selectedRunRef.current = selectedRunId

  const loadRun = useCallback(
    async (runId: string, epoch = epochRef.current): Promise<boolean> => {
      const [snapshotResult, artifactsResult, eventsResult] = await Promise.allSettled([
        getRun(runId),
        getArtifacts(runId),
        getEvents(runId)
      ])
      const current =
        repoRef.current === repoId && epochRef.current === epoch && selectedRunRef.current === runId
      if (!current) return false

      let snapshotLoaded = false
      if (snapshotResult.status === 'fulfilled') {
        if (!snapshotBelongsToRepository(snapshotResult.value, repoId, runId)) {
          setRefreshWarning('Agentum rejected run data that belonged to another repository.')
          return false
        }
        setSnapshot(snapshotResult.value)
        snapshotLoaded = true
      }
      if (artifactsResult.status === 'fulfilled') setArtifacts(artifactsResult.value)
      if (eventsResult.status === 'fulfilled') {
        setEvents(eventsResult.value)
        eventCursorRef.current = eventsResult.value.reduce(
          (cursor, event) => Math.max(cursor, event.cursor),
          eventCursorRef.current
        )
      }

      const failed: string[] = []
      if (snapshotResult.status === 'rejected') failed.push('run state')
      if (artifactsResult.status === 'rejected') failed.push('artifacts')
      if (eventsResult.status === 'rejected') failed.push('activity')
      setRefreshWarning(
        failed.length > 0
          ? `Saved state is intact, but ${failed.join(', ')} could not refresh.`
          : null
      )
      return snapshotLoaded
    },
    [repoId]
  )

  const loadSpec = useCallback(
    async (specId: string, preferredRunId?: string): Promise<void> => {
      const epoch = ++epochRef.current
      setSelectedSpecId(specId)
      setSnapshot(null)
      setArtifacts([])
      setEvents([])
      setRuns([])
      setDeliveryPreview(null)
      const result = await getSpec(specId)
      if (repoRef.current !== repoId || epochRef.current !== epoch) return
      if (result.spec.repoId !== repoId || result.spec.specId !== specId) {
        throw new Error('Agentum returned a specification from another repository.')
      }
      const availableRuns = (result.runs ?? (result.run ? [result.run] : [])).filter(
        (run) => run.repoId === repoId && run.specId === specId
      )
      setRuns(availableRuns)
      const run =
        availableRuns.find((candidate) => candidate.runId === preferredRunId) ??
        availableRuns[0] ??
        null
      const runId = run?.runId ?? ''
      selectedRunRef.current = runId
      setSelectedRunId(runId)
      if (runId) await loadRun(runId, epoch)
    },
    [loadRun, repoId]
  )

  const restore = useCallback(
    async (preferredSpecId?: string, preferredRunId?: string): Promise<void> => {
      const epoch = ++epochRef.current
      setLoading(true)
      setRefreshWarning(null)
      try {
        const nextSpecs = (await listSpecs(repoId)).filter((spec) => spec.repoId === repoId)
        if (repoRef.current !== repoId || epochRef.current !== epoch) return
        setSpecs(nextSpecs)
        const selected =
          nextSpecs.find((spec) => spec.specId === preferredSpecId) ??
          nextSpecs.find((spec) => spec.specId === selectedSpecId) ??
          nextSpecs[0]
        if (!selected) {
          setSelectedSpecId('')
          setSelectedRunId('')
          selectedRunRef.current = ''
          setSnapshot(null)
          setArtifacts([])
          setEvents([])
          setRuns([])
          return
        }
        await loadSpec(selected.specId, preferredRunId)
      } catch (error) {
        if (repoRef.current === repoId) {
          setRefreshWarning(errorMessage(error, 'Could not restore spec runs'))
        }
      } finally {
        if (repoRef.current === repoId) setLoading(false)
      }
    },
    [loadSpec, repoId, selectedSpecId]
  )

  useEffect(() => {
    epochRef.current += 1
    eventCursorRef.current = 0
    mutationRequestIdsRef.current.clear()
    createAttemptRef.current = null
    configureRunAttemptRef.current = null
    sourcePreviewRequestRef.current += 1
    setSpecs([])
    setRuns([])
    setSelectedSpecId('')
    setSelectedRunId('')
    selectedRunRef.current = ''
    setSnapshot(null)
    setArtifacts([])
    setEvents([])
    setDeliveryPreview(null)
    setDeliveryIntent(EMPTY_DELIVERY_INTENT)
    setConfigureRunOpen(false)
    setRunConfiguration(EMPTY_RUN_CONFIGURATION)
    setConfiguringRun(false)
    setCapabilities(null)
    setCapabilitiesError(null)
    setRemoteCapability(null)
    setRemoteCapabilityError(null)
    setSourcePreview(null)
    setSourcePreviewing(false)
    setJiraOauth(null)
    setJiraConnections([])
    setJiraConnecting(false)
    remoteCapabilityRequestRef.current += 1
    void restore()
  }, [repoId]) // restore intentionally excluded: repository identity owns this reset.

  useEffect(() => {
    if (presentation !== 'page') return
    return subscribeNewSpecPrefill(repoId, (intent) => {
      setDraft({
        ...EMPTY_DRAFT,
        title: intent.title,
        goal: intent.goal,
        sourceKind: intent.sourceKind,
        sourceReference: intent.sourceReference
      })
      setSourcePreview(null)
      setExpanded(true)
      setNewSpecOpen(true)
    })
  }, [presentation, repoId])

  useEffect(() => {
    const unsubscribe = subscribeSddEvents({
      repoId,
      after: eventCursorRef.current,
      onOpen: () => {
        if (repoRef.current !== repoId) return
        setLiveConnected(true)
        const runId = selectedRunRef.current
        if (runId) void loadRun(runId, epochRef.current)
      },
      onError: () => {
        if (repoRef.current === repoId) setLiveConnected(false)
      },
      onEvent: (event) => {
        if (repoRef.current !== repoId || event.repoId !== repoId) return
        eventCursorRef.current = Math.max(eventCursorRef.current, event.cursor)
        const runId = selectedRunRef.current
        if (runId) setEvents((current) => appendDurableEvent(current, event, runId))
        if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current)
        refreshTimerRef.current = setTimeout(() => {
          const selected = selectedRunRef.current
          if (selected && repoRef.current === repoId) void loadRun(selected, epochRef.current)
        }, 150)
      }
    })
    return () => {
      setLiveConnected(false)
      if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current)
      refreshTimerRef.current = null
      unsubscribe()
    }
  }, [loadRun, repoId])

  useEffect(() => {
    if (!capabilities) {
      void getSddCapabilities()
        .then((value) => {
          if (repoRef.current === repoId) {
            setCapabilities(value)
            setCapabilitiesError(null)
          }
        })
        .catch((error) => {
          if (repoRef.current === repoId) {
            setCapabilitiesError(errorMessage(error, 'Could not verify SDD capabilities'))
          }
        })
    }
  }, [capabilities, repoId])

  useEffect(() => {
    const provider = draft.provider.trim()
    if (!provider) {
      setRemoteCapability(null)
      setRemoteCapabilityError(null)
      return
    }
    const request = ++remoteCapabilityRequestRef.current
    setRemoteCapability(null)
    setRemoteCapabilityError(null)
    void getSddRemoteCapability(repoId, provider, draft.baseRef.trim() || 'HEAD')
      .then((value) => {
        if (repoRef.current !== repoId || remoteCapabilityRequestRef.current !== request) return
        setRemoteCapability(value)
      })
      .catch((error) => {
        if (repoRef.current !== repoId || remoteCapabilityRequestRef.current !== request) return
        setRemoteCapabilityError(
          errorMessage(error, 'Could not verify repository execution capability')
        )
      })
  }, [draft.baseRef, draft.provider, repoId])

  const reloadCapabilities = useCallback(async (): Promise<void> => {
    const value = await getSddCapabilities()
    if (repoRef.current !== repoId) return
    setCapabilities(value)
    setCapabilitiesError(null)
  }, [repoId])

  const startJiraConnection = useCallback(async (): Promise<void> => {
    setJiraConnecting(true)
    try {
      const flow = await startJiraOauth(crypto.randomUUID())
      setJiraOauth(flow)
      await api.shell.openUrl(flow.authorizationUrl)
    } catch (error) {
      toast.error(errorMessage(error, 'Could not start Jira authorization'))
    } finally {
      setJiraConnecting(false)
    }
  }, [])

  const finishJiraConnection = useCallback(async (): Promise<void> => {
    if (!jiraOauth) return
    setJiraConnecting(true)
    try {
      const connection = await redeemJiraOauth(
        crypto.randomUUID(),
        jiraOauth.flowId,
        jiraOauth.revision
      )
      setJiraOauth(null)
      const connections = connection.selectedSiteId ? [connection] : await listJiraConnections()
      setJiraConnections(connections)
      await reloadCapabilities()
      toast.success(
        connection.selectedSiteId
          ? 'Jira Cloud connected'
          : 'Jira Cloud connected. Select one site to continue.'
      )
    } catch (error) {
      toast.error(errorMessage(error, 'Could not finish Jira authorization'))
    } finally {
      setJiraConnecting(false)
    }
  }, [jiraOauth, reloadCapabilities])

  const chooseJiraSite = useCallback(
    async (connection: JiraConnection, siteId: string): Promise<void> => {
      setJiraConnecting(true)
      try {
        await selectJiraSite(connection.connectionId, {
          requestId: crypto.randomUUID(),
          siteId,
          expectedCredentialRevision: connection.credentialRevision
        })
        setJiraConnections([])
        await reloadCapabilities()
        toast.success('Jira Cloud site selected')
      } catch (error) {
        toast.error(errorMessage(error, 'Could not select Jira Cloud site'))
      } finally {
        setJiraConnecting(false)
      }
    },
    [reloadCapabilities]
  )

  const connectJiraWithApiToken = useCallback(
    async (input: { email: string; apiToken: string; siteUrl: string }): Promise<void> => {
      setJiraConnecting(true)
      try {
        await connectJiraApiToken({
          requestId: crypto.randomUUID(),
          ...input,
          acknowledgeRisk: true,
          expectedRevision: 0
        })
        await reloadCapabilities()
        toast.success('Jira Cloud API token stored in the secure vault')
      } catch (error) {
        toast.error(errorMessage(error, 'Could not connect Jira API token'))
        throw error
      } finally {
        setJiraConnecting(false)
      }
    },
    [reloadCapabilities]
  )

  const updateDraft = useCallback((next: NewSpecDraft): void => {
    setDraft(next)
    setSourcePreview(null)
    setSourcePreviewing(false)
    sourcePreviewRequestRef.current += 1
    createAttemptRef.current = null
  }, [])

  const closeNewSpec = useCallback((open: boolean): void => {
    if (!open) {
      setDraft(EMPTY_DRAFT)
      setSourcePreview(null)
      setSourcePreviewing(false)
      sourcePreviewRequestRef.current += 1
      createAttemptRef.current = null
    }
    setNewSpecOpen(open)
  }, [])

  const openRunConfiguration = useCallback((): void => {
    const firstAvailable = capabilities?.providers.find((provider) => provider.available === true)
    setRunConfiguration((current) =>
      providerAvailable(capabilities, current.provider) || !firstAvailable
        ? current
        : { ...current, provider: firstAvailable.id }
    )
    setConfigureRunOpen(true)
  }, [capabilities])

  const closeRunConfiguration = useCallback((open: boolean): void => {
    if (!open) {
      setRunConfiguration(EMPTY_RUN_CONFIGURATION)
      configureRunAttemptRef.current = null
    }
    setConfigureRunOpen(open)
  }, [])

  const configureDiscoveredRun = useCallback(async (): Promise<void> => {
    const selected = specs.find((spec) => spec.specId === selectedSpecId)
    if (!selected || !providerAvailable(capabilities, runConfiguration.provider)) return
    const signature = JSON.stringify({
      specId: selected.specId,
      expectedRevision: selected.aggregateRevision,
      ...runConfiguration
    })
    const attempt =
      configureRunAttemptRef.current?.signature === signature
        ? configureRunAttemptRef.current
        : { signature, requestId: crypto.randomUUID() }
    configureRunAttemptRef.current = attempt
    setConfiguringRun(true)
    try {
      const result = await createSpecRun(selected.specId, {
        requestId: attempt.requestId,
        expectedRevision: selected.aggregateRevision,
        ...runConfiguration,
        baseRef: runConfiguration.baseRef.trim() || 'HEAD'
      })
      configureRunAttemptRef.current = null
      setConfigureRunOpen(false)
      setRunConfiguration(EMPTY_RUN_CONFIGURATION)
      const runId = 'run' in result ? result.run.runId : result.runId
      await restore(selected.specId, runId)
      toast.success('Run configured. Specification approval is required.')
    } catch (error) {
      toast.error(errorMessage(error, 'Could not configure specification run'))
    } finally {
      setConfiguringRun(false)
    }
  }, [capabilities, restore, runConfiguration, selectedSpecId, specs])

  const previewSource = useCallback(async (): Promise<void> => {
    const source = sourceFromDraft(
      draft,
      undefined,
      capabilities?.sources.find((entry) => entry.id === draft.sourceKind)
    )
    if (!source || !draft.title.trim()) return
    const request = ++sourcePreviewRequestRef.current
    setSourcePreviewing(true)
    try {
      const preview = await previewSddSource(repoId, draft.title.trim(), source)
      if (sourcePreviewRequestRef.current !== request || repoRef.current !== repoId) return
      setSourcePreview(preview)
      createAttemptRef.current = null
    } catch (error) {
      if (sourcePreviewRequestRef.current !== request || repoRef.current !== repoId) return
      setSourcePreview(null)
      toast.error(errorMessage(error, 'Could not preview source'))
    } finally {
      if (sourcePreviewRequestRef.current === request && repoRef.current === repoId) {
        setSourcePreviewing(false)
      }
    }
  }, [capabilities, draft, repoId])

  const create = useCallback(async (): Promise<void> => {
    const provider = draft.provider
    const goal = sourceGoal(draft.sourceKind, draft.goal, draft.sourceReference)
    if (
      !draft.title.trim() ||
      !goal ||
      !provider ||
      remoteCapabilityError ||
      !repositorySddAvailable(remoteCapability)
    )
      return
    const signature = JSON.stringify({ repoId, ...draft, provider, goal })
    const attempt =
      createAttemptRef.current?.signature === signature
        ? createAttemptRef.current
        : { signature, requestId: crypto.randomUUID() }
    createAttemptRef.current = attempt
    setCreating(true)
    try {
      const source = sourceFromDraft(
        draft,
        sourcePreview?.kind === draft.sourceKind ? sourcePreview.sourceRevision : undefined,
        capabilities?.sources.find((entry) => entry.id === draft.sourceKind)
      )
      const result = await createSpec(repoId, {
        requestId: attempt.requestId,
        expectedRevision: 0,
        title: draft.title.trim(),
        goal,
        profile: draft.profile,
        control: draft.control,
        provider,
        baseRef: draft.baseRef.trim() || 'HEAD',
        sourceCheckout: draft.sourceCheckout,
        ...(source ? { source } : {})
      })
      createAttemptRef.current = null
      setNewSpecOpen(false)
      setDraft(EMPTY_DRAFT)
      setSourcePreview(null)
      setExpanded(true)
      setView('Spec')
      await restore(result.specId, result.runId)
      toast.success(result.nextAction || 'Specification authored')
    } catch (error) {
      toast.error(errorMessage(error, 'Could not create spec'))
    } finally {
      setCreating(false)
    }
  }, [capabilities, draft, remoteCapability, remoteCapabilityError, repoId, restore, sourcePreview])

  const executeCommand = useCallback(
    async (type: string, extra: Record<string, unknown> = {}): Promise<SddCommandResult | null> => {
      if (!snapshot) return null
      const { run } = snapshot
      const signature = `${run.runId}:${run.aggregateRevision}:${type}:${JSON.stringify(extra)}`
      const requestId = mutationRequestIdsRef.current.get(signature) ?? crypto.randomUUID()
      mutationRequestIdsRef.current.set(signature, requestId)
      setActionPending(type)
      try {
        const result = await command(run.runId, {
          type,
          requestId,
          expectedRevision: run.aggregateRevision,
          ...extra
        })
        mutationRequestIdsRef.current.delete(signature)
        setSnapshot((current) => {
          if (!current || current.run.runId !== result.runId) return current
          if (result.revision <= current.run.aggregateRevision) return current
          return {
            ...current,
            run: {
              ...current.run,
              aggregateRevision: result.revision,
              phase: result.phase ?? current.run.phase,
              status: result.status ?? current.run.status
            }
          }
        })
        const refreshed = await loadRun(run.runId, epochRef.current)
        if (!refreshed) {
          setRefreshWarning(
            'The command succeeded. Refresh Run Center to reload its durable state.'
          )
        }
        return result
      } catch (error) {
        const message = errorMessage(error, 'Run command failed')
        if (/\b(409|412)\b/.test(message)) {
          mutationRequestIdsRef.current.delete(signature)
          await loadRun(run.runId, epochRef.current)
        }
        toast.error(message)
        return null
      } finally {
        setActionPending(null)
      }
    },
    [loadRun, snapshot]
  )

  const decide = useCallback(
    async (decision: 'approve' | 'reject', reason?: string): Promise<void> => {
      if (!snapshot?.approval || snapshot.approval.status !== 'pending') return
      await executeCommand('decideApproval', {
        approvalId: snapshot.approval.approvalId,
        digest: snapshot.approval.digest,
        decision,
        ...(reason?.trim() ? { reason: reason.trim() } : {})
      })
    },
    [executeCommand, snapshot]
  )

  const runAction = useCallback(
    async (action: RunAction): Promise<void> => {
      if (action === 'previewDelivery') {
        if (!snapshot) return
        const intents = deliveryIntentsFromDraft(deliveryIntent, snapshot.spec.title)
        if (intents.length === 0) {
          toast.error('Select at least one delivery action to preview')
          return
        }
        const result = await executeCommand(action, { actions: intents })
        if (!result?.previewToken) return
        const actions = result.actions ?? []
        setDeliveryPreview({
          previewToken: result.previewToken,
          digest: result.digest,
          expiresAt: result.expiresAt,
          summary: result.summary,
          actions
        })
        setDeliverySelections(selectableDeliveryActions(actions))
        return
      }
      await executeCommand(action)
    },
    [deliveryIntent, executeCommand, snapshot]
  )

  const confirmDelivery = useCallback(async (): Promise<void> => {
    if (!deliveryPreview) return
    const result = await executeCommand('confirmDelivery', {
      previewToken: deliveryPreview.previewToken,
      actions: deliverySelections
    })
    if (result) setDeliveryPreview(null)
  }, [deliveryPreview, deliverySelections, executeCommand])

  const chooseSpec = useCallback(
    async (specId: string): Promise<void> => {
      setLoading(true)
      try {
        await loadSpec(specId)
      } catch (error) {
        toast.error(errorMessage(error, 'Could not load specification'))
      } finally {
        if (repoRef.current === repoId) setLoading(false)
      }
    },
    [loadSpec, repoId]
  )

  const chooseRun = useCallback(
    async (runId: string): Promise<void> => {
      const epoch = ++epochRef.current
      selectedRunRef.current = runId
      setSelectedRunId(runId)
      setSnapshot(null)
      setArtifacts([])
      setEvents([])
      setLoading(true)
      try {
        const loaded = await loadRun(runId, epoch)
        if (!loaded) throw new Error('Could not load the selected run.')
      } catch (error) {
        toast.error(errorMessage(error, 'Could not load run'))
      } finally {
        if (repoRef.current === repoId && epochRef.current === epoch) setLoading(false)
      }
    },
    [loadRun, repoId]
  )

  const selectedSpec = specs.find((spec) => spec.specId === selectedSpecId) ?? null
  const specArtifact = artifacts.find((artifact) => artifact.metadata.kind === 'specification')
  const phaseIndex = snapshot ? PHASES.findIndex((phase) => phase.id === snapshot.run.phase) : -1
  const nextAction = nextActionLabel(snapshot)
  const snapshotProviderAvailable = providerAvailable(capabilities, snapshot?.spec.provider)
  const actions = snapshot
    ? availableRunActions(snapshot).filter((action) => {
        if (action === 'pause' || action === 'cancel') return true
        if (action === 'previewDelivery') return capabilities?.delivery === true
        if (action === 'startAuthoring') return snapshotProviderAvailable
        return capabilities?.readyLifecycle === true && snapshotProviderAvailable
      })
    : []
  const page = presentation === 'page'
  const domId = `run-center-${repoId.replace(/[^A-Za-z0-9_-]/g, '-')}-${instanceId.replace(/[^A-Za-z0-9_-]/g, '-')}`
  const headerSummary = (
    <>
      {!page ? (
        expanded ? (
          <ChevronUp className="size-4" />
        ) : (
          <ChevronDown className="size-4" />
        )
      ) : null}
      <span className="font-semibold">Run Center</span>
      {loading ? (
        <Loader2 className="size-3.5 animate-spin text-muted-foreground" aria-label="Loading" />
      ) : snapshot ? (
        <>
          <span className="max-w-64 truncate text-sm text-muted-foreground">
            {snapshot.spec.title}
          </span>
          <RunStatus snapshot={snapshot} />
          <span className="hidden text-xs text-muted-foreground lg:inline">{nextAction}</span>
        </>
      ) : (
        <span className="text-sm text-muted-foreground">No spec run</span>
      )}
    </>
  )

  return (
    <>
      <section
        className={
          page
            ? 'flex h-full min-h-0 flex-col bg-background'
            : 'relative z-30 border-b border-border/70 bg-background/95'
        }
        aria-label="Run Center"
        data-repo-id={repoId}
      >
        <div className="flex min-h-11 items-center gap-2 border-b border-border/60 px-3 py-1.5">
          {page ? (
            <div className="flex min-w-0 items-center gap-2 text-left">{headerSummary}</div>
          ) : (
            <button
              type="button"
              className="flex min-w-0 items-center gap-2 rounded-sm text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              onClick={() => setExpanded((value) => !value)}
              aria-expanded={expanded}
              aria-controls={domId}
            >
              {headerSummary}
            </button>
          )}
          <span
            className={`hidden text-[10px] sm:inline ${liveConnected ? 'text-emerald-600' : 'text-muted-foreground'}`}
            title={
              liveConnected ? 'Durable activity stream connected' : 'Activity stream reconnecting'
            }
          >
            {liveConnected ? 'Live' : 'Reconnecting'}
          </span>
          <Button
            className="ml-auto h-7 gap-1.5 px-2.5"
            size="sm"
            variant={snapshot ? 'outline' : 'default'}
            onClick={() => setNewSpecOpen(true)}
          >
            <FilePlus2 className="size-3.5" />
            New Spec
          </Button>
        </div>

        {refreshWarning ? (
          <div
            className="flex items-center gap-2 border-b border-amber-500/30 bg-amber-500/5 px-4 py-2 text-xs text-amber-700 dark:text-amber-300"
            role="status"
          >
            <CircleAlert className="size-3.5 shrink-0" />
            <span>{refreshWarning}</span>
            <Button
              size="sm"
              variant="outline"
              className="ml-auto h-7"
              onClick={() => void restore(selectedSpecId, selectedRunId)}
            >
              <RefreshCw className="mr-1 size-3" /> Refresh
            </Button>
          </div>
        ) : null}

        {expanded ? (
          <div id={domId} className={`min-h-0 ${page ? 'flex flex-1 flex-col' : ''}`}>
            {capabilitiesError ? (
              <div
                className="border-b border-destructive/30 bg-destructive/5 px-4 py-2 text-xs text-destructive"
                role="alert"
              >
                Run Center capabilities could not be verified. New work, resume, retry, approval,
                and delivery actions are disabled; pause and cancel remain available.
              </div>
            ) : capabilities?.readyLifecycle === false ? (
              <div
                className="border-b border-sky-500/30 bg-sky-500/5 px-4 py-2 text-xs text-sky-800 dark:text-sky-200"
                role="status"
              >
                This build supports the authoring checkpoint only; design through Ready is
                unavailable.
              </div>
            ) : null}
            {capabilities && capabilities.browserEvidence?.available !== true ? (
              <div
                className="border-b border-amber-500/30 bg-amber-500/5 px-4 py-2 text-xs text-amber-800 dark:text-amber-200"
                role="status"
              >
                Rich browser evidence unavailable.{' '}
                {capabilities.browserEvidence?.reason ??
                  'This server did not advertise a run-bound browser evidence capability.'}
              </div>
            ) : null}
            <div className="flex flex-wrap items-end gap-2 border-b border-border/60 px-4 py-2">
              <Labeled label="Specification" compact>
                <select
                  className="h-8 min-w-52 max-w-80 rounded-md border bg-background px-2 text-xs"
                  value={selectedSpecId}
                  onChange={(event) => void chooseSpec(event.target.value)}
                  disabled={loading || specs.length === 0}
                  aria-label="Selected specification"
                >
                  {specs.length === 0 ? <option value="">No specifications</option> : null}
                  {specs.map((spec) => (
                    <option key={spec.specId} value={spec.specId}>
                      {spec.title} · r{spec.currentRevision}
                    </option>
                  ))}
                </select>
              </Labeled>
              <Labeled label="Run" compact>
                <select
                  className="h-8 min-w-44 rounded-md border bg-background px-2 font-mono text-xs"
                  value={selectedRunId}
                  onChange={(event) => void chooseRun(event.target.value)}
                  disabled={loading || runs.length <= 1}
                  aria-label="Selected run"
                >
                  {runs.length === 0 ? <option value="">No run</option> : null}
                  {runs.map((run) => (
                    <option key={run.runId} value={run.runId}>
                      {run.runId.slice(0, 8)} · {run.phase} / {run.status}
                    </option>
                  ))}
                </select>
              </Labeled>
              {snapshot ? (
                <span className="ml-auto pb-1 text-xs text-muted-foreground">{nextAction}</span>
              ) : null}
            </div>

            {snapshot ? (
              <>
                <ol className="grid grid-cols-9 gap-1 px-4 pt-3" aria-label="Specification phases">
                  {PHASES.map((phase, index) => (
                    <li
                      key={phase.id}
                      className="min-w-0"
                      aria-current={index === phaseIndex ? 'step' : undefined}
                    >
                      <div
                        className={`h-1.5 rounded-full ${index < phaseIndex ? 'bg-emerald-500' : index === phaseIndex ? 'bg-primary' : 'bg-muted'}`}
                      />
                      <div
                        className={`mt-1 truncate text-[10px] ${index === phaseIndex ? 'font-semibold text-foreground' : 'text-muted-foreground'}`}
                      >
                        {phase.label}
                      </div>
                    </li>
                  ))}
                </ol>

                {snapshot.run.phase === 'ready' && capabilities?.delivery === true ? (
                  <DeliveryIntentComposer
                    draft={deliveryIntent}
                    onDraft={setDeliveryIntent}
                    disabled={actionPending !== null}
                  />
                ) : null}

                <div
                  className="mt-3 flex flex-wrap items-center gap-1 border-b border-border/60 px-4"
                  role="tablist"
                  aria-label="Run details"
                >
                  {VIEWS.map((entry, index) => (
                    <button
                      type="button"
                      role="tab"
                      aria-selected={view === entry}
                      aria-controls={`${domId}-panel`}
                      id={`${domId}-tab-${entry.toLowerCase()}`}
                      tabIndex={view === entry ? 0 : -1}
                      key={entry}
                      className={`border-b-2 px-2 py-1.5 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${view === entry ? 'border-primary font-medium text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground'}`}
                      onClick={() => setView(entry)}
                      onKeyDown={(event) => {
                        const last = VIEWS.length - 1
                        const nextIndex =
                          event.key === 'Home'
                            ? 0
                            : event.key === 'End'
                              ? last
                              : event.key === 'ArrowRight'
                                ? (index + 1) % VIEWS.length
                                : event.key === 'ArrowLeft'
                                  ? (index - 1 + VIEWS.length) % VIEWS.length
                                  : null
                        if (nextIndex === null) return
                        event.preventDefault()
                        const nextView = VIEWS[nextIndex]
                        setView(nextView)
                        document.getElementById(`${domId}-tab-${nextView.toLowerCase()}`)?.focus()
                      }}
                    >
                      {entry}
                    </button>
                  ))}
                  <RunActions
                    actions={actions}
                    pending={actionPending}
                    onAction={runAction}
                    canReopen={
                      capabilities?.readyLifecycle === true &&
                      ['ready', 'delivery', 'completed'].includes(snapshot.run.phase)
                    }
                    reopenPhase={reopenPhase}
                    onReopenPhase={setReopenPhase}
                    onReopen={() => void executeCommand('reopenPhase', { phase: reopenPhase })}
                  />
                </div>

                <div
                  id={`${domId}-panel`}
                  className={`${page ? 'min-h-0 flex-1 overflow-auto' : 'max-h-80 overflow-auto'} px-4 py-3 text-sm`}
                  role="tabpanel"
                  aria-labelledby={`${domId}-tab-${view.toLowerCase()}`}
                >
                  <RunViewContent
                    view={view}
                    snapshot={snapshot}
                    artifacts={artifacts}
                    events={events}
                    specContent={specArtifact?.content ?? ''}
                    actionPending={actionPending !== null}
                    approvalUnavailable={capabilities === null || !snapshotProviderAvailable}
                    onDecide={decide}
                    deliveryPreview={deliveryPreview}
                    deliverySelections={deliverySelections}
                    onDeliverySelections={setDeliverySelections}
                    onConfirmDelivery={confirmDelivery}
                  />
                </div>
              </>
            ) : (
              <div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
                <FilePlus2 className="size-8 text-muted-foreground" />
                {selectedSpec ? (
                  <>
                    <div>
                      <p className="font-medium">Configure a run for {selectedSpec.title}</p>
                      <p className="mt-1 max-w-xl text-xs text-muted-foreground">
                        This specification was discovered from repository artifacts. Choose its
                        provider and workflow policy before approval; preserved later-phase files
                        remain historical until Agentum regenerates them.
                      </p>
                    </div>
                    <Button onClick={openRunConfiguration}>Configure Run</Button>
                  </>
                ) : (
                  <>
                    <div>
                      <p className="font-medium">No specification for {projectName}</p>
                      <p className="mt-1 text-xs text-muted-foreground">
                        Create a spec to author requirements in an isolated Agentum worktree.
                      </p>
                    </div>
                    <Button onClick={() => setNewSpecOpen(true)}>New Spec</Button>
                  </>
                )}
              </div>
            )}
          </div>
        ) : null}
      </section>

      <NewSpecDialog
        open={newSpecOpen}
        onOpenChange={closeNewSpec}
        projectName={projectName}
        draft={draft}
        onDraft={updateDraft}
        creating={creating}
        capabilities={capabilities}
        capabilitiesError={capabilitiesError}
        remoteCapability={remoteCapability}
        remoteCapabilityError={remoteCapabilityError}
        sourcePreview={sourcePreview}
        sourcePreviewing={sourcePreviewing}
        jiraOauth={jiraOauth}
        jiraConnections={jiraConnections}
        jiraConnecting={jiraConnecting}
        onStartJiraOauth={startJiraConnection}
        onFinishJiraOauth={finishJiraConnection}
        onSelectJiraSite={chooseJiraSite}
        onConnectJiraApiToken={connectJiraWithApiToken}
        onPreview={previewSource}
        onCreate={create}
      />
      {selectedSpec ? (
        <RunConfigurationDialog
          open={configureRunOpen}
          onOpenChange={closeRunConfiguration}
          spec={selectedSpec}
          draft={runConfiguration}
          onDraft={setRunConfiguration}
          configuring={configuringRun}
          capabilities={capabilities}
          capabilitiesError={capabilitiesError}
          onCreate={configureDiscoveredRun}
        />
      ) : null}
    </>
  )
}

function RunStatus({ snapshot }: { snapshot: SddSnapshot }): React.JSX.Element {
  const blocked = snapshot.run.status === 'blocked' || snapshot.run.quarantined === 1
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ${blocked ? 'bg-destructive/10 text-destructive' : snapshot.run.status === 'waiting' ? 'bg-amber-500/10 text-amber-600' : snapshot.run.phase === 'ready' ? 'bg-emerald-500/10 text-emerald-600' : 'bg-muted text-muted-foreground'}`}
    >
      {blocked ? <CircleAlert className="size-3" /> : null}
      {snapshot.run.phase} · {snapshot.run.status}
    </span>
  )
}

function RunActions({
  actions,
  pending,
  onAction,
  canReopen,
  reopenPhase,
  onReopenPhase,
  onReopen
}: {
  actions: RunAction[]
  pending: string | null
  onAction: (action: RunAction) => Promise<void>
  canReopen: boolean
  reopenPhase: SddPhase
  onReopenPhase: (phase: SddPhase) => void
  onReopen: () => void
}): React.JSX.Element {
  const labels: Record<RunAction, string> = {
    startAuthoring: 'Re-author',
    startRun: 'Start',
    pause: 'Pause',
    resume: 'Resume',
    retry: 'Retry',
    resolveBlock: 'Resolve block',
    cancel: 'Cancel',
    previewDelivery: 'Preview delivery'
  }
  return (
    <div className="ml-auto flex flex-wrap items-center gap-1 pb-1">
      {canReopen ? (
        <>
          <select
            className="h-7 rounded-md border bg-background px-1 text-[11px]"
            value={reopenPhase}
            onChange={(event) => onReopenPhase(event.target.value as SddPhase)}
            aria-label="Phase to reopen"
          >
            {PHASES.slice(0, 6).map((phase) => (
              <option key={phase.id} value={phase.id}>
                {phase.label}
              </option>
            ))}
          </select>
          <Button
            size="sm"
            variant="outline"
            className="h-7"
            disabled={pending !== null}
            onClick={onReopen}
          >
            <RotateCcw className="mr-1 size-3" /> Reopen
          </Button>
        </>
      ) : null}
      {actions.map((action) => (
        <Button
          key={action}
          size="sm"
          variant={
            action === 'cancel' ? 'ghost' : action === 'previewDelivery' ? 'default' : 'outline'
          }
          className={`h-7 ${action === 'cancel' ? 'text-destructive' : ''}`}
          disabled={pending !== null}
          onClick={() => void onAction(action)}
        >
          {pending === action ? <Loader2 className="mr-1 size-3 animate-spin" /> : null}
          {labels[action]}
        </Button>
      ))}
    </div>
  )
}

export function RunViewContent({
  view,
  snapshot,
  artifacts,
  events,
  specContent,
  actionPending,
  approvalUnavailable = false,
  onDecide,
  deliveryPreview = null,
  deliverySelections = [],
  onDeliverySelections = () => undefined,
  onConfirmDelivery = async () => undefined
}: {
  view: RunView
  snapshot: SddSnapshot
  artifacts: SddArtifact[]
  events: SddEvent[]
  specContent: string
  actionPending: boolean
  approvalUnavailable?: boolean
  onDecide: (decision: 'approve' | 'reject', reason?: string) => Promise<void>
  deliveryPreview?: DeliveryPreview | null
  deliverySelections?: string[]
  onDeliverySelections?: (ids: string[]) => void
  onConfirmDelivery?: () => Promise<void>
}): React.JSX.Element {
  const historicalImportedArtifact = (artifact: SddArtifact): boolean =>
    snapshot.run.phase === 'specification' &&
    artifact.metadata.submittedBy.startsWith('agentum:filesystem-discovery:')
  if (view === 'Spec') {
    const requirements = specContent
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => /^- (RQ|AC)-\d+/.test(line))
    return (
      <div className="space-y-2">
        {artifacts.some((artifact) => artifact.externallyModified) ? (
          <p
            className="rounded-md border border-destructive/40 bg-destructive/5 p-2 text-destructive"
            role="alert"
          >
            An artifact changed outside Agentum. Approval is blocked until it is imported or
            repaired.
          </p>
        ) : null}
        <h3 className="font-semibold">Requirements and acceptance criteria</h3>
        {requirements.length > 0 ? (
          <ul className="space-y-1">
            {requirements.map((requirement) => (
              <li
                key={requirement}
                className="rounded-md bg-muted/50 px-2 py-1.5 font-mono text-xs"
              >
                {requirement.slice(2)}
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-muted-foreground">No stable RQ-* or AC-* entries were found.</p>
        )}
        <details>
          <summary className="cursor-pointer text-xs text-muted-foreground">Full spec.md</summary>
          <pre className="mt-2 whitespace-pre-wrap rounded-md bg-muted/40 p-3 font-mono text-xs">
            {specContent}
          </pre>
        </details>
      </div>
    )
  }
  if (view === 'Activity') {
    return events.length > 0 ? (
      <ol className="space-y-2">
        {[...events].reverse().map((event) => (
          <li
            key={event.eventId}
            className="grid gap-1 rounded-md border border-border/50 p-2 sm:grid-cols-[10rem_1fr]"
          >
            <time className="text-xs text-muted-foreground" dateTime={event.createdAt}>
              {new Date(event.createdAt).toLocaleString()}
            </time>
            <div className="min-w-0">
              <span className="font-mono text-xs">{event.kind}</span>
              {event.payload && Object.keys(event.payload as object).length > 0 ? (
                <pre className="mt-1 overflow-auto whitespace-pre-wrap text-[10px] text-muted-foreground">
                  {JSON.stringify(event.payload, null, 2)}
                </pre>
              ) : null}
            </div>
          </li>
        ))}
      </ol>
    ) : (
      <p className="text-muted-foreground">No durable activity yet.</p>
    )
  }
  if (view === 'Plan') {
    const artifact = artifacts.find((entry) => entry.metadata.kind === 'plan')
    if (artifact && historicalImportedArtifact(artifact)) {
      return <HistoricalArtifactNotice label="plan.json" />
    }
    return artifact ? (
      <pre className="whitespace-pre-wrap rounded-md bg-muted/40 p-3 font-mono text-xs">
        {prettyJson(artifact.content)}
      </pre>
    ) : (
      <p className="text-muted-foreground">No real plan artifact exists yet.</p>
    )
  }
  if (view === 'Tasks') {
    const plan = artifacts.find((entry) => entry.metadata.kind === 'plan')
    if (plan && historicalImportedArtifact(plan)) {
      return <HistoricalArtifactNotice label="plan.json task DAG" />
    }
    const tasks = parsePlanTasks(artifacts)
    return tasks.length > 0 ? (
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-left text-xs">
          <thead>
            <tr className="border-b">
              <th className="p-2">Task</th>
              <th className="p-2">Dependencies</th>
              <th className="p-2">Risk</th>
              <th className="p-2">Verification</th>
            </tr>
          </thead>
          <tbody>
            {tasks.map((task) => (
              <tr key={task.id} className="border-b border-border/50">
                <td className="p-2">
                  <span className="font-mono">{task.id}</span>
                  <p className="mt-1">{task.objective}</p>
                  <p className="mt-1 text-[10px] text-muted-foreground">
                    {task.parallelSafe ? 'Parallel-safe' : 'Serialized'} ·{' '}
                    {task.acceptanceCriteria.join(', ') || 'No AC refs'}
                  </p>
                </td>
                <td className="p-2 font-mono">{task.dependencies.join(', ') || '—'}</td>
                <td className="p-2">{task.risk}</td>
                <td className="p-2">{task.verificationCount}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    ) : (
      <p className="text-muted-foreground">No typed task DAG exists yet.</p>
    )
  }
  if (view === 'Evidence') {
    return <BrowserEvidenceView snapshot={snapshot} events={events} />
  }
  if (view === 'Review') {
    const artifact = artifacts.find((entry) => entry.metadata.kind === 'review')
    if (artifact && historicalImportedArtifact(artifact)) {
      return <HistoricalArtifactNotice label="review.md" />
    }
    return artifact ? (
      <pre className="whitespace-pre-wrap rounded-md bg-muted/40 p-3 font-mono text-xs">
        {artifact.content}
      </pre>
    ) : (
      <p className="text-muted-foreground">Independent review has not produced review.md yet.</p>
    )
  }

  const pendingApproval = snapshot.approval?.status === 'pending' ? snapshot.approval : null
  const autopilotSpecStart =
    pendingApproval?.purpose === 'specification' && snapshot.spec.control === 'autopilot'
  const approvalBlocked = artifacts.some((artifact) => artifact.externallyModified)
  return (
    <div className="space-y-3">
      <div className="grid gap-3 md:grid-cols-[1fr_auto]">
        <div>
          <p className="font-medium">{nextActionLabel(snapshot)}</p>
          <p className="mt-1 text-xs text-muted-foreground">
            {snapshot.spec.specId} · revision {snapshot.spec.currentRevision} ·{' '}
            {snapshot.spec.provider} · {snapshot.spec.profile} / {snapshot.spec.control}
          </p>
          <p className="mt-1 text-xs text-muted-foreground">
            Base {snapshot.run.baseRef} at {snapshot.run.baseCommit.slice(0, 12)}
          </p>
          {snapshot.run.blocker ? (
            <p className="mt-2 text-destructive" role="alert">
              {snapshot.run.blocker}
            </p>
          ) : null}
          {snapshot.run.quarantined === 1 ? (
            <p className="mt-2 text-destructive" role="alert">
              This run is quarantined. Preserve its recovery evidence and resolve it before
              continuing.
            </p>
          ) : null}
        </div>
        {pendingApproval && autopilotSpecStart ? (
          <div className="min-w-72 rounded-md border border-amber-500/30 bg-amber-500/5 p-2">
            <p className="text-xs font-semibold">Start authorizes this exact specification</p>
            <p
              className="mt-1 break-all font-mono text-[10px] text-muted-foreground"
              title={pendingApproval.digest}
            >
              {pendingApproval.digest}
            </p>
            <p className="mt-2 text-xs text-muted-foreground">
              Use Start in the action bar to authorize this digest and begin the Autopilot run.
            </p>
          </div>
        ) : pendingApproval ? (
          <ApprovalControls
            label={approvalLabel(pendingApproval.purpose)}
            digest={pendingApproval.digest}
            disabled={
              actionPending ||
              approvalUnavailable ||
              approvalBlocked ||
              snapshot.run.status !== 'waiting'
            }
            onDecide={onDecide}
          />
        ) : null}
      </div>
      {deliveryPreview ? (
        <DeliveryPreviewPanel
          preview={deliveryPreview}
          selected={deliverySelections}
          onSelected={onDeliverySelections}
          pending={actionPending}
          onConfirm={onConfirmDelivery}
        />
      ) : null}
      <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <Summary label="Phase" value={phaseLabel(snapshot.run.phase)} />
        <Summary label="Status" value={snapshot.run.status.replaceAll('_', ' ')} />
        <Summary label="Artifacts" value={String(artifacts.length)} />
        <Summary label="Durable events" value={String(events.length)} />
      </div>
    </div>
  )
}

function BrowserEvidenceView({
  snapshot,
  events
}: {
  snapshot: SddSnapshot
  events: SddEvent[]
}): React.JSX.Element {
  const [captureUrls, setCaptureUrls] = useState<Record<string, string>>({})
  const [loadingCapture, setLoadingCapture] = useState<string | null>(null)
  const [captureError, setCaptureError] = useState<string | null>(null)
  const captureUrlsRef = useRef<Record<string, string>>({})
  useEffect(() => {
    captureUrlsRef.current = captureUrls
  }, [captureUrls])
  useEffect(
    () => () => {
      for (const url of Object.values(captureUrlsRef.current)) URL.revokeObjectURL(url)
    },
    []
  )

  const loadCapture = async (record: SddBrowserEvidence, sha256: string): Promise<void> => {
    const key = `${record.evidenceId}:${sha256}`
    if (captureUrls[key]) return
    setLoadingCapture(key)
    setCaptureError(null)
    try {
      const blob = await getBrowserEvidenceBlob(record.runId, record.evidenceId, sha256)
      const url = URL.createObjectURL(blob)
      setCaptureUrls((current) => ({ ...current, [key]: url }))
    } catch (error) {
      setCaptureError(errorMessage(error, 'Could not load the immutable capture'))
    } finally {
      setLoadingCapture(null)
    }
  }

  const durableEvents = events.filter((event) =>
    /(verification|browser|patch|attempt|review)/i.test(event.kind)
  )
  if (snapshot.browserEvidence.length === 0 && durableEvents.length === 0) {
    return <p className="text-muted-foreground">No verification or patch evidence exists yet.</p>
  }
  return (
    <div className="space-y-3">
      {captureError ? (
        <p className="rounded-md border border-destructive/40 p-2 text-destructive" role="alert">
          {captureError}
        </p>
      ) : null}
      {snapshot.browserEvidence.map((record) => {
        const manifest = record.evidence
        const screenshot = record.blobs.find(
          (blob) => blob.role === 'capture' && blob.mediaType.startsWith('image/')
        )
        const captureKey = screenshot ? `${record.evidenceId}:${screenshot.sha256}` : ''
        const captureUrl = captureKey ? captureUrls[captureKey] : undefined
        return (
          <article key={record.evidenceId} className="space-y-3 rounded-md border p-3">
            <div className="flex flex-wrap items-start justify-between gap-2">
              <div>
                <h3 className="font-mono text-xs font-semibold">{record.checkId}</h3>
                <p className="mt-1 text-[10px] text-muted-foreground">
                  Attempt {record.attemptId} · {new Date(record.capturedAt).toLocaleString()}
                </p>
              </div>
              <span
                className={`rounded-full px-2 py-0.5 text-[10px] font-semibold ${
                  record.status === 'passed'
                    ? 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
                    : 'bg-destructive/10 text-destructive'
                }`}
              >
                {record.status.toUpperCase()}
              </span>
            </div>
            <dl className="grid gap-2 text-[11px] sm:grid-cols-2">
              <div>
                <dt className="text-muted-foreground">Redacted target</dt>
                <dd className="font-mono">{manifest.target.origin}{manifest.target.path}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Runtime</dt>
                <dd>
                  {manifest.browser.name} {manifest.browser.version} ·{' '}
                  {manifest.browser.viewportWidth}×{manifest.browser.viewportHeight}
                </dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Console attribution</dt>
                <dd>
                  {manifest.console.coverage} · {manifest.console.errors} errors /{' '}
                  {manifest.console.warnings} warnings
                </dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Network attribution</dt>
                <dd>
                  {manifest.network.coverage} · {manifest.network.failedRequests}/
                  {manifest.network.requests} failed
                </dd>
              </div>
            </dl>
            <ul className="space-y-1">
              {manifest.assertions.map((assertion) => (
                <li key={assertion.id} className="flex flex-wrap items-center gap-2 text-xs">
                  <span
                    className={
                      assertion.status === 'passed' ? 'text-emerald-600' : 'text-destructive'
                    }
                  >
                    {assertion.status === 'passed' ? 'PASS' : 'FAIL'}
                  </span>
                  <span className="font-mono">{assertion.id}</span>
                  <span className="text-muted-foreground">
                    {assertion.acceptanceCriteria.join(', ')}
                  </span>
                </li>
              ))}
            </ul>
            {screenshot ? (
              <div>
                {captureUrl ? (
                  <img
                    src={captureUrl}
                    alt={`Browser evidence capture for ${record.checkId}`}
                    className="max-h-96 rounded-md border object-contain"
                  />
                ) : (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={loadingCapture === captureKey}
                    onClick={() => void loadCapture(record, screenshot.sha256)}
                  >
                    {loadingCapture === captureKey ? (
                      <Loader2 className="mr-1 size-3 animate-spin" />
                    ) : null}
                    Load capture ({Math.ceil(screenshot.byteLength / 1024)} KiB)
                  </Button>
                )}
              </div>
            ) : null}
            <p className="break-all font-mono text-[10px] text-muted-foreground">
              manifest sha256:{record.manifestSha256}
            </p>
          </article>
        )
      })}
      {durableEvents.length > 0 ? (
        <details>
          <summary className="cursor-pointer text-xs text-muted-foreground">
            Durable verification activity ({durableEvents.length})
          </summary>
          <ol className="mt-2 space-y-1">
            {[...durableEvents].reverse().map((event) => (
              <li key={event.eventId} className="rounded-md border p-2">
                <span className="font-mono text-xs">{event.kind}</span>
                <span className="ml-2 text-xs text-muted-foreground">
                  revision {event.revision}
                </span>
              </li>
            ))}
          </ol>
        </details>
      ) : null}
    </div>
  )
}

function ApprovalControls({
  label,
  digest,
  disabled,
  onDecide
}: {
  label: string
  digest: string
  disabled: boolean
  onDecide: (decision: 'approve' | 'reject', reason?: string) => Promise<void>
}): React.JSX.Element {
  const [requestingChanges, setRequestingChanges] = useState(false)
  const [reason, setReason] = useState('')
  return (
    <div className="min-w-72 rounded-md border border-amber-500/30 bg-amber-500/5 p-2">
      <p className="text-xs font-semibold">{label}</p>
      <p className="mt-1 break-all font-mono text-[10px] text-muted-foreground" title={digest}>
        {digest}
      </p>
      {requestingChanges ? (
        <div className="mt-2 grid gap-2">
          <label className="grid gap-1 text-xs">
            <span>Required changes</span>
            <textarea
              className="min-h-16 rounded-md border bg-background p-2"
              value={reason}
              onChange={(event) => setReason(event.target.value)}
              autoFocus
            />
          </label>
          <div className="flex justify-end gap-1">
            <Button
              size="sm"
              variant="ghost"
              className="h-7"
              disabled={disabled}
              onClick={() => setRequestingChanges(false)}
            >
              Back
            </Button>
            <Button
              size="sm"
              variant="destructive"
              className="h-7"
              disabled={disabled || !reason.trim()}
              onClick={() => void onDecide('reject', reason)}
            >
              <X className="mr-1 size-3" /> Request changes
            </Button>
          </div>
        </div>
      ) : (
        <div className="mt-2 flex justify-end gap-1">
          <Button
            size="sm"
            variant="outline"
            className="h-7"
            disabled={disabled}
            onClick={() => setRequestingChanges(true)}
          >
            <X className="mr-1 size-3" /> Request changes
          </Button>
          <Button
            size="sm"
            className="h-7"
            disabled={disabled}
            onClick={() => void onDecide('approve')}
          >
            <Check className="mr-1 size-3" /> Approve exact digest
          </Button>
        </div>
      )}
    </div>
  )
}

function HistoricalArtifactNotice({ label }: { label: string }): React.JSX.Element {
  return (
    <div className="space-y-2">
      <p
        className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-amber-800 dark:text-amber-200"
        role="status"
      >
        Historical imported {label} is preserved for recovery only. It is not approved current
        intent and will not drive this run from the specification phase.
      </p>
    </div>
  )
}

function DeliveryIntentComposer({
  draft,
  onDraft,
  disabled
}: {
  draft: DeliveryIntentDraft
  onDraft: (draft: DeliveryIntentDraft) => void
  disabled: boolean
}): React.JSX.Element {
  const toggle = (key: keyof DeliveryIntentDraft, value: boolean): void => {
    onDraft({ ...draft, [key]: value })
  }
  return (
    <section
      className="mx-4 mt-3 rounded-md border border-border/70 bg-muted/20 p-3"
      aria-label="Delivery actions"
    >
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 className="text-xs font-semibold">Delivery intent</h3>
          <p className="mt-0.5 text-[10px] text-muted-foreground">
            Choose explicit external effects. Preview binds these values to the current Ready
            digest; nothing runs until confirmation.
          </p>
        </div>
      </div>
      <div className="mt-2 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <DeliveryIntentOption
          label="Commit"
          checked={draft.commit}
          disabled={disabled}
          onChecked={(value) => toggle('commit', value)}
        >
          <input
            aria-label="Commit message"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.commitMessage}
            disabled={!draft.commit || disabled}
            placeholder="Agentum: spec title"
            onChange={(event) => onDraft({ ...draft, commitMessage: event.target.value })}
          />
        </DeliveryIntentOption>
        <DeliveryIntentOption
          label="Push"
          checked={draft.push}
          disabled={disabled}
          onChecked={(value) => toggle('push', value)}
        >
          <input
            aria-label="Push remote"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.remote}
            disabled={!draft.push || disabled}
            onChange={(event) => onDraft({ ...draft, remote: event.target.value })}
          />
        </DeliveryIntentOption>
        <DeliveryIntentOption
          label="Pull request"
          checked={draft.pullRequest}
          disabled={disabled}
          onChecked={(value) =>
            onDraft({
              ...draft,
              pullRequest: value,
              commit: value || draft.commit,
              push: value || draft.push
            })
          }
        >
          <input
            aria-label="Pull request title"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.pullRequestTitle}
            disabled={!draft.pullRequest || disabled}
            placeholder="Spec title"
            onChange={(event) => onDraft({ ...draft, pullRequestTitle: event.target.value })}
          />
          <input
            aria-label="Pull request base"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.pullRequestBase}
            disabled={!draft.pullRequest || disabled}
            placeholder="main"
            onChange={(event) => onDraft({ ...draft, pullRequestBase: event.target.value })}
          />
          <textarea
            aria-label="Pull request body"
            className="min-h-14 w-full rounded border bg-background px-2 py-1 text-[11px]"
            value={draft.pullRequestBody}
            disabled={!draft.pullRequest || disabled}
            onChange={(event) => onDraft({ ...draft, pullRequestBody: event.target.value })}
          />
        </DeliveryIntentOption>
        <DeliveryIntentOption
          label="Tracker comment"
          checked={draft.trackerComment}
          disabled={disabled}
          onChecked={(value) => toggle('trackerComment', value)}
        >
          <textarea
            aria-label="Tracker comment body"
            className="min-h-14 w-full rounded border bg-background px-2 py-1 text-[11px]"
            value={draft.trackerCommentBody}
            disabled={!draft.trackerComment || disabled}
            onChange={(event) => onDraft({ ...draft, trackerCommentBody: event.target.value })}
          />
        </DeliveryIntentOption>
        <DeliveryIntentOption
          label="Tracker status"
          checked={draft.trackerStatus}
          disabled={disabled}
          onChecked={(value) => toggle('trackerStatus', value)}
        >
          <input
            aria-label="Tracker target status"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.trackerStatusName}
            disabled={!draft.trackerStatus || disabled}
            placeholder="Done"
            onChange={(event) => onDraft({ ...draft, trackerStatusName: event.target.value })}
          />
        </DeliveryIntentOption>
        <DeliveryIntentOption
          label="Tracker field"
          checked={draft.trackerField}
          disabled={disabled}
          onChecked={(value) => toggle('trackerField', value)}
        >
          <input
            aria-label="Tracker field ID"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.trackerFieldId}
            disabled={!draft.trackerField || disabled}
            placeholder="customfield_10042"
            onChange={(event) => onDraft({ ...draft, trackerFieldId: event.target.value })}
          />
          <input
            aria-label="Tracker field value"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.trackerFieldValue}
            disabled={!draft.trackerField || disabled}
            onChange={(event) => onDraft({ ...draft, trackerFieldValue: event.target.value })}
          />
        </DeliveryIntentOption>
        <DeliveryIntentOption
          label="Release"
          checked={draft.release}
          disabled={disabled}
          onChecked={(value) => toggle('release', value)}
        >
          <input
            aria-label="Release tag"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.releaseTag}
            disabled={!draft.release || disabled}
            placeholder="v1.0.0"
            onChange={(event) => onDraft({ ...draft, releaseTag: event.target.value })}
          />
          <input
            aria-label="Release name"
            className="h-7 w-full rounded border bg-background px-2 text-[11px]"
            value={draft.releaseName}
            disabled={!draft.release || disabled}
            placeholder="Release name"
            onChange={(event) => onDraft({ ...draft, releaseName: event.target.value })}
          />
          <textarea
            aria-label="Release notes"
            className="min-h-14 w-full rounded border bg-background px-2 py-1 text-[11px]"
            value={draft.releaseNotes}
            disabled={!draft.release || disabled}
            onChange={(event) => onDraft({ ...draft, releaseNotes: event.target.value })}
          />
          <label className="flex items-center gap-1 text-[10px]">
            <input
              type="checkbox"
              checked={draft.prerelease}
              disabled={!draft.release || disabled}
              onChange={(event) => onDraft({ ...draft, prerelease: event.target.checked })}
            />
            Prerelease
          </label>
        </DeliveryIntentOption>
        <DeliveryIntentOption
          label="OpenSpec export"
          checked={draft.openSpecExport}
          disabled={disabled}
          onChecked={(value) => toggle('openSpecExport', value)}
        >
          <span className="text-[10px] text-muted-foreground">
            One-shot export; Agentum remains authoritative.
          </span>
        </DeliveryIntentOption>
      </div>
    </section>
  )
}

function DeliveryIntentOption({
  label,
  checked,
  disabled,
  onChecked,
  children
}: {
  label: string
  checked: boolean
  disabled: boolean
  onChecked: (checked: boolean) => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className={`rounded-md border bg-background p-2 ${checked ? 'border-primary/50' : ''}`}>
      <label className="flex items-center gap-2 text-xs font-medium">
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(event) => onChecked(event.target.checked)}
        />
        {label}
      </label>
      <div className="mt-2 grid gap-1">{children}</div>
    </div>
  )
}

function DeliveryPreviewPanel({
  preview,
  selected,
  onSelected,
  pending,
  onConfirm
}: {
  preview: DeliveryPreview
  selected: string[]
  onSelected: (ids: string[]) => void
  pending: boolean
  onConfirm: () => Promise<void>
}): React.JSX.Element {
  return (
    <section
      className="rounded-md border border-primary/30 bg-primary/5 p-3"
      aria-label="Delivery preview"
    >
      <h3 className="font-semibold">Hash-bound delivery preview</h3>
      {preview.summary ? <p className="mt-1 text-xs">{preview.summary}</p> : null}
      {preview.digest ? (
        <p className="mt-1 break-all font-mono text-[10px] text-muted-foreground">
          {preview.digest}
        </p>
      ) : null}
      <div className="mt-2 grid gap-2 sm:grid-cols-2">
        {preview.actions.map((action) => {
          const unavailable = action.enabled === false || Boolean(action.blockedReason)
          return (
            <label
              key={action.id}
              className={`flex items-start gap-2 rounded-md border bg-background p-2 ${unavailable ? 'opacity-60' : ''}`}
            >
              <input
                type="checkbox"
                className="mt-0.5"
                checked={selected.includes(action.id)}
                disabled={unavailable || pending}
                onChange={(event) =>
                  onSelected(
                    event.target.checked
                      ? [...selected, action.id]
                      : selected.filter((id) => id !== action.id)
                  )
                }
              />
              <span>
                <span className="text-xs font-medium">
                  {action.label ?? action.type.replaceAll('_', ' ')}
                </span>
                {action.description ? (
                  <span className="block text-[10px] text-muted-foreground">
                    {action.description}
                  </span>
                ) : null}
                {action.blockedReason ? (
                  <span className="block text-[10px] text-destructive">{action.blockedReason}</span>
                ) : null}
              </span>
            </label>
          )
        })}
      </div>
      {preview.expiresAt ? (
        <p className="mt-2 text-[10px] text-muted-foreground">
          Expires {new Date(preview.expiresAt).toLocaleString()}
        </p>
      ) : null}
      <div className="mt-2 flex justify-end">
        <Button disabled={pending || selected.length === 0} onClick={() => void onConfirm()}>
          Confirm selected delivery actions
        </Button>
      </div>
    </section>
  )
}

function Summary({ label, value }: { label: string; value: string }): React.JSX.Element {
  return (
    <div className="rounded-md border border-border/60 p-2">
      <p className="text-[10px] uppercase tracking-wide text-muted-foreground">{label}</p>
      <p className="mt-1 text-xs font-medium capitalize">{value}</p>
    </div>
  )
}

function RunConfigurationDialog({
  open,
  onOpenChange,
  spec,
  draft,
  onDraft,
  configuring,
  capabilities,
  capabilitiesError,
  onCreate
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  spec: SddSpec
  draft: RunConfigurationDraft
  onDraft: (draft: RunConfigurationDraft) => void
  configuring: boolean
  capabilities: SddCapabilities | null
  capabilitiesError: string | null
  onCreate: () => Promise<void>
}): React.JSX.Element {
  const selectedProvider = capabilities?.providers.find((entry) => entry.id === draft.provider)
  const providerReady = selectedProvider?.available === true
  const valid =
    Boolean(draft.baseRef.trim()) &&
    providerReady &&
    capabilities?.localProviderExecution.available === true
  return (
    <Dialog open={open} onOpenChange={(next) => !configuring && onOpenChange(next)}>
      <DialogContent className="max-h-[90vh] overflow-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Configure Run</DialogTitle>
          <DialogDescription>
            Create the first durable run for {spec.title}. Repository-discovered design, plan,
            decisions, and review files stay historical and unapproved until their phases run
            again.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <Labeled label="Specification">
            <input
              className="h-9 rounded-md border bg-muted/30 px-3 text-sm"
              value={`${spec.title} · r${spec.currentRevision}`}
              disabled
            />
          </Labeled>
          <div className="grid gap-3 sm:grid-cols-2">
            <Labeled label="Profile">
              <select
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.profile}
                onChange={(event) =>
                  onDraft({
                    ...draft,
                    profile: event.target.value as RunConfigurationDraft['profile']
                  })
                }
              >
                <option value="standard">Standard</option>
                <option value="high_risk">High risk</option>
              </select>
            </Labeled>
            <Labeled label="Control">
              <select
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.control}
                onChange={(event) =>
                  onDraft({
                    ...draft,
                    control: event.target.value as RunConfigurationDraft['control']
                  })
                }
              >
                <option value="guarded">Guarded</option>
                <option value="interactive">Interactive</option>
                <option value="autopilot">Autopilot</option>
              </select>
            </Labeled>
            <Labeled label="Provider">
              <select
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.provider}
                onChange={(event) => onDraft({ ...draft, provider: event.target.value })}
              >
                {providerOptions(capabilities).map((provider) => (
                  <option key={provider.id} value={provider.id} disabled={provider.available !== true}>
                    {String(provider.label ?? PROVIDER_LABELS[provider.id] ?? provider.id)}
                    {provider.available === true ? '' : ' (unavailable)'}
                  </option>
                ))}
              </select>
            </Labeled>
            <Labeled label="Base">
              <input
                className="h-9 rounded-md border bg-background px-3 text-sm"
                value={draft.baseRef}
                onChange={(event) => onDraft({ ...draft, baseRef: event.target.value })}
              />
            </Labeled>
            <Labeled label="Source checkout">
              <select
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.sourceCheckout}
                onChange={(event) =>
                  onDraft({
                    ...draft,
                    sourceCheckout: event.target.value as RunConfigurationDraft['sourceCheckout']
                  })
                }
              >
                <option value="require_clean">Require clean checkout</option>
                <option value="committed_base">Use committed HEAD</option>
                <option value="snapshot">Capture recoverable snapshot</option>
              </select>
            </Labeled>
          </div>
          {capabilitiesError ? (
            <p
              className="rounded-md border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive"
              role="alert"
            >
              {capabilitiesError}. Run creation is disabled until capabilities can be verified.
            </p>
          ) : !capabilities ? (
            <p className="text-xs text-muted-foreground" role="status">
              Checking provider capabilities…
            </p>
          ) : !providerReady ? (
            <p
              className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-300"
              role="alert"
            >
              {selectedProvider?.reason ?? 'The selected provider is unavailable.'}
            </p>
          ) : null}
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={configuring} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={configuring || !valid} onClick={() => void onCreate()}>
            {configuring ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
            Create Run
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function NewSpecDialog({
  open,
  onOpenChange,
  projectName,
  draft,
  onDraft,
  creating,
  capabilities,
  capabilitiesError,
  remoteCapability,
  remoteCapabilityError,
  sourcePreview,
  sourcePreviewing,
  jiraOauth,
  jiraConnections,
  jiraConnecting,
  onStartJiraOauth,
  onFinishJiraOauth,
  onSelectJiraSite,
  onConnectJiraApiToken,
  onPreview,
  onCreate
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  projectName: string
  draft: NewSpecDraft
  onDraft: (draft: NewSpecDraft) => void
  creating: boolean
  capabilities: SddCapabilities | null
  capabilitiesError: string | null
  remoteCapability: SddRemoteCapability | null
  remoteCapabilityError: string | null
  sourcePreview: SddSourcePreview | null
  sourcePreviewing: boolean
  jiraOauth: JiraOauthStart | null
  jiraConnections: JiraConnection[]
  jiraConnecting: boolean
  onStartJiraOauth: () => Promise<void>
  onFinishJiraOauth: () => Promise<void>
  onSelectJiraSite: (connection: JiraConnection, siteId: string) => Promise<void>
  onConnectJiraApiToken: (input: {
    email: string
    apiToken: string
    siteUrl: string
  }) => Promise<void>
  onPreview: () => Promise<void>
  onCreate: () => Promise<void>
}): React.JSX.Element {
  const [jiraEmail, setJiraEmail] = useState('')
  const [jiraApiToken, setJiraApiToken] = useState('')
  const [jiraSiteUrl, setJiraSiteUrl] = useState('')
  const [jiraRiskAcknowledged, setJiraRiskAcknowledged] = useState(false)
  useEffect(() => {
    if (open) return
    setJiraEmail('')
    setJiraApiToken('')
    setJiraSiteUrl('')
    setJiraRiskAcknowledged(false)
  }, [open])
  const selectedSource =
    SOURCE_OPTIONS.find((option) => option.id === draft.sourceKind) ?? SOURCE_OPTIONS[0]
  const providerCapability = capabilities?.providers.find((entry) => entry.id === draft.provider)
  const sourceCapability = capabilities?.sources.find((entry) => entry.id === draft.sourceKind)
  const jiraCapability = capabilities?.sources.find((entry) => entry.id === 'jira')
  const jiraBrokerConfigured = jiraCapability?.brokerConfigured === true
  const jiraApiTokenFallbackEnabled = jiraCapability?.apiTokenFallbackEnabled === true
  const selectedProviderAvailable = providerCapability?.available === true
  const localProviderExecutionAvailable =
    capabilities?.localProviderExecution.available === true
  const selectedSourceAvailable = sourceCapability?.available === true
  const referenceRequired = sourceNeedsReference(draft.sourceKind)
  const previewAvailable = sourceCapability?.preview === true && referenceRequired
  const goal = sourceGoal(draft.sourceKind, draft.goal, draft.sourceReference)
  const repositoryAvailable = repositorySddAvailable(remoteCapability)
  const remoteRepository =
    remoteCapability !== null && remoteCapability.reason !== 'repository_is_local'
  const valid =
    Boolean(draft.title.trim() && goal) &&
    (!referenceRequired || Boolean(draft.sourceReference.trim())) &&
    (remoteRepository
      ? remoteCapability?.available === true
      : selectedProviderAvailable && localProviderExecutionAvailable) &&
    selectedSourceAvailable &&
    (!remoteRepository || ['description', 'socratic', 'markdown'].includes(draft.sourceKind)) &&
    repositoryAvailable &&
    !remoteCapabilityError

  return (
    <Dialog open={open} onOpenChange={(next) => !creating && onOpenChange(next)}>
      <DialogContent className="max-h-[90vh] overflow-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>New Spec</DialogTitle>
          <DialogDescription>
            Nothing is written until Create &amp; Author succeeds. Canceling discards this local
            draft and creates no durable run.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid gap-3 sm:grid-cols-2">
            <Labeled label="Project">
              <input
                className="h-9 rounded-md border bg-muted/30 px-3 text-sm"
                value={projectName}
                disabled
              />
            </Labeled>
            <Labeled label="Title">
              <input
                autoFocus
                className="h-9 rounded-md border bg-background px-3 text-sm"
                value={draft.title}
                onChange={(event) => onDraft({ ...draft, title: event.target.value })}
                placeholder="Refresh access tokens"
              />
            </Labeled>
          </div>
          <fieldset className="grid gap-2">
            <legend className="text-xs font-medium text-muted-foreground">Authoring source</legend>
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {SOURCE_OPTIONS.map((option) => (
                <label
                  key={option.id}
                  className={`rounded-md border p-2 text-xs ${draft.sourceKind === option.id ? 'border-primary bg-primary/5' : 'border-border'} ${capabilities?.sources.find((entry) => entry.id === option.id)?.available === true ? 'cursor-pointer' : 'cursor-not-allowed opacity-55'}`}
                >
                  <input
                    type="radio"
                    className="sr-only"
                    name="sdd-source"
                    value={option.id}
                    checked={draft.sourceKind === option.id}
                    disabled={
                      capabilities?.sources.find((entry) => entry.id === option.id)?.available !==
                      true
                    }
                    onChange={() =>
                      onDraft({
                        ...draft,
                        sourceKind: option.id,
                        sourceReference: ''
                      })
                    }
                  />
                  <span className="font-medium">{option.label}</span>
                  <span className="mt-0.5 block text-[10px] text-muted-foreground">
                    {option.hint}
                  </span>
                  {capabilities?.sources.find((entry) => entry.id === option.id)?.available ===
                  false ? (
                    <span className="mt-1 block text-[10px] text-amber-700 dark:text-amber-300">
                      {capabilities.sources.find((entry) => entry.id === option.id)?.reason ??
                        'Adapter unavailable'}
                    </span>
                  ) : null}
                </label>
              ))}
            </div>
          </fieldset>
          {jiraCapability?.available !== true &&
          (jiraBrokerConfigured || jiraApiTokenFallbackEnabled) ? (
            <section
              className="rounded-md border border-amber-500/30 bg-amber-500/5 p-3"
              aria-label="Jira Cloud connection"
            >
              <h3 className="text-xs font-semibold">Connect Jira Cloud</h3>
              <p className="mt-1 text-[10px] text-muted-foreground">
                Credentials stay in the secure vault. Jira remains read-only until an explicit,
                hash-bound Deliver confirmation.
              </p>
              {jiraBrokerConfigured ? (
                <div className="mt-2 flex flex-wrap gap-2">
                  {!jiraOauth ? (
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={jiraConnecting}
                      onClick={() => void onStartJiraOauth()}
                    >
                      {jiraConnecting ? <Loader2 className="mr-1 size-3 animate-spin" /> : null}
                      Authorize Jira Cloud
                    </Button>
                  ) : (
                    <>
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={jiraConnecting}
                        onClick={() => void api.shell.openUrl(jiraOauth.authorizationUrl)}
                      >
                        Reopen authorization
                      </Button>
                      <Button
                        size="sm"
                        disabled={jiraConnecting}
                        onClick={() => void onFinishJiraOauth()}
                      >
                        {jiraConnecting ? <Loader2 className="mr-1 size-3 animate-spin" /> : null}
                        Finish connection
                      </Button>
                    </>
                  )}
                </div>
              ) : null}
              {jiraConnections.flatMap((connection) =>
                connection.selectedSiteId
                  ? []
                  : connection.sites.map((site) => (
                      <Button
                        key={`${connection.connectionId}:${site.id}`}
                        className="mr-2 mt-2"
                        size="sm"
                        variant="outline"
                        disabled={jiraConnecting}
                        onClick={() => void onSelectJiraSite(connection, site.id)}
                      >
                        Use {site.name}
                      </Button>
                    ))
              )}
              {jiraApiTokenFallbackEnabled ? (
                <details className="mt-3 rounded border bg-background p-2">
                  <summary className="cursor-pointer text-xs font-medium">
                    Advanced local API-token authentication
                  </summary>
                  <p className="mt-2 text-[10px] text-amber-700 dark:text-amber-300">
                    This fallback is only for a local desktop or self-hosted Agentum. The token is
                    sent directly to your Atlassian tenant, never to Agentum's OAuth broker.
                  </p>
                  <div className="mt-2 grid gap-2 sm:grid-cols-3">
                    <Labeled label="Atlassian email">
                      <input
                        className="h-8 rounded border bg-background px-2 text-xs"
                        autoComplete="username"
                        value={jiraEmail}
                        onChange={(event) => setJiraEmail(event.target.value)}
                      />
                    </Labeled>
                    <Labeled label="Jira site URL">
                      <input
                        className="h-8 rounded border bg-background px-2 text-xs"
                        placeholder="https://team.atlassian.net"
                        value={jiraSiteUrl}
                        onChange={(event) => setJiraSiteUrl(event.target.value)}
                      />
                    </Labeled>
                    <Labeled label="API token">
                      <input
                        type="password"
                        className="h-8 rounded border bg-background px-2 text-xs"
                        autoComplete="off"
                        value={jiraApiToken}
                        onChange={(event) => setJiraApiToken(event.target.value)}
                      />
                    </Labeled>
                  </div>
                  <label className="mt-2 flex items-start gap-2 text-[10px]">
                    <input
                      className="mt-0.5"
                      type="checkbox"
                      checked={jiraRiskAcknowledged}
                      onChange={(event) => setJiraRiskAcknowledged(event.target.checked)}
                    />
                    I understand this credential can perform Jira actions allowed by my Atlassian
                    account, and writes still require Deliver preview and confirmation.
                  </label>
                  <div className="mt-2 flex justify-end">
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={
                        jiraConnecting ||
                        !jiraRiskAcknowledged ||
                        !jiraEmail.trim() ||
                        !jiraApiToken ||
                        !jiraSiteUrl.trim()
                      }
                      onClick={() =>
                        void (async () => {
                          try {
                            await onConnectJiraApiToken({
                              email: jiraEmail.trim(),
                              apiToken: jiraApiToken,
                              siteUrl: jiraSiteUrl.trim()
                            })
                            setJiraApiToken('')
                            setJiraRiskAcknowledged(false)
                          } catch {
                            /* Parent reports the typed server error. */
                          }
                        })()
                      }
                    >
                      Store API token securely
                    </Button>
                  </div>
                </details>
              ) : null}
            </section>
          ) : null}
          {referenceRequired ? (
            <Labeled label={selectedSource.valueLabel ?? 'Source reference'}>
              <input
                className="h-9 rounded-md border bg-background px-3 text-sm"
                value={draft.sourceReference}
                onChange={(event) => onDraft({ ...draft, sourceReference: event.target.value })}
                placeholder={selectedSource.valuePlaceholder}
              />
            </Labeled>
          ) : null}
          {previewAvailable ? (
            <div className="flex justify-end">
              <Button
                variant="outline"
                disabled={
                  sourcePreviewing ||
                  !draft.title.trim() ||
                  !draft.sourceReference.trim() ||
                  !repositoryAvailable ||
                  Boolean(remoteCapabilityError)
                }
                onClick={() => void onPreview()}
              >
                {sourcePreviewing ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
                Preview source
              </Button>
            </div>
          ) : null}
          {sourcePreview ? (
            <section
              className="rounded-md border border-primary/30 bg-primary/5 p-3 text-xs"
              aria-label="Source preview"
            >
              <p className="font-medium">{sourcePreview.title}</p>
              <p className="mt-1 break-all font-mono text-[10px] text-muted-foreground">
                {sourcePreview.sourceRevision}
              </p>
              <p className="mt-1 text-muted-foreground">
                Immutable {sourcePreview.kind} snapshot · {sourcePreview.taskCount} imported task
                {sourcePreview.taskCount === 1 ? '' : 's'}
                {sourcePreview.designAvailable ? ' · design available' : ''}
              </p>
              {sourcePreview.diagnostics.map((diagnostic) => (
                <p
                  key={`${diagnostic.code}:${diagnostic.path ?? ''}`}
                  className="mt-1 text-amber-700 dark:text-amber-300"
                >
                  {diagnostic.message}
                </p>
              ))}
            </section>
          ) : null}
          <Labeled
            label={
              draft.sourceKind === 'markdown'
                ? 'Markdown'
                : draft.sourceKind === 'socratic'
                  ? 'Starting context and questions'
                  : referenceRequired
                    ? 'Additional goal or constraints'
                    : 'Goal'
            }
          >
            <textarea
              className="min-h-28 resize-y rounded-md border bg-background px-3 py-2 text-sm"
              value={draft.goal}
              onChange={(event) => onDraft({ ...draft, goal: event.target.value })}
              placeholder={
                draft.sourceKind === 'markdown'
                  ? '# Goal\n\nRefresh tokens without interrupting sessions…'
                  : 'Refresh access tokens without interrupting active sessions'
              }
            />
          </Labeled>
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-5">
            <Labeled label="Profile">
              <select
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.profile}
                onChange={(event) =>
                  onDraft({
                    ...draft,
                    profile: event.target.value as NewSpecDraft['profile']
                  })
                }
              >
                <option value="standard">Standard</option>
                <option value="high_risk">High risk</option>
              </select>
            </Labeled>
            <Labeled label="Control">
              <select
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.control}
                onChange={(event) =>
                  onDraft({
                    ...draft,
                    control: event.target.value as NewSpecDraft['control']
                  })
                }
              >
                <option value="guarded">Guarded</option>
                <option value="interactive">Interactive</option>
                <option value="autopilot">Autopilot</option>
              </select>
            </Labeled>
            <Labeled label="Provider">
              <select
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.provider}
                onChange={(event) => onDraft({ ...draft, provider: event.target.value })}
              >
                {providerOptions(capabilities).map((capability) => (
                  <option
                    key={capability.id}
                    value={capability.id}
                    disabled={!remoteRepository && !capability.available}
                  >
                    {capability.label ?? PROVIDER_LABELS[capability.id] ?? capability.id}
                    {!remoteRepository && !capability.available
                      ? ` — ${capability.reason ?? 'unavailable'}`
                      : ''}
                  </option>
                ))}
              </select>
            </Labeled>
            <Labeled label="Base">
              <input
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.baseRef}
                onChange={(event) => onDraft({ ...draft, baseRef: event.target.value })}
              />
            </Labeled>
            <Labeled label="Source checkout">
              <select
                aria-label="Source checkout"
                className="h-9 rounded-md border bg-background px-2 text-sm"
                value={draft.sourceCheckout}
                onChange={(event) =>
                  onDraft({
                    ...draft,
                    sourceCheckout: event.target.value as NewSpecDraft['sourceCheckout']
                  })
                }
              >
                <option value="require_clean">Require clean checkout</option>
                <option value="committed_base">Use committed HEAD</option>
                <option value="snapshot" disabled={remoteRepository}>
                  Capture recoverable snapshot
                </option>
              </select>
            </Labeled>
          </div>
          {draft.sourceCheckout === 'committed_base' ? (
            <p className="text-[10px] text-amber-700 dark:text-amber-300">
              Uncommitted source-checkout changes are excluded; the run starts from the selected
              committed base.
            </p>
          ) : null}
          {draft.sourceCheckout === 'snapshot' ? (
            <p className="text-[10px] text-muted-foreground">
              Agentum validates and hashes supported dirty-checkout content into recoverable
              external snapshot state before creating the run.
            </p>
          ) : null}
          {!remoteRepository && capabilities?.localProviderExecution.available === false ? (
            <p
              className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-300"
              role="alert"
            >
              {capabilities.localProviderExecution.reason ??
                'Agentum local provider execution is unavailable on this platform.'}
            </p>
          ) : null}
          {capabilitiesError ? (
            <p
              className="rounded-md border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive"
              role="alert"
            >
              {capabilitiesError}. Creation is disabled until capabilities can be verified.
            </p>
          ) : null}
          {!capabilities ? (
            <p className="text-xs text-muted-foreground" role="status">
              Checking provider and source capabilities…
            </p>
          ) : null}
          {!remoteCapability && !remoteCapabilityError ? (
            <p className="text-xs text-muted-foreground" role="status">
              Verifying whether this repository can run SDD locally or through the fixed remote
              worker…
            </p>
          ) : null}
          {remoteCapabilityError ? (
            <p
              className="rounded-md border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive"
              role="alert"
            >
              {remoteCapabilityError}. Creation is disabled because Agentum cannot safely determine
              the repository execution boundary.
            </p>
          ) : null}
          {remoteRepository && remoteCapability && !remoteCapability.available ? (
            <p
              className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-300"
              role="alert"
            >
              {remoteCapabilityMessage(remoteCapability)}
            </p>
          ) : null}
          {capabilities &&
          capabilities.localProviderExecution.available !== false &&
          !selectedProviderAvailable ? (
            <p
              className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-300"
              role="alert"
            >
              {providerCapability?.reason ?? 'The selected provider is unavailable.'}
            </p>
          ) : null}
          {capabilities && !selectedSourceAvailable ? (
            <p
              className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-300"
              role="alert"
            >
              {sourceCapability?.reason ?? 'The selected source adapter is unavailable.'}
            </p>
          ) : null}
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={creating} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button disabled={creating || !valid} onClick={() => void onCreate()}>
            {creating ? <Loader2 className="mr-2 size-4 animate-spin" /> : null}
            Create &amp; Author
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Labeled({
  label,
  children,
  compact = false
}: {
  label: string
  children: React.ReactNode
  compact?: boolean
}): React.JSX.Element {
  return (
    <label
      className={`grid gap-1 text-xs font-medium text-muted-foreground ${compact ? 'gap-0.5' : ''}`}
    >
      <span>{label}</span>
      {children}
    </label>
  )
}

export function nextActionLabel(snapshot: SddSnapshot | null): string {
  if (!snapshot) return 'Create a spec'
  if (snapshot.run.quarantined === 1) return 'Recovery required'
  if (snapshot.approval?.status === 'pending') {
    if (
      snapshot.approval.purpose === 'specification' &&
      snapshot.spec.control === 'autopilot'
    ) {
      return 'Start to authorize the exact specification digest'
    }
    return approvalLabel(snapshot.approval.purpose)
  }
  if (snapshot.run.status === 'blocked') return snapshot.run.blocker ?? 'Resolve blocker'
  if (snapshot.run.status === 'paused') return 'Resume run'
  if (snapshot.run.phase === 'ready') return 'Preview delivery'
  if (snapshot.run.status === 'canceled') return 'Run canceled'
  if (snapshot.run.status === 'failed') return 'Retry or reopen the failed phase'
  if (snapshot.run.phase === 'completed') return 'Workflow completed'
  return `${phaseLabel(snapshot.run.phase)} · ${snapshot.run.status.replaceAll('_', ' ')}`
}

function prettyJson(value: string): string {
  try {
    return JSON.stringify(JSON.parse(value), null, 2)
  } catch {
    return value
  }
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback
}
