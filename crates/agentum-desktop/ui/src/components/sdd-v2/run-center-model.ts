import type {
  SddArtifact,
  SddDeliveryAction,
  SddEvent,
  SddPhase,
  SddSnapshot,
  SddSourceKind
} from '@/runtime/sdd-v2-client'

export type RunAction =
  | 'startAuthoring'
  | 'startRun'
  | 'pause'
  | 'resume'
  | 'retry'
  | 'resolveBlock'
  | 'cancel'
  | 'previewDelivery'

export function snapshotBelongsToRepository(
  snapshot: SddSnapshot,
  repoId: string,
  runId?: string
): boolean {
  return (
    snapshot.spec.repoId === repoId &&
    snapshot.run.repoId === repoId &&
    snapshot.run.specId === snapshot.spec.specId &&
    (!runId || snapshot.run.runId === runId)
  )
}

/** Commands the human-facing surface may honestly offer in the current state. */
export function availableRunActions(snapshot: SddSnapshot): RunAction[] {
  const { run } = snapshot
  const pendingApproval = snapshot.approval?.status === 'pending'
  const autopilotSpecStart =
    pendingApproval &&
    snapshot.approval?.purpose === 'specification' &&
    snapshot.spec.control === 'autopilot' &&
    run.phase === 'specification' &&
    run.status === 'waiting'
  if (run.quarantined === 1 || run.status === 'canceled' || run.phase === 'completed') {
    return []
  }

  const actions: RunAction[] = []
  if (autopilotSpecStart) {
    actions.push('startRun')
  } else if (!pendingApproval) {
    if (run.phase === 'ready') actions.push('previewDelivery')
    if (
      run.phase === 'specification' &&
      (run.status === 'idle' || run.status === 'paused' || run.status === 'blocked')
    ) {
      actions.push('startAuthoring')
    } else {
      if (run.status === 'idle' || run.status === 'queued') actions.push('startRun')
      if (run.status === 'paused') actions.push('resume')
      if (run.status === 'blocked') actions.push('resolveBlock')
    }
    if (run.status === 'failed' || run.status === 'retry_scheduled') actions.push('retry')
    if (run.status === 'queued' || run.status === 'running') actions.push('pause')
  }
  actions.push('cancel')
  return actions
}

export function appendDurableEvent(events: SddEvent[], event: SddEvent, runId: string): SddEvent[] {
  if (event.runId !== runId || events.some((current) => current.eventId === event.eventId)) {
    return events
  }
  const next = [...events, event]
  next.sort((left, right) => left.cursor - right.cursor)
  return next.slice(-500)
}

export type PlanTaskView = {
  id: string
  objective: string
  dependencies: string[]
  acceptanceCriteria: string[]
  risk: string
  parallelSafe: boolean
  verificationCount: number
}

export function parsePlanTasks(artifacts: SddArtifact[]): PlanTaskView[] {
  const artifact = artifacts.find((entry) => entry.metadata.kind === 'plan')
  if (!artifact) return []
  try {
    const value = JSON.parse(artifact.content) as { tasks?: unknown }
    if (!Array.isArray(value.tasks)) return []
    return value.tasks.flatMap((raw) => {
      if (!raw || typeof raw !== 'object') return []
      const task = raw as Record<string, unknown>
      if (typeof task.id !== 'string' || typeof task.objective !== 'string') return []
      return [
        {
          id: task.id,
          objective: task.objective,
          dependencies: stringArray(task.dependencies),
          acceptanceCriteria: stringArray(task.acceptanceCriteria),
          risk: typeof task.risk === 'string' ? task.risk : 'unspecified',
          parallelSafe: task.parallelSafe === true,
          verificationCount: Array.isArray(task.verification) ? task.verification.length : 0
        }
      ]
    })
  } catch {
    return []
  }
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []
}

export type SourceOption = {
  id: SddSourceKind
  label: string
  hint: string
  valueLabel?: string
  valuePlaceholder?: string
  requiresIntegration?: 'github' | 'linear' | 'jira' | 'openspec'
}

export const SOURCE_OPTIONS: SourceOption[] = [
  { id: 'description', label: 'Description', hint: 'Author from a concrete goal.' },
  { id: 'socratic', label: 'Socratic', hint: 'Start from questions and constraints.' },
  { id: 'markdown', label: 'Markdown', hint: 'Use pasted Markdown as authoring input.' },
  { id: 'github', label: 'GitHub', hint: 'Import a GitHub work item.', valueLabel: 'GitHub URL', valuePlaceholder: 'https://github.com/org/repo/issues/123', requiresIntegration: 'github' },
  { id: 'linear', label: 'Linear', hint: 'Import a Linear issue.', valueLabel: 'Linear URL or key', valuePlaceholder: 'ENG-123', requiresIntegration: 'linear' },
  { id: 'jira', label: 'Jira Cloud', hint: 'Import an authorized Jira issue.', valueLabel: 'Jira URL or key', valuePlaceholder: 'ENG-123', requiresIntegration: 'jira' },
  { id: 'openspec', label: 'OpenSpec', hint: 'Import a conventional change.', valueLabel: 'Change path', valuePlaceholder: 'openspec/changes/example', requiresIntegration: 'openspec' }
]

export function sourceNeedsReference(kind: SddSourceKind): boolean {
  return ['github', 'linear', 'jira', 'openspec'].includes(kind)
}

export function sourceGoal(kind: SddSourceKind, goal: string, reference: string): string {
  const trimmed = goal.trim()
  if (trimmed) return trimmed
  return sourceNeedsReference(kind) ? `Author a specification from ${reference.trim()}.` : ''
}

export function approvalLabel(purpose: string): string {
  const labels: Record<string, string> = {
    specification: 'Spec approval required',
    design: 'Design approval required',
    planning: 'Plan approval required',
    implementation: 'Implementation approval required',
    verification: 'Verification approval required',
    review: 'Review approval required'
  }
  return labels[purpose] ?? 'Approval required'
}

export function phaseLabel(phase: SddPhase): string {
  return (
    {
      specification: 'Specification',
      design: 'Design',
      planning: 'Planning',
      implementation: 'Implementation',
      verification: 'Verification',
      review: 'Review',
      ready: 'Ready',
      delivery: 'Delivery',
      completed: 'Completed'
    } satisfies Record<SddPhase, string>
  )[phase]
}

export function selectableDeliveryActions(actions: SddDeliveryAction[]): string[] {
  return actions.filter((action) => action.enabled !== false && !action.blockedReason).map((action) => action.id)
}
